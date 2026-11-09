//! Deterministic randomized sequences of every operation. After each step the
//! state must stay canonical, a rejected call must change nothing, the contract
//! must physically hold what it owns, and value must be conserved: everything
//! deposited is held, in flight, or delivered.

mod common;

use avm_multisig_treasury::{
    KIND_ADD_OWNER, KIND_REMOVE_OWNER, KIND_SET_POLICY, KIND_TRANSFER, KIND_UNFREEZE,
    SHELL_CURRENCY_ID,
};
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
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

const CANDIDATES: [[u8; 32]; 5] = [OWNER_A, OWNER_B, OWNER_C, OWNER_D, [0x55; 32]];

fn run(seed: u64, steps: usize) -> (usize, usize) {
    let mut rng = Rng(seed);
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let mut deposited = [1_000_000u128; 2];
    let mut delivered = [0u128; 2];
    let (mut accepted_calls, mut transfers_done) = (0, 0);
    for step in 0..steps {
        let state = sim.state();
        let owners: Vec<[u8; 32]> =
            state.owners.iter().copied().filter(|k| *k != [0; 32]).collect();
        let actor = if rng.below(20) == 0 { OUTSIDER } else { rng.pick(&owners) };
        let live: Vec<u64> = state.proposals.iter().map(|p| p.id).collect();
        let existing = |rng: &mut Rng| {
            if live.is_empty() || rng.below(15) == 0 {
                rng.below(40) + 1
            } else {
                rng.pick(&live)
            }
        };
        let queued: Vec<u64> = state
            .proposals
            .iter()
            .filter(|p| p.status == avm_multisig_treasury::STATUS_QUEUED)
            .map(|p| p.id)
            .collect();
        let before = sim.state.clone();
        let currency = rng.pick(&[C7, C9]);
        let side = usize::from(currency == C9);
        let mut deposit = None;
        let mut delivered_now = None;
        let output = match rng.below(100) {
            0..=7 => {
                let amount = u128::from(rng.below(50_000)) + 1;
                deposit = Some((side, amount));
                sim.deposit(actor, &[(currency, amount)])
            }
            8..=27 => {
                let to = addr(0x70 + rng.below(6) as u8);
                let amount = u128::from(rng.below(30_000)) + 1;
                sim.propose(actor, KIND_TRANSFER, currency, 0, amount, target_of(to))
            }
            28..=31 => {
                let key = rng.pick(&CANDIDATES);
                sim.propose(actor, KIND_ADD_OWNER, 0, 0, 0, key_target(key))
            }
            32..=34 => {
                let key = rng.pick(&CANDIDATES);
                sim.propose(actor, KIND_REMOVE_OWNER, 0, 0, 0, key_target(key))
            }
            40..=42 => {
                let fast = u128::from(rng.below(20_000));
                let on = rng.pick(&[0, 1, 1, 2]);
                sim.propose(actor, KIND_SET_POLICY, currency, on, fast, [0; 64])
            }
            43 => sim.propose(actor, KIND_UNFREEZE, 0, 0, 0, [0; 64]),
            44..=58 => {
                let id = existing(&mut rng);
                sim.approve(actor, id)
            }
            63..=65 => {
                let id = existing(&mut rng);
                sim.cancel(actor, id)
            }
            66..=79 => {
                let id = if queued.is_empty() || rng.below(10) == 0 {
                    existing(&mut rng)
                } else {
                    rng.pick(&queued)
                };
                sim.execute(actor, id)
            }
            80 => {
                if rng.below(4) == 0 {
                    sim.freeze(actor)
                } else {
                    continue;
                }
            }
            81 => {
                sim.advance(DELAY / 2);
                continue;
            }
            82..=89 => {
                sim.advance(rng.below(u64::from(2 * DELAY)) as u32);
                continue;
            }
            _ => {
                if sim.outbox.is_empty() {
                    continue;
                }
                let payout = sim.outbox[0].clone();
                let (currency, amount) = payout.extra[0];
                let side = usize::from(currency == C9);
                if rng.below(3) == 0 {
                    sim.settle(payout.recipient, Settle::Bounced(vec![(currency, amount)]))
                } else {
                    delivered_now = Some((side, amount));
                    sim.settle(payout.recipient, Settle::Delivered(payout.body))
                }
            }
        };
        if output.status_code == 0 {
            accepted_calls += 1;
            if let Some((side, amount)) = deposit {
                deposited[side] += amount;
            }
            if let Some((side, amount)) = delivered_now {
                delivered[side] += amount;
            }
            if sim.events.iter().any(|e| e == "msig.transfer_pending") {
                transfers_done += 1;
            }
        } else {
            assert_eq!(sim.state, before, "seed {seed} step {step}: a failure mutated state");
        }
        sim.assert_accounting();
        let after = sim.state();
        for (side, currency) in [C7, C9].into_iter().enumerate() {
            let in_flight: u128 =
                after.pending.iter().filter(|p| p.currency == currency).map(|p| p.amount).sum();
            assert_eq!(
                deposited[side],
                after.held(currency) + in_flight + delivered[side],
                "seed {seed} step {step}: value leaked in currency {currency}"
            );
        }
        // Governance invariants the type system cannot express.
        assert!(usize::from(after.threshold) <= after.owner_count());
        assert!(after.proposals.iter().all(|p| p.approvals & !after.owner_mask() == 0
            || p.status == avm_multisig_treasury::STATUS_EXECUTING));
        assert!(after.held(SHELL_CURRENCY_ID) <= 1_000_000);
    }
    // Drain: bounce everything still in flight; the books must close.
    while let Some(payout) = sim.outbox.first().cloned() {
        accepted(&sim.settle(payout.recipient, Settle::Bounced(payout.extra.clone())));
        sim.assert_accounting();
    }
    assert!(sim.state().pending.is_empty());
    (accepted_calls, transfers_done)
}

#[test]
fn randomized_operation_sequences_preserve_every_invariant() {
    let (mut accepted_total, mut transfers_total) = (0, 0);
    for seed in [1u64, 7, 42, 0xdead_beef, 0x1234_5678_9abc, 99_999, 31_337, 8_675_309] {
        let (done, transfers) = run(seed, 2_500);
        accepted_total += done;
        transfers_total += transfers;
    }
    // The run must actually exercise the machinery, not just bounce off validation.
    assert!(accepted_total > 2_500, "only {accepted_total} accepted calls");
    assert!(transfers_total > 100, "only {transfers_total} transfers started");
}

#[test]
fn the_fuzzer_reaches_governance_and_freeze_paths() {
    // Guard against a fuzzer that silently stops covering a feature: random
    // governance proposals driven through approve, wait and execute.
    let mut seen = std::collections::BTreeSet::new();
    for seed in [3u64, 5, 11] {
        let mut rng = Rng(seed);
        let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
        for _ in 0..600 {
            let owners: Vec<[u8; 32]> =
                sim.state().owners.iter().copied().filter(|k| *k != [0; 32]).collect();
            let proposer = rng.pick(&owners);
            let output = match rng.below(4) {
                0 => sim.propose(
                    proposer,
                    KIND_ADD_OWNER,
                    0,
                    0,
                    0,
                    key_target(rng.pick(&CANDIDATES)),
                ),
                1 => sim.propose(
                    proposer,
                    KIND_REMOVE_OWNER,
                    0,
                    0,
                    0,
                    key_target(rng.pick(&CANDIDATES)),
                ),
                2 => sim.freeze(proposer),
                _ => sim.propose(proposer, KIND_UNFREEZE, 0, 0, 0, [0; 64]),
            };
            if output.status_code != 0 {
                continue;
            }
            seen.extend(sim.events.clone());
            let Some(id) = sim.state().proposals.last().map(|p| p.id) else { continue };
            for approver in &owners {
                let _ = sim.approve(*approver, id);
                seen.extend(sim.events.clone());
            }
            sim.advance(DELAY);
            if sim.execute(rng.pick(&owners), id).status_code == 0 {
                seen.extend(sim.events.clone());
            }
            sim.assert_accounting();
        }
    }
    for name in ["msig.owner_added", "msig.owner_removed", "msig.frozen", "msig.unfrozen"] {
        assert!(seen.contains(name), "the fuzzer never produced {name}");
    }
}
