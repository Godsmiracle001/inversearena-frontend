#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Bytes,
    BytesN, Env, IntoVal, String, Symbol, Vec, xdr::ToXdr,
};

mod bounds;

// ── Constants ─────────────────────────────────────────────────────────────────

const ADMIN_KEY: Symbol = symbol_short!("ADMIN");
const TOKEN_KEY: Symbol = symbol_short!("TOKEN");
const CAPACITY_KEY: Symbol = symbol_short!("CAPACITY");
const PRIZE_POOL_KEY: Symbol = symbol_short!("POOL");
const SURVIVOR_COUNT_KEY: Symbol = symbol_short!("S_COUNT");
const CANCELLED_KEY: Symbol = symbol_short!("CANCEL");
const GAME_FINISHED_KEY: Symbol = symbol_short!("FINISHED");
const WINNER_SET_KEY: Symbol = symbol_short!("WIN_SET");
const PAUSED_KEY: Symbol = symbol_short!("PAUSED");

const TOPIC_ROUND_STARTED: Symbol = symbol_short!("R_START");
const TOPIC_CHOICE_SUBMITTED: Symbol = symbol_short!("SUBMIT");
const TOPIC_ROUND_RESOLVED: Symbol = symbol_short!("RSLVD");
const TOPIC_ROUND_TIMEOUT: Symbol = symbol_short!("R_TOUT");
const TOPIC_CLAIM: Symbol = symbol_short!("CLAIM");
const TOPIC_WINNER_SET: Symbol = symbol_short!("WIN_SET");
const TOPIC_CANCELLED: Symbol = symbol_short!("CANCEL");
const TOPIC_PAUSED: Symbol = symbol_short!("PAUSED");
const TOPIC_UNPAUSED: Symbol = symbol_short!("UNPAUSED");
const TOPIC_LEAVE: Symbol = symbol_short!("LEAVE");
const TOPIC_STATE_CHANGED: Symbol = symbol_short!("STATE");
const TOPIC_UPGRADE_PROPOSED: Symbol = symbol_short!("UP_PROP");
const TOPIC_UPGRADE_EXECUTED: Symbol = symbol_short!("UP_EXEC");
const TOPIC_UPGRADE_CANCELLED: Symbol = symbol_short!("UP_CANC");

const TIMELOCK_PERIOD: u64 = 48 * 3600; // 48 hours
const GAME_TTL_THRESHOLD: u32 = 17280; // ~1 day
const GAME_TTL_EXTEND_TO: u32 = 518400; // ~30 days

const EVENT_VERSION: u32 = 1;

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
    MaxSubmissionsPerRound = 20,
    PlayerEliminated = 21,
    WrongRoundNumber = 22,
    NotEnoughPlayers = 23,
    InvalidCapacity = 24,
    NoPendingUpgrade = 25,
    TimelockNotExpired = 26,
    GameNotFinished = 27,
    TokenConfigurationLocked = 28,
    UpgradeAlreadyPending = 29,
    WinnerAlreadySet = 30,
    WinnerNotSet = 31,
    AlreadyCancelled = 32,
    InvalidMaxRounds = 33,
    NameTooLong = 34,
    NameEmpty = 35,
    DescriptionTooLong = 36,
    NoCommitment = 37,
    CommitmentMismatch = 38,
    RevealDeadlinePassed = 39,
    CommitDeadlinePassed = 40,
    AlreadyCommitted = 41,
}

// ── Types ─────────────────────────────────────────────────────────────────────

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
    pub required_stake_amount: i128,
    pub max_rounds: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoundState {
    pub round_number: u32,
    pub round_start_ledger: u32,
    pub round_deadline_ledger: u32,
    pub reveal_deadline_ledger: u32,
    pub active: bool,
    pub total_submissions: u32,
    pub timed_out: bool,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaState {
    Pending,
    Active,
    Completed,
    Cancelled,
}

impl ArenaState {
    pub fn is_terminal_state(&self) -> bool {
        matches!(self, ArenaState::Completed | ArenaState::Cancelled)
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArenaStateChanged {
    pub old_state: ArenaState,
    pub new_state: ArenaState,
}

macro_rules! assert_state {
    ($current:expr, $expected:pat) => {
        match $current {
            $expected => {},
            _ => panic!("Invalid state transition: current state {:?} is not allowed for this operation", $current),
        }
    };
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
#[derive(Clone, Debug)]
pub struct ArenaMetadata {
    pub arena_id: u64,
    pub name: String,
    pub description: Option<String>,
    pub host: Address,
    pub created_at: u64,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Config,
    Round,
    RoundChoices(u32),
    Commitment(u32, Address),
    Survivor(Address),
    Eliminated(Address),
    PrizeClaimed(Address),
    Winner(Address),
    AllPlayers,
    Refunded(Address),
    State,
    Metadata(u64),
    ContractAdmin,
    UpgradeHash,
    UpgradeTimestamp,
    FactoryAddress,
    ArenaId,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct ArenaContract;

#[contractimpl]
impl ArenaContract {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::ContractAdmin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::ContractAdmin, &admin);
        env.storage().instance().extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);
    }

    pub fn admin(env: Env) -> Address {
        env.storage().instance()
            .get(&DataKey::ContractAdmin)
            .expect("not initialized")
    }

    pub fn set_admin(env: Env, new_admin: Address) {
        let admin: Address = Self::admin(env.clone());
        admin.require_auth();
        env.storage().instance().set(&DataKey::ContractAdmin, &new_admin);
    }

    pub fn init_factory(env: Env, factory: Address, _creator: Address) {
        let admin = Self::admin(env.clone());
        admin.require_auth();

        if env.storage().instance().has(&DataKey::FactoryAddress) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::FactoryAddress, &factory);
    }

    pub fn init(
        env: Env,
        round_speed_in_ledgers: u32,
        required_stake_amount: i128,
    ) -> Result<(), ArenaError> {
        let admin = Self::admin(env.clone());
        admin.require_auth();

        if env.storage().instance().has(&DataKey::Config) {
            return Err(ArenaError::AlreadyInitialized);
        }
        if round_speed_in_ledgers == 0 || round_speed_in_ledgers > bounds::MAX_SPEED_LEDGERS {
            return Err(ArenaError::InvalidRoundSpeed);
        }
        if required_stake_amount < bounds::MIN_REQUIRED_STAKE {
            return Err(ArenaError::InvalidAmount);
        }
        
        env.storage().instance().extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);
        env.storage().instance().set(
            &DataKey::Config,
            &ArenaConfig {
                round_speed_in_ledgers,
                required_stake_amount,
                max_rounds: bounds::DEFAULT_MAX_ROUNDS,
            },
        );
        env.storage().instance().set(
            &DataKey::Round,
            &RoundState {
                round_number: 0,
                round_start_ledger: 0,
                round_deadline_ledger: 0,
                reveal_deadline_ledger: 0,
                active: false,
                total_submissions: 0,
                timed_out: false,
                finished: false,
            },
        );
        set_state(&env, ArenaState::Pending);
        Ok(())
    }

    pub fn pause(env: Env) {
        let admin = Self::admin(env.clone());
        admin.require_auth();
        env.storage().instance().set(&PAUSED_KEY, &true);
        env.events().publish((TOPIC_PAUSED,), (EVENT_VERSION,));
    }

    pub fn unpause(env: Env) {
        let admin = Self::admin(env.clone());
        admin.require_auth();
        env.storage().instance().set(&PAUSED_KEY, &false);
        env.events().publish((TOPIC_UNPAUSED,), (EVENT_VERSION,));
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get::<_, bool>(&PAUSED_KEY)
            .unwrap_or(false)
    }

    pub fn set_token(env: Env, token: Address) -> Result<(), ArenaError> {
        let admin = Self::admin(env.clone());
        admin.require_auth();
        
        let survivor_count: u32 = env.storage().instance().get(&SURVIVOR_COUNT_KEY).unwrap_or(0);
        let prize_pool: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);
        
        if survivor_count > 0 || prize_pool > 0 {
            return Err(ArenaError::TokenConfigurationLocked);
        }
        env.storage().instance().set(&TOKEN_KEY, &token);
        Ok(())
    }

    pub fn set_capacity(env: Env, capacity: u32) -> Result<(), ArenaError> {
        let admin = Self::admin(env.clone());
        admin.require_auth();

        if !(bounds::MIN_ARENA_PARTICIPANTS..=bounds::MAX_ARENA_PARTICIPANTS).contains(&capacity) {
            return Err(ArenaError::InvalidCapacity);
        }
        env.storage().instance().set(&CAPACITY_KEY, &capacity);
        Ok(())
    }

    pub fn set_winner(
        env: Env,
        player: Address,
        stake: i128,
        yield_comp: i128,
    ) -> Result<(), ArenaError> {
        require_not_paused(&env)?;
        let admin = Self::admin(env.clone());
        admin.require_auth();

        let current_state = get_state(&env);
        assert_state!(current_state, ArenaState::Active);

        if !env.storage().persistent().has(&DataKey::Survivor(player.clone())) {
            return Err(ArenaError::NotASurvivor);
        }

        if env.storage().instance().get::<_, bool>(&WINNER_SET_KEY).unwrap_or(false) {
            return Err(ArenaError::WinnerAlreadySet);
        }
        if stake < 0 || yield_comp < 0 {
            return Err(ArenaError::InvalidAmount);
        }
        let prize = stake
            .checked_add(yield_comp)
            .ok_or(ArenaError::InvalidAmount)?;

        let mut pool: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);
        pool = pool.checked_add(prize).ok_or(ArenaError::InvalidAmount)?;
        
        env.storage().instance().set(&PRIZE_POOL_KEY, &pool);
        env.storage().instance().set(&WINNER_SET_KEY, &true);
        env.storage().persistent().set(&DataKey::Winner(player.clone()), &true);
        bump(&env, &DataKey::Winner(player.clone()));

        env.events()
            .publish((TOPIC_WINNER_SET,), (player, stake, yield_comp, EVENT_VERSION));
        Ok(())
    }

    pub fn join(env: Env, player: Address, amount: i128) -> Result<(), ArenaError> {
        player.require_auth();
        require_not_paused(&env)?;
        let current_state = get_state(&env);
        assert_state!(current_state, ArenaState::Pending);

        let config = get_config(&env)?;
        if amount != config.required_stake_amount {
            return Err(ArenaError::InvalidAmount);
        }

        let survivor_key = DataKey::Survivor(player.clone());
        if env.storage().persistent().has(&survivor_key) {
            return Err(ArenaError::AlreadyJoined);
        }

        let capacity: u32 = env.storage().instance().get(&CAPACITY_KEY).unwrap_or(bounds::MAX_ARENA_PARTICIPANTS);
        let count: u32 = env.storage().instance().get(&SURVIVOR_COUNT_KEY).unwrap_or(0);
        if count >= capacity {
            return Err(ArenaError::ArenaFull);
        }

        let token: Address = env.storage().instance().get(&TOKEN_KEY).ok_or(ArenaError::TokenNotSet)?;
        token::Client::new(&env, &token).transfer(&player, &env.current_contract_address(), &amount);

        env.storage().persistent().set(&survivor_key, &());
        bump(&env, &survivor_key);
        env.storage().instance().set(&SURVIVOR_COUNT_KEY, &(count + 1));
            
        let mut all_players: Vec<Address> = env.storage().persistent().get(&DataKey::AllPlayers).unwrap_or(Vec::new(&env));
        all_players.push_back(player.clone());
        env.storage().persistent().set(&DataKey::AllPlayers, &all_players);
        bump(&env, &DataKey::AllPlayers);

        let pool: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);
        env.storage().instance().set(&PRIZE_POOL_KEY, &(pool + amount));

        Ok(())
    }

    pub fn cancel_arena(env: Env) -> Result<(), ArenaError> {
        require_not_paused(&env)?;
        let admin = Self::admin(env.clone());
        admin.require_auth();

        if env.storage().instance().get::<_, bool>(&CANCELLED_KEY).unwrap_or(false) {
            return Err(ArenaError::AlreadyCancelled);
        }
        if env.storage().instance().get::<_, bool>(&GAME_FINISHED_KEY).unwrap_or(false) {
            return Err(ArenaError::GameAlreadyFinished);
        }

        let all_players: Vec<Address> = env.storage().persistent().get(&DataKey::AllPlayers).unwrap_or(Vec::new(&env));
        if !all_players.is_empty() {
            let config = get_config(&env)?;
            let token: Address = env.storage().instance().get(&TOKEN_KEY).ok_or(ArenaError::TokenNotSet)?;
            let refund_amount = config.required_stake_amount;
            let token_client = token::Client::new(&env, &token);

            for player in all_players.iter() {
                if env.storage().persistent().has(&DataKey::Survivor(player.clone()))
                    && !env.storage().persistent().has(&DataKey::Refunded(player.clone()))
                {
                    env.storage().persistent().set(&DataKey::Refunded(player.clone()), &());
                    bump(&env, &DataKey::Refunded(player.clone()));
                    token_client.transfer(&env.current_contract_address(), &player, &refund_amount);
                }
            }
            env.storage().instance().set(&PRIZE_POOL_KEY, &0i128);
        }

        env.storage().instance().set(&CANCELLED_KEY, &true);
        env.storage().instance().set(&GAME_FINISHED_KEY, &true);
        set_state(&env, ArenaState::Cancelled);
        env.events().publish((TOPIC_CANCELLED,), (EVENT_VERSION,));

        Ok(())
    }

    pub fn set_max_rounds(env: Env, max_rounds: u32) -> Result<(), ArenaError> {
        let admin = Self::admin(env.clone());
        admin.require_auth();

        if max_rounds < bounds::MIN_MAX_ROUNDS || max_rounds > bounds::MAX_MAX_ROUNDS {
            return Err(ArenaError::InvalidMaxRounds);
        }

        let mut config = get_config(&env)?;
        config.max_rounds = max_rounds;
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    pub fn is_cancelled(env: Env) -> bool {
        env.storage().instance().get::<_, bool>(&CANCELLED_KEY).unwrap_or(false)
    }

    pub fn leave(env: Env, player: Address) -> Result<i128, ArenaError> {
        player.require_auth();
        require_not_paused(&env)?;
        let current_state = get_state(&env);
        assert_state!(current_state, ArenaState::Pending);

        let round = get_round(&env)?;
        if round.round_number != 0 {
            return Err(ArenaError::RoundAlreadyActive);
        }

        let survivor_key = DataKey::Survivor(player.clone());
        if !env.storage().persistent().has(&survivor_key) {
            return Err(ArenaError::NotASurvivor);
        }

        let config = get_config(&env)?;
        let refund = config.required_stake_amount;
        let token: Address = env.storage().instance().get(&TOKEN_KEY).ok_or(ArenaError::TokenNotSet)?;

        env.storage().persistent().remove(&survivor_key);
        let count: u32 = env.storage().instance().get(&SURVIVOR_COUNT_KEY).unwrap_or(0);
        env.storage().instance().set(&SURVIVOR_COUNT_KEY, &count.saturating_sub(1));
            
        let mut all_players: Vec<Address> = env.storage().persistent().get(&DataKey::AllPlayers).unwrap_or(Vec::new(&env));
        if let Some(i) = all_players.first_index_of(&player) {
            all_players.remove(i);
        }
        env.storage().persistent().set(&DataKey::AllPlayers, &all_players);
        bump(&env, &DataKey::AllPlayers);

        let pool: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);
        env.storage().instance().set(&PRIZE_POOL_KEY, &(pool - refund));
        token::Client::new(&env, &token).transfer(&env.current_contract_address(), &player, &refund);
        env.events().publish((TOPIC_LEAVE,), (player, refund));

        Ok(refund)
    }

    pub fn start_round(env: Env) -> Result<RoundState, ArenaError> {
        require_not_paused(&env)?;
        let current_state = get_state(&env);
        assert_state!(current_state, ArenaState::Pending | ArenaState::Active);

        if env.storage().instance().get::<_, bool>(&GAME_FINISHED_KEY).unwrap_or(false) {
            return Err(ArenaError::GameAlreadyFinished);
        }

        let mut round = get_round(&env)?;
        if round.active {
            return Err(ArenaError::RoundAlreadyActive);
        }

        let survivor_count: u32 = env.storage().instance().get(&SURVIVOR_COUNT_KEY).unwrap_or(0);
        if survivor_count < bounds::MIN_ARENA_PARTICIPANTS {
            return Err(ArenaError::NotEnoughPlayers);
        }

        let config = get_config(&env)?;
        let round_start_ledger = env.ledger().sequence();
        let commit_deadline_ledger = round_start_ledger
            .checked_add(config.round_speed_in_ledgers)
            .ok_or(ArenaError::RoundDeadlineOverflow)?;
        let reveal_deadline_ledger = commit_deadline_ledger
            .checked_add(config.round_speed_in_ledgers)
            .ok_or(ArenaError::RoundDeadlineOverflow)?;

        let previous_round_number = round.round_number;
        round = RoundState {
            round_number: previous_round_number + 1,
            round_start_ledger,
            round_deadline_ledger: commit_deadline_ledger,
            reveal_deadline_ledger,
            active: true,
            total_submissions: 0,
            timed_out: false,
            finished: false,
        };

        env.storage().instance().set(&DataKey::Round, &round);

        if round.round_number == 1 {
            set_state(&env, ArenaState::Active);
        }

        env.events().publish(
            (TOPIC_ROUND_STARTED,),
            (round.round_number, round_start_ledger, commit_deadline_ledger, reveal_deadline_ledger, EVENT_VERSION),
        );
        Ok(round)
    }

    pub fn commit_choice(
        env: Env,
        player: Address,
        round_number: u32,
        commitment: BytesN<32>,
    ) -> Result<(), ArenaError> {
        require_not_paused(&env)?;
        player.require_auth();

        let round = get_round(&env)?;
        if !round.active || round.round_number != round_number {
            return Err(ArenaError::WrongRoundNumber);
        }

        if env.ledger().sequence() > round.round_deadline_ledger {
            return Err(ArenaError::CommitDeadlinePassed);
        }

        if !env.storage().persistent().has(&DataKey::Survivor(player.clone())) {
            return Err(ArenaError::NotASurvivor);
        }

        let key = DataKey::Commitment(round_number, player.clone());
        if env.storage().persistent().has(&key) {
            return Err(ArenaError::AlreadyCommitted);
        }

        env.storage().persistent().set(&key, &commitment);
        bump(&env, &key);

        Ok(())
    }

    pub fn reveal_choice(
        env: Env,
        player: Address,
        round_number: u32,
        choice: Choice,
        nonce: BytesN<32>,
    ) -> Result<(), ArenaError> {
        require_not_paused(&env)?;
        player.require_auth();

        let mut round = get_round(&env)?;
        if !round.active || round.round_number != round_number {
            return Err(ArenaError::WrongRoundNumber);
        }

        let seq = env.ledger().sequence();
        if seq <= round.round_deadline_ledger {
            return Err(ArenaError::SubmissionWindowClosed);
        }
        if seq > round.reveal_deadline_ledger {
            return Err(ArenaError::RevealDeadlinePassed);
        }

        let commit_key = DataKey::Commitment(round_number, player.clone());
        let commitment: BytesN<32> = env.storage().persistent()
            .get(&commit_key)
            .ok_or(ArenaError::NoCommitment)?;

        let mut bytes = Bytes::new(&env);
        let choice_byte: u8 = match choice {
            Choice::Heads => 0,
            Choice::Tails => 1,
        };
        bytes.append(&Bytes::from_array(&env, &[choice_byte]));
        bytes.append(&nonce.into());
        bytes.append(&player.clone().to_xdr(&env));
        
        let hash: BytesN<32> = env.crypto().sha256(&bytes).into();
        if hash != commitment {
            return Err(ArenaError::CommitmentMismatch);
        }

        let mut choices = get_round_choices(&env, round_number);
        if choices.contains_key(player.clone()) {
            return Err(ArenaError::SubmissionAlreadyExists);
        }
        choices.set(player.clone(), choice);
        set_round_choices(&env, round_number, &choices);

        round.total_submissions += 1;
        env.storage().instance().set(&DataKey::Round, &round);

        env.events().publish(
            (TOPIC_CHOICE_SUBMITTED,),
            (player, round.round_number, EVENT_VERSION),
        );

        let survivor_count: u32 = env.storage().instance().get(&SURVIVOR_COUNT_KEY).unwrap_or(0);
        if survivor_count > 0 && round.total_submissions == survivor_count {
            round.active = false;
            env.storage().instance().set(&DataKey::Round, &round);
            resolve_round_internal(&env)?;
        }

        Ok(())
    }

    pub fn timeout_round(env: Env) -> Result<RoundState, ArenaError> {
        require_not_paused(&env)?;
        let mut round = get_round(&env)?;
        if !round.active {
            return Err(ArenaError::NoActiveRound);
        }
        if env.ledger().sequence() <= round.reveal_deadline_ledger {
            return Err(ArenaError::RoundStillOpen);
        }
        
        round.active = false;
        round.timed_out = true;
        env.storage().instance().set(&DataKey::Round, &round);

        env.events().publish(
            (TOPIC_ROUND_TIMEOUT,),
            (round.round_number, round.total_submissions, EVENT_VERSION),
        );
        Ok(round)
    }

    pub fn resolve_round(env: Env) -> Result<RoundState, ArenaError> {
        require_not_paused(&env)?;
        let mut round = get_round(&env)?;
        if round.finished {
            return Err(ArenaError::NoActiveRound);
        }
        if round.active {
            if env.ledger().sequence() <= round.reveal_deadline_ledger {
                return Err(ArenaError::RoundStillOpen);
            }
            round.active = false;
            round.timed_out = true;
            env.storage().instance().set(&DataKey::Round, &round);
        }

        resolve_round_internal(&env)
    }

    pub fn claim(env: Env, winner: Address) -> Result<i128, ArenaError> {
        require_not_paused(&env)?;
        let current_state = get_state(&env);
        assert_state!(current_state, ArenaState::Completed);
        winner.require_auth();

        if !env.storage().persistent().has(&DataKey::Survivor(winner.clone())) {
             return Err(ArenaError::NotASurvivor);
        }

        let prize: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);
        if prize <= 0 {
            return Err(ArenaError::NoPrizeToClaim);
        }

        if env.storage().persistent().has(&DataKey::PrizeClaimed(winner.clone())) {
            return Err(ArenaError::AlreadyClaimed);
        }

        env.storage().persistent().set(&DataKey::PrizeClaimed(winner.clone()), &prize);
        bump(&env, &DataKey::PrizeClaimed(winner.clone()));
        env.storage().instance().set(&PRIZE_POOL_KEY, &0i128);
        env.storage().instance().set(&GAME_FINISHED_KEY, &true);
        
        let mut round = get_round(&env)?;
        round.finished = true;
        env.storage().instance().set(&DataKey::Round, &round);

        let token: Address = env.storage().instance().get(&TOKEN_KEY).ok_or(ArenaError::TokenNotSet)?;
        token::Client::new(&env, &token).transfer(&env.current_contract_address(), &winner, &prize);
        
        env.events().publish((TOPIC_CLAIM,), (winner, prize, EVENT_VERSION));
        Ok(prize)
    }

    pub fn get_config(env: Env) -> Result<ArenaConfig, ArenaError> {
        get_config(&env)
    }

    pub fn get_round(env: Env) -> Result<RoundState, ArenaError> {
        get_round(&env)
    }

    pub fn get_choice(env: Env, round_number: u32, player: Address) -> Option<Choice> {
        get_round_choices(&env, round_number).get(player)
    }

    pub fn get_arena_state(env: Env) -> Result<ArenaStateView, ArenaError> {
        let count: u32 = env.storage().instance().get(&SURVIVOR_COUNT_KEY).unwrap_or(0);
        let capacity: u32 = env.storage().instance().get(&CAPACITY_KEY).unwrap_or(0);
        let round = get_round(&env)?;
        let prize: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);

        Ok(ArenaStateView {
            survivors_count: count,
            max_capacity: capacity,
            round_number: round.round_number,
            current_stake: prize,
            potential_payout: prize,
        })
    }

    pub fn get_user_state(env: Env, player: Address) -> UserStateView {
        let is_active = env.storage().persistent().has(&DataKey::Survivor(player.clone()));
        let finished = env.storage().instance().get::<_, bool>(&GAME_FINISHED_KEY).unwrap_or(false);
        let winner = env.storage().persistent().has(&DataKey::Winner(player.clone()));
        UserStateView { is_active, has_won: finished && winner }
    }

    pub fn get_full_state(env: Env, player: Address) -> Result<FullStateView, ArenaError> {
        let arena = Self::get_arena_state(env.clone())?;
        let user = Self::get_user_state(env, player);
        Ok(FullStateView {
            survivors_count: arena.survivors_count,
            max_capacity: arena.max_capacity,
            round_number: arena.round_number,
            current_stake: arena.current_stake,
            potential_payout: arena.potential_payout,
            is_active: user.is_active,
            has_won: user.has_won,
        })
    }

    pub fn set_metadata(
        env: Env,
        arena_id: u64,
        name: String,
        description: Option<String>,
        host: Address,
    ) -> Result<(), ArenaError> {
        let admin = Self::admin(env.clone());
        admin.require_auth();

        if name.len() == 0 {
            return Err(ArenaError::NameEmpty);
        }
        if name.len() > 64 {
            return Err(ArenaError::NameTooLong);
        }
        if let Some(ref desc) = description {
            if desc.len() > 256 {
                return Err(ArenaError::DescriptionTooLong);
            }
        }

        let metadata = ArenaMetadata {
            arena_id,
            name,
            description,
            host,
            created_at: env.ledger().timestamp(),
        };

        env.storage().persistent().set(&DataKey::Metadata(arena_id), &metadata);
        bump(&env, &DataKey::Metadata(arena_id));

        // Store the single ArenaId to use for factory notifications
        env.storage().instance().set(&DataKey::ArenaId, &arena_id);

        Ok(())
    }

    pub fn get_metadata(env: Env, arena_id: u64) -> Option<ArenaMetadata> {
        env.storage().persistent().get(&DataKey::Metadata(arena_id))
    }

    pub fn propose_upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), ArenaError> {
        let admin = Self::admin(env.clone());
        admin.require_auth();
        if env.storage().instance().has(&DataKey::UpgradeHash) {
            return Err(ArenaError::UpgradeAlreadyPending);
        }
        let execute_after: u64 = env.ledger().timestamp() + TIMELOCK_PERIOD;
        env.storage().instance().set(&DataKey::UpgradeHash, &new_wasm_hash);
        env.storage().instance().set(&DataKey::UpgradeTimestamp, &execute_after);
        env.events().publish((TOPIC_UPGRADE_PROPOSED,), (EVENT_VERSION, new_wasm_hash, execute_after));
        Ok(())
    }

    pub fn execute_upgrade(env: Env) -> Result<(), ArenaError> {
        let admin = Self::admin(env.clone());
        admin.require_auth();
        let execute_after: u64 = env.storage().instance().get(&DataKey::UpgradeTimestamp).ok_or(ArenaError::NoPendingUpgrade)?;
        if env.ledger().timestamp() < execute_after {
            return Err(ArenaError::TimelockNotExpired);
        }
        let new_wasm_hash: BytesN<32> = env.storage().instance().get(&DataKey::UpgradeHash).ok_or(ArenaError::NoPendingUpgrade)?;
        env.storage().instance().remove(&DataKey::UpgradeHash);
        env.storage().instance().remove(&DataKey::UpgradeTimestamp);
        env.events().publish((TOPIC_UPGRADE_EXECUTED,), (EVENT_VERSION, new_wasm_hash.clone()));
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    pub fn cancel_upgrade(env: Env) -> Result<(), ArenaError> {
        let admin = Self::admin(env.clone());
        admin.require_auth();
        if !env.storage().instance().has(&DataKey::UpgradeHash) {
            return Err(ArenaError::NoPendingUpgrade);
        }
        env.storage().instance().remove(&DataKey::UpgradeHash);
        env.storage().instance().remove(&DataKey::UpgradeTimestamp);
        env.events().publish((TOPIC_UPGRADE_CANCELLED,), (EVENT_VERSION,));
        Ok(())
    }

    pub fn pending_upgrade(env: Env) -> Option<(BytesN<32>, u64)> {
        let hash: Option<BytesN<32>> = env.storage().instance().get(&DataKey::UpgradeHash);
        let after: Option<u64> = env.storage().instance().get(&DataKey::UpgradeTimestamp);
        match (hash, after) {
            (Some(h), Some(a)) => Some((h, a)),
            _ => None,
        }
    }

    pub fn state(env: Env) -> ArenaState {
        get_state(&env)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn resolve_round_internal(env: &Env) -> Result<RoundState, ArenaError> {
    let mut round = get_round(env)?;
    let config = get_config(env)?;

    // Handle max rounds forced draw
    if round.round_number > 0 && round.round_number >= config.max_rounds {
        return resolve_max_rounds_draw(env, &mut round);
    }

    let choices = get_round_choices(env, round.round_number);
    let mut heads_count = 0u32;
    let mut tails_count = 0u32;
    let mut heads_players = Vec::new(env);
    let mut tails_players = Vec::new(env);

    for (player, choice) in choices.iter() {
        match choice {
            Choice::Heads => {
                heads_count += 1;
                heads_players.push_back(player);
            }
            Choice::Tails => {
                tails_count += 1;
                tails_players.push_back(player);
            }
        }
    }

    let surviving_choice = choose_surviving_side(env, heads_count, tails_count);
    
    // Players who didn't reveal or chose the losing side are eliminated.
    // We iterate over all current survivors.
    let all_players: Vec<Address> = env.storage().persistent().get(&DataKey::AllPlayers).unwrap_or(Vec::new(env));
    let mut eliminated_count = 0u32;
    
    for player in all_players.iter() {
        let survivor_key = DataKey::Survivor(player.clone());
        if env.storage().persistent().has(&survivor_key) {
            let revealed_choice = choices.get(player.clone());
            let survives = match (revealed_choice, &surviving_choice) {
                (Some(c), Some(s)) => c == *s,
                _ => false, // Didn't reveal or no surviving side chosen
            };

            if !survives {
                env.storage().persistent().remove(&survivor_key);
                let eliminated_key = DataKey::Eliminated(player.clone());
                env.storage().persistent().set(&eliminated_key, &true);
                bump(env, &eliminated_key);
                eliminated_count += 1;
            }
        }
    }

    let survivor_count: u32 = env.storage().instance().get(&SURVIVOR_COUNT_KEY).unwrap_or(0);
    let updated_survivor_count = survivor_count.saturating_sub(eliminated_count);
    env.storage().instance().set(&SURVIVOR_COUNT_KEY, &updated_survivor_count);
    
    if updated_survivor_count <= 1 {
        env.storage().instance().set(&GAME_FINISHED_KEY, &true);
        set_state(env, ArenaState::Completed);
    }

    round.finished = true;
    env.storage().instance().set(&DataKey::Round, &round);

    env.events().publish(
        (TOPIC_ROUND_RESOLVED,),
        (
            round.round_number,
            heads_count,
            tails_count,
            outcome_symbol(&surviving_choice),
            eliminated_count,
            updated_survivor_count,
            EVENT_VERSION,
        ),
    );

    Ok(round)
}

fn resolve_max_rounds_draw(env: &Env, round: &mut RoundState) -> Result<RoundState, ArenaError> {
    let all_players: Vec<Address> = env.storage().persistent().get(&DataKey::AllPlayers).unwrap_or(Vec::new(env));
    let mut survivors = Vec::new(env);
    for p in all_players.iter() {
        if env.storage().persistent().has(&DataKey::Survivor(p.clone())) {
            survivors.push_back(p);
        }
    }

    let prize: i128 = env.storage().instance().get(&PRIZE_POOL_KEY).unwrap_or(0);
    if !survivors.is_empty() && prize > 0 {
        let token: Address = env.storage().instance().get(&TOKEN_KEY).ok_or(ArenaError::TokenNotSet)?;
        let share = prize / (survivors.len() as i128);
        let dust = prize % (survivors.len() as i128);
        let token_client = token::Client::new(env, &token);

        for s in survivors.iter() {
            token_client.transfer(&env.current_contract_address(), &s, &share);
        }
        if dust > 0 {
            token_client.transfer(&env.current_contract_address(), &survivors.get(0).unwrap(), &dust);
        }
        env.storage().instance().set(&PRIZE_POOL_KEY, &0i128);
    }

    env.storage().instance().set(&GAME_FINISHED_KEY, &true);
    round.finished = true;
    env.storage().instance().set(&DataKey::Round, round);
    set_state(env, ArenaState::Completed);
    Ok(round.clone())
}

fn get_config(env: &Env) -> Result<ArenaConfig, ArenaError> {
    env.storage().instance().get(&DataKey::Config).ok_or(ArenaError::NotInitialized)
}

fn get_round(env: &Env) -> Result<RoundState, ArenaError> {
    env.storage().instance().get(&DataKey::Round).ok_or(ArenaError::NotInitialized)
}

fn get_round_choices(env: &Env, round: u32) -> soroban_sdk::Map<Address, Choice> {
    env.storage().persistent().get(&DataKey::RoundChoices(round)).unwrap_or(soroban_sdk::Map::new(env))
}

fn set_round_choices(env: &Env, round: u32, choices: &soroban_sdk::Map<Address, Choice>) {
    let key = DataKey::RoundChoices(round);
    env.storage().persistent().set(&key, choices);
    bump(env, &key);
}

fn choose_surviving_side(env: &Env, heads_count: u32, tails_count: u32) -> Option<Choice> {
    match (heads_count, tails_count) {
        (0, 0) => None,
        (0, _) => Some(Choice::Tails),
        (_, 0) => Some(Choice::Heads),
        _ if heads_count == tails_count => {
            if (env.prng().r#gen::<u64>() & 1) == 0 {
                Some(Choice::Heads)
            } else {
                Some(Choice::Tails)
            }
        }
        _ if heads_count < tails_count => Some(Choice::Heads),
        _ => Some(Choice::Tails),
    }
}

fn outcome_symbol(outcome: &Option<Choice>) -> Symbol {
    match outcome {
        Some(Choice::Heads) => symbol_short!("HEADS"),
        Some(Choice::Tails) => symbol_short!("TAILS"),
        None => symbol_short!("NONE"),
    }
}

fn get_state(env: &Env) -> ArenaState {
    env.storage().instance().get(&DataKey::State).unwrap_or(ArenaState::Pending)
}

fn set_state(env: &Env, new_state: ArenaState) {
    let old_state = get_state(env);
    if old_state == new_state { return; }
    env.storage().instance().set(&DataKey::State, &new_state);
    env.events().publish((TOPIC_STATE_CHANGED,), ArenaStateChanged { old_state, new_state });

    // Notify the factory if it is linked
    if let (Some(factory), Some(arena_id)) = (
        env.storage().instance().get::<_, Address>(&DataKey::FactoryAddress),
        env.storage().instance().get::<_, u64>(&DataKey::ArenaId)
    ) {
        // The enum variants match 1:1 between ArenaState and Factory's ArenaStatus
        env.invoke_contract::<()>(
            &factory,
            &soroban_sdk::Symbol::new(env, "update_arena_status"),
            soroban_sdk::vec![env, arena_id.into_val(env), new_state.into_val(env)],
        );
    }
}

fn bump(env: &Env, key: &DataKey) {
    match key {
        DataKey::Survivor(_) | DataKey::Eliminated(_) | DataKey::Commitment(_, _) | 
        DataKey::RoundChoices(_) | DataKey::Metadata(_) | DataKey::PrizeClaimed(_) |
        DataKey::Winner(_) | DataKey::Refunded(_) | DataKey::AllPlayers => {
            env.storage().persistent().extend_ttl(key, GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);
        }
        _ => {
            env.storage().instance().extend_ttl(GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);
        }
    }
}

fn require_not_paused(env: &Env) -> Result<(), ArenaError> {
    if env.storage().instance().get::<_, bool>(&PAUSED_KEY).unwrap_or(false) {
        return Err(ArenaError::Paused);
    }
    Ok(())
}

#[cfg(test)]
mod abi_guard;
// #[cfg(test)]
// mod auto_advance_tests;
// #[cfg(all(test, feature = "integration-tests"))]
// mod integration_tests;
// #[cfg(test)]
// mod metadata_tests;
// #[cfg(test)]
// mod state_machine_tests;
// #[cfg(test)]
// mod submit_choice_tests;
#[cfg(test)]
mod commit_reveal_tests;
// #[cfg(test)]
// mod test;
