use soroban_sdk::{contracttype, Address, Env, Vec};

// ── TTL constants ────────────────────────────────────────────────────────────

const ONE_DAY_LEDGERS: u32 = 17_280;

const INSTANCE_BUMP_AMOUNT: u32 = 30 * ONE_DAY_LEDGERS;
const INSTANCE_LIFETIME_THRESHOLD: u32 = INSTANCE_BUMP_AMOUNT - ONE_DAY_LEDGERS;

const PERSISTENT_BUMP_AMOUNT: u32 = 120 * ONE_DAY_LEDGERS;
const PERSISTENT_LIFETIME_THRESHOLD: u32 = PERSISTENT_BUMP_AMOUNT - 20 * ONE_DAY_LEDGERS;

// ── Data keys ────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    Reserves,
    VaultPos(Address),
    Keeper,
}

// ── Config ───────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug)]
pub struct Config {
    /// The underlying asset (e.g. USDC)
    pub asset: Address,
    /// Blend pool address
    pub pool: Address,
    /// Reserve index in the pool
    pub reserve_id: u32,
    /// BLND token address
    pub blend_token: Address,
    /// Soroswap router address
    pub router: Address,
    /// Emission claim IDs (supply + borrow sides)
    pub claim_ids: Vec<u32>,
    /// Minimum BLND balance to trigger harvest swap
    pub reward_threshold: i128,
    /// Collateral factor (1e7 scaled, e.g. 9_500_000 = 0.95)
    pub c_factor: i128,
    /// Target number of supply+borrow loops
    pub target_loops: u32,
    /// Minimum health factor (1e7 scaled, e.g. 1_050_000 = 1.05)
    pub min_hf: i128,
    /// Orange-zone threshold: HF below this triggers partial unwind (must be > min_hf).
    /// e.g. 1_150_000 = 1.15 — if HF drops below 1.15, unwind just enough loops to
    /// restore min_hf rather than forcing a full close.
    pub orange_hf: i128,
}

pub fn set_config(e: &Env, config: Config) {
    e.storage().instance().set(&DataKey::Config, &config);
}

pub fn get_config(e: &Env) -> Config {
    e.storage()
        .instance()
        .get(&DataKey::Config)
        .expect("Config not initialized")
}

// ── Leverage reserves (strategy-level accounting) ────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeverageReserves {
    /// Total shares outstanding across all depositors
    pub total_shares: i128,
    /// Total b-tokens (supply tokens) held by the strategy in the pool
    pub total_b_tokens: i128,
    /// Total d-tokens (debt tokens) owed by the strategy in the pool
    pub total_d_tokens: i128,
    /// Last known b_rate from the pool
    pub b_rate: i128,
    /// Last known d_rate from the pool
    pub d_rate: i128,
}

impl Default for LeverageReserves {
    fn default() -> Self {
        Self {
            total_shares: 0,
            total_b_tokens: 0,
            total_d_tokens: 0,
            b_rate: 0,
            d_rate: 0,
        }
    }
}

pub fn set_strategy_reserves(e: &Env, reserves: LeverageReserves) {
    e.storage()
        .persistent()
        .set(&DataKey::Reserves, &reserves);
    e.storage().persistent().extend_ttl(
        &DataKey::Reserves,
        PERSISTENT_LIFETIME_THRESHOLD,
        PERSISTENT_BUMP_AMOUNT,
    );
}

pub fn get_strategy_reserves(e: &Env) -> LeverageReserves {
    e.storage()
        .persistent()
        .get(&DataKey::Reserves)
        .unwrap_or_default()
}

// ── Per-user vault shares ────────────────────────────────────────────────────

pub fn set_vault_shares(e: &Env, address: &Address, shares: i128) {
    let key = DataKey::VaultPos(address.clone());
    e.storage().persistent().set(&key, &shares);
    e.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
}

pub fn get_vault_shares(e: &Env, address: &Address) -> i128 {
    let key = DataKey::VaultPos(address.clone());
    let shares = e.storage().persistent().get(&key).unwrap_or(0i128);
    if shares > 0 {
        e.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }
    shares
}

// ── Keeper ───────────────────────────────────────────────────────────────────

pub fn set_keeper(e: &Env, keeper: &Address) {
    e.storage()
        .persistent()
        .set(&DataKey::Keeper, keeper);
    e.storage().persistent().extend_ttl(
        &DataKey::Keeper,
        PERSISTENT_LIFETIME_THRESHOLD,
        PERSISTENT_BUMP_AMOUNT,
    );
}

pub fn get_keeper(e: &Env) -> Address {
    e.storage()
        .persistent()
        .get(&DataKey::Keeper)
        .expect("Keeper not set")
}

// ── Instance TTL ─────────────────────────────────────────────────────────────

pub fn extend_instance_ttl(e: &Env) {
    e.storage()
        .instance()
        .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
}
