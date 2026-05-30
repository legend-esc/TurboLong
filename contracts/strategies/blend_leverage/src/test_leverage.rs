#![cfg(test)]

//! Unit tests for leverage math, equity calculation, share accounting, and safety checks.

use crate::constants::{FIRST_DEPOSIT_LOCKUP, SCALAR_12, SCALAR_7};
use crate::leverage::{
    compute_equity, compute_health_factor, compute_loop_pairs, compute_totals, compute_unwind_loops,
    shares_to_underlying, underlying_to_shares,
};
use crate::storage::LeverageReserves;

// ── compute_loop_pairs ───────────────────────────────────────────────────────

#[test]
fn test_loop_pairs_basic_3_loops() {
    // c_factor = 0.95 (9_500_000 in 1e7), initial = 1000_0000000 (1000 USDC in 7 dec)
    let initial = 1_000_0000000_i128;
    let c_factor = 9_500_000_i128;
    let (supplies, borrows, count) = compute_loop_pairs(initial, c_factor, 3);

    assert_eq!(count, 4); // 3 loops + 1 final supply

    // Loop 0: supply 1000, borrow 1000*0.95 = 950
    assert_eq!(supplies[0], 1_000_0000000);
    assert_eq!(borrows[0], 950_0000000);

    // Loop 1: supply 950, borrow 950*0.95 = 902.5
    assert_eq!(supplies[1], 950_0000000);
    assert_eq!(borrows[1], 902_5000000);

    // Loop 2: supply 902.5, borrow 902.5*0.95 = 857.375
    assert_eq!(supplies[2], 902_5000000);
    assert_eq!(borrows[2], 857_3750000);

    // Final: supply 857.375, borrow 0
    assert_eq!(supplies[3], 857_3750000);
    assert_eq!(borrows[3], 0);
}

#[test]
fn test_loop_pairs_zero_loops() {
    let (supplies, borrows, count) = compute_loop_pairs(1_000_0000000, 9_500_000, 0);
    assert_eq!(count, 1);
    assert_eq!(supplies[0], 1_000_0000000);
    assert_eq!(borrows[0], 0);
}

#[test]
fn test_loop_pairs_one_loop() {
    let initial = 100_0000000_i128;
    let c_factor = 9_500_000_i128;
    let (supplies, borrows, count) = compute_loop_pairs(initial, c_factor, 1);
    assert_eq!(count, 2);
    assert_eq!(supplies[0], 100_0000000);
    assert_eq!(borrows[0], 95_0000000);
    assert_eq!(supplies[1], 95_0000000);
    assert_eq!(borrows[1], 0);
}

#[test]
fn test_loop_pairs_capped_at_20() {
    let (_, _, count) = compute_loop_pairs(1_000_0000000, 9_500_000, 25);
    assert_eq!(count, 21); // capped at 20 loops + 1 final = 21
}

// ── compute_totals ───────────────────────────────────────────────────────────

#[test]
fn test_totals_match_loop_pairs() {
    let initial = 1_000_0000000_i128;
    let c = 9_500_000_i128;
    let n = 8;

    let (total_supply, total_borrow) = compute_totals(initial, c, n);

    // Verify against manual sum of loop pairs
    let (supplies, borrows, count) = compute_loop_pairs(initial, c, n);
    let mut sum_s = 0i128;
    let mut sum_b = 0i128;
    for i in 0..count as usize {
        sum_s += supplies[i];
        sum_b += borrows[i];
    }
    assert_eq!(total_supply, sum_s);
    assert_eq!(total_borrow, sum_b);
}

#[test]
fn test_totals_leverage_ratio() {
    // With c=0.95 and 8 loops, leverage ≈ (1 - 0.95^9) / (1 - 0.95) ≈ 8.3
    let initial = 1_000_0000000_i128;
    let (total_supply, total_borrow) = compute_totals(initial, 9_500_000, 8);

    let leverage_x100 = total_supply * 100 / initial;
    // Leverage should be between 7 and 9
    assert!(leverage_x100 > 700 && leverage_x100 < 900,
        "Leverage {}.{} out of expected range", leverage_x100 / 100, leverage_x100 % 100);

    // Borrow should be supply - initial (equity)
    assert_eq!(total_supply - total_borrow, initial);
}

#[test]
fn test_totals_net_equals_initial() {
    // For any number of loops, total_supply - total_borrow = initial deposit
    for n in 0..15 {
        let initial = 1_000_0000000_i128;
        let (total_supply, total_borrow) = compute_totals(initial, 9_500_000, n);
        assert_eq!(total_supply - total_borrow, initial,
            "Net supply != initial at {} loops", n);
    }
}

// ── compute_equity ───────────────────────────────────────────────────────────

#[test]
fn test_equity_no_debt() {
    let reserves = LeverageReserves {
        total_shares: 1_000_0000000,
        total_b_tokens: 1_000_0000000,
        total_d_tokens: 0,
        b_rate: SCALAR_12, // 1:1 rate
        d_rate: SCALAR_12,
    };
    let equity = compute_equity(&reserves).unwrap();
    assert_eq!(equity, 1_000_0000000); // all supply is equity
}

#[test]
fn test_equity_with_leverage() {
    // Simulating ~2x leverage: 2000 supply, 1000 debt
    // b_rate = d_rate = 1.0 (SCALAR_12)
    let reserves = LeverageReserves {
        total_shares: 1_000_0000000,
        total_b_tokens: 2_000_0000000,
        total_d_tokens: 1_000_0000000,
        b_rate: SCALAR_12,
        d_rate: SCALAR_12,
    };
    let equity = compute_equity(&reserves).unwrap();
    assert_eq!(equity, 1_000_0000000); // 2000 - 1000 = 1000
}

#[test]
fn test_equity_with_accrued_rates() {
    // b_rate grew 5% (1.05), d_rate grew 8% (1.08)
    // Supply value = 2000 * 1.05 = 2100
    // Debt value = 1000 * 1.08 = 1080
    // Equity = 2100 - 1080 = 1020
    let b_rate = SCALAR_12 * 105 / 100; // 1.05e12
    let d_rate = SCALAR_12 * 108 / 100; // 1.08e12

    let reserves = LeverageReserves {
        total_shares: 1_000_0000000,
        total_b_tokens: 2_000_0000000,
        total_d_tokens: 1_000_0000000,
        b_rate,
        d_rate,
    };
    let equity = compute_equity(&reserves).unwrap();
    // 2000 * 1.05 - 1000 * 1.08 = 2100 - 1080 = 1020
    assert_eq!(equity, 1_020_0000000);
}

#[test]
fn test_equity_underwater() {
    // Debt has grown past supply value
    let b_rate = SCALAR_12;
    let d_rate = SCALAR_12 * 3; // 3x — debt exploded

    let reserves = LeverageReserves {
        total_shares: 1_000_0000000,
        total_b_tokens: 1_500_0000000,
        total_d_tokens: 1_000_0000000,
        b_rate,
        d_rate,
    };
    // Equity = 1500 - 3000 = -1500 (would be Err or negative)
    let result = compute_equity(&reserves);
    assert!(result.is_err() || result.unwrap() < 0);
}

// ── shares_to_underlying / underlying_to_shares ──────────────────────────────

#[test]
fn test_shares_to_underlying_simple() {
    let reserves = LeverageReserves {
        total_shares: 1_000_0000000,
        total_b_tokens: 2_000_0000000,
        total_d_tokens: 1_000_0000000,
        b_rate: SCALAR_12,
        d_rate: SCALAR_12,
    };
    // Total equity = 1000. Full shares = full equity.
    let value = shares_to_underlying(1_000_0000000, &reserves).unwrap();
    assert_eq!(value, 1_000_0000000);

    // Half shares = half equity
    let half = shares_to_underlying(500_0000000, &reserves).unwrap();
    assert_eq!(half, 500_0000000);
}

#[test]
fn test_shares_to_underlying_zero_shares() {
    let reserves = LeverageReserves {
        total_shares: 0,
        total_b_tokens: 0,
        total_d_tokens: 0,
        b_rate: SCALAR_12,
        d_rate: SCALAR_12,
    };
    assert_eq!(shares_to_underlying(0, &reserves).unwrap(), 0);
}

#[test]
fn test_underlying_to_shares_first_deposit() {
    let reserves = LeverageReserves {
        total_shares: 0,
        total_b_tokens: 0,
        total_d_tokens: 0,
        b_rate: SCALAR_12,
        d_rate: SCALAR_12,
    };
    // First deposit: 1 share = 1 unit
    assert_eq!(underlying_to_shares(1_000_0000000, &reserves).unwrap(), 1_000_0000000);
}

#[test]
fn test_underlying_to_shares_proportional() {
    let reserves = LeverageReserves {
        total_shares: 1_000_0000000,
        total_b_tokens: 2_000_0000000,
        total_d_tokens: 1_000_0000000,
        b_rate: SCALAR_12,
        d_rate: SCALAR_12,
    };
    // Equity = 1000. Depositing 500 should get 500 shares.
    let shares = underlying_to_shares(500_0000000, &reserves).unwrap();
    assert_eq!(shares, 500_0000000);
}

#[test]
fn test_shares_roundtrip() {
    let reserves = LeverageReserves {
        total_shares: 3_000_0000000,
        total_b_tokens: 6_000_0000000,
        total_d_tokens: 3_500_0000000,
        b_rate: SCALAR_12 * 103 / 100, // 1.03
        d_rate: SCALAR_12 * 106 / 100, // 1.06
    };
    let equity = compute_equity(&reserves).unwrap();
    assert!(equity > 0);

    // Convert equity -> shares -> equity, should be close to original
    let shares = underlying_to_shares(equity, &reserves).unwrap();
    let recovered = shares_to_underlying(shares, &reserves).unwrap();
    // Allow 1 stroop rounding
    assert!((recovered - equity).abs() <= 1,
        "Roundtrip error: equity={}, recovered={}", equity, recovered);
}

// ── compute_health_factor ────────────────────────────────────────────────────

#[test]
fn test_hf_no_debt() {
    let hf = compute_health_factor(1_000_0000000, 0, SCALAR_12, SCALAR_12, 9_500_000).unwrap();
    assert_eq!(hf, i128::MAX);
}

#[test]
fn test_hf_equal_rates() {
    // b_tokens=2000, d_tokens=1000, both rates=1.0, c_factor=0.95
    // HF = (2000 * 1.0 * 0.95) / (1000 * 1.0) = 1.9 in 1e7 = 19_000_000
    let hf = compute_health_factor(
        2_000_0000000,
        1_000_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
    ).unwrap();
    // HF = supply_value * c_factor / debt_value = 2000 * 9500000 / 1000 = 19_000_000
    assert_eq!(hf, 19_000_000);
}

#[test]
fn test_hf_near_liquidation() {
    // 8x leverage: b=8000, d=7000, c=0.95
    // HF = 8000*0.95/7000 ≈ 1.0857 → 10_857_142 in 1e7
    let hf = compute_health_factor(
        8_000_0000000,
        7_000_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
    ).unwrap();
    // 8000 * 9500000 / 7000 = 76000000000000000 / 7000_0000000 = 10_857_142
    assert_eq!(hf, 10_857_142);
    assert!(hf > SCALAR_7); // HF > 1.0
}

#[test]
fn test_hf_below_one() {
    // b=1000, d=1000, c=0.95 → HF = 0.95 → 9_500_000
    let hf = compute_health_factor(
        1_000_0000000,
        1_000_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
    ).unwrap();
    assert_eq!(hf, 9_500_000);
    assert!(hf < SCALAR_7); // HF < 1.0 → liquidatable
}

// ── compute_unwind_loops ─────────────────────────────────────────────────────

#[test]
fn test_unwind_already_at_target_returns_zero() {
    // HF = 1.9 >> 1.05 → already healthy, no unwinding needed
    let loops = compute_unwind_loops(
        2_000_0000000,
        1_000_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
        10_500_000, // target_hf = 1.05
    ).unwrap();
    assert_eq!(loops, 0);
}

#[test]
fn test_unwind_healthy_position_returns_zero() {
    // HF = 1.9 >> 1.05 → no unwinding needed
    let loops = compute_unwind_loops(
        2_000_0000000,
        1_000_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
        10_500_000, // min_hf = 1.05
    ).unwrap();
    assert_eq!(loops, 0);
}

#[test]
fn test_unwind_unhealthy_position() {
    // HF just below 1.05 → should need some unwinding
    // b=10500, d=9500, c=0.95 → HF = 10500*0.95/9500 ≈ 1.05 exactly
    // Make it slightly below: b=10499, d=9500
    let loops = compute_unwind_loops(
        10_499_0000000,
        9_500_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
        10_500_000,
    ).unwrap();
    assert!(loops > 0, "Should need at least 1 unwind loop");
    assert!(loops <= 20, "Should not exceed safety cap");
}

#[test]
fn test_unwind_no_debt() {
    let loops = compute_unwind_loops(
        1_000_0000000,
        0,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
        10_500_000,
    ).unwrap();
    assert_eq!(loops, 0);
}

#[test]
fn test_unwind_single_loop_position() {
    // 1-loop position: b=1950, d=950, c=0.95
    // HF = 1950 * 0.95 / 950 = 1852.5 / 950 ≈ 1.95 → healthy
    let loops = compute_unwind_loops(
        1_950_0000000,
        950_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
        10_500_000,
    ).unwrap();
    assert_eq!(loops, 0, "Single-loop healthy position needs no unwind");

    // Now make it unhealthy: b=1000, d=950 → HF = 1000*0.95/950 ≈ 1.0 < 1.05
    let loops = compute_unwind_loops(
        1_000_0000000,
        950_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
        10_500_000,
    ).unwrap();
    assert!(loops > 0, "Single-loop unhealthy position needs unwind");
}

#[test]
fn test_unwind_max_loops_position() {
    // 20-loop position deeply underwater: b=1000, d=999, c=0.95
    // HF = 1000*0.95/999 ≈ 0.9509 < 1.05 → needs unwinding
    let loops = compute_unwind_loops(
        1_000_0000000,
        999_0000000,
        SCALAR_12,
        SCALAR_12,
        9_500_000,
        10_500_000,
    ).unwrap();
    assert!(loops > 0, "Deeply leveraged position needs unwind");
    assert!(loops <= 20, "Should not exceed safety cap of 20");
}

#[test]
fn test_unwind_result_achieves_target_hf() {
    // Verify that after simulating `loops` unwind steps, HF >= target_hf
    let b = 10_499_0000000_i128;
    let d = 9_500_0000000_i128;
    let c = 9_500_000_i128;
    let target = 10_500_000_i128;

    let loops = compute_unwind_loops(b, d, SCALAR_12, SCALAR_12, c, target).unwrap();

    // Simulate the same steps
    let mut cb = b;
    let mut cd = d;
    for _ in 0..loops {
        let layer = cd * (SCALAR_7 - c) / SCALAR_7;
        cb -= layer;
        cd -= layer;
    }
    let final_hf = compute_health_factor(cb, cd, SCALAR_12, SCALAR_12, c).unwrap();
    assert!(final_hf >= target,
        "After {} loops, HF={} should be >= target={}", loops, final_hf, target);
}

#[test]
fn test_unwind_is_minimal() {
    // Verify that one fewer loop would NOT achieve target_hf
    let b = 10_499_0000000_i128;
    let d = 9_500_0000000_i128;
    let c = 9_500_000_i128;
    let target = 10_500_000_i128;

    let loops = compute_unwind_loops(b, d, SCALAR_12, SCALAR_12, c, target).unwrap();

    if loops > 0 {
        let mut cb = b;
        let mut cd = d;
        for _ in 0..(loops - 1) {
            let layer = cd * (SCALAR_7 - c) / SCALAR_7;
            cb -= layer;
            cd -= layer;
        }
        let hf_before_last = compute_health_factor(cb, cd, SCALAR_12, SCALAR_12, c).unwrap();
        assert!(hf_before_last < target,
            "One fewer loop ({}) should NOT achieve target: HF={}", loops - 1, hf_before_last);
    }
}

// ── Leverage table validation (cross-reference with simulate.rs) ─────────────

#[test]
fn test_leverage_table_matches_simulator() {
    // From simulate.rs: leverage(n, c) = (1 - c^(n+1)) / (1 - c)
    // Our compute_totals should produce the same leverage ratio.
    let initial = 1_000_0000000_i128;
    let c = 9_500_000_i128;

    for n in 0..=13 {
        let (total_supply, _) = compute_totals(initial, c, n);
        let our_lev_x1000 = total_supply * 1000 / initial;

        // Compute expected via float formula
        let c_f = 0.95_f64;
        let expected_lev = (1.0 - c_f.powi(n as i32 + 1)) / (1.0 - c_f);
        let expected_x1000 = (expected_lev * 1000.0).round() as i128;

        // Allow 1‰ tolerance for integer rounding
        let diff = (our_lev_x1000 - expected_x1000).abs();
        assert!(diff <= 1,
            "Loop {}: our={}.{:03}x, expected={}.{:03}x (diff={})",
            n, our_lev_x1000/1000, our_lev_x1000%1000,
            expected_x1000/1000, expected_x1000%1000, diff);
    }
}

// ── Deposit/withdraw accounting (with Soroban Env for storage) ───────────────

extern crate std;

use crate::reserves;
use crate::storage;
use soroban_sdk::{testutils::Address as _, Address, Env};

fn make_reserves(b: i128, d: i128, shares: i128) -> LeverageReserves {
    LeverageReserves {
        total_shares: shares,
        total_b_tokens: b,
        total_d_tokens: d,
        b_rate: SCALAR_12,
        d_rate: SCALAR_12,
    }
}

/// Minimal contract for unit-test storage context (avoids real constructor).
#[soroban_sdk::contract]
struct TestStorageContract;

#[soroban_sdk::contractimpl]
impl TestStorageContract {}

/// Register a minimal contract and run the closure inside its context.
/// This is needed because Soroban storage functions only work within a contract.
fn with_contract<F: FnOnce(&Env, &Address)>(e: &Env, f: F) {
    let contract_id = e.register(TestStorageContract, ());
    e.as_contract(&contract_id, || {
        f(e, &contract_id);
    });
}

#[test]
fn test_deposit_first_depositor() {
    let e = Env::default();
    with_contract(&e, |e, _| {
        let user = Address::generate(e);

        // Set up empty reserves in storage
        let init_reserves = make_reserves(0, 0, 0);
        storage::set_strategy_reserves(e, init_reserves.clone());

        // First deposit: 1000 equity → 1000 shares - 1000 lockup
        // Simulating: b_delta = 8000 (leverage 8x), d_delta = 7000, equity = 1000
        let b_delta = 8_000_0000000_i128;
        let d_delta = 7_000_0000000_i128;
        let (vault_shares, updated) = reserves::deposit(e, &user, b_delta, d_delta, &init_reserves).unwrap();

        // Equity added = 8000 - 7000 = 1000 (since rates = 1.0)
        // First deposit: new_shares = 1000, vault_minted = 1000 - 1000(lockup) = 999.9999
        assert_eq!(vault_shares, 1_000_0000000 - FIRST_DEPOSIT_LOCKUP);
        assert_eq!(updated.total_shares, 1_000_0000000); // includes lockup
        assert_eq!(updated.total_b_tokens, b_delta);
        assert_eq!(updated.total_d_tokens, d_delta);
    });
}

#[test]
fn test_deposit_second_depositor() {
    let e = Env::default();
    with_contract(&e, |e, _| {
        let user1 = Address::generate(e);
        let user2 = Address::generate(e);

        // First deposit
        let init = make_reserves(0, 0, 0);
        storage::set_strategy_reserves(e, init.clone());
        let (_, after_first) = reserves::deposit(e, &user1, 8_000_0000000, 7_000_0000000, &init).unwrap();

        // Second deposit: same equity (1000)
        let (user2_shares, after_second) = reserves::deposit(
            e, &user2, 8_000_0000000, 7_000_0000000, &after_first
        ).unwrap();

        // User2 should get proportional shares (1000 out of total 2000)
        assert_eq!(user2_shares, 1_000_0000000);
        assert_eq!(after_second.total_shares, 2_000_0000000);
        assert_eq!(after_second.total_b_tokens, 16_000_0000000);
        assert_eq!(after_second.total_d_tokens, 14_000_0000000);
    });
}

#[test]
fn test_withdraw_full() {
    let e = Env::default();
    with_contract(&e, |e, _| {
        let user = Address::generate(e);

        // Set up: user has all shares
        let reserves_state = make_reserves(8_000_0000000, 7_000_0000000, 1_000_0000000);
        storage::set_strategy_reserves(e, reserves_state.clone());
        storage::set_vault_shares(e, &user, 1_000_0000000);

        // Withdraw all equity (1000)
        let (remaining, b_remove, d_remove, updated) =
            reserves::withdraw(e, &user, 1_000_0000000, &reserves_state).unwrap();

        assert_eq!(remaining, 0);
        assert_eq!(b_remove, 8_000_0000000);
        assert_eq!(d_remove, 7_000_0000000);
        assert_eq!(updated.total_shares, 0);
        assert_eq!(updated.total_b_tokens, 0);
        assert_eq!(updated.total_d_tokens, 0);
    });
}

#[test]
fn test_withdraw_partial() {
    let e = Env::default();
    with_contract(&e, |e, _| {
        let user = Address::generate(e);

        let reserves_state = make_reserves(8_000_0000000, 7_000_0000000, 1_000_0000000);
        storage::set_strategy_reserves(e, reserves_state.clone());
        storage::set_vault_shares(e, &user, 1_000_0000000);

        // Withdraw half equity (500)
        let (remaining, b_remove, d_remove, updated) =
            reserves::withdraw(e, &user, 500_0000000, &reserves_state).unwrap();

        assert_eq!(remaining, 500_0000000);
        assert_eq!(b_remove, 4_000_0000000); // half of 8000
        assert_eq!(d_remove, 3_500_0000000); // half of 7000
        assert_eq!(updated.total_shares, 500_0000000);
    });
}

#[test]
fn test_withdraw_insufficient_balance() {
    let e = Env::default();
    with_contract(&e, |e, _| {
        let user = Address::generate(e);

        let reserves_state = make_reserves(8_000_0000000, 7_000_0000000, 1_000_0000000);
        storage::set_strategy_reserves(e, reserves_state.clone());
        storage::set_vault_shares(e, &user, 500_0000000); // only has 500

        // Try to withdraw more than equity
        let result = reserves::withdraw(e, &user, 600_0000000, &reserves_state);
        assert!(result.is_err());
    });
}

// ── Harvest accounting ───────────────────────────────────────────────────────

#[test]
fn test_harvest_increases_share_value() {
    // Pure math test - no storage needed
    // Start: 8000 b-tokens, 7000 d-tokens, 1000 shares, equity = 1000
    let reserves_state = make_reserves(8_000_0000000, 7_000_0000000, 1_000_0000000);

    let pre_value = shares_to_underlying(1_000_0000000, &reserves_state).unwrap();

    // Harvest adds 500 b-tokens and 400 d-tokens (net +100 equity from BLND compound)
    let mut updated = reserves_state.clone();
    updated.total_b_tokens += 500_0000000;
    updated.total_d_tokens += 400_0000000;
    // total_shares stays the same — that's the point of harvest

    let post_value = shares_to_underlying(1_000_0000000, &updated).unwrap();

    assert!(post_value > pre_value,
        "Share value should increase after harvest: pre={}, post={}", pre_value, post_value);
    assert_eq!(post_value - pre_value, 100_0000000); // +100 equity
}

// ── Edge cases ───────────────────────────────────────────────────────────────

#[test]
fn test_deposit_zero_b_tokens_fails() {
    let e = Env::default();
    with_contract(&e, |e, _| {
        let user = Address::generate(e);
        let reserves_state = make_reserves(0, 0, 0);
        storage::set_strategy_reserves(e, reserves_state.clone());

        let result = reserves::deposit(e, &user, 0, 0, &reserves_state);
        assert!(result.is_err());
    });
}

#[test]
fn test_deposit_negative_equity_fails() {
    let e = Env::default();
    with_contract(&e, |e, _| {
        let user = Address::generate(e);
        let reserves_state = make_reserves(0, 0, 0);
        storage::set_strategy_reserves(e, reserves_state.clone());

        // More debt than supply → negative equity
        let result = reserves::deposit(e, &user, 1_000_0000000, 2_000_0000000, &reserves_state);
        assert!(result.is_err());
    });
}

#[test]
fn test_multi_user_proportional() {
    let e = Env::default();
    with_contract(&e, |e, _| {
        let alice = Address::generate(e);
        let bob = Address::generate(e);

        let init = make_reserves(0, 0, 0);
        storage::set_strategy_reserves(e, init.clone());

        // Alice deposits first: equity = 1000
        let (alice_shares, after_alice) =
            reserves::deposit(e, &alice, 8_000_0000000, 7_000_0000000, &init).unwrap();

        // Bob deposits: equity = 2000 (double Alice)
        let (bob_shares, after_bob) =
            reserves::deposit(e, &bob, 16_000_0000000, 14_000_0000000, &after_alice).unwrap();

        // Bob should have ~2x Alice's shares
        let alice_actual = alice_shares; // minus lockup
        assert!(
            (bob_shares as f64 / alice_actual as f64 - 2.0).abs() < 0.01,
            "Bob should have ~2x Alice's shares: alice={}, bob={}", alice_actual, bob_shares
        );

        // Total equity should be 3000
        let total_equity = compute_equity(&after_bob).unwrap();
        assert_eq!(total_equity, 3_000_0000000);

        // Alice's value should be ~1000
        let alice_value = shares_to_underlying(alice_shares, &after_bob).unwrap();
        // Allow for lockup adjustment
        let expected = 1_000_0000000 - (FIRST_DEPOSIT_LOCKUP * 1_000_0000000 / after_bob.total_shares);
        // Allow small rounding from fixed-point math (up to 1000 stroops)
        assert!((alice_value - expected).abs() <= 1000,
            "Alice value={}, expected~={}", alice_value, expected);
    });
}

// ── Safety: utilization check ────────────────────────────────────────────────

#[test]
#[should_panic(expected = "Error(Contract, #422)")]
fn test_safety_rejects_high_utilization() {
    use crate::leverage::check_deposit_safety;
    use crate::storage::Config;

    let e = Env::default();
    let dummy = Address::generate(&e);
    let config = Config {
        asset: dummy.clone(),
        pool: dummy.clone(),
        reserve_id: 0,
        blend_token: dummy.clone(),
        router: dummy.clone(),
        claim_ids: soroban_sdk::Vec::new(&e),
        reward_threshold: 1,
        c_factor: 9_500_000,
        target_loops: 8,
        min_hf: 10_500_000,
        orange_hf: 11_500_000,
    };

    // Pool at 96% utilization → should panic (above 95% limit)
    check_deposit_safety(
        &e,
        1_000_0000000,   // pool supply
        960_0000000,     // pool borrow (96%)
        100_0000000,     // add supply
        50_0000000,      // add borrow
        1_000_0000000,   // post b
        500_0000000,     // post d
        SCALAR_12,
        SCALAR_12,
        &config,
    ).unwrap();
}

#[test]
fn test_safety_allows_healthy_pool() {
    use crate::leverage::check_deposit_safety;
    use crate::storage::Config;

    let e = Env::default();
    let dummy = Address::generate(&e);
    let config = Config {
        asset: dummy.clone(),
        pool: dummy.clone(),
        reserve_id: 0,
        blend_token: dummy.clone(),
        router: dummy.clone(),
        claim_ids: soroban_sdk::Vec::new(&e),
        reward_threshold: 1,
        c_factor: 9_500_000,
        target_loops: 8,
        min_hf: 10_500_000,
        orange_hf: 11_500_000,
    };

    // Pool at 50% utilization, healthy HF
    let result = check_deposit_safety(
        &e,
        1_000_0000000,
        500_0000000,      // 50% util
        100_0000000,
        50_0000000,
        2_000_0000000,    // plenty of collateral
        500_0000000,
        SCALAR_12,
        SCALAR_12,
        &config,
    );
    assert!(result.is_ok(), "Should allow at 50% utilization with healthy HF");
}
