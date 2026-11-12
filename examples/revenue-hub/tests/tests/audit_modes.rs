//! Application regressions against the current in-tree guest SDK.
mod support;
use avm_contract_sdk::{encode_message_output, Contract, StateOutputMode, StateUpdate};
use avm_revenue_audit::*;
use avm_revenue_core::*;
use support::*;

fn init_spec(mode: StateOutputMode) -> Spec {
    Spec { destination: address(9), deploy: true, body: envelope(&[16]), mode, ..Spec::default() }
}
fn bind_spec(index: u8, mode: StateOutputMode) -> Spec {
    let mut body = vec![index];
    body.extend_from_slice(&address(index + 20).dapp_id);
    body.extend_from_slice(&address(index + 20).account_id);
    Spec { destination: address(9), method: ids::AUDIT_BIND, body, mode, ..Spec::default() }
}

#[test]
fn initialization_requires_full_replace_and_rejects_patch_without_effects() {
    let bytes = frame(&[], &init_spec(StateOutputMode::DataPatchList));
    let out = AuditLedger::init(input(&bytes));
    fails(&out, E_AUDIT_ARGS);
    encode_message_output(&out).unwrap();
}

#[test]
fn first_post_init_binding_already_uses_patch_and_preserves_all_other_bytes() {
    let spec = init_spec(StateOutputMode::FullReplace);
    let bytes = frame(&[], &spec);
    let init = AuditLedger::init(input(&bytes));
    assert_eq!(init.status_code, 0);
    assert!(matches!(init.state_update, Some(StateUpdate::FullReplace(_))));
    let state = apply(&[], init.state_update.as_ref().unwrap());
    assert_eq!(state.len(), STATE_BYTES);
    assert_eq!(encode_message_output(&init).unwrap().len(), 65_573);
    let out = invoke::<AuditLedger>(&state, &bind_spec(0, StateOutputMode::DataPatchList));
    assert_eq!(out.status_code, 0);
    let Some(StateUpdate::DataPatchList(patches)) = &out.state_update else { panic!() };
    assert_eq!(patches.len(), 1);
    assert_eq!((patches[0].offset, patches[0].old_len, patches[0].new_bytes.len()), (68, 64, 64));
    let next = apply(&state, out.state_update.as_ref().unwrap());
    assert_eq!(&state[..68], &next[..68]);
    assert_eq!(&state[132..], &next[132..]);
    assert_eq!(address_at(&next, 68).unwrap(), address(20));
    // Status immediately after binding is an empty patch, never a full copy.
    let status = invoke::<AuditLedger>(
        &next,
        &Spec {
            destination: address(9),
            method: ids::AUDIT_STATUS,
            mode: StateOutputMode::DataPatchList,
            ..Spec::default()
        },
    );
    assert_eq!(status.status_code, 0);
    assert_eq!(status.state_update, Some(StateUpdate::DataPatchList(vec![])));
}

#[test]
fn isolated_binding_status_and_receipt_follow_runtime_mode() {
    for mode in [StateOutputMode::FullReplace, StateOutputMode::DataPatchList] {
        // FullReplace here tests the guest's requested-mode contract only;
        // it does not assert host admission for this large prior account.
        let state = initialize::<AuditLedger>(&init_spec(StateOutputMode::FullReplace));
        let bind = invoke::<AuditLedger>(&state, &bind_spec(0, mode));
        assert_eq!(bind.status_code, 0);
        assert_eq!(bind.state_update.as_ref().unwrap().mode(), mode);
        let state = apply(&state, bind.state_update.as_ref().unwrap());
        let receipt = invoke::<AuditLedger>(
            &state,
            &Spec {
                source: Some(address(20)),
                destination: address(9),
                method: ids::AUDIT_RECORD,
                body: payment_body(0, 1, 17),
                mode,
                ..Spec::default()
            },
        );
        assert_eq!(receipt.status_code, 0);
        assert_eq!(receipt.state_update.as_ref().unwrap().mode(), mode);
        let state = apply(&state, receipt.state_update.as_ref().unwrap());
        assert_eq!(record(&state, 0).unwrap(), Record { last_nonce: 1, total: 17, count: 1 });
        let status = invoke::<AuditLedger>(
            &state,
            &Spec { destination: address(9), method: ids::AUDIT_STATUS, mode, ..Spec::default() },
        );
        assert_eq!(status.status_code, 0);
        assert_eq!(status.state_update.as_ref().unwrap().mode(), mode);
        assert_eq!(apply(&state, status.state_update.as_ref().unwrap()), state);
    }
}

#[test]
fn malformed_state_fails_atomically_in_both_modes() {
    for mode in [StateOutputMode::FullReplace, StateOutputMode::DataPatchList] {
        let receipt = Spec { mode, ..audit_receipt(0, 1, 7) };
        fails(&invoke::<AuditLedger>(&[], &receipt), E_AUDIT_ARGS);
    }
}

#[test]
fn all_sixteen_members_receive_disjoint_bounded_patches() {
    let mut state = initialize::<AuditLedger>(&init_spec(StateOutputMode::FullReplace));
    for i in 0..16 {
        let out = invoke::<AuditLedger>(&state, &bind_spec(i, StateOutputMode::DataPatchList));
        assert_eq!(out.status_code, 0);
        state = apply(&state, out.state_update.as_ref().unwrap());
    }
    // 16 receipts for each member, in reversed member order.
    let receipts = (1..=256_usize)
        .map(|n| {
            let i = (256 - n) % 16;
            Spec {
                source: Some(address(20 + i as u8)),
                destination: address(9),
                method: ids::AUDIT_RECORD,
                body: payment_body(i as u8, n as u64, 7),
                mode: StateOutputMode::DataPatchList,
                ..Spec::default()
            }
        })
        .collect::<Vec<_>>();
    for (k, receipt) in receipts.iter().enumerate() {
        let out = invoke::<AuditLedger>(&state, receipt);
        assert_eq!(out.status_code, 0);
        assert_eq!(out.event_intents.len(), 1);
        let Some(StateUpdate::DataPatchList(patches)) = &out.state_update else { panic!() };
        assert_eq!(patches.len(), 1);
        let i = (255 - k) % 16;
        assert_eq!(patches[0].offset as usize, RECORD_BASE + i * RECORD_BYTES);
        assert_eq!(patches[0].old_len as usize, RECORD_BYTES);
        assert_eq!(patches[0].new_bytes.len(), RECORD_BYTES);
        state = apply(&state, out.state_update.as_ref().unwrap());
    }
    for i in 0..16 {
        let r = record(&state, i).unwrap();
        assert_eq!(r.total, 16 * 7);
        assert_eq!(r.count, 16);
    }
    for receipt in &receipts {
        fails(&invoke::<AuditLedger>(&state, receipt), E_AUDIT_REPLAY);
    }
}
