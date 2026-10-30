mod support;
use avm_contract_sdk::{
    decode_batch_input, encode_batch_output, BatchContract, StateOutputMode, StateUpdate,
};
use avm_revenue_audit::*;
use avm_revenue_core::*;
use support::*;
fn execute(state: &[u8], messages: &[Vec<u8>]) -> avm_contract_sdk::BatchOutput {
    let b = batch_frame(state, messages);
    let out = AuditLedger::process_batch(decode_batch_input(&b).unwrap());
    encode_batch_output(&out).unwrap();
    out
}
#[test]
fn audit_uses_65540_bytes_but_patches_only_one_32_byte_record() {
    let s = audit_state();
    assert_eq!(s.len(), 65540);
    let out = execute(&s, &[audit_message(0, 1, 500)]);
    assert_eq!(out.status_codes, vec![0]);
    let StateUpdate::DataPatchList(p) = &out.state_update else { panic!() };
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].offset, p[0].old_len, p[0].new_bytes.len()), (2052, 32, 32));
    let next = apply(&s, &out.state_update);
    assert_eq!(record(&next, 0).unwrap().total, 500);
    assert_eq!(s[..2052], next[..2052]);
    assert_eq!(s[2084..], next[2084..]);
}
#[test]
fn multiple_messages_to_same_member_are_coalesced() {
    let s = audit_state();
    let msgs = (1..=20).map(|n| audit_message(0, n, 7)).collect::<Vec<_>>();
    let out = execute(&s, &msgs);
    assert_eq!(out.status_codes, vec![0; 20]);
    let StateUpdate::DataPatchList(p) = &out.state_update else { panic!() };
    assert_eq!(p.len(), 1);
    assert_eq!(
        record(&apply(&s, &out.state_update), 0).unwrap(),
        Record { last_nonce: 20, total: 140, count: 20 }
    );
}
#[test]
fn mixed_valid_and_replayed_messages_have_independent_statuses() {
    let s = audit_state();
    let out = execute(
        &s,
        &[audit_message(0, 1, 500), audit_message(0, 1, 500), audit_message(1, 2, 300)],
    );
    assert_eq!(out.status_codes, vec![0, E_AUDIT_REPLAY, 0]);
    let next = apply(&s, &out.state_update);
    assert_eq!(record(&next, 0).unwrap().total, 500);
    assert_eq!(record(&next, 1).unwrap().total, 300);
    assert_eq!(out.event_intents.len(), 2);
    assert_eq!(out.event_intents[1].input_index, 2);
}
#[test]
fn patches_are_sorted_when_input_members_are_not() {
    let s = audit_state();
    let out =
        execute(&s, &[audit_message(2, 1, 2), audit_message(0, 1, 5), audit_message(1, 1, 3)]);
    let StateUpdate::DataPatchList(p) = &out.state_update else { panic!() };
    assert_eq!(p.iter().map(|p| p.offset).collect::<Vec<_>>(), vec![2052, 2084, 2116]);
}
#[test]
fn spoofed_member_source_is_rejected_without_credit() {
    let s = audit_state();
    let msg = message(&Spec {
        source: Some(address(7)),
        destination: address(9),
        method: ids::AUDIT_RECORD,
        body: payment_body(0, 1, 500),
        ..Spec::default()
    });
    let out = execute(&s, &[msg]);
    assert_eq!(out.status_codes, vec![E_AUDIT_AUTH]);
    assert_eq!(apply(&s, &out.state_update), s);
}
#[test]
fn audit_rejects_attached_principal_or_other_assets() {
    let s = audit_state();
    for (value, extra) in [(1, vec![]), (0, vec![(7, 5)])] {
        let msg = message(&Spec {
            source: Some(address(2)),
            destination: address(9),
            method: ids::AUDIT_RECORD,
            body: payment_body(0, 1, 500),
            value,
            extra,
            ..Spec::default()
        });
        assert_eq!(execute(&s, &[msg]).status_codes, vec![E_AUDIT_ARGS]);
    }
}
#[test]
fn audit_binding_requires_owner_and_is_once_only() {
    let s = audit_state();
    let mut body = vec![0];
    body.extend_from_slice(&address(8).dapp_id);
    body.extend_from_slice(&address(8).account_id);
    fails(
        &invoke::<AuditLedger>(
            &s,
            &Spec {
                actor: STRANGER,
                destination: address(9),
                method: ids::AUDIT_BIND,
                body: body.clone(),
                ..Spec::default()
            },
        ),
        E_AUDIT_AUTH,
    );
    fails(
        &invoke::<AuditLedger>(
            &s,
            &Spec { destination: address(9), method: ids::AUDIT_BIND, body, ..Spec::default() },
        ),
        E_AUDIT_ARGS,
    );
}
#[test]
fn record_amount_overflow_does_not_block_other_member() {
    let mut s = audit_state();
    s[2052..2084].copy_from_slice(&Record { last_nonce: 1, total: u128::MAX, count: 1 }.encode());
    let out = execute(&s, &[audit_message(0, 2, 1), audit_message(1, 1, 20)]);
    assert_eq!(out.status_codes, vec![E_AUDIT_OVERFLOW, 0]);
    assert_eq!(record(&apply(&s, &out.state_update), 0).unwrap().total, u128::MAX);
}
#[test]
fn count_overflow_is_atomic() {
    let mut s = audit_state();
    s[2052..2084].copy_from_slice(&Record { last_nonce: 1, total: 100, count: u64::MAX }.encode());
    let out = execute(&s, &[audit_message(0, 2, 20)]);
    assert_eq!(out.status_codes, vec![E_AUDIT_OVERFLOW]);
    assert_eq!(apply(&s, &out.state_update), s);
}
#[test]
fn batch_size_limit_is_explicit() {
    let s = audit_state();
    let msg = audit_message(0, 1, 10);
    let out = execute(&s, &vec![msg; 257]);
    assert_eq!(out.status_codes, vec![E_AUDIT_ARGS; 257]);
    assert_eq!(apply(&s, &out.state_update), s);
}
#[test]
fn isolated_fallback_and_batch_produce_same_ledger() {
    let s = audit_state();
    let spec = Spec {
        source: Some(address(2)),
        destination: address(9),
        method: ids::AUDIT_RECORD,
        body: payment_body(0, 1, 500),
        mode: StateOutputMode::DataPatchList,
        ..Spec::default()
    };
    let isolated = invoke::<AuditLedger>(&s, &spec);
    assert_eq!(isolated.status_code, 0);
    let batch = execute(&s, &[message(&spec)]);
    assert_eq!(apply(&s, isolated.state_update.as_ref().unwrap()), apply(&s, &batch.state_update));
}
#[test]
fn invalid_retained_padding_fails_closed() {
    let mut s = audit_state();
    s[65000] = 1;
    let out = execute(&s, &[audit_message(0, 1, 500)]);
    assert_eq!(out.status_codes, vec![E_AUDIT_ARGS]);
    assert_eq!(apply(&s, &out.state_update), s);
}
#[test]
fn empty_batch_emits_no_patch_or_event() {
    let s = audit_state();
    let out = execute(&s, &[]);
    assert!(out.status_codes.is_empty());
    assert_eq!(apply(&s, &out.state_update), s);
    assert!(out.event_intents.is_empty());
}
