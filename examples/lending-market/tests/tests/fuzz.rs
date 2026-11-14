//! Randomised sequences of every operation against a model of the invariants:
//! the ledger always equals what the market physically holds, value is
//! conserved, the share price never falls (except through a write-off), a
//! rejected call changes nothing, and borrowing never leaves a position above
//! its limit.

mod common;

use avm_lending_market::{ids, math::mul_div_floor, State, SHELL_CURRENCY_ID};
use common::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }
    fn amount(&mut self, max: u128) -> u128 {
        if max == 0 {
            return 0;
        }
        match self.below(4) {
            0 => 1 + u128::from(self.next()) % max.min(100),
            1 => max,
            _ => 1 + u128::from(self.next()) % max,
        }
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

const ACTORS: [[u8; 32]; 5] = [ALICE, BOB, CAROL, [0x55; 32], ADMIN];

#[derive(Default, Debug)]
struct Coverage {
    deposit: u32,
    supply: u32,
    redeem: u32,
    add_collateral: u32,
    remove_collateral: u32,
    borrow: u32,
    repay: u32,
    liquidate: u32,
    write_off: u32,
    withdraw: u32,
    delivered: u32,
    bounced: u32,
    reserves: u32,
    rejected: u32,
    stale: u32,
    price_cap: u32,
}

fn limit(state: &State, collateral: u128, bps: u16) -> u128 {
    let value = mul_div_floor(collateral, state.price, ONE).unwrap();
    mul_div_floor(value, bps.into(), 10_000).unwrap()
}

/// `a/b <= c/d` for nonzero b and d.
fn ratio_le(a: u128, b: u128, c: u128, d: u128) -> bool {
    use avm_lending_market::math::mul_wide;
    let (left, right) = (mul_wide(a, d), mul_wide(c, b));
    (left.hi, left.lo) <= (right.hi, right.lo)
}

/// The ledger equals the physical balance, exactly, and everything ever
/// deposited is held, in flight or delivered.
fn check_ledger(sim: &Sim, deposited: &[u128; 2], delivered: &[u128; 2], name: &str) {
    let state = sim.state();
    state.validate().unwrap();
    for side in 0..2 {
        let currency = state.config.currency[side];
        assert_eq!(
            state.held(side).unwrap(),
            sim.balance(currency),
            "{name}: ledger and vault differ for currency {currency}"
        );
        let in_flight: u128 = sim
            .outbox
            .iter()
            .flat_map(|d| d.extra.iter())
            .filter(|(c, _)| *c == currency)
            .map(|(_, a)| *a)
            .sum();
        assert_eq!(
            deposited[side],
            sim.balance(currency) + in_flight + delivered[side],
            "{name}: value not conserved for currency {currency}"
        );
    }
}

/// A market crash: the price falls 20% at a time while everyone liquidates
/// everyone and dead positions are written off, then it recovers.
fn crash(sim: &mut Sim, coverage: &mut Coverage) {
    let people = [ALICE, BOB, CAROL, [0x55; 32]];
    for _ in 0..16 {
        let price = sim.state().price;
        let _ = sim.set_price((price * 80 / 100).max(1));
        for liquidator in [CAROL, [0x55; 32]] {
            for target in people {
                if sim.liquidate(liquidator, target, u128::MAX).status_code == 0 {
                    coverage.liquidate += 1;
                }
            }
        }
        for target in people {
            if sim.call(ADMIN, ids::WRITE_OFF, &target, &[]).status_code == 0 {
                coverage.write_off += 1;
            }
        }
    }
    for _ in 0..16 {
        let price = sim.state().price;
        let _ = sim.set_price(price * 120 / 100);
    }
}

fn run(seed: u64, steps: usize, coverage: &mut Coverage) {
    let mut rng = Rng(seed | 1);
    let mut sim = Sim::new();
    let mut deposited = [0u128; 2];
    let mut delivered = [0u128; 2];
    accepted(&sim.call(ADMIN, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 100_000_000)]));
    accepted(&sim.set_price(PRICE));
    for step in 0..steps {
        if step % 400 == 399 {
            crash(&mut sim, coverage);
            check_ledger(&sim, &deposited, &delivered, "crash");
            continue;
        }
        let before = sim.state();
        let actor = rng.pick(&ACTORS);
        let balance = sim.state().account_index(actor).map(|i| sim.state().accounts[i]);
        let (idle, col, shares, scaled) = balance.map_or((0, 0, 0, 0), |a| {
            (a.balance[0], a.collateral, a.shares, sim.state().debt_of(a.scaled).unwrap())
        });
        let idle_debt = balance.map_or(0, |a| a.balance[1]);
        let mut wrote_off = false;
        let mut borrowed_or_removed = false;
        let mut liquidated = None;
        let (name, output) = match rng.below(20) {
            0 | 1 => {
                let (c, side) = if rng.below(2) == 0 { (DEBT, 1) } else { (COL, 0) };
                let amount = 1 + u128::from(rng.next()) % 2_000_000;
                let output = sim.deposit(actor, &[(c, amount)]);
                if output.status_code == 0 {
                    deposited[side] += amount;
                }
                ("deposit", output)
            }
            2 | 3 => ("supply", {
                let extra = rng.below(3) as u128;
                let amount = rng.amount(idle_debt + extra);
                sim.amount(actor, ids::SUPPLY, amount)
            }),
            4 => ("redeem", {
                let extra = rng.below(2) as u128;
                let amount = rng.amount(shares + extra);
                sim.amount(actor, ids::REDEEM, amount)
            }),
            5 | 6 => ("add_collateral", {
                let amount = rng.amount(idle);
                sim.amount(actor, ids::ADD_COLLATERAL, amount)
            }),
            7 => {
                borrowed_or_removed = true;
                ("remove_collateral", {
                    let amount = rng.amount(col);
                    sim.amount(actor, ids::REMOVE_COLLATERAL, amount)
                })
            }
            8 | 9 => {
                borrowed_or_removed = true;
                ("borrow", {
                    let amount = rng.amount(1_000_000);
                    sim.amount(actor, ids::BORROW, amount)
                })
            }
            10 | 11 => ("repay", {
                let amount = rng.amount(scaled + 10);
                sim.amount(actor, ids::REPAY, amount)
            }),
            12 | 13 => {
                let target = rng.pick(&ACTORS);
                let debt_before = balance
                    .and(sim.state().account_index(target))
                    .map(|i| sim.state().debt_of(sim.state().accounts[i].scaled).unwrap());
                liquidated = Some((target, debt_before.unwrap_or(0)));
                let amount = rng.amount(5_000_000);
                ("liquidate", sim.liquidate(actor, target, amount))
            }
            14 => {
                wrote_off = true;
                let target = rng.pick(&ACTORS);
                ("write_off", sim.call(actor, ids::WRITE_OFF, &target, &[]))
            }
            15 => ("withdraw", {
                let (c, held) = if rng.below(2) == 0 { (DEBT, idle_debt) } else { (COL, idle) };
                let to = addr(20 + rng.below(4) as u8);
                let extra = rng.below(2) as u128;
                let amount = rng.amount(held + extra);
                sim.withdraw(actor, c, amount, to)
            }),
            16 => ("settle", {
                if sim.outbox.is_empty() {
                    sim.set_price(sim.state().price)
                } else {
                    let index = rng.below(sim.outbox.len() as u64) as usize;
                    let payout = sim.outbox[index].clone();
                    let mut extra = payout.extra.clone();
                    extra.sort();
                    if rng.below(3) == 0 {
                        let output = sim.settle(payout.recipient, Settle::Bounced(extra));
                        if output.status_code == 0 {
                            coverage.bounced += 1;
                        }
                        output
                    } else {
                        let output = sim.settle(payout.recipient, Settle::Delivered(payout.body));
                        if output.status_code == 0 {
                            coverage.delivered += 1;
                            for (c, amount) in payout.extra {
                                delivered[usize::from(c == DEBT)] += amount;
                            }
                        }
                        output
                    }
                }
            }),
            17 => ("collect", sim.call(ADMIN, ids::COLLECT_RESERVES, &[], &[])),
            18 => {
                // Mostly small moves, occasionally past the 20% cap.
                let price = sim.state().price;
                let percent = match rng.below(10) {
                    0 => 70 + rng.below(70) as u128,
                    _ => 85 + rng.below(31) as u128,
                };
                let next = (price / 100 * percent).max(1);
                let output = sim.set_price(next);
                if output.status_code == avm_lending_market::ERR_PRICE_MOVE {
                    coverage.price_cap += 1;
                }
                ("set_price", output)
            }
            _ => {
                let seconds = match rng.below(6) {
                    0 => 4_000 + rng.below(100_000) as u32,
                    1 => YEAR / 4,
                    _ => rng.below(900) as u32,
                };
                sim.advance(seconds);
                // Usually re-stamp the price; sometimes let it go stale.
                if rng.below(4) == 0 {
                    ("advance", sim.call(ADMIN, ids::TOPUP, &[], &[(SHELL_CURRENCY_ID, 1)]))
                } else {
                    ("advance", sim.set_price(sim.state().price))
                }
            }
        };

        let after = sim.state();
        if output.status_code != 0 {
            coverage.rejected += 1;
            if output.status_code == avm_lending_market::ERR_STALE {
                coverage.stale += 1;
            }
            assert!(
                output.state_update.is_none()
                    && output.outbound_intents.is_empty()
                    && output.event_intents.is_empty(),
                "{name} failed with effects at step {step}"
            );
            assert_eq!(after, before, "{name} changed state while failing");
            continue;
        }
        match name {
            "deposit" => coverage.deposit += 1,
            "supply" => coverage.supply += 1,
            "redeem" => coverage.redeem += 1,
            "add_collateral" => coverage.add_collateral += 1,
            "remove_collateral" => coverage.remove_collateral += 1,
            "borrow" => coverage.borrow += 1,
            "repay" => coverage.repay += 1,
            "liquidate" => coverage.liquidate += 1,
            "write_off" => coverage.write_off += 1,
            "withdraw" => coverage.withdraw += 1,
            "collect" => coverage.reserves += 1,
            _ => {}
        }

        check_ledger(&sim, &deposited, &delivered, name);
        // 3. The share price never falls, except when bad debt is written off.
        if !wrote_off && before.total_shares > 0 && after.total_shares > 0 {
            assert!(
                ratio_le(
                    before.supply_value().unwrap(),
                    before.total_shares,
                    after.supply_value().unwrap(),
                    after.total_shares
                ),
                "{name}: share price fell at step {step}"
            );
        }
        // 4. Whoever just borrowed or pulled collateral is within the limit.
        if borrowed_or_removed {
            let account = after.accounts[after.account_index(actor).unwrap()];
            let debt = after.debt_of(account.scaled).unwrap();
            assert!(
                debt == 0 || debt <= limit(&after, account.collateral, after.config.ltv),
                "{name}: position above ltv at step {step}"
            );
        }
        // 5. A liquidation only touches an unhealthy position and at most half its debt.
        if let (Some((target, debt_before)), "liquidate") = (liquidated, name) {
            let old = before.accounts[before.account_index(target).unwrap()];
            assert!(debt_before > limit(&before, old.collateral, before.config.liq));
            let debt_after = after
                .account_index(target)
                .map_or(0, |i| after.debt_of(after.accounts[i].scaled).unwrap());
            assert!(debt_before - debt_after <= debt_before - debt_before / 2);
        }
        // 6. Debt, collateral and shares totals are the sums of the accounts (validate).
        after.validate().unwrap();
    }
}

#[test]
fn random_operations_preserve_every_invariant() {
    let mut coverage = Coverage::default();
    for seed in 1..=40u64 {
        run(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15), 1_500, &mut coverage);
    }
    println!("{coverage:?}");
    assert!(coverage.deposit > 2_000);
    assert!(coverage.supply > 300, "{coverage:?}");
    assert!(coverage.redeem > 100, "{coverage:?}");
    assert!(coverage.borrow > 200, "{coverage:?}");
    assert!(coverage.repay > 200, "{coverage:?}");
    assert!(coverage.liquidate > 20, "{coverage:?}");
    assert!(coverage.write_off > 0, "{coverage:?}");
    assert!(coverage.withdraw > 100, "{coverage:?}");
    assert!(coverage.delivered > 20 && coverage.bounced > 10, "{coverage:?}");
    assert!(coverage.reserves > 5, "{coverage:?}");
    assert!(coverage.stale > 0 && coverage.price_cap > 0, "{coverage:?}");
}
