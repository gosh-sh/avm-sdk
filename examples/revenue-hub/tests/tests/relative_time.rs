mod support;
use avm_revenue_core::{vault::*, *};
use avm_revenue_vault::RevenueVault;
use support::*;
fn vault_at(origin: u32, delay: u32) -> Vault {
    let mut cfg = config(); cfg.delay = delay;
    let spec = Spec { deploy: true, body: envelope(&cfg.encode()), time: origin, ..Spec::default() };
    Vault { state: initialize::<RevenueVault>(&spec), time: origin, lt: 10 }
}
fn proposal() -> Vec<u8> { Proposal { nonce: 1, action: PAUSE, ..Proposal::default() }.body() }
#[test]
fn relative_proposal_offsets_do_not_contain_wall_clock_deadlines() {
    for origin in [0, 1_800_000_000, u32::MAX - 10] {
        let mut v = vault_at(origin, 10);
        assert_eq!(v.call(SIGNERS[0], ids::PROPOSE, proposal(), 0).status_code, 0);
        assert_eq!(v.snapshot().proposal.created, 1);
        assert_eq!(v.snapshot().proposal.ready, 11);
        assert_eq!(v.snapshot().proposal.expires, 111);
    }
}
#[test]
fn relative_expired_vote_and_execute_acknowledge_without_applying_action() {
    for method in [ids::VOTE, ids::EXECUTE] {
        let mut v = vault_at(1000, 10);
        assert_eq!(v.call(SIGNERS[0], ids::PROPOSE, proposal(), 0).status_code, 0);
        assert_eq!(v.call(SIGNERS[1], ids::VOTE, proposal(), 0).status_code, 0);
        let before = v.snapshot().proposal;
        v.time = 1200;
        let body = if method == ids::VOTE { proposal() } else { 1u64.to_le_bytes().to_vec() };
        let out = v.call(SIGNERS[2], method, body, 0);
        assert_eq!(out.status_code, 0);
        assert_eq!(out.event_intents[0].event_name, "hub.expired");
        assert_eq!(v.snapshot().proposal, before);
        assert!(!v.snapshot().paused);
    }
}
#[test]
fn relative_zero_governance_delay_allows_immediate_quorum_execution() {
    let mut v = vault_at(1000, 0);
    assert_eq!(v.call(SIGNERS[0], ids::PROPOSE, proposal(), 0).status_code, 0);
    assert_eq!(v.call(SIGNERS[1], ids::VOTE, proposal(), 0).status_code, 0);
    assert_eq!(v.call(STRANGER, ids::EXECUTE, 1u64.to_le_bytes().to_vec(), 0).status_code, 0);
    assert!(v.snapshot().paused);
}
#[test]
fn relative_proposal_near_the_clock_limit_has_no_timestamp_overflow() {
    let mut v = vault_at(0, 10); v.time = u32::MAX - 1;
    assert_eq!(v.call(SIGNERS[0], ids::PROPOSE, proposal(), 0).status_code, 0);
}
