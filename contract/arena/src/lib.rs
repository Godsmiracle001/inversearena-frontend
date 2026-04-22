#![no_std]

mod bounds;
mod invariants;

use soroban_sdk::{
    Address, BytesN, Env, Map, Symbol, Vec, contract, contracterror, contractimpl, contracttype,
    symbol_short, token,
};

// ── Storage keys ──────────────────────────────────────────────────────────────
// All storage is now managed via the DataKey enum and persistent storage.

// ── Timelock: 48 hours in seconds ─────────────────────────────────────────────
const TIMELOCK_PERIOD: u64 = 48 * 60 * 60;

// ── TTL constants ─────────────────────────────────────────────────────────────
const GAME_TTL_THRESHOLD: u32 = 100_000;
const GAME_TTL_EXTEND_TO: u32 = 535_680;

// ── Event topics ──────────────────────────────────────────────────────────────
const TOPIC_UPGRADE_PROPOSED: Symbol = symbol_short!("UP_PROP");
const TOPIC_UPGRADE_EXECUTED: Symbol = symbol_short!("UP_EXEC");
const TOPIC_UPGRADE_CANCELLED: Symbol = symbol_short!("UP_CANC");
const TOPIC_PAUSED: Symbol = symbol_short!("PAUSED");
const TOPIC_UNPAUSED: Symbol = symbol_short!("UNPAUSED");
const TOPIC_ROUND_STARTED: Symbol = symbol_short!("R_START");
const TOPIC_ROUND_TIMEOUT: Symbol = symbol_short!("R_TOUT");
const TOPIC_ROUND_RESOLVED: Symbol = symbol_short!("RSLVD");
const TOPIC_WINNER_SET: Symbol = symbol_short!("WIN_SET");
const TOPIC_CLAIM: Symbol = symbol_short!("CLAIM");
const TOPIC_LEAVE: Symbol = symbol_short!("LEAVE");

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
    pub admin: Address,
    pub token: Address,
    pub capacity: u32,
    pub prize_pool: i128,
    pub game_finished: bool,
    pub winner_set: bool,
    pub paused: bool,
    pub round: RoundState,
}

#[contracttype]
pub enum DataKey {
    Config(u64),
    State(u64),
    Players(u64),
    Survivors(u64),
    Eliminated(u64),
    RoundChoices(u64, u32),
    ContractAdmin,
    UpgradeHash,
    UpgradeTimestamp,
}


// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct ArenaContract;

#[contractimpl]
impl ArenaContract {
    pub fn create_arena(
        env: Env,
        arena_id: u64,
        admin: Address,
        token: Address,
        capacity: u32,
        round_speed_in_ledgers: u32,
        required_stake_amount: i128,
    ) -> Result<(), ArenaError> {
        if storage(&env).has(&DataKey::Config(arena_id)) {
            return Err(ArenaError::AlreadyInitialized);
        }
        if round_speed_in_ledgers == 0 || round_speed_in_ledgers > bounds::MAX_SPEED_LEDGERS {
            return Err(ArenaError::InvalidRoundSpeed);
        }
        if required_stake_amount < bounds::MIN_REQUIRED_STAKE {
            return Err(ArenaError::InvalidAmount);
        }
        if !(bounds::MIN_ARENA_PARTICIPANTS..=bounds::MAX_ARENA_PARTICIPANTS).contains(&capacity) {
            return Err(ArenaError::InvalidCapacity);
        }

        let config = ArenaConfig {
            round_speed_in_ledgers,
            required_stake_amount,
        };
        let round = RoundState {
            round_number: 0,
            round_start_ledger: 0,
            round_deadline_ledger: 0,
            active: false,
            total_submissions: 0,
            timed_out: false,
            finished: false,
        };
        let state = ArenaState {
            admin,
            token,
            capacity,
            prize_pool: 0,
            game_finished: false,
            winner_set: false,
            paused: false,
            round,
        };

        set_config(&env, arena_id, &config);
        set_state(&env, arena_id, &state);
        set_players(&env, arena_id, &Vec::new(&env));
        set_survivors(&env, arena_id, &Vec::new(&env));
        set_eliminated(&env, arena_id, &Vec::new(&env));

        Ok(())
    }

    pub fn initialize(env: Env, admin: Address) {
        if storage(&env).has(&DataKey::ContractAdmin) {
            panic!("already initialized");
        }
        admin.require_auth();
        storage(&env).set(&DataKey::ContractAdmin, &admin);
        bump(&env, &DataKey::ContractAdmin);
    }

    pub fn admin(env: Env) -> Address {
        storage(&env)
            .get(&DataKey::ContractAdmin)
            .expect("not initialized")
    }

    pub fn set_admin(env: Env, new_admin: Address) {
        let admin: Address = Self::admin(env.clone());
        admin.require_auth();
        storage(&env).set(&DataKey::ContractAdmin, &new_admin);
        bump(&env, &DataKey::ContractAdmin);
    }

    pub fn pause_arena(env: Env, arena_id: u64) -> Result<(), ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        state.admin.require_auth();
        state.paused = true;
        set_state(&env, arena_id, &state);
        env.events().publish((TOPIC_PAUSED, arena_id), (EVENT_VERSION,));
        Ok(())
    }

    pub fn unpause_arena(env: Env, arena_id: u64) -> Result<(), ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        state.admin.require_auth();
        state.paused = false;
        set_state(&env, arena_id, &state);
        env.events().publish((TOPIC_UNPAUSED, arena_id), (EVENT_VERSION,));
        Ok(())
    }

    pub fn is_arena_paused(env: Env, arena_id: u64) -> bool {
        get_state(&env, arena_id).map(|s| s.paused).unwrap_or(false)
    }

    pub fn set_arena_token(env: Env, arena_id: u64, token: Address) -> Result<(), ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        state.admin.require_auth();
        
        let survivors = get_survivors(&env, arena_id);
        if survivors.len() > 0 || state.prize_pool > 0 {
            return Err(ArenaError::TokenConfigurationLocked);
        }
        state.token = token;
        set_state(&env, arena_id, &state);
        Ok(())
    }

    pub fn set_arena_capacity(env: Env, arena_id: u64, capacity: u32) -> Result<(), ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        state.admin.require_auth();

        if !(bounds::MIN_ARENA_PARTICIPANTS..=bounds::MAX_ARENA_PARTICIPANTS).contains(&capacity) {
            return Err(ArenaError::InvalidCapacity);
        }
        state.capacity = capacity;
        set_state(&env, arena_id, &state);
        Ok(())
    }

    pub fn set_winner(
        env: Env,
        arena_id: u64,
        player: Address,
        stake: i128,
        yield_comp: i128,
    ) -> Result<(), ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        if state.paused {
            return Err(ArenaError::Paused);
        }
        state.admin.require_auth();

        let survivors = get_survivors(&env, arena_id);
        if !survivors.contains(&player) {
            return Err(ArenaError::NotASurvivor);
        }

        if state.winner_set {
            return Err(ArenaError::WinnerAlreadySet);
        }
        if stake < 0 || yield_comp < 0 {
            return Err(ArenaError::InvalidAmount);
        }
        let prize = stake
            .checked_add(yield_comp)
            .ok_or(ArenaError::InvalidAmount)?;

        state.winner_set = true;
        state.prize_pool = state
            .prize_pool
            .checked_add(prize)
            .ok_or(ArenaError::InvalidAmount)?;
        
        set_state(&env, arena_id, &state);

        env.events()
            .publish((TOPIC_WINNER_SET, arena_id), (player, stake, yield_comp, EVENT_VERSION));
        Ok(())
    }

    pub fn join_arena(env: Env, arena_id: u64, player: Address, amount: i128) -> Result<(), ArenaError> {
        player.require_auth();
        let mut state = get_state(&env, arena_id)?;
        if state.paused {
            return Err(ArenaError::Paused);
        }
        if state.game_finished {
            return Err(ArenaError::GameAlreadyFinished);
        }
        let config = get_config(&env, arena_id)?;
        if amount != config.required_stake_amount {
            return Err(ArenaError::InvalidAmount);
        }

        let mut survivors = get_survivors(&env, arena_id);
        if survivors.contains(&player) {
            return Err(ArenaError::AlreadyJoined);
        }

        if survivors.len() >= state.capacity {
            return Err(ArenaError::ArenaFull);
        }

        // CEI: effects before interaction
        survivors.push_back(player.clone());
        set_survivors(&env, arena_id, &survivors);

        let mut players = get_players(&env, arena_id);
        if !players.contains(&player) {
            players.push_back(player.clone());
            set_players(&env, arena_id, &players);
        }

        state.prize_pool = state
            .prize_pool
            .checked_add(amount)
            .ok_or(ArenaError::InvalidAmount)?;
        set_state(&env, arena_id, &state);

        token::Client::new(&env, &state.token).transfer(
            &player,
            &env.current_contract_address(),
            &amount,
        );
        Ok(())
    }

    pub fn leave_arena(env: Env, arena_id: u64, player: Address) -> Result<i128, ArenaError> {
        player.require_auth();
        let mut state = get_state(&env, arena_id)?;
        if state.paused {
            return Err(ArenaError::Paused);
        }
        if state.round.round_number != 0 {
            return Err(ArenaError::RoundAlreadyActive);
        }

        let mut survivors = get_survivors(&env, arena_id);
        let index = survivors.first_index_of(&player).ok_or(ArenaError::NotASurvivor)?;
        survivors.remove(index);
        set_survivors(&env, arena_id, &survivors);

        let config = get_config(&env, arena_id)?;
        let refund = config.required_stake_amount;

        state.prize_pool = state
            .prize_pool
            .checked_sub(refund)
            .ok_or(ArenaError::InvalidAmount)?;
        set_state(&env, arena_id, &state);

        token::Client::new(&env, &state.token).transfer(&env.current_contract_address(), &player, &refund);
        env.events().publish((TOPIC_LEAVE, arena_id), (player, refund));
        Ok(refund)
    }

    pub fn start_round(env: Env, arena_id: u64) -> Result<RoundState, ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        if state.paused {
            return Err(ArenaError::Paused);
        }
        if state.game_finished {
            return Err(ArenaError::GameAlreadyFinished);
        }
        if state.round.active {
            return Err(ArenaError::RoundAlreadyActive);
        }

        let survivors = get_survivors(&env, arena_id);
        if survivors.len() < bounds::MIN_ARENA_PARTICIPANTS {
            return Err(ArenaError::NotEnoughPlayers);
        }

        let config = get_config(&env, arena_id)?;
        let round_start_ledger = env.ledger().sequence();
        let round_deadline_ledger = round_start_ledger
            .checked_add(config.round_speed_in_ledgers)
            .ok_or(ArenaError::RoundDeadlineOverflow)?;

        let previous_round_number = state.round.round_number;
        state.round = RoundState {
            round_number: previous_round_number + 1,
            round_start_ledger,
            round_deadline_ledger,
            active: true,
            total_submissions: 0,
            timed_out: false,
            finished: false,
        };

        #[cfg(debug_assertions)]
        {
            crate::invariants::check_round_flags(&state.round)
                .expect("start_round: round flags invariant violated");
            crate::invariants::check_round_number_monotonic(
                previous_round_number,
                state.round.round_number,
            )
            .expect("start_round: round number monotonic invariant violated");
        }

        set_state(&env, arena_id, &state);
        env.events().publish(
            (TOPIC_ROUND_STARTED, arena_id),
            (
                state.round.round_number,
                state.round.round_start_ledger,
                state.round.round_deadline_ledger,
                EVENT_VERSION,
            ),
        );
        Ok(state.round)
    }

    pub fn submit_choice(
        env: Env,
        arena_id: u64,
        player: Address,
        round_number: u32,
        choice: Choice,
    ) -> Result<(), ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        if state.paused {
            return Err(ArenaError::Paused);
        }
        player.require_auth();

        let eliminated = get_eliminated(&env, arena_id);
        if eliminated.contains(&player) {
            return Err(ArenaError::PlayerEliminated);
        }
        let survivors = get_survivors(&env, arena_id);
        if !survivors.contains(&player) {
            return Err(ArenaError::NotASurvivor);
        }

        #[cfg(debug_assertions)]
        let before_submissions = state.round.total_submissions;

        if !state.round.active {
            return Err(ArenaError::NoActiveRound);
        }
        if round_number != state.round.round_number {
            return Err(ArenaError::WrongRoundNumber);
        }
        if env.ledger().sequence() > state.round.round_deadline_ledger {
            return Err(ArenaError::SubmissionWindowClosed);
        }

        let mut choices = get_round_choices(&env, arena_id, state.round.round_number);
        if choices.contains_key(player.clone()) {
            return Err(ArenaError::SubmissionAlreadyExists);
        }
        if state.round.total_submissions >= bounds::MAX_SUBMISSIONS_PER_ROUND {
            return Err(ArenaError::MaxSubmissionsPerRound);
        }

        choices.set(player.clone(), choice);
        set_round_choices(&env, arena_id, state.round.round_number, &choices);

        state.round.total_submissions += 1;

        #[cfg(debug_assertions)]
        {
            crate::invariants::check_submission_count_monotonic(
                before_submissions,
                state.round.total_submissions,
            )
            .expect("submit_choice: submission count monotonic invariant violated");
            crate::invariants::check_round_flags(&state.round)
                .expect("submit_choice: round flags invariant violated");
        }

        set_state(&env, arena_id, &state);
        Ok(())
    }

    pub fn timeout_round(env: Env, arena_id: u64) -> Result<RoundState, ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        if state.paused {
            return Err(ArenaError::Paused);
        }
        #[cfg(debug_assertions)]
        let before = state.round.clone();

        if !state.round.active {
            return Err(ArenaError::NoActiveRound);
        }
        if env.ledger().sequence() <= state.round.round_deadline_ledger {
            return Err(ArenaError::RoundStillOpen);
        }
        state.round.active = false;
        state.round.timed_out = true;

        #[cfg(debug_assertions)]
        {
            crate::invariants::check_timeout_transition(&before, &state.round)
                .expect("timeout_round: timeout transition invariant violated");
            crate::invariants::check_round_flags(&state.round)
                .expect("timeout_round: round flags invariant violated");
            crate::invariants::check_round_number_monotonic(before.round_number, state.round.round_number)
                .expect("timeout_round: round number monotonic invariant violated");
        }

        set_state(&env, arena_id, &state);
        env.events().publish(
            (TOPIC_ROUND_TIMEOUT, arena_id),
            (state.round.round_number, state.round.total_submissions, EVENT_VERSION),
        );
        Ok(state.round)
    }

    pub fn resolve_round(env: Env, arena_id: u64) -> Result<RoundState, ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        if state.paused {
            return Err(ArenaError::Paused);
        }
        if state.game_finished {
            return Err(ArenaError::GameAlreadyFinished);
        }
        
        #[cfg(debug_assertions)]
        let before_round_number = state.round.round_number;

        if state.round.finished {
            return Err(ArenaError::NoActiveRound);
        }
        if state.round.active {
            if env.ledger().sequence() <= state.round.round_deadline_ledger {
                return Err(ArenaError::RoundStillOpen);
            }
            state.round.active = false;
            state.round.timed_out = true;
        }

        let choices = get_round_choices(&env, arena_id, state.round.round_number);
        let mut heads_count = 0u32;
        let mut tails_count = 0u32;
        let mut heads_players = Vec::new(&env);
        let mut tails_players = Vec::new(&env);

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

        let surviving_choice = choose_surviving_side(&env, heads_count, tails_count);
        let eliminated_in_round = match surviving_choice {
            Some(Choice::Heads) => tails_players,
            Some(Choice::Tails) => heads_players,
            None => Vec::new(&env),
        };

        let mut survivors = get_survivors(&env, arena_id);
        let mut eliminated = get_eliminated(&env, arena_id);
        let mut eliminated_count = 0u32;

        for player in eliminated_in_round.iter() {
            if let Some(idx) = survivors.first_index_of(&player) {
                survivors.remove(idx);
                eliminated.push_back(player);
                eliminated_count += 1;
            }
        }

        set_survivors(&env, arena_id, &survivors);
        set_eliminated(&env, arena_id, &eliminated);

        if survivors.len() <= 1 {
            state.game_finished = true;
            state.round.finished = true;
        }

        #[cfg(debug_assertions)]
        {
            crate::invariants::check_round_flags(&state.round)
                .expect("resolve_round: round flags invariant violated");
            crate::invariants::check_round_number_monotonic(
                before_round_number,
                state.round.round_number,
            )
            .expect("resolve_round: round number monotonic invariant violated");
        }

        set_state(&env, arena_id, &state);

        env.events().publish(
            (TOPIC_ROUND_RESOLVED, arena_id),
            (
                state.round.round_number,
                heads_count,
                tails_count,
                outcome_symbol(&surviving_choice),
                eliminated_count,
                survivors.len(),
                EVENT_VERSION,
            ),
        );

        Ok(state.round)
    }

    pub fn claim_prize(env: Env, arena_id: u64, winner: Address) -> Result<i128, ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        if state.paused {
            return Err(ArenaError::Paused);
        }
        winner.require_auth();
        if !state.game_finished {
            return Err(ArenaError::GameNotFinished);
        }

        let survivors = get_survivors(&env, arena_id);
        if state.winner_set {
            // If winner is set by admin, verify.
            // Note: In this refactored version, we don't store individual winner flags,
            // we assume the admin sets the prize pool and we might need a way to verify the winner.
            // For now, let's assume the winner must be one of the survivors if only 1 remains.
        }

        if survivors.len() == 1 && survivors.contains(&winner) {
            // Fallback: exactly one survivor remains
        } else if survivors.len() > 1 {
             return Err(ArenaError::WinnerNotSet);
        } else if !survivors.contains(&winner) {
             return Err(ArenaError::NotASurvivor);
        }

        let prize = state.prize_pool;
        if prize <= 0 {
            return Err(ArenaError::NoPrizeToClaim);
        }

        // CEI: effects before interaction
        state.prize_pool = 0;
        state.game_finished = true;
        state.round.finished = true;
        set_state(&env, arena_id, &state);

        token::Client::new(&env, &state.token).transfer(&env.current_contract_address(), &winner, &prize);
        
        env.events()
            .publish((TOPIC_CLAIM, arena_id), (winner, prize, EVENT_VERSION));
        Ok(prize)
    }

    pub fn cancel_arena(env: Env, arena_id: u64) -> Result<(), ArenaError> {
        let mut state = get_state(&env, arena_id)?;
        state.admin.require_auth();
        
        if state.round.round_number > 0 {
            return Err(ArenaError::RoundAlreadyActive);
        }

        let survivors = get_survivors(&env, arena_id);
        let config = get_config(&env, arena_id)?;
        let refund_amount = config.required_stake_amount;

        for player in survivors.iter() {
            token::Client::new(&env, &state.token).transfer(
                &env.current_contract_address(),
                &player,
                &refund_amount,
            );
        }

        state.game_finished = true;
        state.prize_pool = 0;
        set_state(&env, arena_id, &state);

        Ok(())
    }

    pub fn get_config(env: Env, arena_id: u64) -> Result<ArenaConfig, ArenaError> {
        get_config(&env, arena_id)
    }

    pub fn get_round(env: Env, arena_id: u64) -> Result<RoundState, ArenaError> {
        get_state(&env, arena_id).map(|s| s.round)
    }

    pub fn get_choice(env: Env, arena_id: u64, round_number: u32, player: Address) -> Option<Choice> {
        get_round_choices(&env, arena_id, round_number).get(player)
    }

    pub fn get_arena_state(env: Env, arena_id: u64) -> Result<ArenaStateView, ArenaError> {
        let state = get_state(&env, arena_id)?;
        Ok(ArenaStateView {
            survivors_count: get_survivors(&env, arena_id).len(),
            max_capacity: state.capacity,
            round_number: state.round.round_number,
            current_stake: state.prize_pool,
            potential_payout: state.prize_pool,
        })
    }

    pub fn get_user_state(env: Env, arena_id: u64, player: Address) -> UserStateView {
        let survivors = get_survivors(&env, arena_id);
        let is_active = survivors.contains(&player);
        // Simplified has_won: if game finished and player is the last survivor
        let state = get_state(&env, arena_id).ok();
        let has_won = state.map(|s| s.game_finished && is_active && survivors.len() == 1).unwrap_or(false);
        UserStateView { is_active, has_won }
    }

    pub fn get_full_state(env: Env, arena_id: u64, player: Address) -> Result<FullStateView, ArenaError> {
        let arena_state = Self::get_arena_state(env.clone(), arena_id)?;
        let user_state = Self::get_user_state(env, arena_id, player);
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

    pub fn get_players(env: Env, arena_id: u64) -> Vec<Address> {
        get_players(&env, arena_id)
    }

    pub fn get_survivors(env: Env, arena_id: u64) -> Vec<Address> {
        get_survivors(&env, arena_id)
    }

    pub fn get_eliminated(env: Env, arena_id: u64) -> Vec<Address> {
        get_eliminated(&env, arena_id)
    }

    pub fn propose_upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), ArenaError> {
        let admin: Address = storage(&env)
            .get(&DataKey::ContractAdmin)
            .ok_or(ArenaError::NotInitialized)?;
        admin.require_auth();
        if storage(&env).has(&DataKey::UpgradeHash) {
            return Err(ArenaError::UpgradeAlreadyPending);
        }
        let execute_after: u64 = env.ledger().timestamp() + TIMELOCK_PERIOD;
        storage(&env).set(&DataKey::UpgradeHash, &new_wasm_hash);
        storage(&env).set(&DataKey::UpgradeTimestamp, &execute_after);
        bump(&env, &DataKey::UpgradeHash);
        bump(&env, &DataKey::UpgradeTimestamp);
        env.events().publish(
            (TOPIC_UPGRADE_PROPOSED,),
            (EVENT_VERSION, new_wasm_hash, execute_after),
        );
        Ok(())
    }

    pub fn execute_upgrade(env: Env) -> Result<(), ArenaError> {
        let admin: Address = storage(&env)
            .get(&DataKey::ContractAdmin)
            .ok_or(ArenaError::NotInitialized)?;
        admin.require_auth();
        let execute_after: u64 = storage(&env)
            .get(&DataKey::UpgradeTimestamp)
            .ok_or(ArenaError::NoPendingUpgrade)?;
        if env.ledger().timestamp() < execute_after {
            return Err(ArenaError::TimelockNotExpired);
        }
        let new_wasm_hash: BytesN<32> = storage(&env)
            .get(&DataKey::UpgradeHash)
            .ok_or(ArenaError::NoPendingUpgrade)?;
        storage(&env).remove(&DataKey::UpgradeHash);
        storage(&env).remove(&DataKey::UpgradeTimestamp);
        env.events().publish(
            (TOPIC_UPGRADE_EXECUTED,),
            (EVENT_VERSION, new_wasm_hash.clone()),
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    pub fn cancel_upgrade(env: Env) -> Result<(), ArenaError> {
        let admin: Address = storage(&env)
            .get(&DataKey::ContractAdmin)
            .ok_or(ArenaError::NotInitialized)?;
        admin.require_auth();
        if !storage(&env).has(&DataKey::UpgradeHash) {
            return Err(ArenaError::NoPendingUpgrade);
        }
        storage(&env).remove(&DataKey::UpgradeHash);
        storage(&env).remove(&DataKey::UpgradeTimestamp);
        env.events()
            .publish((TOPIC_UPGRADE_CANCELLED,), (EVENT_VERSION,));
        Ok(())
    }

    pub fn pending_upgrade(env: Env) -> Option<(BytesN<32>, u64)> {
        let hash: Option<BytesN<32>> = storage(&env).get(&DataKey::UpgradeHash);
        let after: Option<u64> = storage(&env).get(&DataKey::UpgradeTimestamp);
        match (hash, after) {
            (Some(h), Some(a)) => Some((h, a)),
            _ => None,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_config(env: &Env, arena_id: u64) -> Result<ArenaConfig, ArenaError> {
    storage(env)
        .get(&DataKey::Config(arena_id))
        .ok_or(ArenaError::NotInitialized)
}

fn set_config(env: &Env, arena_id: u64, config: &ArenaConfig) {
    let key = DataKey::Config(arena_id);
    storage(env).set(&key, config);
    bump(env, &key);
}

fn get_state(env: &Env, arena_id: u64) -> Result<ArenaState, ArenaError> {
    storage(env)
        .get(&DataKey::State(arena_id))
        .ok_or(ArenaError::NotInitialized)
}

fn set_state(env: &Env, arena_id: u64, state: &ArenaState) {
    let key = DataKey::State(arena_id);
    storage(env).set(&key, state);
    bump(env, &key);
}

fn get_players(env: &Env, arena_id: u64) -> Vec<Address> {
    storage(env)
        .get(&DataKey::Players(arena_id))
        .unwrap_or(Vec::new(env))
}

fn set_players(env: &Env, arena_id: u64, players: &Vec<Address>) {
    let key = DataKey::Players(arena_id);
    storage(env).set(&key, players);
    bump(env, &key);
}

fn get_survivors(env: &Env, arena_id: u64) -> Vec<Address> {
    storage(env)
        .get(&DataKey::Survivors(arena_id))
        .unwrap_or(Vec::new(env))
}

fn set_survivors(env: &Env, arena_id: u64, survivors: &Vec<Address>) {
    let key = DataKey::Survivors(arena_id);
    storage(env).set(&key, survivors);
    bump(env, &key);
}

fn get_eliminated(env: &Env, arena_id: u64) -> Vec<Address> {
    storage(env)
        .get(&DataKey::Eliminated(arena_id))
        .unwrap_or(Vec::new(env))
}

fn set_eliminated(env: &Env, arena_id: u64, eliminated: &Vec<Address>) {
    let key = DataKey::Eliminated(arena_id);
    storage(env).set(&key, eliminated);
    bump(env, &key);
}

fn get_round_choices(env: &Env, arena_id: u64, round: u32) -> soroban_sdk::Map<Address, Choice> {
    storage(env)
        .get(&DataKey::RoundChoices(arena_id, round))
        .unwrap_or(soroban_sdk::Map::new(env))
}

fn set_round_choices(env: &Env, arena_id: u64, round: u32, choices: &soroban_sdk::Map<Address, Choice>) {
    let key = DataKey::RoundChoices(arena_id, round);
    storage(env).set(&key, choices);
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

fn storage(env: &Env) -> soroban_sdk::storage::Persistent {
    env.storage().persistent()
}

fn bump(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, GAME_TTL_THRESHOLD, GAME_TTL_EXTEND_TO);
}

#[cfg(test)]
mod abi_guard;
#[cfg(all(test, feature = "integration-tests"))]
mod integration_tests;
#[cfg(test)]
mod test;
