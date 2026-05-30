#![no_std]

mod blend_pool;
mod constants;
mod leverage;
mod reserves;
mod soroswap;
mod storage;

#[cfg(test)]
mod test_leverage;
#[cfg(test)]
mod test_integration;

use constants::SCALAR_12;
pub use defindex_strategy_core::{event, DeFindexStrategyTrait, StrategyError};
use leverage::{
    check_deposit_safety, compute_health_factor, compute_totals, compute_unwind_loops,
    shares_to_underlying,
};
use soroban_sdk::{
    contract, contractimpl, token::TokenClient, Address, Bytes, Env, IntoVal, String, Val, Vec,
};
use storage::{extend_instance_ttl, Config};

fn check_positive_amount(amount: i128) -> Result<(), StrategyError> {
    if amount <= 0 {
        Err(StrategyError::OnlyPositiveAmountAllowed)
    } else {
        Ok(())
    }
}

const STRATEGY_NAME: &str = "BlendLeverageStrategy";

#[contract]
pub struct BlendLeverageStrategy;

#[contractimpl]
impl DeFindexStrategyTrait for BlendLeverageStrategy {
    /// Initialize the strategy with configuration.
    ///
    /// init_args layout:
    ///   [0] pool: Address          — Blend pool
    ///   [1] blend_token: Address   — BLND token
    ///   [2] router: Address        — Soroswap router
    ///   [3] reward_threshold: i128 — min BLND to trigger harvest
    ///   [4] keeper: Address        — authorized harvest caller
    ///   [5] c_factor: i128         — collateral factor (1e7)
    ///   [6] target_loops: u32      — number of leverage loops
    ///   [7] min_hf: i128           — minimum health factor (1e7)
    ///   [8] orange_hf: i128        — orange-zone threshold (1e7, must be > min_hf)
    fn __constructor(e: Env, asset: Address, init_args: Vec<Val>) {
        let pool: Address = init_args
            .get(0)
            .expect("Missing: pool address")
            .into_val(&e);
        let blend_token: Address = init_args
            .get(1)
            .expect("Missing: blend_token")
            .into_val(&e);
        let router: Address = init_args
            .get(2)
            .expect("Missing: router")
            .into_val(&e);
        let reward_threshold: i128 = init_args
            .get(3)
            .expect("Missing: reward_threshold")
            .into_val(&e);
        let keeper: Address = init_args
            .get(4)
            .expect("Missing: keeper")
            .into_val(&e);
        let c_factor: i128 = init_args
            .get(5)
            .expect("Missing: c_factor")
            .into_val(&e);
        let target_loops: u32 = init_args
            .get(6)
            .expect("Missing: target_loops")
            .into_val(&e);
        let min_hf: i128 = init_args
            .get(7)
            .expect("Missing: min_hf")
            .into_val(&e);
        let orange_hf: i128 = init_args
            .get(8)
            .expect("Missing: orange_hf")
            .into_val(&e);

        // Look up the reserve index from the pool
        let pool_client = blend_contract_sdk::pool::Client::new(&e, &pool);
        let reserve = pool_client.get_reserve(&asset);
        let reserve_id = reserve.config.index;

        // Claim IDs: supply side = index*2+1, borrow side = index*2
        let claim_ids: Vec<u32> = Vec::from_array(
            &e,
            [reserve_id * 2 + 1, reserve_id * 2],
        );

        check_positive_amount(reward_threshold).expect("reward_threshold must be positive");

        let config = Config {
            asset: asset.clone(),
            pool,
            reserve_id,
            blend_token,
            router,
            claim_ids,
            reward_threshold,
            c_factor,
            target_loops,
            min_hf,
            orange_hf,
        };

        storage::set_config(&e, config);
        storage::set_keeper(&e, &keeper);
    }

    fn asset(e: Env) -> Result<Address, StrategyError> {
        extend_instance_ttl(&e);
        Ok(storage::get_config(&e).asset)
    }

    /// Deposit underlying asset, execute leverage loop, mint shares.
    ///
    /// Flow:
    /// 1. Transfer `amount` from `from` to the strategy contract
    /// 2. Execute N-loop leverage: SupplyCollateral+Borrow × N + final SupplyCollateral
    /// 3. Track b/d token deltas, mint proportional shares
    /// 4. Return the depositor's underlying balance
    fn deposit(e: Env, amount: i128, from: Address) -> Result<i128, StrategyError> {
        extend_instance_ttl(&e);
        check_positive_amount(amount)?;
        from.require_auth();

        let config = storage::get_config(&e);
        let reserves = reserves::get_strategy_reserves_updated(&e, &config);

        // Safety: check pool utilization before depositing
        let (pool_supply, pool_borrow) = blend_pool::get_pool_utilization(&e, &config);
        let (add_supply, add_borrow) =
            compute_totals(amount, config.c_factor, config.target_loops);

        // Compute projected position for HF check
        let (b_rate, d_rate) = blend_pool::get_rates(&e, &config);
        let proj_b = reserves
            .total_b_tokens
            .checked_add(
                add_supply
                    .checked_mul(SCALAR_12)
                    .unwrap_or(0)
                    .checked_div(b_rate.max(1))
                    .unwrap_or(0),
            )
            .unwrap_or(reserves.total_b_tokens);
        let proj_d = reserves
            .total_d_tokens
            .checked_add(
                add_borrow
                    .checked_mul(SCALAR_12)
                    .unwrap_or(0)
                    .checked_div(d_rate.max(1))
                    .unwrap_or(0),
            )
            .unwrap_or(reserves.total_d_tokens);

        check_deposit_safety(
            &e,
            pool_supply,
            pool_borrow,
            add_supply,
            add_borrow,
            proj_b,
            proj_d,
            b_rate,
            d_rate,
            &config,
        )?;

        // Transfer the initial deposit from user to strategy contract
        let token_client = TokenClient::new(&e, &config.asset);
        token_client.transfer(&from, &e.current_contract_address(), &amount);

        // Execute the leverage loop — contract sends `amount` to pool,
        // pool processes supply+borrow atomically, netting means only `amount` leaves
        let (b_delta, d_delta) = blend_pool::submit_leverage_loop(&e, amount, &config)?;

        // Account for the deposit: mint shares proportional to equity added
        let (vault_shares, updated_reserves) =
            reserves::deposit(&e, &from, b_delta, d_delta, &reserves)?;

        let underlying_balance = shares_to_underlying(vault_shares, &updated_reserves)?;

        event::emit_deposit(
            &e,
            String::from_str(&e, STRATEGY_NAME),
            amount,
            from,
        );

        Ok(underlying_balance)
    }

    /// Harvest BLND emissions, swap to underlying, re-leverage.
    ///
    /// Callable only by the keeper. Claims from both supply and borrow emission
    /// sides, swaps BLND → underlying via Soroswap, then re-leverages proceeds.
    /// No new shares are minted — this increases per-share equity.
    fn harvest(e: Env, from: Address, data: Option<Bytes>) -> Result<(), StrategyError> {
        extend_instance_ttl(&e);

        let keeper = storage::get_keeper(&e);
        keeper.require_auth();

        if from != keeper {
            return Err(StrategyError::NotAuthorized);
        }

        let config = storage::get_config(&e);

        // Claim BLND from both supply and borrow sides
        let harvested_blnd = blend_pool::claim(&e, &config);

        // Parse minimum swap output from data bytes
        let amount_out_min: i128 = match &data {
            Some(bytes) if !bytes.is_empty() => {
                let mut slice = [0u8; 16];
                bytes.copy_into_slice(&mut slice);
                i128::from_be_bytes(slice)
            }
            _ => 0,
        };

        // Swap BLND → underlying, then re-leverage
        let (b_delta, d_delta) = blend_pool::perform_reinvest(&e, &config, amount_out_min)?;

        // Update reserves without minting shares (yield accrues to existing holders)
        if b_delta > 0 {
            let updated_reserves = reserves::harvest(&e, b_delta, d_delta, &config)?;
            event::emit_harvest(
                &e,
                String::from_str(&e, STRATEGY_NAME),
                harvested_blnd,
                keeper,
                shares_to_underlying(SCALAR_12, &updated_reserves)?,
            );
        }

        Ok(())
    }

    /// Withdraw underlying by unwinding proportional leverage.
    ///
    /// Flow:
    /// 1. Calculate proportional b/d tokens for the requested amount
    /// 2. Submit unwind: repay proportional debt, withdraw proportional collateral
    /// 3. Burn shares, transfer equity to `to`
    /// 4. Return the depositor's remaining underlying balance
    fn withdraw(e: Env, amount: i128, from: Address, to: Address) -> Result<i128, StrategyError> {
        extend_instance_ttl(&e);
        check_positive_amount(amount)?;
        from.require_auth();

        let config = storage::get_config(&e);
        let reserves = reserves::get_strategy_reserves_updated(&e, &config);

        // Calculate proportional b/d tokens to unwind
        let (remaining_shares, b_to_remove, d_to_remove, updated_reserves) =
            reserves::withdraw(&e, &from, amount, &reserves)?;

        // Execute unwind on the pool — net equity flows to `to`
        blend_pool::submit_unwind(&e, b_to_remove, d_to_remove, &to, &config)?;

        let underlying_balance = shares_to_underlying(remaining_shares, &updated_reserves)?;

        event::emit_withdraw(
            &e,
            String::from_str(&e, STRATEGY_NAME),
            amount,
            from,
        );

        Ok(underlying_balance)
    }

    /// Query the underlying asset balance for an address.
    ///
    /// balance = caller_shares / total_shares × (supply_value - debt_value)
    fn balance(e: Env, from: Address) -> Result<i128, StrategyError> {
        extend_instance_ttl(&e);

        let vault_shares = storage::get_vault_shares(&e, &from);
        if vault_shares <= 0 {
            return Ok(0);
        }

        let config = storage::get_config(&e);
        let reserves = reserves::get_strategy_reserves_updated(&e, &config);
        shares_to_underlying(vault_shares, &reserves)
    }
}

// ── Additional public methods (not part of the trait) ────────────────────────

#[contractimpl]
impl BlendLeverageStrategy {
    /// Rebalance: partial-unwind if HF is in the orange zone (orange_hf > HF >= min_hf boundary).
    /// Unwinds the minimum number of loops to restore min_hf.
    /// Callable by anyone (permissionless — protects the vault).
    pub fn rebalance(e: Env) -> Result<(), StrategyError> {
        extend_instance_ttl(&e);

        let config = storage::get_config(&e);
        let (b_rate, d_rate) = blend_pool::get_rates(&e, &config);
        let (b_tokens, d_tokens) = blend_pool::get_strategy_positions(&e, &config);

        if d_tokens == 0 {
            return Ok(()); // No debt, nothing to rebalance
        }

        let hf = compute_health_factor(b_tokens, d_tokens, b_rate, d_rate, config.c_factor)?;

        // Only act if HF is in the orange zone (below orange_hf threshold)
        if hf >= config.orange_hf {
            return Ok(());
        }

        // Compute minimum loops to restore min_hf
        let unwind_count = compute_unwind_loops(
            b_tokens, d_tokens, b_rate, d_rate, config.c_factor, config.min_hf,
        )?;

        if unwind_count == 0 {
            return Ok(());
        }

        let (b_removed, d_removed) =
            blend_pool::submit_deleverage(&e, unwind_count, &config)?;

        reserves::deleverage(&e, b_removed, d_removed, &config)?;

        Ok(())
    }

    /// Partial-unwind liquidation protection: keeper- or user-triggered.
    ///
    /// If HF is below `orange_hf`, unwinds the minimum number of loops to
    /// bring HF back to `min_hf`. No-ops if HF is already at or above `orange_hf`.
    ///
    /// Returns the number of loops unwound.
    pub fn partial_unwind(e: Env, caller: Address) -> Result<u32, StrategyError> {
        extend_instance_ttl(&e);

        // Keeper or any user may call this (permissionless protection)
        caller.require_auth();

        let config = storage::get_config(&e);
        let (b_rate, d_rate) = blend_pool::get_rates(&e, &config);
        let (b_tokens, d_tokens) = blend_pool::get_strategy_positions(&e, &config);

        if d_tokens == 0 {
            return Ok(0);
        }

        let hf = compute_health_factor(b_tokens, d_tokens, b_rate, d_rate, config.c_factor)?;

        if hf >= config.orange_hf {
            return Ok(0); // Not in orange zone
        }

        let unwind_count = compute_unwind_loops(
            b_tokens, d_tokens, b_rate, d_rate, config.c_factor, config.min_hf,
        )?;

        if unwind_count == 0 {
            return Ok(0);
        }

        let (b_removed, d_removed) =
            blend_pool::submit_deleverage(&e, unwind_count, &config)?;

        reserves::deleverage(&e, b_removed, d_removed, &config)?;

        Ok(unwind_count)
    }

    /// Set a new keeper address. Only the current keeper can call this.
    pub fn set_keeper(e: Env, new_keeper: Address) -> Result<(), StrategyError> {
        extend_instance_ttl(&e);
        let old_keeper = storage::get_keeper(&e);
        old_keeper.require_auth();
        storage::set_keeper(&e, &new_keeper);
        Ok(())
    }

    /// Get the current keeper address.
    pub fn get_keeper(e: Env) -> Result<Address, StrategyError> {
        extend_instance_ttl(&e);
        Ok(storage::get_keeper(&e))
    }

    /// Get current health factor (1e7 scaled).
    pub fn health_factor(e: Env) -> Result<i128, StrategyError> {
        extend_instance_ttl(&e);
        let config = storage::get_config(&e);
        let (b_rate, d_rate) = blend_pool::get_rates(&e, &config);
        let (b_tokens, d_tokens) = blend_pool::get_strategy_positions(&e, &config);
        compute_health_factor(b_tokens, d_tokens, b_rate, d_rate, config.c_factor)
    }

    /// Get current strategy position details.
    /// Returns (total_equity, total_shares, b_tokens, d_tokens, b_rate, d_rate).
    pub fn position(e: Env) -> Result<(i128, i128, i128, i128, i128, i128), StrategyError> {
        extend_instance_ttl(&e);
        let config = storage::get_config(&e);
        let reserves = reserves::get_strategy_reserves_updated(&e, &config);
        let equity = leverage::compute_equity(&reserves)?;
        Ok((
            equity,
            reserves.total_shares,
            reserves.total_b_tokens,
            reserves.total_d_tokens,
            reserves.b_rate,
            reserves.d_rate,
        ))
    }
}
