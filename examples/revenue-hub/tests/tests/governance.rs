mod support;
use avm_revenue_core::{vault::*, *};
use support::*;
fn p(nonce: u64, action: u8, value: u128) -> Vec<u8> {
    Proposal { nonce, action, value, ..Proposal::default() }.body()
}
#[test]
fn outsider_cannot_create_or_vote_proposal() {
    let mut v = Vault::new();
    fails(&v.call(STRANGER, ids::PROPOSE, p(1, PAUSE, 0), 0), E_AUTH);
    v.call(SIGNERS[0], ids::PROPOSE, p(1, PAUSE, 0), 0);
    fails(&v.call(STRANGER, ids::VOTE, p(1, PAUSE, 0), 0), E_AUTH);
}
#[test]
fn proposer_counts_as_one_vote_but_cannot_execute_alone() {
    let mut v = Vault::new();
    v.call(SIGNERS[0], ids::PROPOSE, p(1, PAUSE, 0), 0);
    v.time = 100;
    fails(&v.call(STRANGER, ids::EXECUTE, 1u64.to_le_bytes().to_vec(), 0), E_QUORUM);
}
#[test]
fn repeated_vote_cannot_create_quorum() {
    let mut v = Vault::new();
    v.call(SIGNERS[0], ids::PROPOSE, p(1, PAUSE, 0), 0);
    fails(&v.call(SIGNERS[0], ids::VOTE, p(1, PAUSE, 0), 0), E_DUPLICATE_VOTE);
    assert_eq!(v.snapshot().proposal.votes.count_ones(), 1);
}
#[test]
fn second_signer_and_timelock_are_both_required() {
    let mut v = Vault::new();
    v.call(SIGNERS[0], ids::PROPOSE, p(1, PAUSE, 0), 0);
    v.call(SIGNERS[1], ids::VOTE, p(1, PAUSE, 0), 0);
    fails(&v.call(STRANGER, ids::EXECUTE, 1u64.to_le_bytes().to_vec(), 0), E_TIMELOCK);
    v.time = v.snapshot().proposal.ready - 1;
    assert_eq!(v.call(STRANGER, ids::EXECUTE, 1u64.to_le_bytes().to_vec(), 0).status_code, 0);
    assert!(v.snapshot().paused);
}
#[test]
fn any_distinct_pair_of_managers_forms_quorum() {
    for pair in [(0, 1), (0, 2), (1, 2)] {
        let mut v = Vault::new();
        v.call(SIGNERS[pair.0], ids::PROPOSE, p(1, PAUSE, 0), 0);
        v.call(SIGNERS[pair.1], ids::VOTE, p(1, PAUSE, 0), 0);
        v.time = v.snapshot().proposal.ready;
        assert_eq!(v.call(STRANGER, ids::EXECUTE, 1u64.to_le_bytes().to_vec(), 0).status_code, 0);
    }
}
#[test]
fn vote_binds_exact_action_and_value() {
    let mut v = Vault::new();
    v.call(SIGNERS[0], ids::PROPOSE, p(1, SET_FUEL, 20000), 0);
    for body in [p(1, PAUSE, 0), p(1, SET_FUEL, 20001), p(2, SET_FUEL, 20000)] {
        fails(&v.call(SIGNERS[1], ids::VOTE, body, 0), E_GOVERNANCE);
    }
}
#[test]
fn active_proposal_cannot_be_replaced() {
    let mut v = Vault::new();
    v.call(SIGNERS[0], ids::PROPOSE, p(1, PAUSE, 0), 0);
    fails(&v.call(SIGNERS[1], ids::PROPOSE, p(2, RESUME, 0), 0), E_GOVERNANCE);
}
#[test]
fn expired_proposal_can_be_replaced_with_next_nonce() {
    let mut v = Vault::new();
    v.call(SIGNERS[0], ids::PROPOSE, p(1, PAUSE, 0), 0);
    v.time = v.snapshot().proposal.expires;
    assert_eq!(v.call(SIGNERS[1], ids::PROPOSE, p(2, RESUME, 0), 0).status_code, 0);
    assert_eq!(v.snapshot().proposal.votes, 2);
}
#[test]
fn expired_votes_and_execution_are_rejected() {
    let mut v = Vault::new();
    v.call(SIGNERS[0], ids::PROPOSE, p(1, PAUSE, 0), 0);
    v.call(SIGNERS[1], ids::VOTE, p(1, PAUSE, 0), 0);
    v.time = v.snapshot().proposal.expires - 1;
    fails(&v.call(SIGNERS[2], ids::VOTE, p(1, PAUSE, 0), 0), E_EXPIRED);
    fails(&v.call(STRANGER, ids::EXECUTE, 1u64.to_le_bytes().to_vec(), 0), E_EXPIRED);
}
#[test]
fn executed_proposal_cannot_be_replayed() {
    let mut v = Vault::new();
    v.governance(PAUSE, 0);
    fails(&v.call(STRANGER, ids::EXECUTE, 1u64.to_le_bytes().to_vec(), 0), E_GOVERNANCE);
    fails(&v.call(SIGNERS[0], ids::PROPOSE, p(1, RESUME, 0), 0), E_GOVERNANCE);
}
#[test]
fn pause_stops_new_deposits_and_claims() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.governance(PAUSE, 0);
    fails(&v.call(STRANGER, ids::DEPOSIT, vec![], 1), E_PAUSED);
    fails(&v.claim(0), E_PAUSED);
}
#[test]
fn resume_restores_operations_without_changing_ownership() {
    let mut v = Vault::new();
    let before = v.snapshot().config.members;
    v.governance(PAUSE, 0);
    v.governance(RESUME, 0);
    v.deposit(1000);
    assert_eq!(v.claim(0).status_code, 0);
    assert_eq!(v.snapshot().config.members, before);
}
#[test]
fn fee_change_requires_quorum_and_does_not_change_principal() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.governance(SET_FUEL, 25000);
    assert_eq!(v.snapshot().fuel_limit, 25000);
    assert_eq!(v.snapshot().received, 1000);
    assert_eq!(v.claim(0).outbound_intents[0].fuel_fee_shells, 25000);
}
#[test]
fn unsupported_governance_actions_cannot_redirect_money() {
    let mut v = Vault::new();
    for action in [0, 4, 255] {
        fails(&v.call(SIGNERS[0], ids::PROPOSE, p(1, action, 0), 0), E_GOVERNANCE);
    }
    assert_eq!(v.snapshot().config.members, config().members);
}
#[test]
fn proposal_timestamp_overflow_fails_without_changes() {
    let mut v = Vault::new();
    v.time = u32::MAX - 1;
    fails(&v.call(SIGNERS[0], ids::PROPOSE, p(1, PAUSE, 0), 0), E_OVERFLOW);
}
