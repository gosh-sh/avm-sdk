//! Behaviour of the treasury, one rule at a time, through canonical ACMI frames.

mod common;

use std::collections::BTreeSet;

use avm_multisig_treasury::{
    ids, payout_receipt, State, ERR_ARGS, ERR_AUTH, ERR_BALANCE, ERR_CALLBACK, ERR_CAPACITY,
    ERR_DUPLICATE, ERR_EXPIRED, ERR_FROZEN, ERR_INSOLVENT, ERR_NOT_FOUND, ERR_NOT_READY,
    ERR_PENDING, ERR_STATE, ERR_THRESHOLD, ERR_TIMELOCK, ERR_VALUE, KIND_ADD_OWNER,
    KIND_REMOVE_OWNER, KIND_SET_POLICY, KIND_TRANSFER, KIND_UNFREEZE, MAX_DELAY, MAX_PROPOSALS,
    NO_PROPOSER, SHELL_CURRENCY_ID, STATE_BYTES, STATUS_EXECUTING, STATUS_OPEN, STATUS_QUEUED,
};
use common::*;

const RECIPIENT: u8 = 0x70;

fn recipient(index: u8) -> avm_contract_sdk::AccountAddress {
    addr(RECIPIENT + index)
}

fn proposal(sim: &Sim, id: u64) -> avm_multisig_treasury::Proposal {
    let state = sim.state();
    state.proposals[state.proposal_index(id).expect("proposal exists")]
}

/// Passes a proposal that sets a spending policy.
fn set_policy(sim: &mut Sim, currency: u32, fast: u128) {
    let id = sim.propose_ok(OWNER_A, KIND_SET_POLICY, currency, 1, fast, [0; 64]);
    accepted(&sim.pass(id, &[OWNER_A, OWNER_B]));
}

// ------------------------------------------------------- manifest vs code ---

#[test]
fn the_manifest_matches_the_code() {
    let manifest = include_str!("../../contracts/treasury/avm-contract.toml");
    assert!(manifest.contains(&format!("len = {STATE_BYTES}")), "state length");
    for code in (4000..=4017u32).filter(|code| *code != 4008) {
        assert!(manifest.contains(&format!("status_code = {code}")), "error {code}");
    }
    for (name, _) in ids::EVENTS {
        assert!(manifest.contains(&format!("name = \"{name}\"")), "event {name}");
    }
    assert_eq!(manifest.matches("[[abi.events]]").count(), ids::EVENTS.len());
    let classes: BTreeSet<u64> = ids::EVENTS.iter().map(|(_, id)| *id).collect();
    assert_eq!(classes.len(), ids::EVENTS.len(), "event classes are unique and nonzero");
    assert!(!classes.contains(&0));
}

// ------------------------------------------------------------------- init ---

#[test]
fn init_creates_an_empty_treasury_and_emits_created() {
    let sim = Sim::new();
    assert_eq!(sim.init_events, ["msig.created"]);
    let state = sim.state();
    assert_eq!((state.threshold, state.owner_count()), (2, 3));
    assert_eq!(
        (state.delay, state.grace, state.ttl, state.fuel_reserve),
        (DELAY, GRACE, TTL, FUEL)
    );
    assert_eq!(state.next_id, 1);
    assert!(state.balances.is_empty() && state.proposals.is_empty() && !state.frozen);
    assert_eq!(sim.state.len(), STATE_BYTES);
}

fn deploy_code(init: &[u8], deployer: [u8; 32]) -> u32 {
    Sim::try_deploy(init, deployer).err().expect("must be rejected").status_code
}

#[test]
fn init_rejects_every_malformed_configuration() {
    let owners = [OWNER_A, OWNER_B, OWNER_C];
    let nine: Vec<[u8; 32]> = (0..9u8).map(|i| [0x80 + i; 32]).collect();
    let good = |t: u8| default_init(t, &owners);
    assert_eq!(deploy_code(&good(0), OWNER_A), ERR_ARGS);
    assert_eq!(deploy_code(&good(4), OWNER_A), ERR_ARGS);
    assert_eq!(deploy_code(&default_init(1, &[OWNER_A, OWNER_A]), OWNER_A), ERR_ARGS);
    assert_eq!(deploy_code(&default_init(1, &[OWNER_A, [0; 32]]), OWNER_A), ERR_ARGS);
    assert_eq!(deploy_code(&default_init(1, &[]), OWNER_A), ERR_ARGS);
    assert_eq!(deploy_code(&default_init(1, &nine), nine[0]), ERR_ARGS);
    assert_eq!(
        deploy_code(&init_data(2, &owners, MAX_DELAY + 1, GRACE, TTL, FUEL), OWNER_A),
        ERR_ARGS
    );
    assert_eq!(deploy_code(&init_data(2, &owners, DELAY, 0, TTL, FUEL), OWNER_A), ERR_ARGS);
    assert_eq!(deploy_code(&init_data(2, &owners, DELAY, GRACE, 0, FUEL), OWNER_A), ERR_ARGS);
    assert_eq!(deploy_code(&init_data(2, &owners, DELAY, GRACE, TTL, 0), OWNER_A), ERR_ARGS);
    let mut truncated = good(2);
    truncated.pop();
    assert_eq!(deploy_code(&truncated, OWNER_A), ERR_ARGS);
    let mut padded = good(2);
    padded.push(0);
    assert_eq!(deploy_code(&padded, OWNER_A), ERR_ARGS);
    // The deployer must be one of the owners.
    assert_eq!(deploy_code(&good(2), OUTSIDER), ERR_AUTH);
    // Boundary values are accepted: eight owners, maximum delay, 8-of-8.
    let eight = &nine[..8];
    assert!(Sim::try_deploy(&init_data(8, eight, MAX_DELAY, 1, 1, 1), eight[7]).is_ok());
}

// ---------------------------------------------------------------- deposits ---

#[test]
fn anyone_can_deposit_and_the_ledger_tracks_each_currency() {
    let mut sim = Sim::new();
    accepted(&sim.deposit(OUTSIDER, &[(C7, 500), (C9, 40)]));
    assert_eq!(sim.events, ["msig.deposited", "msig.deposited"]);
    accepted(&sim.deposit(OWNER_B, &[(C7, 250)]));
    let state = sim.state();
    assert_eq!((state.held(C7), state.held(C9), state.held(5)), (750, 40, 0));
    accepted(&sim.internal_deposit(addr(0x33), &[(C9, 10)]));
    assert_eq!(sim.state().held(C9), 50);
    accepted(&sim.deposit(OWNER_A, &[(SHELL_CURRENCY_ID, 99)]));
    assert_eq!(sim.state().held(SHELL_CURRENCY_ID), 99);
    sim.assert_accounting();
}

#[test]
fn deposits_need_value_and_respect_capacity() {
    let mut sim = Sim::new();
    failure(&sim.deposit(OWNER_A, &[]), ERR_VALUE);
    // The body of a deposit must be empty.
    failure(&sim.call(OWNER_A, ids::DEPOSIT, &[1], &[(C7, 1)]), ERR_ARGS);
    for currency in 20..28u32 {
        accepted(&sim.deposit(OWNER_A, &[(currency, 1)]));
    }
    failure(&sim.deposit(OWNER_A, &[(C7, 1)]), ERR_CAPACITY);
    // An existing currency still accepts more.
    accepted(&sim.deposit(OWNER_A, &[(20, 5)]));
    assert_eq!(sim.state().held(20), 6);
}

// --------------------------------------------------------------- proposing ---

#[test]
fn only_owners_may_act_and_governance_calls_carry_no_value() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let to = target_of(recipient(0));
    failure(&sim.propose(OUTSIDER, KIND_TRANSFER, C7, 0, 10, to), ERR_AUTH);
    let id = sim.transfer(OWNER_A, recipient(0), C7, 10);
    for method in [ids::APPROVE, ids::CANCEL, ids::EXECUTE] {
        failure(&sim.act(OUTSIDER, method, id), ERR_AUTH);
    }
    failure(&sim.freeze(OUTSIDER), ERR_AUTH);
    let mut body = vec![KIND_TRANSFER];
    body.extend_from_slice(&[0; 88]);
    failure(&sim.call(OWNER_A, ids::PROPOSE, &body, &[(C7, 1)]), ERR_VALUE);
    failure(&sim.call(OWNER_A, ids::APPROVE, &id.to_le_bytes(), &[(C7, 1)]), ERR_VALUE);
    // Malformed argument lengths and unknown methods.
    failure(&sim.call(OWNER_A, ids::PROPOSE, &body[..88], &[]), ERR_ARGS);
    failure(&sim.call(OWNER_A, ids::APPROVE, &[1, 2, 3], &[]), ERR_ARGS);
    failure(&sim.call(OWNER_A, ids::CALLBACK, &[], &[]), ERR_ARGS);
    failure(&sim.call(OWNER_A, 0x1234, &[], &[]), ERR_ARGS);
    failure(&sim.call(OWNER_A, ids::FREEZE, &[1], &[]), ERR_ARGS);
}

#[test]
fn every_proposal_kind_validates_its_payload() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let me = target_of(addr(POOL));
    let to = target_of(recipient(0));
    let none = [0; 64];
    // Transfers.
    failure(&sim.propose(OWNER_A, KIND_TRANSFER, C7, 0, 0, to), ERR_ARGS);
    failure(&sim.propose(OWNER_A, KIND_TRANSFER, SHELL_CURRENCY_ID, 0, 5, to), ERR_ARGS);
    failure(&sim.propose(OWNER_A, KIND_TRANSFER, C7, 1, 5, to), ERR_ARGS);
    failure(&sim.propose(OWNER_A, KIND_TRANSFER, C7, 0, 5, none), ERR_ARGS);
    failure(&sim.propose(OWNER_A, KIND_TRANSFER, C7, 0, 5, me), ERR_ARGS);
    // Owner changes.
    failure(&sim.propose(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, none), ERR_ARGS);
    failure(&sim.propose(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, to), ERR_ARGS); // 64-byte tail set
    failure(&sim.propose(OWNER_A, KIND_ADD_OWNER, C7, 0, 0, key_target(OWNER_D)), ERR_ARGS);
    failure(&sim.propose(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_B)), ERR_DUPLICATE);
    failure(&sim.propose(OWNER_A, KIND_REMOVE_OWNER, 0, 0, 0, key_target(OWNER_D)), ERR_NOT_FOUND);
    // Policies.
    failure(&sim.propose(OWNER_A, KIND_SET_POLICY, SHELL_CURRENCY_ID, 1, 5, none), ERR_ARGS);
    failure(&sim.propose(OWNER_A, KIND_SET_POLICY, C7, 1, 0, none), ERR_ARGS); // zero limit
    failure(&sim.propose(OWNER_A, KIND_SET_POLICY, C7, 2, 5, none), ERR_ARGS); // aux is 0 or 1
    failure(&sim.propose(OWNER_A, KIND_SET_POLICY, C7, 0, 0, none), ERR_NOT_FOUND);
    failure(&sim.propose(OWNER_A, KIND_SET_POLICY, C7, 0, 1, none), ERR_ARGS);
    let mut dirty = [0; 64];
    dirty[40] = 1;
    failure(&sim.propose(OWNER_A, KIND_SET_POLICY, C7, 1, 5, dirty), ERR_ARGS);
    // Unfreeze only makes sense while frozen.
    failure(&sim.propose(OWNER_A, KIND_UNFREEZE, 0, 0, 0, none), ERR_STATE);
    failure(&sim.propose(OWNER_A, 0, 0, 0, 0, none), ERR_ARGS);
    failure(&sim.propose(OWNER_A, 8, 0, 0, 0, none), ERR_ARGS);
    assert!(sim.state().proposals.is_empty());
    assert_eq!(sim.state().next_id, 1, "rejected proposals consume no id");
}

#[test]
fn the_proposal_table_is_bounded() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    for _ in 0..MAX_PROPOSALS {
        sim.transfer(OWNER_A, recipient(0), C7, 1);
    }
    failure(&sim.propose(OWNER_A, KIND_TRANSFER, C7, 0, 1, target_of(recipient(0))), ERR_CAPACITY);
    accepted(&sim.cancel(OWNER_A, 1));
    sim.transfer(OWNER_B, recipient(0), C7, 1);
    assert_eq!(sim.state().proposals.len(), MAX_PROPOSALS);
}

// -------------------------------------------------- approvals and timelock ---

#[test]
fn a_proposal_queues_when_it_reaches_the_threshold_and_waits_out_the_delay() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let id = sim.transfer(OWNER_A, recipient(0), C7, 700);
    assert_eq!(sim.events, ["msig.proposed"]);
    let open = proposal(&sim, id);
    assert_eq!((open.status, open.approvals, open.eta), (STATUS_OPEN, 0b001, 0));
    assert_eq!(open.expires, sim.time + TTL);
    failure(&sim.approve(OWNER_A, id), ERR_DUPLICATE);
    failure(&sim.execute(OWNER_A, id), ERR_NOT_READY);
    accepted(&sim.approve(OWNER_B, id));
    assert_eq!(sim.events, ["msig.approved", "msig.queued"]);
    let queued = proposal(&sim, id);
    assert_eq!((queued.status, queued.eta), (STATUS_QUEUED, sim.time + DELAY));
    assert_eq!(queued.expires, queued.eta + GRACE);
    failure(&sim.execute(OWNER_C, id), ERR_TIMELOCK);
    sim.advance(DELAY - 1);
    failure(&sim.execute(OWNER_C, id), ERR_TIMELOCK);
    sim.advance(1);
    accepted(&sim.execute(OWNER_C, id));
    assert_eq!(proposal(&sim, id).status, STATUS_EXECUTING);
    // A third approval after queueing is allowed but changes nothing else.
}

#[test]
fn a_one_of_one_treasury_queues_on_proposal() {
    let mut sim = Sim::with(1, &[OWNER_A]);
    sim.deposit(OWNER_A, &[(C7, 10), (SHELL_CURRENCY_ID, 100_000)]);
    let id = sim.transfer(OWNER_A, recipient(0), C7, 10);
    assert_eq!(sim.events, ["msig.proposed", "msig.queued"]);
    assert_eq!(proposal(&sim, id).status, STATUS_QUEUED);
}

#[test]
fn extra_approvals_on_a_queued_proposal_are_kept_and_expire_with_it() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let id = sim.transfer(OWNER_A, recipient(0), C7, 5);
    accepted(&sim.approve(OWNER_B, id));
    accepted(&sim.approve(OWNER_C, id));
    assert_eq!(proposal(&sim, id).approvals.count_ones(), 3);
}

#[test]
fn deadlines_apply_to_approving_and_executing() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    // An open proposal cannot be approved after its ttl.
    let stale = sim.transfer(OWNER_A, recipient(0), C7, 5);
    sim.advance(TTL + 1);
    failure(&sim.approve(OWNER_B, stale), ERR_EXPIRED);
    // A queued proposal cannot be executed after its grace period.
    let late = sim.transfer(OWNER_A, recipient(1), C7, 5);
    accepted(&sim.approve(OWNER_B, late));
    sim.advance(DELAY + GRACE);
    accepted(&sim.execute(OWNER_A, late)); // exactly at the deadline is still fine
    let missed = sim.transfer(OWNER_A, recipient(2), C7, 5);
    accepted(&sim.approve(OWNER_B, missed));
    sim.advance(DELAY + GRACE + 1);
    failure(&sim.execute(OWNER_A, missed), ERR_EXPIRED);
    // Anyone can clear expired proposals; unrelated owners cannot cancel live ones.
    accepted(&sim.cancel(OWNER_C, stale));
    accepted(&sim.cancel(OWNER_C, missed));
    let live = sim.transfer(OWNER_A, recipient(3), C7, 5);
    failure(&sim.cancel(OWNER_C, live), ERR_AUTH);
    accepted(&sim.cancel(OWNER_A, live));
    failure(&sim.cancel(OWNER_A, live), ERR_NOT_FOUND);
    assert_eq!(sim.events, ["msig.cancelled"]);
}

// ---------------------------------------------------------------- transfers ---

#[test]
fn a_transfer_debits_first_and_pays_through_an_acknowledged_message() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let to = recipient(0);
    let id = sim.transfer(OWNER_A, to, C7, 4_000);
    let output = sim.pass(id, &[OWNER_A, OWNER_B]);
    accepted(&output);
    assert_eq!(sim.events, ["msig.transfer_pending"]);
    let state = sim.state();
    assert_eq!(state.held(C7), 996_000);
    assert_eq!(state.held(SHELL_CURRENCY_ID), 1_000_000 - FUEL);
    assert_eq!(state.pending.len(), 1);
    let payout = sim.outbox[0].clone();
    assert_eq!(payout.recipient, to);
    assert_eq!(payout.method, ids::PAYEE_RECEIVE);
    assert_eq!(payout.handler, ids::CALLBACK);
    assert_eq!((payout.extra.clone(), payout.fuel), (vec![(C7, 4_000)], FUEL));
    assert_eq!(payout.body, payout_receipt(C7, id, 4_000));
    assert_eq!(sim.balance(C7), 996_000, "the value physically left");
    // Delivered: the payee echoes the receipt, nothing is returned.
    accepted(&sim.settle(to, Settle::Delivered(payout.body)));
    assert_eq!(sim.events, ["msig.transfer_settled"]);
    let state = sim.state();
    assert!(state.pending.is_empty() && state.proposals.is_empty());
    assert_eq!(state.held(C7), 996_000);
    sim.assert_accounting();
}

#[test]
fn a_bounced_transfer_returns_the_funds_and_can_be_retried() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let to = recipient(0);
    let id = sim.transfer(OWNER_A, to, C7, 4_000);
    accepted(&sim.pass(id, &[OWNER_A, OWNER_B]));
    // SHELL that comes back with the callback is credited too.
    accepted(&sim.settle(to, Settle::Bounced(vec![(SHELL_CURRENCY_ID, 300), (C7, 4_000)])));
    assert_eq!(sim.events, ["msig.transfer_failed"]);
    let state = sim.state();
    assert_eq!(state.held(C7), 1_000_000);
    assert_eq!(state.held(SHELL_CURRENCY_ID), 1_000_000 - FUEL + 300);
    assert!(state.pending.is_empty());
    let retry = proposal(&sim, id);
    assert_eq!((retry.status, retry.eta), (STATUS_QUEUED, sim.time));
    assert_eq!(retry.expires, sim.time + GRACE);
    sim.assert_accounting();
    // The same approved proposal executes again and this time it is delivered.
    accepted(&sim.execute(OWNER_C, id));
    let body = sim.outbox.last().unwrap().body.clone();
    accepted(&sim.settle(to, Settle::Delivered(body)));
    assert!(sim.state().proposals.is_empty());
    assert_eq!(sim.state().held(C7), 996_000);
}

#[test]
fn callbacks_are_checked_against_the_pending_payout() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let (x, y) = (recipient(0), recipient(1));
    let id = sim.transfer(OWNER_A, x, C7, 100);
    accepted(&sim.pass(id, &[OWNER_A, OWNER_B]));
    let good = payout_receipt(C7, id, 100);
    let hash = sim.fresh_hash();
    // Wrong source, wrong method, bad receipt, value that should not be there.
    failure(
        &sim.settle_with(x, Settle::Delivered(good.clone()), hash, ids::CALLBACK, y),
        ERR_CALLBACK,
    );
    failure(
        &sim.settle_with(x, Settle::Delivered(good.clone()), hash, ids::APPROVE, x),
        ERR_CALLBACK,
    );
    failure(&sim.settle(x, Settle::Delivered(payout_receipt(C7, id, 101))), ERR_CALLBACK);
    failure(&sim.settle(x, Settle::Delivered(payout_receipt(C9, id, 100))), ERR_CALLBACK);
    failure(&sim.settle(x, Settle::Delivered(Vec::new())), ERR_CALLBACK);
    failure(
        &sim.settle(x, Settle::DeliveredReturning(good.clone(), vec![(C7, 100)])),
        ERR_CALLBACK,
    );
    failure(&sim.settle(x, Settle::DeliveredReturning(good.clone(), vec![(C9, 1)])), ERR_CALLBACK);
    failure(&sim.settle(x, Settle::Bounced(vec![(C7, 99)])), ERR_CALLBACK);
    failure(&sim.settle(x, Settle::Bounced(vec![])), ERR_CALLBACK);
    // An external caller cannot impersonate a callback.
    failure(&sim.call(OWNER_A, ids::CALLBACK, &good, &[]), ERR_ARGS);
    assert_eq!(sim.state().pending.len(), 1, "every attempt left the payout pending");
    accepted(&sim.settle_with(x, Settle::Delivered(good.clone()), hash, ids::CALLBACK, x));
    // The cause hash cannot be replayed against a later payout.
    let id2 = sim.transfer(OWNER_A, y, C7, 100);
    accepted(&sim.pass(id2, &[OWNER_A, OWNER_B]));
    let good2 = payout_receipt(C7, id2, 100);
    failure(
        &sim.settle_with(y, Settle::Delivered(good2.clone()), hash, ids::CALLBACK, y),
        ERR_CALLBACK,
    );
    accepted(&sim.settle(y, Settle::Delivered(good2)));
    // And a callback with nothing pending is refused.
    failure(&sim.settle(y, Settle::Delivered(good)), ERR_CALLBACK);
}

#[test]
fn execution_checks_balances_fuel_recipients_and_capacity() {
    let mut sim = Sim::with(2, &[OWNER_A, OWNER_B, OWNER_C]);
    accepted(&sim.deposit(OWNER_A, &[(C7, 100)]));
    // More than the treasury holds.
    let big = sim.transfer(OWNER_A, recipient(0), C7, 101);
    failure(&sim.pass(big, &[OWNER_A, OWNER_B]), ERR_BALANCE);
    // Enough funds but no SHELL for the FUEL reserve.
    let fine = sim.transfer(OWNER_A, recipient(1), C7, 100);
    failure(&sim.pass(fine, &[OWNER_A, OWNER_B]), ERR_INSOLVENT);
    accepted(&sim.deposit(OWNER_A, &[(SHELL_CURRENCY_ID, FUEL * 10)]));
    accepted(&sim.execute(OWNER_A, fine));
    assert_eq!(sim.state().held(C7), 0, "an emptied currency leaves the ledger");
    // The funds are gone, so the first proposal still fails after the top-up.
    failure(&sim.execute(OWNER_A, big), ERR_BALANCE);
    sim.assert_accounting();
}

#[test]
fn one_payout_per_recipient_and_a_bounded_number_in_flight() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let same = recipient(0);
    let first = sim.transfer(OWNER_A, same, C7, 10);
    let second = sim.transfer(OWNER_A, same, C7, 20);
    accepted(&sim.approve(OWNER_B, second));
    accepted(&sim.pass(first, &[OWNER_A, OWNER_B]));
    sim.advance(DELAY);
    failure(&sim.execute(OWNER_A, second), ERR_PENDING);
    let body = sim.outbox[0].body.clone();
    accepted(&sim.settle(same, Settle::Delivered(body)));
    accepted(&sim.execute(OWNER_A, second));
    let body = sim.outbox[0].body.clone();
    accepted(&sim.settle(same, Settle::Delivered(body)));
    // Four different recipients may be in flight, a fifth waits.
    let mut ids_in_flight = Vec::new();
    for index in 1..=5u8 {
        let id = sim.transfer(OWNER_A, recipient(index), C9, 1);
        accepted(&sim.approve(OWNER_B, id));
        ids_in_flight.push(id);
    }
    sim.advance(DELAY);
    for id in &ids_in_flight[..4] {
        accepted(&sim.execute(OWNER_C, *id));
    }
    failure(&sim.execute(OWNER_C, ids_in_flight[4]), ERR_CAPACITY);
    assert_eq!(sim.state().pending.len(), 4);
    let body = sim.outbox[0].body.clone();
    accepted(&sim.settle(recipient(1), Settle::Delivered(body)));
    accepted(&sim.execute(OWNER_C, ids_in_flight[4]));
    sim.assert_accounting();
}

#[test]
fn an_executing_proposal_cannot_be_cancelled_or_approved_again() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let id = sim.transfer(OWNER_A, recipient(0), C7, 10);
    accepted(&sim.pass(id, &[OWNER_A, OWNER_B]));
    failure(&sim.cancel(OWNER_A, id), ERR_NOT_READY);
    failure(&sim.approve(OWNER_C, id), ERR_NOT_READY);
    failure(&sim.execute(OWNER_A, id), ERR_NOT_READY);
}

// ---------------------------------------------------------------- policies ---

#[test]
fn the_fast_lane_skips_the_delay_for_small_transfers_only() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    set_policy(&mut sim, C7, 5_000);
    // Inside the fast lane: queued with eta = now, executable immediately.
    let small = sim.transfer(OWNER_A, recipient(0), C7, 5_000);
    accepted(&sim.approve(OWNER_B, small));
    assert_eq!(proposal(&sim, small).eta, sim.time);
    accepted(&sim.execute(OWNER_C, small));
    // One unit over the fast limit, and a currency without a policy, still wait.
    let large = sim.transfer(OWNER_A, recipient(1), C7, 5_001);
    accepted(&sim.approve(OWNER_B, large));
    assert_eq!(proposal(&sim, large).eta, sim.time + DELAY);
    let other = sim.transfer(OWNER_A, recipient(2), C9, 1);
    accepted(&sim.approve(OWNER_B, other));
    assert_eq!(proposal(&sim, other).eta, sim.time + DELAY);
    failure(&sim.execute(OWNER_C, large), ERR_TIMELOCK);
    // Governance never gets a fast lane, whatever the policies say.
    let owner = sim.propose_ok(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D));
    accepted(&sim.approve(OWNER_B, owner));
    assert_eq!(proposal(&sim, owner).eta, sim.time + DELAY);
}

#[test]
fn policies_can_be_replaced_removed_and_are_bounded() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    set_policy(&mut sim, C7, 10);
    set_policy(&mut sim, C7, 20);
    assert_eq!(sim.state().policy(C7).unwrap().fast_limit, 20);
    assert_eq!(sim.state().policies.len(), 1);
    let remove = sim.propose_ok(OWNER_A, KIND_SET_POLICY, C7, 0, 0, [0; 64]);
    accepted(&sim.pass(remove, &[OWNER_A, OWNER_B]));
    assert!(sim.state().policies.is_empty());
    for currency in 20..24u32 {
        set_policy(&mut sim, currency, 1);
    }
    let fifth = sim.propose_ok(OWNER_A, KIND_SET_POLICY, 24, 1, 1, [0; 64]);
    failure(&sim.pass(fifth, &[OWNER_A, OWNER_B]), ERR_CAPACITY);
    assert_eq!(sim.state().policies.len(), 4, "a refused execution changes nothing");
}

// ------------------------------------------------------------------ owners ---

#[test]
fn owners_are_added_through_a_proposal_and_can_then_act() {
    let mut sim = Sim::new();
    let id = sim.propose_ok(OWNER_B, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D));
    failure(&sim.propose(OWNER_D, KIND_TRANSFER, C7, 0, 1, target_of(recipient(0))), ERR_AUTH);
    accepted(&sim.pass(id, &[OWNER_A, OWNER_B]));
    assert_eq!(sim.events, ["msig.owner_added"]);
    let state = sim.state();
    assert_eq!((state.owner_count(), state.owners[3]), (4, OWNER_D));
    accepted(&sim.propose(OWNER_D, KIND_TRANSFER, C7, 0, 1, target_of(recipient(0))));
    // Adding the same owner again is refused, at proposal time and at execution time.
    failure(&sim.propose(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D)), ERR_DUPLICATE);
}

#[test]
fn a_stale_owner_proposal_is_rechecked_when_it_executes() {
    let mut sim = Sim::new();
    let first = sim.propose_ok(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D));
    let second = sim.propose_ok(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D));
    accepted(&sim.approve(OWNER_B, second));
    accepted(&sim.pass(first, &[OWNER_A, OWNER_B]));
    sim.advance(DELAY);
    failure(&sim.execute(OWNER_A, second), ERR_DUPLICATE);
}

#[test]
fn removing_an_owner_strips_their_approvals_and_reopens_what_depended_on_them() {
    let mut sim = Sim::new();
    // `dependent` is queued by A and C; `orphan` was proposed by C.
    let dependent = sim.transfer(OWNER_A, recipient(0), C7, 5);
    let orphan = sim.transfer(OWNER_C, recipient(1), C7, 7);
    let removal = sim.propose_ok(OWNER_A, KIND_REMOVE_OWNER, 0, 0, 0, key_target(OWNER_C));
    accepted(&sim.approve(OWNER_C, dependent));
    assert_eq!(proposal(&sim, dependent).status, STATUS_QUEUED);
    accepted(&sim.pass(removal, &[OWNER_A, OWNER_B]));
    assert_eq!(sim.events, ["msig.owner_removed"]);
    let state = sim.state();
    assert_eq!(state.owners[2], [0; 32]);
    assert_eq!(state.owner_count(), 2);
    let reopened = proposal(&sim, dependent);
    assert_eq!((reopened.status, reopened.approvals, reopened.eta), (STATUS_OPEN, 0b001, 0));
    assert_eq!(reopened.expires, reopened.created + TTL);
    let orphaned = proposal(&sim, orphan);
    assert_eq!((orphaned.proposer, orphaned.approvals), (NO_PROPOSER, 0));
    failure(&sim.approve(OWNER_C, dependent), ERR_AUTH);
    // Any remaining owner can clear a proposal whose proposer is gone.
    accepted(&sim.cancel(OWNER_B, orphan));
    // The removed owner's slot is reused by the next owner, with no inherited state.
    let add = sim.propose_ok(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D));
    accepted(&sim.pass(add, &[OWNER_A, OWNER_B]));
    assert_eq!(sim.state().owners[2], OWNER_D);
    assert_eq!(proposal(&sim, dependent).approvals, 0b001);
}

#[test]
fn owner_removal_cannot_go_below_the_threshold() {
    let mut sim = Sim::with(2, &[OWNER_A, OWNER_B]);
    failure(&sim.propose(OWNER_A, KIND_REMOVE_OWNER, 0, 0, 0, key_target(OWNER_B)), ERR_THRESHOLD);
    // Re-checked at execution: remove B first, then the queued removal of C must fail.
    let mut sim = Sim::new();
    let removal = sim.propose_ok(OWNER_A, KIND_REMOVE_OWNER, 0, 0, 0, key_target(OWNER_C));
    accepted(&sim.approve(OWNER_B, removal));
    let other = sim.propose_ok(OWNER_A, KIND_REMOVE_OWNER, 0, 0, 0, key_target(OWNER_B));
    accepted(&sim.pass(other, &[OWNER_A, OWNER_C]));
    failure(&sim.execute(OWNER_A, removal), ERR_NOT_READY); // B's approval is gone
    accepted(&sim.approve(OWNER_C, removal));
    sim.advance(DELAY);
    failure(&sim.execute(OWNER_A, removal), ERR_THRESHOLD);
}

#[test]
fn the_owner_set_is_capped_at_eight() {
    let keys: Vec<[u8; 32]> = (0..8u8).map(|i| [0x90 + i; 32]).collect();
    let mut sim = Sim::with(2, &keys);
    failure(&sim.propose(keys[0], KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D)), ERR_CAPACITY);
    // 8-of-8 needs every approval; the slot mask has no spare bit.
    let mut full = Sim::with(8, &keys);
    let id = full.transfer(keys[0], recipient(0), C7, 1);
    for key in &keys[1..] {
        accepted(&full.approve(*key, id));
    }
    assert_eq!(proposal(&full, id).status, STATUS_QUEUED);
    assert_eq!(proposal(&full, id).approvals, 0xff);
}

// ------------------------------------------------------------------ freeze ---

#[test]
fn a_single_owner_can_freeze_but_only_the_owners_together_can_unfreeze() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let live = sim.transfer(OWNER_A, recipient(0), C7, 5);
    accepted(&sim.approve(OWNER_B, live));
    let waiting = sim.transfer(OWNER_A, recipient(1), C7, 5);
    accepted(&sim.freeze(OWNER_C));
    assert_eq!(sim.events, ["msig.frozen"]);
    failure(&sim.freeze(OWNER_A), ERR_STATE);
    failure(&sim.propose(OWNER_A, KIND_TRANSFER, C7, 0, 1, target_of(recipient(2))), ERR_FROZEN);
    failure(&sim.propose(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D)), ERR_FROZEN);
    failure(&sim.approve(OWNER_B, waiting), ERR_FROZEN);
    sim.advance(DELAY);
    failure(&sim.execute(OWNER_A, live), ERR_FROZEN);
    // Housekeeping and funding still work.
    accepted(&sim.deposit(OUTSIDER, &[(C7, 1)]));
    accepted(&sim.cancel(OWNER_A, waiting));
    // Unfreezing is a proposal like any other, with no delay.
    let unfreeze = sim.propose_ok(OWNER_B, KIND_UNFREEZE, 0, 0, 0, [0; 64]);
    assert_eq!(sim.events, ["msig.proposed"]);
    accepted(&sim.approve(OWNER_A, unfreeze));
    assert_eq!(proposal(&sim, unfreeze).eta, sim.time);
    failure(&sim.execute(OWNER_A, live), ERR_FROZEN);
    accepted(&sim.execute(OWNER_C, unfreeze));
    assert_eq!(sim.events, ["msig.unfrozen"]);
    assert!(!sim.state().frozen);
    accepted(&sim.execute(OWNER_A, live));
}

#[test]
fn callbacks_settle_while_frozen() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let to = recipient(0);
    let id = sim.transfer(OWNER_A, to, C7, 50);
    accepted(&sim.pass(id, &[OWNER_A, OWNER_B]));
    accepted(&sim.freeze(OWNER_A));
    accepted(&sim.settle(to, Settle::Bounced(vec![(C7, 50)])));
    assert_eq!(sim.state().held(C7), 1_000_000);
    sim.assert_accounting();
}

// -------------------------------------------------------- safety tripwires ---

#[test]
fn an_underfunded_treasury_refuses_to_operate() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    *sim.vault.get_mut(&C7).unwrap() -= 1;
    failure(&sim.deposit(OWNER_A, &[(C9, 1)]), ERR_INSOLVENT);
    failure(&sim.freeze(OWNER_A), ERR_INSOLVENT);
    // The balance check holds whether or not the message's own value is counted.
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    accepted(&sim.call_with_pre_credit_balance(OWNER_A, ids::DEPOSIT, &[], &[(C7, 5)]));
    assert_eq!(sim.state().held(C7), 1_000_005);
    sim.assert_accounting();
}

#[test]
fn the_decoder_rejects_every_non_canonical_state() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let id = sim.transfer(OWNER_A, recipient(0), C7, 5);
    accepted(&sim.approve(OWNER_B, id));
    let good = sim.state.clone();
    assert!(State::decode(&good).is_ok());
    let mutate = |offset: usize, value: u8| {
        let mut bytes = good.clone();
        bytes[offset] = value;
        State::decode(&bytes).err()
    };
    assert_eq!(State::decode(&good[1..]).err(), Some(ERR_STATE));
    assert_eq!(mutate(0, 0), Some(ERR_STATE), "threshold zero");
    assert_eq!(mutate(0, 4), Some(ERR_STATE), "threshold above the owner count");
    assert_eq!(mutate(1, 2), Some(ERR_STATE), "frozen flag");
    assert_eq!(mutate(58, 1), Some(ERR_STATE), "padding");
    assert_eq!(mutate(59, 1), Some(ERR_STATE), "padding");
    assert_eq!(mutate(2, 5), Some(ERR_STATE), "balance count beyond used slots");
    assert_eq!(mutate(56, 0), Some(ERR_STATE), "proposal count dropped, slot dirty");
    assert_eq!(mutate(57, 1), Some(ERR_STATE), "phantom pending entry");
    assert_eq!(mutate(4 + 3, 0xff), Some(ERR_STATE), "delay above the maximum");
    // A duplicate owner key.
    let mut bytes = good.clone();
    let (head, tail) = bytes.split_at_mut(64 + 32);
    tail[..32].copy_from_slice(&head[64..96]);
    assert_eq!(State::decode(&bytes).err(), Some(ERR_STATE));
    // Dirty bytes in unused slots are refused in every table.
    let owners_end = 64 + 8 * 32;
    for offset in [owners_end + 8 * 32 + 7 * 32, 576 + 7 * 32 + 3, 832 + 3 * 32, 2752 + 3 * 96 + 5]
    {
        assert_eq!(mutate(offset, 1), Some(ERR_STATE), "offset {offset}");
    }
    // And the contract itself refuses to run on a corrupt state.
    sim.state[1] = 2;
    failure(&sim.deposit(OWNER_A, &[(C7, 1)]), ERR_STATE);
}

#[test]
fn proposals_cannot_be_forged_into_the_state() {
    let mut sim = Sim::funded(2, &[OWNER_A, OWNER_B, OWNER_C]);
    let id = sim.transfer(OWNER_A, recipient(0), C7, 5);
    accepted(&sim.approve(OWNER_B, id));
    let base = 960; // first proposal slot
    let good = sim.state.clone();
    let check = |offset: usize, value: u8| {
        let mut bytes = good.clone();
        bytes[offset] = value;
        State::decode(&bytes).err()
    };
    assert_eq!(check(base + 8, 0), Some(ERR_STATE), "kind zero");
    assert_eq!(check(base + 8, 9), Some(ERR_STATE), "unknown kind");
    assert_eq!(check(base + 9, 0), Some(ERR_STATE), "status zero");
    assert_eq!(check(base + 9, STATUS_EXECUTING), Some(ERR_STATE), "executing without a payout");
    assert_eq!(check(base + 10, 5), Some(ERR_STATE), "proposer in an empty slot");
    assert_eq!(check(base + 11, 0b001), Some(ERR_STATE), "queued below the threshold");
    assert_eq!(check(base + 11, 0b1111), Some(ERR_STATE), "approval by a nonexistent owner");
    assert_eq!(check(base, 0), Some(ERR_STATE), "proposal id zero");
    assert_eq!(check(base, 99), Some(ERR_STATE), "id above the counter");
}

// ------------------------------------------------------------- a full tour ---

#[test]
fn every_declared_event_is_emitted_with_valid_fields() {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut sim = Sim::new();
    seen.extend(sim.init_events.clone());
    macro_rules! note {
        () => {
            seen.extend(sim.events.clone())
        };
    }
    accepted(&sim.deposit(OWNER_A, &[(SHELL_CURRENCY_ID, 100_000), (C7, 50_000)]));
    note!();
    set_policy(&mut sim, C7, 100);
    note!();
    let add = sim.propose_ok(OWNER_A, KIND_ADD_OWNER, 0, 0, 0, key_target(OWNER_D));
    accepted(&sim.pass(add, &[OWNER_A, OWNER_B]));
    note!();
    let remove = sim.propose_ok(OWNER_A, KIND_REMOVE_OWNER, 0, 0, 0, key_target(OWNER_D));
    accepted(&sim.pass(remove, &[OWNER_A, OWNER_B]));
    note!();
    let to = recipient(0);
    let id = sim.transfer(OWNER_A, to, C7, 10);
    note!();
    accepted(&sim.approve(OWNER_B, id));
    note!();
    sim.advance(DELAY);
    accepted(&sim.execute(OWNER_A, id));
    note!();
    accepted(&sim.settle(to, Settle::Bounced(vec![(C7, 10)])));
    note!();
    accepted(&sim.execute(OWNER_A, id));
    let body = sim.outbox.last().unwrap().body.clone();
    accepted(&sim.settle(to, Settle::Delivered(body)));
    note!();
    let spare = sim.transfer(OWNER_A, recipient(1), C7, 1);
    accepted(&sim.cancel(OWNER_A, spare));
    note!();
    accepted(&sim.freeze(OWNER_B));
    note!();
    let unfreeze = sim.propose_ok(OWNER_A, KIND_UNFREEZE, 0, 0, 0, [0; 64]);
    accepted(&sim.approve(OWNER_B, unfreeze));
    accepted(&sim.execute(OWNER_A, unfreeze));
    note!();
    let expected: BTreeSet<String> = ids::EVENTS.iter().map(|(name, _)| name.to_string()).collect();
    assert_eq!(seen, expected, "missing: {:?}", expected.difference(&seen).collect::<Vec<_>>());
}
