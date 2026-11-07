//! Behaviour of the lending market, one rule at a time.

mod common;

use avm_lending_market::{
    ids, Config, State, ERR_ARGS, ERR_AUTH, ERR_BALANCE, ERR_CAPACITY, ERR_HEALTHY, ERR_INSOLVENT,
    ERR_LIQUIDITY, ERR_PAUSED, ERR_PENDING, ERR_PRICE_MOVE, ERR_STALE, ERR_STATE, ERR_UNHEALTHY,
    ERR_VALUE, ERR_ZERO, SHELL_CURRENCY_ID,
};
use common::*;

fn live() -> Sim {
    Sim::live()
}

#[test]
fn deploys_with_a_clean_state_and_one_event() {
    let sim = Sim::new();
    assert_eq!(sim.init_events, ["lend.created"]);
    let state = sim.state();
    assert_eq!(state.index, ONE);
    assert_eq!(state.price, 0);
    assert!(state.accounts.is_empty());
    assert_eq!(state.config, default_config());
}

#[test]
fn deployment_rejects_bad_configs_and_wrong_deployers() {
    let mut cases = Vec::new();
    let base = default_config();
    cases.push(Config { ltv: 0, ..base });
    cases.push(Config { ltv: 8_000, ..base }); // ltv must stay below liq
    cases.push(Config { liq: 9_600, ..base });
    cases.push(Config { bonus: 2_001, ..base });
    cases.push(Config { liq: 9_500, bonus: 2_000, ..base }); // bonus would eat the margin
    cases.push(Config { reserve_factor: 5_001, ..base });
    cases.push(Config { kink: 0, ..base });
    cases.push(Config { kink: 10_000, ..base });
    cases.push(Config { max_move: 0, ..base });
    cases.push(Config { max_age: 0, ..base });
    cases.push(Config { fuel_reserve: 0, ..base });
    cases.push(Config { currency: [COL, COL], ..base });
    cases.push(Config { currency: [SHELL_CURRENCY_ID, DEBT], ..base });
    cases.push(Config { admin: [0; 32], ..base });
    for config in cases {
        let bytes = config.encode();
        let output = Sim::try_deploy(&bytes, ADMIN).err().expect("must be rejected");
        assert_eq!(output.status_code, ERR_ARGS);
    }
    let output = Sim::try_deploy(&init_data(&base), OUTSIDER).err().unwrap();
    assert_eq!(output.status_code, ERR_AUTH);
    let mut padded = init_data(&base);
    padded[58] = 1;
    assert!(Sim::try_deploy(&padded, ADMIN).is_err());
    assert!(Sim::try_deploy(&padded[..79], ADMIN).is_err());
}

#[test]
fn supplying_mints_shares_one_to_one_at_the_start() {
    let sim = live();
    let alice = sim.account(ALICE);
    assert_eq!(alice.shares, 1_000_000);
    assert_eq!(alice.balance, [0, 0]);
    let state = sim.state();
    assert_eq!((state.cash, state.total_shares), (1_000_000, 1_000_000));
    sim.assert_accounting();
}

#[test]
fn deposits_credit_the_ledger_and_reject_foreign_value() {
    let mut sim = Sim::new();
    accepted(&sim.deposit(ALICE, &[(DEBT, 5), (COL, 7)]));
    assert_eq!(sim.account(ALICE).balance, [7, 5]);
    assert_eq!(sim.events, ["lend.action", "lend.action"]);
    failure(&sim.deposit(ALICE, &[(77, 1)]), ERR_VALUE);
    failure(&sim.deposit(ALICE, &[(SHELL_CURRENCY_ID, 1)]), ERR_VALUE);
    failure(&sim.deposit(ALICE, &[]), ERR_VALUE);
    failure(&sim.call(ALICE, ids::DEPOSIT, &[1], &[(DEBT, 1)]), ERR_ARGS);
    // Contracts cannot deposit: every account is a signing key.
    failure(&sim.internal_deposit(addr(3), &[(DEBT, 1)]), ERR_AUTH);
}

#[test]
fn borrowing_stops_at_the_loan_to_value_limit() {
    let mut sim = live();
    failure(&sim.amount(BOB, ids::BORROW, 750_001), ERR_UNHEALTHY);
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    assert_eq!(sim.account(BOB).balance, [0, 750_000]);
    assert_eq!(sim.debt(BOB), 750_000);
    assert_eq!(sim.state().cash, 250_000);
    failure(&sim.amount(BOB, ids::BORROW, 1), ERR_UNHEALTHY);
    sim.assert_accounting();
}

#[test]
fn borrowing_is_limited_by_the_cash_in_the_market() {
    let mut sim = live();
    accepted(&sim.deposit(BOB, &[(COL, 9_000_000)]));
    accepted(&sim.amount(BOB, ids::ADD_COLLATERAL, 9_000_000));
    failure(&sim.amount(BOB, ids::BORROW, 1_000_001), ERR_LIQUIDITY);
    accepted(&sim.amount(BOB, ids::BORROW, 1_000_000));
    assert_eq!(sim.state().cash, 0);
}

#[test]
fn a_missing_or_stale_price_blocks_risky_actions() {
    let mut sim = Sim::new();
    accepted(&sim.deposit(ALICE, &[(DEBT, 1_000)]));
    accepted(&sim.amount(ALICE, ids::SUPPLY, 1_000));
    accepted(&sim.deposit(BOB, &[(COL, 1_000)]));
    accepted(&sim.amount(BOB, ids::ADD_COLLATERAL, 1_000));
    failure(&sim.amount(BOB, ids::BORROW, 100), ERR_STALE); // never priced
    accepted(&sim.set_price(PRICE));
    accepted(&sim.amount(BOB, ids::BORROW, 100));
    sim.advance(3_601);
    failure(&sim.amount(BOB, ids::BORROW, 1), ERR_STALE);
    failure(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 1), ERR_STALE); // has debt
    failure(&sim.liquidate(CAROL, BOB, 1), ERR_BALANCE); // no account, checked first
    accepted(&sim.deposit(CAROL, &[(DEBT, 10)]));
    failure(&sim.liquidate(CAROL, BOB, 1), ERR_STALE);
    // Repaying never needs a price (a little interest accrued: top up and clear it).
    accepted(&sim.deposit(BOB, &[(DEBT, 10)]));
    accepted(&sim.amount(BOB, ids::REPAY, u128::MAX));
    // With the debt gone, collateral is free to leave without a price.
    accepted(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 1_000));
    sim.assert_accounting();
}

#[test]
fn interest_follows_the_utilisation_curve() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    // 75% utilisation: 2% + 4% * 75/80 = 5.75% a year.
    sim.advance(YEAR);
    accepted(&sim.set_price(PRICE)); // any call accrues
    assert_eq!(sim.debt(BOB), 793_125);
    let state = sim.state();
    assert_eq!(state.index, ONE + ONE / 10_000 * 575);
    assert_eq!(state.reserves, 4_312); // 10% of 43_125, rounded down
    assert_eq!(state.supply_value().unwrap(), 250_000 + 793_125 - 4_312);
}

#[test]
fn the_rate_steepens_above_the_kink() {
    use avm_lending_market::math::borrow_rate_bps;
    assert_eq!(borrow_rate_bps(0, 8_000, 200, 400, 6_000), 200);
    assert_eq!(borrow_rate_bps(8_000, 8_000, 200, 400, 6_000), 600);
    assert_eq!(borrow_rate_bps(9_000, 8_000, 200, 400, 6_000), 600 + 3_000);
    assert_eq!(borrow_rate_bps(10_000, 8_000, 200, 400, 6_000), 600 + 6_000);
}

#[test]
fn suppliers_earn_the_interest_and_the_cycle_closes_exactly() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    sim.advance(YEAR);
    // Bob tops up to cover interest and repays everything.
    accepted(&sim.deposit(BOB, &[(DEBT, 50_000)]));
    accepted(&sim.amount(BOB, ids::REPAY, u128::MAX));
    assert_eq!(sim.debt(BOB), 0);
    assert_eq!(sim.account(BOB).balance, [0, 800_000 - 793_125]);
    // All cash is back: Alice redeems the lot, minus the protocol's cut.
    let shares = sim.account(ALICE).shares;
    accepted(&sim.amount(ALICE, ids::REDEEM, shares));
    // 1_000_000 + 43_125 interest - 4_312 reserves.
    assert_eq!(sim.account(ALICE).balance[1], 1_038_813);
    assert_eq!(sim.state().total_shares, 0);
    assert_eq!(sim.state().cash, 4_312);
    sim.assert_accounting();
}

#[test]
fn a_late_supplier_buys_in_at_the_higher_share_price() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    sim.advance(YEAR);
    accepted(&sim.deposit(CAROL, &[(DEBT, 1_038_813)]));
    accepted(&sim.amount(CAROL, ids::SUPPLY, 1_038_813));
    // Value 1_038_813 for 1_000_000 shares: the newcomer receives the same count.
    assert_eq!(sim.account(CAROL).shares, 1_000_000);
    accepted(&sim.deposit(CAROL, &[(DEBT, 1)]));
    failure(&sim.amount(CAROL, ids::SUPPLY, 1), ERR_ZERO); // rounds to zero shares
}

#[test]
fn redemption_cannot_drain_cash_that_is_lent_out() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    failure(&sim.amount(ALICE, ids::REDEEM, 500_000), ERR_LIQUIDITY);
    accepted(&sim.amount(ALICE, ids::REDEEM, 250_000));
    failure(&sim.amount(ALICE, ids::REDEEM, 1_000_000), ERR_BALANCE);
    failure(&sim.amount(OUTSIDER, ids::REDEEM, 1), ERR_BALANCE);
}

#[test]
fn collateral_can_leave_only_while_the_position_stays_within_ltv() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 600_000));
    // 600_000 / 0.75 = 800_000 needs to stay locked.
    failure(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 200_001), ERR_UNHEALTHY);
    accepted(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 200_000));
    assert_eq!(sim.account(BOB).balance[0], 200_000);
    failure(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 1_000_000), ERR_BALANCE);
    sim.assert_accounting();
}

#[test]
fn repaying_is_capped_at_the_debt_and_needs_funds() {
    let mut sim = live();
    failure(&sim.amount(BOB, ids::REPAY, 1), ERR_ZERO); // nothing owed
    accepted(&sim.amount(BOB, ids::BORROW, 100_000));
    // Park the borrowed funds as a supply position: nothing idle is left to repay with.
    accepted(&sim.amount(BOB, ids::SUPPLY, 100_000));
    failure(&sim.amount(BOB, ids::REPAY, 1), ERR_BALANCE);
    let shares = sim.account(BOB).shares;
    accepted(&sim.amount(BOB, ids::REDEEM, shares));
    accepted(&sim.deposit(BOB, &[(DEBT, 5)]));
    accepted(&sim.amount(BOB, ids::REPAY, 40_000));
    assert_eq!(sim.debt(BOB), 60_000);
    accepted(&sim.amount(BOB, ids::REPAY, u128::MAX));
    assert_eq!(sim.debt(BOB), 0);
    assert_eq!(sim.account(BOB).balance[1], 5);
}

#[test]
fn a_healthy_position_cannot_be_liquidated_an_unhealthy_one_can() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    accepted(&sim.deposit(CAROL, &[(DEBT, 1_000_000)]));
    // 0.95: collateral worth 950_000, threshold 760_000 >= 750_000.
    accepted(&sim.set_price(PRICE / 100 * 95));
    failure(&sim.liquidate(CAROL, BOB, 1_000), ERR_HEALTHY);
    // 0.85: threshold 680_000 < 750_000.
    accepted(&sim.set_price(PRICE / 100 * 85));
    failure(&sim.liquidate(CAROL, CAROL, 1_000), ERR_ARGS);
    failure(&sim.liquidate(CAROL, BOB, 0), ERR_ARGS);
    failure(&sim.liquidate(CAROL, OUTSIDER, 1), ERR_ARGS);
    // Close factor 50%: at most 375_000 even if more is offered.
    accepted(&sim.liquidate(CAROL, BOB, 1_000_000));
    assert_eq!(sim.debt(BOB), 375_000);
    // 375_000 * 1.05 / 0.85 = 463_235.29
    assert_eq!(sim.account(CAROL).balance, [463_235, 1_000_000 - 375_000]);
    assert_eq!(sim.account(BOB).collateral, 1_000_000 - 463_235);
    sim.assert_accounting();
}

#[test]
fn liquidation_takes_everything_when_collateral_cannot_cover_the_bonus() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    accepted(&sim.deposit(CAROL, &[(DEBT, 1_000_000)]));
    // Walk the price down so the collateral is worth less than the debt.
    for price in [80, 64, 52, 42, 40] {
        accepted(&sim.set_price(PRICE / 100 * price));
    }
    accepted(&sim.liquidate(CAROL, BOB, 1_000_000));
    // 375_000 repaid would need 375_000*1.05/0.40 = 984_375 collateral.
    assert_eq!(sim.account(BOB).collateral, 15_625);
    let carol = sim.account(CAROL);
    assert_eq!(carol.balance[0], 984_375);
    sim.assert_accounting();
}

#[test]
fn bad_debt_is_absorbed_by_reserves_first_then_by_suppliers() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    sim.advance(YEAR / 4);
    accepted(&sim.set_price(PRICE));
    let reserves = sim.state().reserves;
    assert!(reserves > 0);
    accepted(&sim.deposit(CAROL, &[(DEBT, 2_000_000)]));
    // Crash: every step is within the 20% cap and fresh.
    for price in [80, 64, 52, 42, 34, 28, 23, 19, 16, 13, 11, 9, 8, 7, 6, 5, 4, 3, 2, 1] {
        accepted(&sim.set_price((PRICE / 100 * price).max(1)));
        sim.liquidate(CAROL, BOB, u128::MAX);
        if sim.account(BOB).collateral == 0 {
            break;
        }
    }
    sim.assert_accounting();
    let value_before = sim.state().supply_value().unwrap();
    let debt = sim.debt(BOB);
    assert!(debt > 0 && sim.account(BOB).collateral == 0, "the crash must leave bad debt");
    {
        // Permissionless: a stranger with no account can clean up dead debt.
        accepted(&sim.call(OUTSIDER, ids::WRITE_OFF, &BOB, &[]));
        let after = sim.state();
        assert_eq!(after.total_scaled, 0);
        let loss = debt;
        let absorbed = loss.min(reserves);
        assert_eq!(after.reserves, reserves - absorbed);
        assert_eq!(after.supply_value().unwrap(), value_before - (loss - absorbed));
    }
    sim.assert_accounting();
}

#[test]
fn write_off_returns_unliquidatable_collateral_to_the_borrower() {
    let mut sim = Sim::new();
    accepted(&sim.set_price(2 * PRICE));
    accepted(&sim.deposit(ALICE, &[(DEBT, 10)]));
    accepted(&sim.amount(ALICE, ids::SUPPLY, 10));
    accepted(&sim.deposit(BOB, &[(COL, 1)]));
    accepted(&sim.amount(BOB, ids::ADD_COLLATERAL, 1));
    accepted(&sim.amount(BOB, ids::BORROW, 1));
    failure(&sim.call(OUTSIDER, ids::WRITE_OFF, &BOB, &[]), ERR_HEALTHY);

    for percent in [160, 128, 103, 83, 67, 54, 50] {
        accepted(&sim.set_price(PRICE / 100 * percent));
    }
    failure(&sim.liquidate(ALICE, BOB, 1), ERR_ZERO);
    sim.advance(default_config().max_age + 1);
    failure(&sim.call(OUTSIDER, ids::WRITE_OFF, &BOB, &[]), ERR_STALE);
    accepted(&sim.set_price(PRICE / 2));
    accepted(&sim.call(OUTSIDER, ids::WRITE_OFF, &BOB, &[]));
    assert_eq!(sim.debt(BOB), 0);
    assert_eq!(sim.account(BOB).collateral, 0);
    assert_eq!(sim.account(BOB).balance[0], 1);
    assert_eq!(sim.state().supply_value().unwrap(), 9);
    sim.assert_accounting();
}

#[test]
fn write_off_charges_reserves_the_change_in_aggregate_debt() {
    let mut sim = live();
    accepted(&sim.deposit(CAROL, &[(DEBT, 1)]));
    let mut state = sim.state();
    state.index = ONE + ONE / 2;
    state.reserves = 2;
    let bob = state.account_index(BOB).unwrap();
    let carol = state.account_index(CAROL).unwrap();
    state.accounts[bob].balance[0] = state.accounts[bob].collateral;
    state.accounts[bob].collateral = 0;
    state.accounts[bob].scaled = 1;
    state.accounts[carol].scaled = 1;
    state.total_collateral = 0;
    state.total_scaled = 2;
    assert_eq!(state.total_debt().unwrap(), 3);
    assert_eq!(state.debt_of(1).unwrap(), 2);
    let value_before = state.supply_value().unwrap();
    sim.state = state.encode();

    accepted(&sim.call(OUTSIDER, ids::WRITE_OFF, &BOB, &[]));
    let after = sim.state();
    assert_eq!(after.total_debt().unwrap(), 2);
    assert_eq!(after.reserves, 1);
    assert_eq!(after.supply_value().unwrap(), value_before);
    sim.assert_accounting();
}

#[test]
fn write_off_needs_a_dead_position() {
    let mut sim = live();
    failure(&sim.call(ALICE, ids::WRITE_OFF, &BOB, &[]), ERR_HEALTHY); // no debt
    accepted(&sim.amount(BOB, ids::BORROW, 100_000));
    failure(&sim.call(ALICE, ids::WRITE_OFF, &BOB, &[]), ERR_HEALTHY); // still collateralised
    failure(&sim.call(ALICE, ids::WRITE_OFF, &[0; 31], &[]), ERR_ARGS);
}

#[test]
fn the_price_can_only_move_so_far_per_update_and_only_by_the_admin() {
    let mut sim = Sim::new();
    failure(&sim.amount(OUTSIDER, ids::SET_PRICE, PRICE), ERR_AUTH);
    failure(&sim.amount(ADMIN, ids::SET_PRICE, 0), ERR_ZERO);
    accepted(&sim.set_price(PRICE * 1_000)); // the first price is free
    failure(&sim.set_price(PRICE * 1_201), ERR_PRICE_MOVE);
    failure(&sim.set_price(PRICE * 799), ERR_PRICE_MOVE);
    accepted(&sim.set_price(PRICE * 1_200));
    accepted(&sim.set_price(PRICE * 960));
    assert_eq!(sim.events, ["lend.price"]);
}

#[test]
fn parameters_are_admin_only_and_bounded() {
    let mut sim = live();
    let mut config = default_config();
    config.ltv = 7_000;
    config.slope2 = 10_000;
    failure(&sim.call(BOB, ids::SET_PARAMS, &params_body(&config, false), &[]), ERR_AUTH);
    accepted(&sim.call(ADMIN, ids::SET_PARAMS, &params_body(&config, false), &[]));
    assert_eq!(sim.state().config.ltv, 7_000);
    assert_eq!(sim.events, ["lend.params"]);
    let mut bad = config;
    bad.liq = 6_900; // below ltv
    failure(&sim.call(ADMIN, ids::SET_PARAMS, &params_body(&bad, false), &[]), ERR_ARGS);
    let mut body = params_body(&config, false);
    body[22] = 2;
    failure(&sim.call(ADMIN, ids::SET_PARAMS, &body, &[]), ERR_ARGS);
    failure(&sim.call(ADMIN, ids::SET_PARAMS, &body[..22], &[]), ERR_ARGS);
    // The new LTV applies immediately.
    failure(&sim.amount(BOB, ids::BORROW, 700_001), ERR_UNHEALTHY);
}

#[test]
fn new_rates_do_not_apply_retroactively() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    sim.advance(YEAR);
    let mut config = default_config();
    config.base = 5_000;
    accepted(&sim.call(ADMIN, ids::SET_PARAMS, &params_body(&config, false), &[]));
    assert_eq!(sim.debt(BOB), 793_125); // the year before was priced at the old curve
}

#[test]
fn pausing_stops_new_risk_but_never_exits() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 500_000));
    accepted(&sim.deposit(CAROL, &[(DEBT, 100), (COL, 100)]));
    let paused = params_body(&default_config(), true);
    accepted(&sim.call(ADMIN, ids::SET_PARAMS, &paused, &[]));
    assert!(sim.state().paused);
    failure(&sim.amount(BOB, ids::BORROW, 1), ERR_PAUSED);
    failure(&sim.amount(CAROL, ids::SUPPLY, 100), ERR_PAUSED);
    failure(&sim.amount(CAROL, ids::ADD_COLLATERAL, 100), ERR_PAUSED);
    // Deposits, repayments, redemptions, collateral exits and liquidations stay open.
    accepted(&sim.deposit(CAROL, &[(DEBT, 1)]));
    accepted(&sim.amount(BOB, ids::REPAY, 100_000));
    accepted(&sim.amount(ALICE, ids::REDEEM, 100_000));
    accepted(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 100_000));
    accepted(&sim.set_price(PRICE / 100 * 85));
    accepted(&sim.set_price(PRICE / 100 * 72));
    accepted(&sim.set_price(PRICE / 100 * 60));
    accepted(&sim.set_price(PRICE / 100 * 50));
    accepted(&sim.deposit(CAROL, &[(DEBT, 500_000)]));
    accepted(&sim.liquidate(CAROL, BOB, 100_000));
    let unpaused = params_body(&default_config(), false);
    accepted(&sim.call(ADMIN, ids::SET_PARAMS, &unpaused, &[]));
    sim.assert_accounting();
}

#[test]
fn only_the_admin_collects_reserves_and_only_when_there_are_some() {
    let mut sim = live();
    failure(&sim.call(ADMIN, ids::COLLECT_RESERVES, &[], &[]), ERR_ZERO);
    accepted(&sim.amount(BOB, ids::BORROW, 750_000));
    sim.advance(YEAR);
    failure(&sim.call(BOB, ids::COLLECT_RESERVES, &[], &[]), ERR_AUTH);
    accepted(&sim.call(ADMIN, ids::COLLECT_RESERVES, &[], &[]));
    assert_eq!(sim.account(ADMIN).balance[1], 4_312);
    assert_eq!(sim.state().reserves, 0);
    assert_eq!(sim.state().cash, 250_000 - 4_312);
    failure(&sim.call(ADMIN, ids::COLLECT_RESERVES, &[1], &[]), ERR_ARGS);
    sim.assert_accounting();
}

#[test]
fn shell_tops_up_fuel_and_only_shell() {
    let mut sim = Sim::new();
    accepted(&sim.call(ADMIN, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 500)]));
    assert_eq!(sim.balance(SHELL_CURRENCY_ID), 500);
    failure(&sim.call(ADMIN, ids::TOPUP, &[], &[(DEBT, 500)]), ERR_VALUE);
    failure(&sim.call(ADMIN, ids::TOPUP, &[], &[]), ERR_VALUE);
    // A framing that shows the balance from before this message still passes.
    let output =
        sim.call_with_pre_credit_balance(ADMIN, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 5)]);
    accepted(&output);
}

#[test]
fn a_market_that_lost_funds_refuses_to_run() {
    let mut sim = live();
    sim.vault.insert(DEBT, 999_999); // one unit short of the ledger
    failure(&sim.amount(BOB, ids::REPAY, 1), ERR_INSOLVENT);
    failure(&sim.set_price(PRICE), ERR_INSOLVENT);
}

#[test]
fn accounts_are_capped_and_empty_ones_are_dropped() {
    let mut sim = Sim::new();
    for index in 0..24u8 {
        accepted(&sim.deposit([0x80 + index; 32], &[(DEBT, 1)]));
    }
    failure(&sim.deposit([0x7f; 32], &[(DEBT, 1)]), ERR_CAPACITY);
    failure(&sim.deposit([0x7f; 32], &[(COL, 1)]), ERR_CAPACITY);
    // Existing accounts keep working at capacity.
    accepted(&sim.deposit([0x80; 32], &[(COL, 1)]));
}

#[test]
fn withdrawals_are_validated_before_anything_moves() {
    let mut sim = live();
    let to = addr(20);
    failure(&sim.withdraw(OUTSIDER, DEBT, 1, to), ERR_BALANCE);
    failure(&sim.withdraw(BOB, 77, 1, to), ERR_ARGS);
    failure(&sim.withdraw(BOB, SHELL_CURRENCY_ID, 1, to), ERR_ARGS);
    failure(&sim.withdraw(BOB, COL, 0, to), ERR_ARGS);
    failure(&sim.withdraw(BOB, COL, 1, addr(POOL)), ERR_ARGS); // to itself
    failure(
        &sim.withdraw(
            BOB,
            COL,
            1,
            avm_contract_sdk::AccountAddress { dapp_id: [0; 32], account_id: [1; 32] },
        ),
        ERR_ARGS,
    );
    failure(&sim.withdraw(BOB, COL, 1, to), ERR_BALANCE); // everything is locked as collateral
    accepted(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 10));
    accepted(&sim.withdraw(BOB, COL, 10, to));
    failure(&sim.withdraw(BOB, COL, 1, addr(21)), ERR_BALANCE);
    // Without a FUEL reserve the payout is refused.
    let mut broke = Sim::new();
    accepted(&broke.deposit(BOB, &[(COL, 5)]));
    failure(&broke.withdraw(BOB, COL, 5, to), ERR_INSOLVENT);
}

#[test]
fn a_second_payout_to_the_same_recipient_waits_for_the_first() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::REMOVE_COLLATERAL, 100));
    accepted(&sim.withdraw(BOB, COL, 40, addr(20)));
    failure(&sim.withdraw(BOB, COL, 40, addr(20)), ERR_PENDING);
    accepted(&sim.withdraw(BOB, COL, 40, addr(21)));
    assert_eq!(sim.state().pending.len(), 2);
    sim.assert_accounting();
}

#[test]
fn corrupt_state_is_rejected_on_every_call() {
    let sim = live();
    let good = sim.state;
    let mut cases: Vec<Vec<u8>> = Vec::new();
    cases.push(good[..good.len() - 1].to_vec());
    for offset in [81usize, 108, 196, 58] {
        let mut bytes = good.clone();
        bytes[offset] = 1;
        cases.push(bytes);
    }
    let mut bytes = good.clone();
    bytes[80] = 2; // flag out of range
    cases.push(bytes);
    let mut bytes = good.clone();
    bytes[176..192].copy_from_slice(&1u128.to_le_bytes()); // index below one
    cases.push(bytes);
    let mut bytes = good.clone();
    bytes[144] ^= 1; // share total no longer matches the accounts
    cases.push(bytes);
    let mut bytes = good.clone();
    bytes[160] ^= 1; // debt total no longer matches
    cases.push(bytes);
    let mut bytes = good.clone();
    bytes[200] ^= 1; // collateral total no longer matches
    cases.push(bytes);
    let mut bytes = good.clone();
    bytes[104] = 0; // price set but never stamped
    bytes[105] = 0;
    cases.push(bytes);
    for bad in cases {
        assert_eq!(State::decode(&bad).err(), Some(ERR_STATE));
    }
    assert!(State::decode(&good).is_ok());
}

#[test]
fn every_state_the_contract_returns_decodes_back_to_itself() {
    let mut sim = live();
    accepted(&sim.amount(BOB, ids::BORROW, 300_000));
    sim.advance(1_234);
    accepted(&sim.set_price(PRICE));
    let state = State::decode(&sim.state).unwrap();
    assert_eq!(state.encode(), sim.state);
}

#[test]
fn the_manifest_declares_the_real_state_size() {
    let manifest = include_str!("../../contracts/lending/avm-contract.toml");
    assert!(manifest.contains(&format!("len = {}", avm_lending_market::STATE_BYTES)));
}
