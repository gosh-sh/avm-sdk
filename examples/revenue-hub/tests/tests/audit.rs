mod support;
use avm_contract_sdk::{MessageOutput, StateUpdate};
use avm_revenue_audit::*;
use avm_revenue_core::*;
use support::*;
/// Delivers each receipt in its own message-isolated call, as the node runs
/// the audit, and applies every state update in order.
fn deliver(state: &[u8], receipts: &[Spec]) -> (Vec<u8>, Vec<MessageOutput>) {
    let mut state = state.to_vec();
    let mut outputs = Vec::new();
    for receipt in receipts {
        let out = invoke::<AuditLedger>(&state, receipt);
        if let Some(update) = &out.state_update {
            state = apply(&state, update);
        }
        outputs.push(out);
    }
    (state, outputs)
}
fn statuses(outputs: &[MessageOutput]) -> Vec<u32> {
    outputs.iter().map(|out| out.status_code).collect()
}
#[test]
fn audit_uses_65540_bytes_but_patches_only_one_32_byte_record() {
    let s = audit_state();
    assert_eq!(s.len(), 65540);
    let out = invoke::<AuditLedger>(&s, &audit_receipt(0, 1, 500));
    assert_eq!(out.status_code, 0);
    let Some(StateUpdate::DataPatchList(p)) = &out.state_update else { panic!() };
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].offset, p[0].old_len, p[0].new_bytes.len()), (2052, 32, 32));
    let next = apply(&s, out.state_update.as_ref().unwrap());
    assert_eq!(record(&next, 0).unwrap().total, 500);
    assert_eq!(s[..2052], next[..2052]);
    assert_eq!(s[2084..], next[2084..]);
}
#[test]
fn receipts_to_same_member_accumulate_in_its_record() {
    let s = audit_state();
    let receipts = (1..=20).map(|n| audit_receipt(0, n, 7)).collect::<Vec<_>>();
    let (next, outputs) = deliver(&s, &receipts);
    assert_eq!(statuses(&outputs), vec![0; 20]);
    for out in &outputs {
        let Some(StateUpdate::DataPatchList(p)) = &out.state_update else { panic!() };
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].offset, p[0].old_len), (2052, 32));
    }
    assert_eq!(record(&next, 0).unwrap(), Record { last_nonce: 20, total: 140, count: 20 });
}
#[test]
fn mixed_valid_and_replayed_messages_have_independent_statuses() {
    let s = audit_state();
    let (next, outputs) = deliver(
        &s,
        &[audit_receipt(0, 1, 500), audit_receipt(0, 1, 500), audit_receipt(1, 2, 300)],
    );
    assert_eq!(statuses(&outputs), vec![0, E_AUDIT_REPLAY, 0]);
    fails(&outputs[1], E_AUDIT_REPLAY);
    assert_eq!(record(&next, 0).unwrap().total, 500);
    assert_eq!(record(&next, 1).unwrap().total, 300);
    for out in [&outputs[0], &outputs[2]] {
        assert_eq!(out.event_intents.len(), 1);
        assert_eq!(out.event_intents[0].input_index, 0);
    }
}
#[test]
fn each_receipt_patches_only_its_member_record() {
    let s = audit_state();
    for (index, offset) in [(2, 2116), (0, 2052), (1, 2084)] {
        let out = invoke::<AuditLedger>(&s, &audit_receipt(index, 1, 5));
        let Some(StateUpdate::DataPatchList(p)) = &out.state_update else { panic!() };
        assert_eq!(p.iter().map(|p| p.offset).collect::<Vec<_>>(), vec![offset]);
    }
}
#[test]
fn spoofed_member_source_is_rejected_without_credit() {
    let s = audit_state();
    let spec = Spec { source: Some(address(7)), ..audit_receipt(0, 1, 500) };
    fails(&invoke::<AuditLedger>(&s, &spec), E_AUDIT_AUTH);
}
#[test]
fn audit_rejects_attached_principal_or_other_assets() {
    let s = audit_state();
    for (value, extra) in [(1, vec![]), (0, vec![(7, 5)])] {
        let spec = Spec { value, extra, ..audit_receipt(0, 1, 500) };
        fails(&invoke::<AuditLedger>(&s, &spec), E_AUDIT_ARGS);
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
    let (next, outputs) = deliver(&s, &[audit_receipt(0, 2, 1), audit_receipt(1, 1, 20)]);
    assert_eq!(statuses(&outputs), vec![E_AUDIT_OVERFLOW, 0]);
    fails(&outputs[0], E_AUDIT_OVERFLOW);
    assert_eq!(record(&next, 0).unwrap().total, u128::MAX);
    assert_eq!(record(&next, 1).unwrap().total, 20);
}
#[test]
fn count_overflow_is_atomic() {
    let mut s = audit_state();
    s[2052..2084].copy_from_slice(&Record { last_nonce: 1, total: 100, count: u64::MAX }.encode());
    fails(&invoke::<AuditLedger>(&s, &audit_receipt(0, 2, 20)), E_AUDIT_OVERFLOW);
}
#[test]
fn invalid_retained_padding_fails_closed() {
    let mut s = audit_state();
    s[65000] = 1;
    fails(&invoke::<AuditLedger>(&s, &audit_receipt(0, 1, 500)), E_AUDIT_ARGS);
}
