#[path = "../../../tests/tests/support/mod.rs"]
mod support;
use avm_contract_sdk::{decode_message_input, Contract, StateOutputMode, StateUpdate};
use avm_revenue_audit as legacy;
use avm_revenue_core::*;
use compact_audit_prototype as compact;
use support::*;

fn initial(n: u8) -> Vec<u8> {
    initialize::<legacy::AuditLedger>(&Spec {
        destination: address(9),
        deploy: true,
        body: envelope(&[n]),
        ..Spec::default()
    })
}
fn bind(n: u8) -> Vec<u8> {
    let mut s = initial(n);
    for i in 0..n {
        let mut body = vec![i];
        body.extend_from_slice(&address(20 + i).dapp_id);
        body.extend_from_slice(&address(20 + i).account_id);
        let out = invoke::<legacy::AuditLedger>(
            &s,
            &Spec { destination: address(9), method: ids::AUDIT_BIND, body, ..Spec::default() },
        );
        assert_eq!(out.status_code, 0);
        s = apply(&s, out.state_update.as_ref().unwrap());
    }
    s
}
fn receipt(i: u8, nonce: u64, amount: u128) -> Spec {
    Spec {
        source: Some(address(20 + i)),
        destination: address(9),
        method: ids::AUDIT_RECORD,
        body: payment_body(i, nonce, amount),
        ..Spec::default()
    }
}
/// Runs each message in its own message-isolated call, as the node does, and
/// returns the final state, the statuses and the event intents.
fn sequence(
    s: &[u8],
    specs: &[Spec],
    mode: StateOutputMode,
    new: bool,
) -> (Vec<u8>, Vec<u32>, Vec<String>) {
    let mut state = s.to_vec();
    let mut statuses = Vec::new();
    let mut events = Vec::new();
    for spec in specs {
        let spec = Spec { mode, ..spec.clone() };
        let out = if new {
            invoke::<compact::AuditLedger>(&state, &spec)
        } else {
            invoke::<legacy::AuditLedger>(&state, &spec)
        };
        assert!(out.outbound_intents.is_empty());
        if let Some(update) = &out.state_update {
            assert!(matches!(
                (update, mode),
                (StateUpdate::FullReplace(_), StateOutputMode::FullReplace)
                    | (StateUpdate::DataPatchList(_), StateOutputMode::DataPatchList)
            ));
            state = apply(&state, update);
        }
        statuses.push(out.status_code);
        events.push(format!("{:?}", out.event_intents));
    }
    (state, statuses, events)
}
fn compare(old: &[u8], new: &[u8], specs: &[Spec], mode: StateOutputMode) -> (Vec<u8>, Vec<u8>) {
    let (old, a, x) = sequence(old, specs, StateOutputMode::DataPatchList, false);
    let (new, b, y) = sequence(new, specs, mode, true);
    assert_eq!(a, b);
    assert_eq!(x, y);
    assert_eq!(compact::expand_legacy(&new).unwrap(), old);
    (old, new)
}
#[test]
fn sizes_init_and_lossless_conversion_all_counts() {
    for n in 1..=16 {
        let old = initial(n);
        let new = compact::compact_legacy(&old).unwrap();
        assert_eq!(new.len(), 44 + 96 * n as usize);
        assert_eq!(compact::expand_legacy(&new).unwrap(), old);
        let init = initialize::<compact::AuditLedger>(&Spec {
            destination: address(9),
            deploy: true,
            body: envelope(&[n]),
            ..Spec::default()
        });
        assert_eq!(new, init);
    }
}
#[test]
fn differential_deterministic_mixed_sequences_restart_all_counts_and_modes() {
    for n in 1..=16 {
        for mode in [StateOutputMode::FullReplace, StateOutputMode::DataPatchList] {
            let mut old = bind(n);
            let mut new = compact::compact_legacy(&old).unwrap();
            for round in 1..=16u64 {
                let mut specs = Vec::new();
                for i in (0..n).rev() {
                    let s = receipt(i, round * 3, 7 + (i as u128));
                    specs.push(s.clone());
                    specs.push(s); // replay
                    specs.push(receipt(i, round * 3 - 1, 2)); // stale nonce
                    let mut forged = receipt(i, round * 3 + 1, 999);
                    forged.source = Some(address(99));
                    specs.push(forged);
                }
                (old, new) = compare(&old, &new, &specs, mode);
                // New byte allocation each round; no process-local cache is carried forward.
                new = new.to_vec();
                assert_eq!(compact::compact_legacy(&old).unwrap(), new);
            }
            for i in 0..n {
                let r = compact::record(&new, i as usize).unwrap();
                assert_eq!(r.count, 16);
                assert_eq!(r.last_nonce, 48);
                assert_eq!(r.total, 16 * (7 + i as u128));
            }
        }
    }
}
#[test]
fn authentication_binding_and_isolated_receipts_match() {
    for mode in [StateOutputMode::FullReplace, StateOutputMode::DataPatchList] {
        let mut old = initial(3);
        let mut new = compact::compact_legacy(&old).unwrap();
        let mut specs = Vec::new();
        for i in 0..3 {
            let mut body = vec![i];
            body.extend_from_slice(&address(20 + i).dapp_id);
            body.extend_from_slice(&address(20 + i).account_id);
            let s = Spec {
                destination: address(9),
                method: ids::AUDIT_BIND,
                body,
                mode,
                ..Spec::default()
            };
            let mut forged = s.clone();
            forged.actor = STRANGER;
            specs.push(forged);
            specs.push(s.clone());
            specs.push(s);
        }
        for i in 0..3 {
            specs.push(receipt(i, 1, 100));
            specs.push(receipt(i, 1, 100));
        }
        for mut s in specs {
            s.mode = mode;
            let a = invoke::<legacy::AuditLedger>(&old, &s);
            let b = invoke::<compact::AuditLedger>(&new, &s);
            assert_eq!(a.status_code, b.status_code);
            assert_eq!(format!("{:?}", a.event_intents), format!("{:?}", b.event_intents));
            if let Some(u) = a.state_update {
                old = apply(&old, &u);
            }
            if let Some(u) = b.state_update {
                new = apply(&new, &u);
            }
            assert_eq!(compact::expand_legacy(&new).unwrap(), old);
        }
    }
}
#[test]
fn overflow_nonce_exhaustion_and_other_members_are_preserved() {
    for r in [
        legacy::Record { last_nonce: 1, total: u128::MAX, count: 1 },
        legacy::Record { last_nonce: 1, total: 1, count: u64::MAX },
        legacy::Record { last_nonce: u64::MAX, total: 1, count: 1 },
    ] {
        let mut old = bind(2);
        old[2052..2084].copy_from_slice(&r.encode());
        let new = compact::compact_legacy(&old).unwrap();
        let (a, b) = compare(
            &old,
            &new,
            &[receipt(0, 2, 1), receipt(1, 1, 9)],
            StateOutputMode::FullReplace,
        );
        assert_eq!(legacy::record(&a, 0).unwrap(), r);
        assert_eq!(compact::record(&b, 1).unwrap().total, 9);
    }
}
#[test]
fn invalid_message_forms_match_and_no_credit() {
    let old = bind(2);
    let new = compact::compact_legacy(&old).unwrap();
    let base = receipt(0, 1, 100);
    let mut cases = Vec::new();
    let mut s = base.clone();
    s.value = 1;
    cases.push(s);
    let mut s = base.clone();
    s.extra = vec![(7, 1)];
    cases.push(s);
    let mut s = base.clone();
    s.request = true;
    cases.push(s);
    let mut s = base.clone();
    s.callback = Some((0, [1; 32]));
    cases.push(s);
    let mut s = base.clone();
    s.method = ids::AUDIT_BIND;
    cases.push(s);
    let mut s = base.clone();
    s.body = payment_body(2, 1, 100);
    cases.push(s);
    let mut s = base.clone();
    s.body = payment_body(0, 0, 100);
    cases.push(s);
    let mut s = base.clone();
    s.body = payment_body(0, 1, 0);
    cases.push(s);
    let mut s = base.clone();
    s.body.pop();
    cases.push(s);
    let mut s = base;
    s.source = None;
    cases.push(s);
    // Every form reaches both contracts.
    assert!(cases.iter().all(|s| decode_message_input(&frame(&new, s)).is_ok()));
    let (a, b) = compare(&old, &new, &cases, StateOutputMode::FullReplace);
    assert_eq!(a, old);
    assert_eq!(b, new);
}
#[test]
fn maximum_receipt_sequence_and_its_replay() {
    let old = bind(16);
    let new = compact::compact_legacy(&old).unwrap();
    let specs =
        (0..256).map(|k| receipt((k % 16) as u8, (k / 16 + 1) as u64, 1)).collect::<Vec<_>>();
    for mode in [StateOutputMode::FullReplace, StateOutputMode::DataPatchList] {
        let (a, b) = compare(&old, &new, &specs, mode);
        assert_eq!(compact::record(&b, 15).unwrap().total, 16);
        // The replayed sequence is rejected receipt by receipt and changes nothing.
        assert_eq!(compare(&a, &b, &specs, mode), (a.clone(), b.clone()));
        assert_eq!(sequence(&b, &specs, mode, true).1, vec![compact::E_AUDIT_REPLAY; 256]);
    }
}
#[test]
fn malformed_states_and_legacy_padding_fail_closed() {
    let old = bind(3);
    let new = compact::compact_legacy(&old).unwrap();
    for offset in [0, 37, 67, 68 + 3 * 64, 2051, 2052 + 3 * 32, 65539] {
        let mut b = old.clone();
        b[offset] ^= 1;
        assert!(compact::compact_legacy(&b).is_err(), "offset {offset}");
    }
    for offset in [0, 4, 40, 41, 43] {
        let mut b = new.clone();
        b[offset] ^= 255;
        assert!(compact::validate(&b).is_err());
    }
    for len in 0..new.len() {
        assert!(compact::validate(&new[..len]).is_err());
        assert!(compact::record(&new[..len], 16).is_err());
    }
    let mut b = new.clone();
    b.push(0);
    assert!(compact::validate(&b).is_err());
    let mut b = new.clone();
    b[8..40].fill(0);
    assert!(compact::validate(&b).is_err());
    let mut b = new.clone();
    let src = b[44..108].to_vec();
    b[108..172].copy_from_slice(&src);
    assert!(compact::validate(&b).is_err());
    assert!(compact::validate(&old).is_err());
    assert!(compact::compact_legacy(&new).is_err());
}
#[test]
fn serialized_restart_files_preserve_replay_barrier() {
    let old = bind(1);
    let new = compact::compact_legacy(&old).unwrap();
    let (_, next) = compare(&old, &new, &[receipt(0, 8, 55)], StateOutputMode::FullReplace);
    let path = std::env::temp_dir().join(format!("compact-restart-{}.bin", std::process::id()));
    std::fs::write(&path, &next).unwrap();
    let restored = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let (state, statuses, _) = sequence(
        &restored,
        &[receipt(0, 8, 55), receipt(0, 9, 10)],
        StateOutputMode::FullReplace,
        true,
    );
    assert_eq!(statuses, vec![compact::E_AUDIT_REPLAY, 0]);
    assert_eq!(compact::record(&state, 0).unwrap().total, 65);
}
#[test]
fn write_guest_vectors_when_requested() {
    let Ok(path) = std::env::var("COMPACT_VECTORS") else {
        return;
    };
    fn hex(b: &[u8]) -> String {
        b.iter().map(|v| format!("{v:02x}")).collect()
    }
    let mut rows = Vec::new();
    let old = bind(16);
    let new = compact::compact_legacy(&old).unwrap();
    for mode in [StateOutputMode::FullReplace, StateOutputMode::DataPatchList] {
        for (name, state) in [("valid", new.clone()), ("malformed", vec![0; 44])] {
            let f = frame(&state, &Spec { mode, ..receipt(0, 1, 55) });
            let out = compact::AuditLedger::process_message(input(&f));
            rows.push(format!(
                "{name}-{mode:?}\tprocess_message\t{}\t{}",
                hex(&f),
                hex(&avm_contract_sdk::encode_message_output(&out).unwrap())
            ));
        }
        let s = Spec {
            destination: address(9),
            deploy: true,
            body: envelope(&[16]),
            mode,
            ..Spec::default()
        };
        let f = frame(&[], &s);
        let out = compact::AuditLedger::init(input(&f));
        rows.push(format!(
            "init-{mode:?}\tinit\t{}\t{}",
            hex(&f),
            hex(&avm_contract_sdk::encode_message_output(&out).unwrap())
        ));
    }
    std::fs::write(path, rows.join("\n") + "\n").unwrap();
}

#[test]
fn legacy_owner_cannot_emit_upgrade_intent_through_reserved_method() {
    let old = bind(3);
    let mut body = b"ACUG".to_vec();
    body.extend_from_slice(&[0x55; 32]);
    body.extend_from_slice(&[0x66; 32]);
    body.extend_from_slice(&0u32.to_le_bytes());
    let out = invoke::<legacy::AuditLedger>(
        &old,
        &Spec {
            destination: address(9),
            actor: SIGNERS[0],
            method: avm_contract_sdk::CODE_UPGRADE_METHOD_ID,
            body,
            mode: StateOutputMode::DataPatchList,
            ..Spec::default()
        },
    );
    assert_eq!(out.status_code, legacy::E_AUDIT_AUTH);
    assert!(out.code_upgrade_intent.is_none());
    assert!(out.state_update.is_none());
}
