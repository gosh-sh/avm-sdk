mod support;
use avm_revenue_core::*;
use avm_revenue_receiver::*;
use support::*;
fn initialized() -> Vec<u8> {
    let mut body = vec![0; 88];
    put_address(&mut body, 0, address(9));
    body[72..88].copy_from_slice(&10000u128.to_le_bytes());
    initialize::<PayoutReceiver>(&Spec {
        destination: address(2),
        deploy: true,
        body: envelope(&body),
        ..Spec::default()
    })
}
fn bound() -> Vec<u8> {
    let s = initialized();
    let mut body = vec![0; 64];
    put_address(&mut body, 0, address(1));
    let out = invoke::<PayoutReceiver>(
        &s,
        &Spec { destination: address(2), method: ids::BIND, body, ..Spec::default() },
    );
    assert_eq!(out.status_code, 0);
    apply(&s, out.state_update.as_ref().unwrap())
}
fn payment(nonce: u64) -> Spec {
    Spec {
        source: Some(address(1)),
        destination: address(2),
        method: ids::RECEIVE,
        body: payment_body(0, nonce, 500),
        value: 500,
        request: true,
        ..Spec::default()
    }
}
#[test]
fn authorized_receipt_echoes_body_and_emits_audit_intent() {
    let s = bound();
    let spec = payment(1);
    let out = invoke::<PayoutReceiver>(&s, &spec);
    assert_eq!(out.status_code, 0);
    assert_eq!(out.callback_result_body, spec.body);
    assert_eq!(out.outbound_intents.len(), 1);
    assert_eq!(out.outbound_intents[0].destination, address(9));
    assert_eq!(out.outbound_intents[0].method_id, ids::AUDIT_RECORD);
    assert!(out.outbound_intents[0].extra.is_empty());
    assert_eq!(received(&apply(&s, out.state_update.as_ref().unwrap())), Ok(500));
}
#[test]
fn unauthorized_source_or_external_sender_cannot_receive() {
    let s = bound();
    let mut spec = payment(1);
    spec.source = Some(address(7));
    fails(&invoke::<PayoutReceiver>(&s, &spec), E_RECEIVER_AUTH);
    spec.source = None;
    fails(&invoke::<PayoutReceiver>(&s, &spec), E_RECEIVER_AUTH);
}
#[test]
fn binding_is_owner_only_and_once_only() {
    let s = bound();
    let mut b = vec![0; 64];
    put_address(&mut b, 0, address(8));
    fails(
        &invoke::<PayoutReceiver>(
            &s,
            &Spec {
                destination: address(2),
                method: ids::BIND,
                body: b.clone(),
                ..Spec::default()
            },
        ),
        E_RECEIVER_ARGS,
    );
    fails(
        &invoke::<PayoutReceiver>(
            &s,
            &Spec {
                actor: STRANGER,
                destination: address(2),
                method: ids::BIND,
                body: b,
                ..Spec::default()
            },
        ),
        E_RECEIVER_AUTH,
    );
}
#[test]
fn receiver_requires_callback_request() {
    let s = bound();
    let mut spec = payment(1);
    spec.request = false;
    fails(&invoke::<PayoutReceiver>(&s, &spec), E_RECEIVER_AUTH);
}
#[test]
fn receiver_rejects_same_nonce_after_success() {
    let s = bound();
    let spec = payment(1);
    let out = invoke::<PayoutReceiver>(&s, &spec);
    let next = apply(&s, out.state_update.as_ref().unwrap());
    fails(&invoke::<PayoutReceiver>(&next, &spec), E_RECEIVER_ARGS);
}
#[test]
fn wrong_member_or_amount_is_rejected() {
    let s = bound();
    let mut spec = payment(1);
    spec.body = payment_body(1, 1, 500);
    fails(&invoke::<PayoutReceiver>(&s, &spec), E_RECEIVER_ARGS);
    spec.body = payment_body(0, 1, 499);
    fails(&invoke::<PayoutReceiver>(&s, &spec), E_RECEIVER_ARGS);
}
#[test]
fn deliberate_failure_does_not_change_receipt_counter() {
    let s = bound();
    let out = invoke::<PayoutReceiver>(
        &s,
        &Spec { destination: address(2), method: ids::REJECT, body: vec![1], ..Spec::default() },
    );
    let next = apply(&s, out.state_update.as_ref().unwrap());
    fails(&invoke::<PayoutReceiver>(&next, &payment(1)), E_REJECT);
    assert_eq!(received(&next), Ok(0));
}
#[test]
fn accumulated_receipt_overflow_is_rejected() {
    let mut s = bound();
    s[176..192].copy_from_slice(&u128::MAX.to_le_bytes());
    fails(&invoke::<PayoutReceiver>(&s, &payment(1)), E_RECEIVER_ARGS);
}
#[test]
fn duplicate_audit_injection_emits_two_intents_but_one_payment_receipt() {
    let s = bound();
    let out = invoke::<PayoutReceiver>(
        &s,
        &Spec {
            destination: address(2),
            method: ids::DUPLICATE_AUDIT,
            body: vec![1],
            ..Spec::default()
        },
    );
    assert_eq!(out.status_code, 0);
    let next = apply(&s, out.state_update.as_ref().unwrap());
    let out = invoke::<PayoutReceiver>(&next, &payment(1));
    assert_eq!(out.status_code, 0);
    assert_eq!(out.outbound_intents.len(), 2);
    assert_eq!(out.outbound_intents[0], out.outbound_intents[1]);
    assert_eq!(received(&apply(&next, out.state_update.as_ref().unwrap())), Ok(500));
}
