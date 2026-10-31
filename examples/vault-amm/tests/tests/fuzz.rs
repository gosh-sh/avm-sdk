//! Deterministic randomized sequences of every operation. After each step the
//! accounting identities must hold, `k` must never fall on a swap, and no value
//! may appear or disappear: deposits equal what the vault holds plus what has
//! been delivered out.

mod common;

use avm_vault_amm::{ids, payout_receipt, SHELL_CURRENCY_ID};
use common::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
    fn amount(&mut self) -> u128 {
        // A mix of dust, ordinary and large trades.
        match self.below(4) {
            0 => u128::from(self.below(50)) + 1,
            1 => u128::from(self.below(10_000)) + 1,
            2 => u128::from(self.below(1_000_000)) + 1,
            _ => u128::from(self.below(900_000_000)) + 1,
        }
    }
}

const ACTORS: [[u8; 32]; 4] = [ALICE, BOB, CAROL, MALLORY];

fn run(seed: u64, steps: usize) {
    let mut rng = Rng(seed);
    let mut sim = Sim::new(config(30, 2_500));
    let mut deposited = [0u128; 2];
    let mut delivered = [0u128; 2];
    let mut accepted_ops = 0usize;
    for step in 0..steps {
        let actor = ACTORS[rng.below(4) as usize];
        let direction = rng.below(2) as u8;
        let before = sim.pool();
        let output = match rng.below(12) {
            0 | 1 => {
                let (a0, a1) = (rng.amount() * 50, rng.amount() * 50);
                let output = sim.deposit(actor, a0, a1);
                if output.status_code == 0 {
                    deposited[0] += a0;
                    deposited[1] += a1;
                }
                output
            }
            2 => {
                let (a, b) = (rng.amount() * 10, rng.amount() * 10);
                sim.add_liquidity(actor, (a, b), (0, 0), NEVER)
            }
            3 => {
                let lp = sim.ledger(actor).1;
                let shares = if lp == 0 { 1 } else { u128::from(rng.below(lp as u64).max(1)) };
                sim.remove_liquidity(actor, shares, (0, 0), NEVER)
            }
            4..=6 => sim.swap_exact_in(actor, direction, rng.amount(), 0),
            7 | 8 => {
                let currency = [C0, C1][rng.below(2) as usize];
                let recipient = addr(10 + rng.below(6) as u8);
                let have = sim.ledger(actor).0[(currency == C1) as usize];
                let amount = if have == 0 { 1 } else { 1 + u128::from(rng.below(have as u64)) };
                sim.withdraw(actor, currency, amount, recipient)
            }
            9 => {
                // Settle the oldest outstanding payout, delivered or bounced.
                if sim.outbox.is_empty() {
                    continue;
                }
                let payout = sim.outbox[0].clone();
                let (currency, amount) = payout.extra[0];
                if rng.below(3) == 0 {
                    sim.settle(payout.recipient, Settle::Bounced(vec![(currency, amount)]))
                } else {
                    let output = sim.settle(payout.recipient, Settle::Delivered(payout.body));
                    if output.status_code == 0 {
                        delivered[(currency == C1) as usize] += amount;
                    }
                    output
                }
            }
            10 => {
                let fee = rng.below(1_001) as u16;
                let protocol = rng.below(5_001) as u16;
                sim.set_params(ADMIN, fee, protocol, rng.below(8) == 0)
            }
            _ => sim.call(ADMIN, ids::COLLECT_FEES, &[], &[]),
        };
        if output.status_code == 0 {
            accepted_ops += 1;
        } else {
            // A rejected call changes nothing at all.
            assert_eq!(sim.pool(), before, "seed {seed} step {step}: a failure mutated state");
        }
        sim.assert_accounting();
        let after = sim.pool();
        let was_swap = output.status_code == 0
            && output.event_intents.first().is_some_and(|e| e.event_name == "amm.swapped");
        if was_swap {
            assert!(
                product(&after) >= product(&before),
                "seed {seed} step {step}: k fell on a swap"
            );
        }
        // Conservation of value: everything deposited is held, in flight, or delivered.
        for side in 0..2 {
            assert_eq!(
                deposited[side],
                after.held(side).unwrap() + after.in_flight(side).unwrap() + delivered[side],
                "seed {seed} step {step}: value leaked on side {side}"
            );
        }
        assert!(after.lp_total == 0 || after.lp_total >= 1_000);
        assert!(sim.balance(SHELL_CURRENCY_ID) <= 1_000_000_000);
    }
    assert!(accepted_ops > steps / 5, "seed {seed}: the run barely did anything");
    // Drain every payout, then check the books close.
    while let Some(payout) = sim.outbox.first().cloned() {
        let (currency, amount) = payout.extra[0];
        accepted(&sim.settle(payout.recipient, Settle::Bounced(vec![(currency, amount)])));
        sim.assert_accounting();
    }
    let pool = sim.pool();
    assert!(pool.pending.is_empty());
}

#[test]
fn randomized_operation_sequences_preserve_every_invariant() {
    for seed in [1u64, 7, 42, 0xdead_beef, 0x1234_5678_9abc, 99_999] {
        run(seed, 1_500);
    }
}

#[test]
fn the_receipt_for_every_payout_is_a_pure_function_of_its_slot() {
    // Receipts are rebuilt by the pool on callback; recompute them independently here.
    let receipt = payout_receipt(C1, 77, 123_456_789_012_345_678_901_234_567_890);
    assert_eq!(receipt.len(), 32);
    assert_eq!(&receipt[..4], b"VAM1");
    assert_eq!(u32::from_le_bytes(receipt[4..8].try_into().unwrap()), C1);
    assert_eq!(u64::from_le_bytes(receipt[8..16].try_into().unwrap()), 77);
    assert_eq!(
        u128::from_le_bytes(receipt[16..32].try_into().unwrap()),
        123_456_789_012_345_678_901_234_567_890
    );
}
