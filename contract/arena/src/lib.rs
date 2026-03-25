#![no_std]

pub mod bounds;

#[cfg(test)]
pub(crate) mod invariants;

#[cfg(test)]
mod abi_guard;

use soroban_sdk::{
    Address, BytesN, Env, Symbol, Vec, contract, contracterror, contractimpl, contracttype,
    symbol_short, token,
};

// ── Storage keys ──────────────────────────────────────────────────────────────

const ADMIN_KEY: Symbol = symbol_short!("ADMIN");
const PAUSED_KEY: Symbol = symbol_short!("PAUSED");
const PENDING_HASH_KEY: Symbol = symbol_short!("P_HASH");
const EXECUTE_AFTER_KEY: Symbol = symbol_short!("P_AFTER");
const SURVIVOR_COUNT_KEY: Symbol = symbol_short!("S_COUNT");
const CAPACITY_KEY: Symbol = symbol_short!("CAPACITY");
const TOKEN_KEY: Symbol = symbol_short!("TOKEN");
const PRIZE_POOL_KEY: Symbol = symbol_short!("PRIZE_P");
const GAME_STATUS_KEY: Symbol = symbol_short!("G_STATUS");

const SCHEMA_VERSION_KEY: Symbol = symbol_short!("S_VER");

/// Current schema version. Bump this when storage layout changes.
const CURRENT_SCHEMA_VERSION: u32 = 1;


// ── Timelock constant: 48 hours in seconds ────────────────────────────────────

const TIMELOCK_PERIOD: u64 = 48 * 60 * 60;

// ── TTL constants (ledgers; mainnet ≈ 5 s/ledger) ────────────────────────────
// Bump entries when remaining TTL falls below ~5.8 days; extend to ~31 days.
// This ensures game state survives the maximum possible round duration.
const GAME_TTL_THRESHOLD: u32 = 100_000; // ~5.8 days
const GAME_TTL_EXTEND_TO: u32 = 535_680; // ~31 days

// ── Event topics ──────────────────────────────────────────────────────────────

const TOPIC_UPGRADE_PROPOSED: Symbol = symbol_short!("UP_PROP");
const TOPIC_UPGRADE_EXECUTED: Symbol = symbol_short!("UP_EXEC");
const TOPIC_UPGRADE_CANCELLED: Symbol = symbol_short!("UP_CANC");
const TOPIC_PAUSED: Symbol = symbol_short!("PAUSED");
const TOPIC_UNPAUSED: Symbol = symbol_short!("UNPAUSED");
const TOPIC_GAME_ENDED: Symbol = symbol_short!("G_END");
const TOPIC_ROUND_STARTED: Symbol = symbol_short!("R_START");
const TOPIC_ROUND_TIMEOUT: Symbol = symbol_short!("R_TOUT");
const TOPIC_WINNER_SET: Symbol = symbol_short!("WIN_SET");
const TOPIC_CLAIM: Symbol = symbol_short!("CLAIM");

/// Event payload version. Include in every event data tuple so consumers
/// can detect schema changes without re-deploying indexers.
const EVENT_VERSION: u32 = 1;
const TOPIC_ROUND_RESOLVED: Symbol = symbol_short!("ROUND_OK");


// ── Error codes ───────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ArenaError {
    AlreadyInitialized = 1,
    InvalidRoundSpeed = 2,
    RoundAlreadyActive = 3,
    NoActiveRound = 4,
    SubmissionWindowClosed = 5,
    SubmissionAlreadyExists = 6,
    RoundStillOpen = 7,
    RoundDeadlineOverflow = 8,
    NotInitialized = 9,
    Paused = 10,

    ArenaFull = 11,
    AlreadyJoined = 12,
    InvalidAmount = 13,
    NoPrizeToClaim = 14,
    AlreadyClaimed = 15,
    ReentrancyGuard = 16,
    NotASurvivor = 17,
    GameAlreadyFinished = 18,
    TokenNotSet = 19,
    /// Per-round submission storage would exceed [`bounds::MAX_SUBMISSIONS_PER_ROUND`](crate::bounds::MAX_SUBMISSIONS_PER_ROUND).
    MaxSubmissionsPerRound = 20,
    PlayerEliminated = 21,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Choice {
    Heads,
    Tails,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArenaConfig {
    pub round_speed_in_ledgers: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoundState {
    pub round_number: u32,
    pub round_start_ledger: u32,
    pub round_deadline_ledger: u32,
    pub active: bool,
    pub total_submissions: u32,
    pub timed_out: bool,
    /// Set to true when `resolve_round` fully processes a round.
    pub finished: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArenaStateView {
    pub survivors_count: u32,
    pub max_capacity: u32,
    pub round_number: u32,
    pub current_stake: i128,
    pub potential_payout: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserStateView {
    pub is_active: bool,
    pub has_won: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullStateView {
    pub survivors_count: u32,
    pub max_capacity: u32,
    pub round_number: u32,
    pub current_stake: i128,
    pub potential_payout: i128,
    pub is_active: bool,
    pub has_won: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArenaState {
    pub survivors_count: u32,
    pub max_capacity: u32,
    pub round_number: u32,
    pub current_stake: i128,
    pub potential_payout: i128,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Config,
    Round,
    Submission(u32, Address),
    RoundPlayers(u32),
    ActivePlayers,
    User(Address),
    PrizeClaimed(Address),
    Survivor(Address),
    Winner(Address),
    Token,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct ArenaContract;

#[contractimpl]
impl ArenaContract {
    // ── Initialisation ───────────────────────────────────────────────────────

    /// Initialise the arena contract. Must be called exactly once after deployment.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `round_speed_in_ledgers` - Number of ledgers each round lasts. Must be > 0.
    ///
    /// # Errors
    /// * [`ArenaError::AlreadyInitialized`] — Contract has already been initialised.
    /// * [`ArenaError::InvalidRoundSpeed`] — `round_speed_in_ledgers` is zero.
    ///
    /// # Authorization
    /// None — permissionless; caller is responsible for calling immediately after deploy.
    pub fn init(env: Env, round_speed_in_ledgers: u32) -> Result<(), ArenaError> {
        if storage(&env).has(&DataKey::Config) {
            return Err(ArenaError::AlreadyInitialized);
        }

        if round_speed_in_ledgers == 0 {
            return Err(ArenaError::InvalidRoundSpeed);
        }

        env.storage()
            .instance()
            .extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);

        storage(&env).set(
            &DataKey::Config,
            &ArenaConfig {
                round_speed_in_ledgers,
            },
        );
        bump(&env, &DataKey::Config);

        storage(&env).set(
            &DataKey::Round,
            &RoundState {
                round_number: 0,
                round_start_ledger: 0,
                round_deadline_ledger: 0,
                active: false,
                total_submissions: 0,
                timed_out: false,
                finished: false,
            },
        );
        bump(&env, &DataKey::Round);

        Ok(())
    }

    // ── Token and Payouts ────────────────────────────────────────────────────

    pub fn set_token(env: Env, token: Address) {
        require_not_paused(&env).unwrap();
        let admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized");
        admin.require_auth();
        env.storage().instance().set(&TOKEN_KEY, &token);
    }

    pub fn set_winner(env: Env, player: Address, stake: i128, yield_comp: i128) {
        require_not_paused(&env).unwrap();
        let admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized");
        admin.require_auth();
        storage(&env).set(&DataKey::Winner(player.clone()), &(stake, yield_comp));
        bump(&env, &DataKey::Winner(player.clone()));
        env.events()
            .publish((TOPIC_WINNER_SET,), (player, stake, yield_comp));
    }

    // ── Admin ────────────────────────────────────────────────────────────────

    /// Set the admin address. Must be called once after deployment before any
    /// upgrade functions can be used.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - Address to designate as the contract administrator.
    ///
    /// # Errors
    /// Panics with `"already initialized"` if an admin has already been set.
    ///
    /// # Authorization
    /// None — permissionless; must be called immediately after deploy.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&ADMIN_KEY) {
            panic!("already initialized");
        }
        env.storage().instance().set(&ADMIN_KEY, &admin);
    }

    /// Return the current admin address.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Errors
    /// Panics with `"not initialized"` if `initialize` has not been called.
    ///
    /// # Authorization
    /// None — read-only, open to any caller.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized")
    }

    /// Set a new admin address. Only the current admin can call this.
    pub fn set_admin(env: Env, new_admin: Address) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized");
        admin.require_auth();
        env.storage().instance().set(&ADMIN_KEY, &new_admin);
    }

    /// Pause the contract. Admin-only.
    pub fn pause(env: Env) {
        let admin = Self::admin(env.clone());
        admin.require_auth();
        env.storage().instance().set(&PAUSED_KEY, &true);
        env.events().publish((TOPIC_PAUSED,), ());
    }

    /// Unpause the contract. Admin-only.
    pub fn unpause(env: Env) {
        let admin = Self::admin(env.clone());
        admin.require_auth();
        env.storage().instance().set(&PAUSED_KEY, &false);
        env.events().publish((TOPIC_UNPAUSED,), ());
    }

    /// Return whether the contract is paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage().instance().get(&PAUSED_KEY).unwrap_or(false)
    }


    // ── Schema versioning ────────────────────────────────────────────────────

    /// Return the persisted schema version (0 if never set).
    pub fn schema_version(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&SCHEMA_VERSION_KEY)
            .unwrap_or(0u32)
    }

    /// Migrate storage from the current persisted version to
    /// `CURRENT_SCHEMA_VERSION`. Admin-only.
    ///
    /// Each version bump should have its own migration block inside
    /// this function. The version is written atomically at the end so
    /// a failed transaction leaves the old version in place.
    ///
    /// Calling `migrate` when already at the current version is a no-op.
    pub fn migrate(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized");
        admin.require_auth();

        let stored: u32 = env
            .storage()
            .instance()
            .get(&SCHEMA_VERSION_KEY)
            .unwrap_or(0u32);

        if stored >= CURRENT_SCHEMA_VERSION {
            return; // already up to date
        }

        // -- v0 -> v1: initial version stamp (no data changes) ------
        // Future migrations would go here as sequential if-blocks:
        //   if stored < 2 { /* v1 -> v2 migration logic */ }

        env.storage()
            .instance()
            .set(&SCHEMA_VERSION_KEY, &CURRENT_SCHEMA_VERSION);
    }

    /// Set the maximum player capacity for this arena. Admin-only.
    ///
    /// # Authorization
    /// Requires admin signature.
    pub fn set_capacity(env: Env, capacity: u32) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized");
        admin.require_auth();
        env.storage().instance().set(&CAPACITY_KEY, &capacity);
    }

    /// Return a snapshot of the arena's live state.
    ///
    /// Pure read — no storage writes, safe to call via simulation.
    /// Serialises as `ScvMap { Symbol → Val }` matching the frontend parser in
    /// `stellar-scval-extract.ts`.
    pub fn get_arena_state(env: Env) -> ArenaState {
        let survivors_count: u32 = env
            .storage()
            .instance()
            .get(&SURVIVOR_COUNT_KEY)
            .unwrap_or(0u32);
        let max_capacity: u32 = env
            .storage()
            .instance()
            .get(&CAPACITY_KEY)
            .unwrap_or(0u32);
        let round_number: u32 = storage(&env)
            .get::<_, RoundState>(&DataKey::Round)
            .map(|r| r.round_number)
            .unwrap_or(0u32);

        ArenaState {
            survivors_count,
            max_capacity,
            round_number,
            // The contract uses per-player Winner records rather than a global
            // prize pool, so these aggregate financials are not tracked on-chain.
            current_stake: 0,
            potential_payout: 0,
        }
    }

    pub fn join(env: Env, player: Address, amount: i128) -> Result<(), ArenaError> {
        require_not_paused(&env)?;
        env.storage()
            .instance()
            .extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);
        player.require_auth();

        if amount <= 0 {
            return Err(ArenaError::InvalidAmount);
        }

        let survivor_key = DataKey::Survivor(player.clone());
        if storage(&env).has(&survivor_key) {
            return Err(ArenaError::AlreadyJoined);
        }

        let configured_cap: u32 = env
            .storage()
            .instance()
            .get(&CAPACITY_KEY)
            .unwrap_or(0u32);
        let effective_cap = if configured_cap == 0 {
            bounds::MAX_ARENA_PARTICIPANTS
        } else {
            configured_cap.min(bounds::MAX_ARENA_PARTICIPANTS)
        };

        let count: u32 = env
            .storage()
            .instance()
            .get(&SURVIVOR_COUNT_KEY)
            .unwrap_or(0u32);

        if count >= effective_cap {
            return Err(ArenaError::ArenaFull);
        }

        // Token must be configured before players can join.
        let token: Address = env
            .storage()
            .instance()
            .get(&TOKEN_KEY)
            .ok_or(ArenaError::TokenNotSet)?;

        // ── EFFECT: register survivor and update counts before external call (CEI) ──
        storage(&env).set(&survivor_key, &());
        bump(&env, &survivor_key);

        env.storage()
            .instance()
            .set(&SURVIVOR_COUNT_KEY, &(count + 1));

        let pool: i128 = env
            .storage()
            .instance()
            .get(&PRIZE_POOL_KEY)
            .unwrap_or(0i128);
        env.storage()
            .instance()
            .set(&PRIZE_POOL_KEY, &(pool + amount));

        // ── INTERACTION: pull stake from player into this contract ─────────────────
        token::Client::new(&env, &token).transfer(&player, &env.current_contract_address(), &amount);

        Ok(())
    }

    // ── Round state machine ──────────────────────────────────────────────────

    /// Start a new round. Increments the round counter and opens the submission window.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Errors
    /// * [`ArenaError::NotInitialized`] — `initialize` has not been called.
    /// * [`ArenaError::RoundAlreadyActive`] — A round is currently in progress.
    /// * [`ArenaError::RoundDeadlineOverflow`] — Ledger sequence + round speed overflows `u32`.
    ///
    /// # Authorization
    /// None — any caller may start a round once the previous one has ended.
    ///
    /// # Events
    /// None emitted directly; callers should observe the returned [`RoundState`].
    pub fn start_round(env: Env) -> Result<RoundState, ArenaError> {
        require_not_paused(&env)?;
        env.storage()
            .instance()
            .extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);

        let config = get_config(&env)?;
        let previous_round = get_round(&env)?;

        if previous_round.active {
            return Err(ArenaError::RoundAlreadyActive);
        }

        let round_start_ledger = env.ledger().sequence();
        let round_deadline_ledger = round_start_ledger
            .checked_add(config.round_speed_in_ledgers)
            .ok_or(ArenaError::RoundDeadlineOverflow)?;

        let next_round = RoundState {
            round_number: previous_round.round_number + 1,
            round_start_ledger,
            round_deadline_ledger,
            active: true,
            total_submissions: 0,
            timed_out: false,
            finished: false,
        };

        storage(&env).set(&DataKey::Round, &next_round);
        bump(&env, &DataKey::Round);

        env.events().publish(
            (TOPIC_ROUND_STARTED,),
            (
                next_round.round_number,
                next_round.round_start_ledger,
                next_round.round_deadline_ledger,
            ),
        );

        Ok(next_round)
    }

    /// Submit a player's choice for the active round.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `player` - Address of the player submitting a choice.
    /// * `choice` - [`Choice::Heads`] or [`Choice::Tails`].
    ///
    /// # Errors
    /// * [`ArenaError::NotInitialized`] — Contract not initialised.
    /// * [`ArenaError::NoActiveRound`] — No round is currently active.
    /// * [`ArenaError::SubmissionWindowClosed`] — Current ledger is past the round deadline.
    /// * [`ArenaError::SubmissionAlreadyExists`] — `player` already submitted in this round.
    /// * [`ArenaError::MaxSubmissionsPerRound`] — [`bounds::MAX_SUBMISSIONS_PER_ROUND`](crate::bounds::MAX_SUBMISSIONS_PER_ROUND) reached for this round.
    ///
    /// # Authorization
    /// Requires `player.require_auth()` — the transaction must be signed by `player`.
    pub fn submit_choice(
        env: Env,
        player: Address,
        round_number: u32,
        choice: Choice,
    ) -> Result<(), ArenaError> {
        require_not_paused(&env)?;
        env.storage()
            .instance()
            .extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);
        player.require_auth();

        let mut round = get_round(&env)?;
        if !round.active {
            return Err(ArenaError::NoActiveRound);
        }

        if round_number != round.round_number {
            return Err(ArenaError::RoundMismatch);
        }

        let current_ledger = env.ledger().sequence();
        if current_ledger > round.round_deadline_ledger {
            return Err(ArenaError::SubmissionWindowClosed);
        }

        let submission_key = DataKey::Submission(round.round_number, player.clone());
        if storage(&env).has(&submission_key) {
            return Err(ArenaError::SubmissionAlreadyExists);
        }

        if round.total_submissions >= bounds::MAX_SUBMISSIONS_PER_ROUND {
            return Err(ArenaError::MaxSubmissionsPerRound);
        }

        if !player_can_submit(&env, &player) {
            return Err(ArenaError::PlayerEliminated);
        }

        storage(&env).set(&submission_key, &choice);

        bump(&env, &submission_key);

        let players_key = DataKey::RoundPlayers(round.round_number);
        let mut players: Vec<Address> = storage(&env)
            .get(&players_key)
            .unwrap_or_else(|| Vec::new(&env));
        players.push_back(player.clone());
        storage(&env).set(&players_key, &players);
        bump(&env, &players_key);

        let user_key = DataKey::User(player);
        storage(&env).set(
            &user_key,
            &UserState {
                active: true,
                won: false,
            },
        );
        bump(&env, &user_key);

        round.total_submissions += 1;
        storage(&env).set(&DataKey::Round, &round);
        bump(&env, &DataKey::Round);

        Ok(())
    }

    /// Mark the active round as timed-out once its deadline has passed.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Errors
    /// * [`ArenaError::NotInitialized`] — Contract not initialised.
    /// * [`ArenaError::NoActiveRound`] — No round is currently active.
    /// * [`ArenaError::RoundStillOpen`] — The round deadline has not yet been reached.
    ///
    /// # Authorization
    /// None — any caller may trigger a timeout once the deadline has passed.
    pub fn timeout_round(env: Env) -> Result<RoundState, ArenaError> {
        require_not_paused(&env)?;
        env.storage()
            .instance()
            .extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);

        let mut round = get_round(&env)?;
        if !round.active {
            return Err(ArenaError::NoActiveRound);
        }

        let current_ledger = env.ledger().sequence();
        if current_ledger <= round.round_deadline_ledger {
            return Err(ArenaError::RoundStillOpen);
        }

        round.active = false;
        round.timed_out = true;
        storage(&env).set(&DataKey::Round, &round);
        bump(&env, &DataKey::Round);

        env.events().publish(
            (TOPIC_ROUND_TIMEOUT,),
            (round.round_number, round.total_submissions, true),
        );

        Ok(round)
    }

    pub fn resolve_round(env: Env) -> Result<RoundResolution, ArenaError> {
        require_not_paused(&env)?;
        env.storage()
            .instance()
            .extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);

        let mut round = get_round(&env)?;
        if !round.active {
            return Err(ArenaError::NoActiveRound);
        }

        let current_ledger = env.ledger().sequence();
        if current_ledger <= round.round_deadline_ledger {
            return Err(ArenaError::RoundStillOpen);
        }

        let players_key = DataKey::RoundPlayers(round.round_number);
        let players: Vec<Address> = storage(&env)
            .get(&players_key)
            .unwrap_or_else(|| Vec::new(&env));

        let mut heads_count = 0u32;
        let mut tails_count = 0u32;
        for player in players.iter() {
            match storage(&env).get::<_, Choice>(&DataKey::Submission(round.round_number, player)) {
                Some(Choice::Heads) => heads_count += 1,
                Some(Choice::Tails) => tails_count += 1,
                None => {}
            }
        }

        let tied = heads_count == tails_count;
        let winning_choice = determine_winning_choice(&env, heads_count, tails_count);
        let mut survivors = Vec::new(&env);
        let mut eliminated = 0u32;

        for player in players.iter() {
            let survives = storage(&env)
                .get::<_, Choice>(&DataKey::Submission(round.round_number, player.clone()))
                == Some(winning_choice.clone());

            if survives {
                survivors.push_back(player.clone());
            } else {
                eliminated += 1;
            }

            let user_key = DataKey::User(player.clone());
            storage(&env).set(
                &user_key,
                &UserState {
                    active: survives,
                    won: false,
                },
            );
            bump(&env, &user_key);
        }

        if survivors.len() == 1 {
            let sole_survivor = survivors.get(0).unwrap();
            let user_key = DataKey::User(sole_survivor.clone());
            storage(&env).set(
                &user_key,
                &UserState {
                    active: true,
                    won: true,
                },
            );
            bump(&env, &user_key);
        }

        storage(&env).set(&DataKey::ActivePlayers, &survivors);
        bump(&env, &DataKey::ActivePlayers);

        round.active = false;
        round.timed_out = false;
        storage(&env).set(&DataKey::Round, &round);
        bump(&env, &DataKey::Round);

        let resolution = RoundResolution {
            round_number: round.round_number,
            winning_choice: winning_choice.clone(),
            survivors: survivors.len(),
            eliminated,
            tied,
        };

        env.events().publish(
            (TOPIC_ROUND_RESOLVED, round.round_number, winning_choice),
            (resolution.survivors, resolution.eliminated, resolution.tied),
        );

        Ok(resolution)
    }

    /// Return the current [`ArenaConfig`].
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Errors
    /// * [`ArenaError::NotInitialized`] — `initialize` has not been called.
    ///
    /// # Authorization
    /// None — read-only, open to any caller.
    pub fn get_config(env: Env) -> Result<ArenaConfig, ArenaError> {
        get_config(&env)
    }

    /// Return the current [`RoundState`].
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Errors
    /// * [`ArenaError::NotInitialized`] — `initialize` has not been called.
    ///
    /// # Authorization
    /// None — read-only, open to any caller.
    pub fn get_round(env: Env) -> Result<RoundState, ArenaError> {
        get_round(&env)
    }

    /// Return the choice a player submitted for a given round, or `None` if they did not submit.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `round_number` - The round to query.
    /// * `player` - Address of the player to look up.
    ///
    /// # Authorization
    /// None — read-only, open to any caller.
    pub fn get_choice(env: Env, round_number: u32, player: Address) -> Option<Choice> {
        storage(&env).get(&DataKey::Submission(round_number, player))
    }

    /// Return contract-level arena state for UI reads.
    pub fn get_arena_state(env: Env) -> Result<ArenaStateView, ArenaError> {
        let round = get_round(&env)?;
        let prize_pool: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);

        Ok(ArenaStateView {
            // This contract currently tracks submissions for the active/latest round.
            survivors_count: round.total_submissions,
            // Capacity is not modeled in this contract yet.
            max_capacity: 0,
            round_number: round.round_number,
            current_stake: prize_pool,
            potential_payout: prize_pool,
        })
    }

    /// Return user-level arena state for UI reads.
    pub fn get_user_state(env: Env, player: Address) -> UserStateView {
        let is_active = storage(&env).has(&DataKey::Survivor(player.clone()));
        let has_won = storage(&env).has(&DataKey::PrizeClaimed(player));

        UserStateView { is_active, has_won }
    }

    /// Return combined arena + user state in one read for RPC efficiency.
    pub fn get_full_state(env: Env, player: Address) -> Result<FullStateView, ArenaError> {
        let arena_state = Self::get_arena_state(env.clone())?;
        let user_state = Self::get_user_state(env, player);

        Ok(FullStateView {
            survivors_count: arena_state.survivors_count,
            max_capacity: arena_state.max_capacity,
            round_number: arena_state.round_number,
            current_stake: arena_state.current_stake,
            potential_payout: arena_state.potential_payout,
            is_active: user_state.is_active,
            has_won: user_state.has_won,
        })
    }

    pub fn claim(env: Env, winner: Address) -> Result<i128, ArenaError> {
        require_not_paused(&env)?;
        winner.require_auth();

        // ── CHECK: re-entrancy guard ──────────────────────────────────────────────
        if env
            .storage()
            .instance()
            .get::<_, bool>(&GAME_STATUS_KEY)
            .unwrap_or(false)
        {
            return Err(ArenaError::ReentrancyGuard);
        }

        // ── CHECK: prize pool must be non-zero ────────────────────────────────────
        let prize: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);
        if prize <= 0 {
            return Err(ArenaError::NoPrizeToClaim);
        }

        // ── CHECK: winner must not have claimed before ────────────────────────────
        let prize_key = DataKey::PrizeClaimed(winner.clone());
        if storage(&env).has(&prize_key) {
            return Err(ArenaError::AlreadyClaimed);
        }

        // ── CHECK: token must be configured ───────────────────────────────────────
        let token: Address = env
            .storage()
            .instance()
            .get(&TOKEN_KEY)
            .ok_or(ArenaError::TokenNotSet)?;

        // ── EFFECT: lock re-entrancy guard ────────────────────────────────────────
        env.storage().instance().set(&GAME_STATUS_KEY, &true);

        // ── EFFECT: record claim and drain prize pool BEFORE external call (CEI) ──
        storage(&env).set(&prize_key, &prize);
        bump(&env, &prize_key);

        env.storage().instance().set(&PRIZE_POOL_KEY, &0i128);

        // ── INTERACTION: transfer prize tokens to winner ──────────────────────────
        token::Client::new(&env, &token).transfer(
            &env.current_contract_address(),
            &winner,
            &prize,
        );

        // ── EFFECT: release re-entrancy guard ─────────────────────────────────────
        env.storage().instance().set(&GAME_STATUS_KEY, &false);

        env.events().publish((TOPIC_CLAIM,), (winner, prize, EVENT_VERSION));

        Ok(prize)
    }

    // ── Upgrade mechanism ────────────────────────────────────────────────────

    /// Propose a WASM upgrade. The new hash is stored together with the
    /// earliest timestamp at which `execute_upgrade` may be called (now + 48 h).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `new_wasm_hash` - 32-byte hash of the new contract WASM to deploy.
    ///
    /// # Errors
    /// Panics with `"not initialized"` if the contract has not been initialized.
    ///
    /// # Authorization
    /// Requires admin signature (`admin.require_auth()`).
    ///
    /// # Events
    /// Emits `UpgradeProposed(new_wasm_hash, execute_after)`.
    pub fn propose_upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        // Pause-exempt: admin must be able to queue a recovery upgrade at any time.
        let admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized");
        admin.require_auth();

        let execute_after: u64 = env.ledger().timestamp() + TIMELOCK_PERIOD;
        env.storage()
            .instance()
            .set(&PENDING_HASH_KEY, &new_wasm_hash);
        env.storage()
            .instance()
            .set(&EXECUTE_AFTER_KEY, &execute_after);

        env.events()
            .publish((TOPIC_UPGRADE_PROPOSED,), (new_wasm_hash, execute_after));
    }

    /// Execute a previously proposed upgrade after the 48-hour timelock.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Errors
    /// Panics with `"not initialized"` if admin is not set.
    /// Panics with `"no pending upgrade"` if no proposal exists.
    /// Panics with `"timelock has not expired"` if called before the timelock elapses.
    ///
    /// # Authorization
    /// Requires admin signature (`admin.require_auth()`).
    ///
    /// # Events
    /// Emits `UpgradeExecuted(new_wasm_hash)`.
    pub fn execute_upgrade(env: Env) {
        // Pause-exempt: admin must be able to deploy the recovery upgrade after the timelock.
        let admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized");
        admin.require_auth();

        let has_pending_hash = env.storage().instance().has(&PENDING_HASH_KEY);
        let has_execute_after = env.storage().instance().has(&EXECUTE_AFTER_KEY);
        match (has_pending_hash, has_execute_after) {
            (false, false) => panic!("no pending upgrade"),
            (true, false) | (false, true) => panic!("malformed upgrade state"),
            (true, true) => {}
        }

        let execute_after: u64 = env
            .storage()
            .instance()
            .get(&EXECUTE_AFTER_KEY)
            .expect("malformed upgrade state");

        if env.ledger().timestamp() < execute_after {
            panic!("timelock has not expired");
        }

        let new_wasm_hash: BytesN<32> = env
            .storage()
            .instance()
            .get(&PENDING_HASH_KEY)
            .expect("malformed upgrade state");

        // Clear pending state before upgrading.
        env.storage().instance().remove(&PENDING_HASH_KEY);
        env.storage().instance().remove(&EXECUTE_AFTER_KEY);

        env.events()
            .publish((TOPIC_UPGRADE_EXECUTED,), new_wasm_hash.clone());

        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Cancel a pending upgrade proposal. Admin-only.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Errors
    /// Panics with `"not initialized"` if admin is not set.
    /// Panics with `"no pending upgrade to cancel"` if no proposal exists.
    ///
    /// # Authorization
    /// Requires admin signature (`admin.require_auth()`).
    ///
    /// # Events
    /// Emits `UpgradeCancelled`.
    pub fn cancel_upgrade(env: Env) {
        // Pause-exempt: admin must be able to retract an incorrect proposal at any time.
        let admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("not initialized");
        admin.require_auth();

        if !env.storage().instance().has(&PENDING_HASH_KEY) {
            panic!("no pending upgrade to cancel");
        }

        env.storage().instance().remove(&PENDING_HASH_KEY);
        env.storage().instance().remove(&EXECUTE_AFTER_KEY);

        env.events().publish((TOPIC_UPGRADE_CANCELLED,), ());
    }

    /// Return the pending WASM hash and the earliest execution timestamp,
    /// or `None` if no upgrade has been proposed.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Authorization
    /// None — read-only, open to any caller.
    pub fn pending_upgrade(env: Env) -> Option<(BytesN<32>, u64)> {
        let hash: Option<BytesN<32>> = env.storage().instance().get(&PENDING_HASH_KEY);
        let after: Option<u64> = env.storage().instance().get(&EXECUTE_AFTER_KEY);
        match (hash, after) {
            (Some(h), Some(a)) => Some((h, a)),
            _ => None,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_config(env: &Env) -> Result<ArenaConfig, ArenaError> {
    storage(env)
        .get(&DataKey::Config)
        .ok_or(ArenaError::NotInitialized)
}

fn get_round(env: &Env) -> Result<RoundState, ArenaError> {
    storage(env)
        .get(&DataKey::Round)
        .ok_or(ArenaError::NotInitialized)
}

fn storage(env: &Env) -> soroban_sdk::storage::Persistent {
    env.storage().persistent()
}

fn require_not_paused(env: &Env) -> Result<(), ArenaError> {
    if env.storage().instance().get(&PAUSED_KEY).unwrap_or(false) {
        return Err(ArenaError::Paused);
    }
    Ok(())
}

fn bump(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);
}

fn player_can_submit(env: &Env, player: &Address) -> bool {
    let active_players: Option<Vec<Address>> = storage(env).get(&DataKey::ActivePlayers);
    match active_players {
        Some(players) if !players.is_empty() => players.contains(player.clone()),
        _ => true,
    }
}

fn determine_winning_choice(env: &Env, heads_count: u32, tails_count: u32) -> Choice {
    if heads_count < tails_count {
        Choice::Heads
    } else if tails_count < heads_count {
        Choice::Tails
    } else if env.ledger().sequence() % 2 == 0 {
        Choice::Heads
    } else {
        Choice::Tails
    }
}

#[cfg(test)]
mod test;

#[cfg(all(test, feature = "integration-tests"))]
mod integration_tests;