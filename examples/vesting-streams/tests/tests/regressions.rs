mod support;
use avm_stream_core::{state::*, *};
use avm_stream_receiver::{
    Receiver, E_RECEIVER_ARGS, E_RECEIVER_AUTH, E_REJECT, STATE_BYTES as RECEIVER_BYTES,
};
use avm_stream_vesting::Vesting;
use support::*;

#[test]
fn python_bigint_oracle_1200_cases() {
    let text = include_str!("../fixtures/python-bigint-vesting.csv");
    assert_eq!(text.lines().count(), 1200);
    for line in text.lines() {
        let v: Vec<u128> = line.split(',').map(|s| s.parse().unwrap()).collect();
        assert_eq!(
            vested(v[0], v[1] as u32, v[2] as u32, v[3] as u32, v[4] as u32, v[5] as u32).unwrap(),
            v[6],
            "{line}"
        )
    }
}
#[test]
fn linear_cliff_and_end_boundaries() {
    for (t, a) in
        [(99, 0), (100, 0), (129, 0), (130, 300), (199, 990), (200, 1000), (u32::MAX, 1000)]
    {
        assert_eq!(vested(1000, 100, 130, 200, 0, t), Ok(a))
    }
}
#[test]
fn stepped_release_and_partial_final_interval() {
    for (t, a) in [(129, 0), (130, 300), (159, 300), (160, 600), (199, 900), (200, 1000)] {
        assert_eq!(vested(1000, 100, 100, 200, 30, t), Ok(a))
    }
}
#[test]
fn terminal_cliff_defers_everything() {
    assert_eq!(vested(u128::MAX, 1, 101, 101, 0, 100), Ok(0));
    assert_eq!(vested(u128::MAX, 1, 101, 101, 0, 101), Ok(u128::MAX))
}
#[test]
fn full_u128_half_and_last_tick() {
    let m = u128::MAX;
    assert_eq!(vested(m, 1, 1, 3, 0, 2), Ok(m / 2));
    assert_eq!(vested(m, 1, 1, u32::MAX, 0, u32::MAX), Ok(m))
}
#[test]
fn invalid_schedule_is_rejected() {
    for (a, b, c, d, e) in [
        (0, 1, 1, 2, 0),
        (1, 0, 0, 2, 0),
        (1, 2, 2, 2, 0),
        (1, 2, 1, 3, 0),
        (1, 2, 4, 3, 0),
        (1, 2, 2, 3, 2),
    ] {
        assert_eq!(vested(a, b, c, d, e, 100), Err(E_ARGS))
    }
}
#[test]
fn tiny_amount_waits_for_rounding() {
    assert_eq!(vested(1, 10, 10, 100, 0, 99), Ok(0));
    assert_eq!(vested(1, 10, 10, 100, 0, 100), Ok(1))
}
#[test]
fn config_and_state_roundtrip() {
    let c = config();
    assert_eq!(Config::decode(&c.encode()).unwrap(), c);
    let v = Book::funded();
    assert_eq!(v.snapshot().encode(), v.state)
}
#[test]
fn all_truncated_config_and_state_frames_fail() {
    let c = config().encode();
    for n in 0..c.len() {
        assert!(Config::decode(&c[..n]).is_err())
    }
    let s = Book::funded().state;
    for n in 0..s.len() {
        assert!(State::decode(&s[..n]).is_err())
    }
}
#[test]
fn invalid_counts_are_rejected() {
    for n in [0, 9, 255] {
        let mut c = config();
        c.count = n;
        assert!(Config::decode(&c.encode()).is_err())
    }
}
#[test]
fn inactive_rows_and_reserved_bytes_are_canonical() {
    let mut c = config();
    c.streams[7] = c.streams[0];
    assert!(Config::decode(&c.encode()).is_err());
    let mut b = config().encode();
    for off in [49, 56 + 193] {
        b[off] = 1;
        assert!(Config::decode(&b).is_err());
        b[off] = 0
    }
}
#[test]
fn invalid_identity_and_address_are_rejected() {
    for field in 0..4 {
        let mut c = config();
        match field {
            0 => c.owner = [0; 32],
            1 => c.streams[0].key = [0; 32],
            2 => c.streams[0].payout = zero_address(),
            _ => c.streams[0].refund = zero_address(),
        };
        assert!(Config::decode(&c.encode()).is_err())
    }
}
#[test]
fn all_payment_sources_must_be_distinct() {
    for source in
        [config().streams[0].payout, config().streams[0].refund, config().streams[1].payout]
    {
        let mut c = config();
        c.streams[1].refund = source;
        assert!(Config::decode(&c.encode()).is_err())
    }
}
#[test]
fn same_beneficiary_can_receive_two_separate_grants() {
    let mut c = config();
    c.streams[1].key = c.streams[0].key;
    assert!(Config::decode(&c.encode()).is_ok())
}
#[test]
fn allocation_sum_overflow_is_rejected() {
    let mut c = config();
    c.streams[0].total = u128::MAX;
    assert!(Config::decode(&c.encode()).is_err())
}
#[test]
fn invalid_schedules_in_config_are_rejected() {
    for n in 0..5 {
        let mut c = config();
        let s = &mut c.streams[0];
        match n {
            0 => s.total = 0,
            1 => s.cliff = s.start - 1,
            2 => s.end = s.start,
            3 => s.step = 101,
            _ => s.cliff = s.end + 1,
        };
        assert!(Config::decode(&c.encode()).is_err())
    }
}
#[test]
fn deploy_envelope_bounds_are_checked() {
    let b = envelope(&config().encode());
    let s = Spec { deploy: true, body: b.clone(), ..Spec::default() };
    assert_eq!(deploy_data(input(&frame(&[], &s))).unwrap(), config().encode());
    for n in 0..b.len() {
        let truncated = Spec { body: b[..n].to_vec(), ..s.clone() };
        assert!(deploy_data(input(&frame(&[], &truncated))).is_err())
    }
    let mut b = b;
    b.push(0);
    let trailing = Spec { body: b, ..s };
    assert!(deploy_data(input(&frame(&[], &trailing))).is_err())
}
fn init_failure(mut s: Spec, code: u32) {
    s.deploy = true;
    if s.body.is_empty() {
        s.body = envelope(&config().encode())
    }
    let b = frame(&[], &s);
    fails(<Vesting as avm_contract_sdk::Contract>::init(input(&b)), code)
}
#[test]
fn init_requires_configured_owner() {
    init_failure(Spec { actor: STRANGER, ..Spec::default() }, E_AUTH)
}
#[test]
fn init_refuses_attached_principal_and_extras() {
    for s in [Spec { value: 1, ..Spec::default() }, Spec { extra: vec![(2, 1)], ..Spec::default() }]
    {
        init_failure(s, E_VALUE)
    }
}
#[test]
fn init_refuses_past_start_and_repeat() {
    init_failure(Spec { time: 100, ..Spec::default() }, E_ARGS);
    let v = Book::new();
    let s = Spec { deploy: true, body: envelope(&config().encode()), ..Spec::default() };
    let b = frame(&v.state, &s);
    fails(<Vesting as avm_contract_sdk::Contract>::init(input(&b)), E_ARGS)
}
#[test]
fn init_refuses_cross_dapp_and_self_recipient() {
    for a in
        [address(1), avm_contract_sdk::AccountAddress { dapp_id: [0x22; 32], account_id: [9; 32] }]
    {
        let mut c = config();
        c.streams[0].payout = a;
        init_failure(Spec { body: envelope(&c.encode()), ..Spec::default() }, E_ARGS)
    }
}
#[test]
fn funding_is_exact_and_once() {
    let mut v = Book::new();
    for n in [0, 1999, 2001] {
        fails(v.call(OWNER, ids::FUND, vec![], n, 11), E_VALUE)
    }
    v.fund();
    fails(v.call(OWNER, ids::FUND, vec![], 2000, 12), E_PHASE)
}
#[test]
fn funding_requires_owner_and_time_before_first_start() {
    let mut v = Book::new();
    fails(v.call(STRANGER, ids::FUND, vec![], 2000, 11), E_AUTH);
    fails(v.call(OWNER, ids::FUND, vec![], 2000, 100), E_PHASE)
}
#[test]
fn funding_refuses_extras_and_insolvent_input() {
    let mut v = Book::new();
    for extras in [vec![(2, 1)], vec![(7, 1)]] {
        let s = Spec { method: ids::FUND, value: 2000, extra: extras, ..Spec::default() };
        fails(invoke::<Vesting>(&v.state, &s), E_VALUE)
    }
    let mut s = v.spec(OWNER, ids::FUND, vec![], 2000, 11);
    s.balance = 1999;
    fails(invoke::<Vesting>(&v.state, &s), E_BALANCE)
}
#[test]
fn unfunded_streams_cannot_claim_cancel_or_refund() {
    let mut v = Book::new();
    for m in [ids::CLAIM, ids::CANCEL, ids::REFUND] {
        fails(v.call(OWNER, m, vec![0], 0, 130), E_PHASE)
    }
}
#[test]
fn beneficiary_can_only_claim_own_stream() {
    let mut v = Book::funded();
    for actor in [OWNER, STRANGER, BENEFICIARIES[1]] {
        fails(v.call(actor, ids::CLAIM, vec![0], 0, 150), E_AUTH)
    }
}
#[test]
fn nothing_is_claimable_before_cliff() {
    let mut v = Book::funded();
    for t in [99, 100, 129] {
        fails(v.claim(0, t), E_EMPTY)
    }
    assert_outbound_principal(v.claim(0, 130), 300)
}
#[test]
fn repeated_claims_pay_only_new_accrual() {
    let mut v = Book::funded();
    assert_outbound_principal(v.claim(0, 130), 300);
    assert_eq!(v.settle(0, 0, true).status_code, 0);
    fails(v.claim(0, 130), E_EMPTY);
    assert_outbound_principal(v.claim(0, 175), 450);
    assert_eq!(v.settle(0, 0, true).status_code, 0);
    assert_outbound_principal(v.claim(0, 200), 250);
    assert_eq!(v.settle(0, 0, true).status_code, 0);
    assert_eq!(v.snapshot().progress[0].claimed, 1000)
}
#[test]
fn step_does_not_release_fractional_interval() {
    let mut v = Book::funded();
    fails(v.claim(1, 129), E_EMPTY);
    assert_outbound_principal(v.claim(1, 130), 210);
    v.settle(1, 0, true);
    fails(v.claim(1, 159), E_EMPTY);
    assert_outbound_principal(v.claim(1, 160), 210)
}
#[test]
fn cancellation_freezes_earned_rights() {
    let mut v = Book::funded();
    assert_eq!(v.cancel(0, 150).status_code, 0);
    assert_eq!(v.snapshot().progress[0].cap, 500);
    assert_outbound_principal(v.claim(0, 1000), 500);
    v.settle(0, 0, true);
    fails(v.claim(0, 2000), E_EMPTY);
    assert_outbound_principal(v.refund(0, 2000), 500);
    v.settle(0, 1, true);
    assert_eq!((v.snapshot().progress[0].claimed, v.snapshot().progress[0].refunded), (500, 500))
}
#[test]
fn cancellation_before_cliff_returns_everything() {
    let mut v = Book::funded();
    assert_eq!(v.cancel(0, 129).status_code, 0);
    fails(v.claim(0, 200), E_EMPTY);
    assert_outbound_principal(v.refund(0, 200), 1000)
}
#[test]
fn cancellation_is_owner_only_and_once() {
    let mut v = Book::funded();
    fails(v.call(STRANGER, ids::CANCEL, vec![0], 0, 150), E_AUTH);
    assert_eq!(v.cancel(0, 150).status_code, 0);
    fails(v.cancel(0, 160), E_PHASE)
}
#[test]
fn irrevocable_and_completed_streams_cannot_cancel() {
    let mut v = Book::funded();
    fails(v.cancel(2, 150), E_IRREVOCABLE);
    fails(v.cancel(0, 200), E_PHASE)
}
#[test]
fn owner_cannot_refund_active_or_earned_balance() {
    let mut v = Book::funded();
    fails(v.refund(0, 150), E_PHASE);
    v.cancel(0, 150);
    fails(v.call(BENEFICIARIES[0], ids::REFUND, vec![0], 0, 150), E_AUTH);
    v.refund(0, 150);
    v.settle(0, 1, true);
    fails(v.refund(0, 160), E_EMPTY);
    assert_outbound_principal(v.claim(0, 160), 500)
}
#[test]
fn cancel_while_beneficiary_payment_pending_preserves_it() {
    let mut v = Book::funded();
    v.claim(0, 150);
    v.cancel(0, 160);
    assert_eq!(v.snapshot().progress[0].cap, 600);
    assert_outbound_principal(v.refund(0, 160), 400);
    v.settle(0, 1, true);
    v.settle(0, 0, true);
    assert_outbound_principal(v.claim(0, 160), 100)
}
#[test]
fn both_lanes_are_independent_and_each_blocks_duplicate_dispatch() {
    let mut v = Book::funded();
    v.cancel(0, 150);
    v.claim(0, 150);
    fails(v.claim(0, 160), E_BUSY);
    v.refund(0, 160);
    fails(v.refund(0, 160), E_BUSY);
    v.settle(0, 0, false);
    assert_eq!(v.claim(0, 160).status_code, 0);
    v.settle(0, 1, false);
    assert_eq!(v.refund(0, 160).status_code, 0)
}
#[test]
fn blocked_lane_does_not_block_other_streams() {
    let mut v = Book::funded();
    v.claim(0, 150);
    assert_eq!(v.claim(1, 150).status_code, 0);
    assert_eq!(v.claim(2, 150).status_code, 0)
}
#[test]
fn observed_time_never_regresses() {
    let mut v = Book::funded();
    v.call(STRANGER, ids::STATUS, vec![], 0, 160);
    v.cancel(0, 140);
    assert_eq!(v.snapshot().progress[0].cutoff, 160);
    assert_outbound_principal(v.claim(0, 130), 600)
}
#[test]
fn success_settles_once_and_replay_has_no_effect() {
    let mut v = Book::funded();
    v.claim(0, 150);
    let receipt = v.receipt(0, 0, true);
    let out = invoke::<Vesting>(&v.state, &receipt);
    v.accept(&out);
    assert_eq!(v.snapshot().progress[0].claimed, 500);
    fails(invoke::<Vesting>(&v.state, &receipt), E_CALLBACK)
}
#[test]
fn hundred_failed_transfers_restore_full_entitlement_then_retry() {
    let mut v = Book::funded();
    for _ in 0..100 {
        assert_eq!(v.claim(0, 150).status_code, 0);
        assert_eq!(v.settle(0, 0, false).status_code, 0);
        assert_eq!(v.snapshot().progress[0].claimed, 0)
    }
    assert_eq!(v.claim(0, 150).status_code, 0);
    assert_eq!(v.settle(0, 0, true).status_code, 0);
    assert_eq!((v.snapshot().nonce, v.snapshot().progress[0].claimed), (101, 500))
}
#[test]
fn failed_refund_restores_only_unvested_right() {
    let mut v = Book::funded();
    v.cancel(0, 150);
    v.refund(0, 150);
    v.settle(0, 1, false);
    assert_outbound_principal(v.refund(0, 200), 500);
    v.settle(0, 1, true);
    assert_outbound_principal(v.claim(0, 200), 500)
}
#[test]
fn failure_requires_exact_principal_and_empty_body() {
    let mut v = Book::funded();
    v.claim(0, 150);
    let s = v.receipt(0, 0, false);
    for n in [0, 499, 501] {
        let mut s = s.clone();
        s.value = n;
        fails(invoke::<Vesting>(&v.state, &s), E_CALLBACK)
    }
    let mut s = s;
    s.body = vec![0];
    assert!(avm_contract_sdk::decode_message_input(&frame(&v.state, &s)).is_err())
}
#[test]
fn success_echo_must_match_every_payment_field() {
    let mut v = Book::funded();
    v.claim(0, 150);
    let s = v.receipt(0, 0, true);
    for o in [0, 1, 2, 10] {
        let mut s = s.clone();
        s.body[o] ^= 1;
        fails(invoke::<Vesting>(&v.state, &s), E_CALLBACK)
    }
    let mut s = s;
    s.value = 1;
    fails(invoke::<Vesting>(&v.state, &s), E_CALLBACK)
}
#[test]
fn callback_source_handler_and_extras_are_authenticated() {
    let mut v = Book::funded();
    v.claim(0, 150);
    let s = v.receipt(0, 0, true);
    for t in 0..4 {
        let mut s = s.clone();
        match t {
            0 => s.source = Some(address(99)),
            1 => s.method = ids::STATUS,
            2 => s.extra = vec![(7, 1)],
            _ => s.request = true,
        };
        if t == 3 {
            assert!(avm_contract_sdk::decode_message_input(&frame(&v.state, &s)).is_err())
        } else {
            fails(invoke::<Vesting>(&v.state, &s), E_CALLBACK)
        }
    }
}
#[test]
fn native_callbacks_accept_separate_shell_fee_refunds() {
    for success in [false, true] {
        let mut v = Book::funded();
        v.claim(0, 150);
        let mut receipt = v.receipt(0, 0, success);
        receipt.extra = vec![(SHELL_CURRENCY_ID, 17)];
        let out = invoke::<Vesting>(&v.state, &receipt);
        assert_eq!(out.status_code, 0);
        v.accept(&out);
        assert_eq!(v.snapshot().progress[0].claimed, if success { 500 } else { 0 });
        assert_eq!(v.snapshot().progress[0].lanes[0].pending, 0);
        assert_eq!(v.snapshot().due(0, 0, 150), Ok(if success { 0 } else { 500 }));
        fails(invoke::<Vesting>(&v.state, &receipt), E_CALLBACK);
    }
}
#[test]
fn fee_refund_never_replaces_or_enlarges_principal() {
    let mut v = Book::funded();
    v.claim(0, 150);
    let receipt = v.receipt(0, 0, false);
    for (value, extra) in [
        (0, vec![(SHELL_CURRENCY_ID, 500)]),
        (499, vec![(SHELL_CURRENCY_ID, 1)]),
        (500, vec![(SHELL_CURRENCY_ID, 0)]),
        (500, vec![(7, 1)]),
    ] {
        let invalid = Spec { value, extra, ..receipt.clone() };
        fails(invoke::<Vesting>(&v.state, &invalid), E_CALLBACK);
    }
}
#[test]
fn callback_currency_order_and_uniqueness_remain_canonical() {
    let mut v = Book::funded();
    v.claim(0, 150);
    let receipt = v.receipt(0, 0, false);
    for extra in [vec![(PRINCIPAL_CURRENCY_ID, 1)], vec![(7, 1), (SHELL_CURRENCY_ID, 1)]] {
        let invalid = Spec { extra, ..receipt.clone() };
        assert!(avm_contract_sdk::decode_message_input(&frame(&v.state, &invalid)).is_err());
    }
}
#[test]
fn stale_hash_and_creation_time_cannot_settle_new_attempt() {
    let mut v = Book::funded();
    v.claim(0, 150);
    let old = v.receipt(0, 0, false);
    v.accept(&invoke::<Vesting>(&v.state, &old));
    v.claim(0, 160);
    fails(invoke::<Vesting>(&v.state, &old), E_CALLBACK);
    let mut new = v.receipt(0, 0, true);
    new.created_lt = v.snapshot().progress[0].lanes[0].after_lt;
    fails(invoke::<Vesting>(&v.state, &new), E_CALLBACK);
    new.created_lt = v.lt - 1;
    new.callback = Some((0, old.callback.unwrap().1));
    fails(invoke::<Vesting>(&v.state, &new), E_CALLBACK)
}
#[test]
fn zero_hash_and_wrong_cause_address_are_rejected() {
    let mut v = Book::funded();
    v.claim(0, 150);
    let mut s = v.receipt(0, 0, true);
    s.callback = Some((0, [0; 32]));
    assert!(avm_contract_sdk::decode_message_input(&frame(&v.state, &s)).is_err());
    s.callback = Some((0, [0x82; 32]));
    let mut b = frame(&v.state, &s);
    let m = b.len() - 8 - message(&s).len();
    let cause = m + 176 + 32;
    b[cause] ^= 1;
    assert!(avm_contract_sdk::decode_message_input(&b).is_err())
}
#[test]
fn ordinary_or_external_calls_cannot_impersonate_callback() {
    let mut v = Book::funded();
    v.claim(0, 150);
    fails(v.call(OWNER, ids::CALLBACK, vec![0], 0, 150), E_ARGS);
    let mut s = v.receipt(0, 0, true);
    s.callback = None;
    fails(invoke::<Vesting>(&v.state, &s), E_AUTH)
}
#[test]
fn fee_reserve_and_principal_insolvency_block_dispatch() {
    let mut v = Book::funded();
    let mut s = v.spec(BENEFICIARIES[0], ids::CLAIM, vec![0], 0, 150);
    s.balance = 1999;
    fails(invoke::<Vesting>(&v.state, &s), E_BALANCE);
    s.balance = 2000;
    s.shells = 9999;
    fails(invoke::<Vesting>(&v.state, &s), E_BALANCE)
}
#[test]
fn only_shell_is_accepted_as_operating_topup() {
    let v = Book::funded();
    for (extra, value, code) in [
        (vec![(2, 1)], 0, 0),
        (vec![(7, 1)], 0, E_VALUE),
        (vec![], 0, E_VALUE),
        (vec![(2, 1)], 1, E_VALUE),
    ] {
        let s = Spec { actor: STRANGER, method: ids::TOPUP, extra, value, ..Spec::default() };
        let out = invoke::<Vesting>(&v.state, &s);
        assert_eq!(out.status_code, code);
        if code == 0 {
            assert_eq!(
                out.state_update,
                Some(avm_contract_sdk::StateUpdate::FullReplace(v.state.clone()))
            )
        }
    }
}
#[test]
fn malformed_arguments_and_unexpected_value_have_no_effects() {
    let mut v = Book::funded();
    for m in [ids::CLAIM, ids::CANCEL, ids::REFUND] {
        for b in [vec![], vec![8], vec![0, 0]] {
            fails(v.call(OWNER, m, b, 0, 150), E_ARGS)
        }
    }
    fails(v.call(OWNER, ids::STATUS, vec![], 1, 150), E_VALUE)
}
#[test]
fn nonce_exhaustion_is_fail_closed() {
    let mut v = Book::funded();
    let mut s = v.snapshot();
    s.nonce = u64::MAX;
    v.state = s.encode();
    fails(v.claim(0, 150), E_OVERFLOW)
}
#[test]
fn accounting_and_reserved_corruption_is_rejected() {
    let v = Book::funded();
    for o in
        [1657, 1673, 1672 + 24, 1672 + 56, 1672 + 56 + 16, 1672 + 56 + 40, STATE_BASE + 7 * 200]
    {
        let mut b = v.state.clone();
        b[o] ^= 1;
        assert!(State::decode(&b).is_err(), "{o}")
    }
}
#[test]
fn all_sixteen_lanes_at_u128_max_settle_in_reverse() {
    let mut c = config();
    c.count = 8;
    let q = u128::MAX / 8;
    for i in 0..8 {
        c.streams[i] = Stream {
            key: BENEFICIARIES[0],
            payout: address(2 + i as u8 * 2),
            refund: address(3 + i as u8 * 2),
            total: q + if i == 7 { 7 } else { 0 },
            start: 100,
            cliff: 100,
            end: 200,
            step: 0,
            revocable: true,
        }
    }
    let mut v = Book::with_config(c);
    v.fund();
    for i in 0..8 {
        v.cancel(i, 150);
        assert_eq!(v.claim(i, 150).status_code, 0);
        assert_eq!(v.refund(i, 150).status_code, 0)
    }
    assert_eq!(v.snapshot().held(), Ok(0));
    assert_eq!(v.snapshot().totals().unwrap().2, u128::MAX);
    for i in (0..8).rev() {
        assert_eq!(v.settle(i, 1, true).status_code, 0);
        assert_eq!(v.settle(i, 0, true).status_code, 0)
    }
    let (a, b, c) = v.snapshot().totals().unwrap();
    assert_eq!(a.checked_add(b), Some(u128::MAX));
    assert_eq!(c, 0);
    assert_eq!(v.snapshot().held(), Ok(0))
}

fn receiver(kind: u8, index: u8) -> Vec<u8> {
    initialize::<Receiver>(&Spec {
        destination: address(2),
        deploy: true,
        body: envelope(&[kind, index]),
        ..Spec::default()
    })
}
fn bound_receiver(kind: u8, index: u8) -> Vec<u8> {
    let b = receiver(kind, index);
    let mut body = vec![0; 64];
    put_address(&mut body, 0, address(1));
    let s = Spec { destination: address(2), method: ids::BIND, body, ..Spec::default() };
    let out = invoke::<Receiver>(&b, &s);
    apply(&b, out.state_update.as_ref().unwrap())
}
fn receipt_spec(kind: u8, index: u8, nonce: u64, amount: u128) -> Spec {
    Spec {
        source: Some(address(1)),
        destination: address(2),
        method: ids::RECEIVE,
        body: payment_body(index, kind, nonce, amount),
        value: amount,
        request: true,
        ..Spec::default()
    }
}
#[test]
fn receiver_is_bound_by_owner_once() {
    let b = receiver(0, 0);
    let mut body = vec![0; 64];
    put_address(&mut body, 0, address(1));
    let s = Spec {
        actor: STRANGER,
        destination: address(2),
        method: ids::BIND,
        body: body.clone(),
        ..Spec::default()
    };
    fails(invoke::<Receiver>(&b, &s), E_RECEIVER_AUTH);
    let b = bound_receiver(0, 0);
    fails(invoke::<Receiver>(&b, &Spec { actor: OWNER, ..s }), E_RECEIVER_ARGS)
}
#[test]
fn receiver_enforces_configured_role_index_and_amount() {
    let b = bound_receiver(0, 0);
    for s in [
        receipt_spec(1, 0, 1, 100),
        receipt_spec(0, 1, 1, 100),
        Spec { value: 99, ..receipt_spec(0, 0, 1, 100) },
    ] {
        fails(invoke::<Receiver>(&b, &s), E_RECEIVER_ARGS)
    }
}
#[test]
fn receiver_requires_bound_source_and_callback_request() {
    let b = bound_receiver(0, 0);
    for s in [
        Spec { source: Some(address(4)), ..receipt_spec(0, 0, 1, 100) },
        Spec { request: false, ..receipt_spec(0, 0, 1, 100) },
        Spec { source: None, ..receipt_spec(0, 0, 1, 100) },
    ] {
        fails(invoke::<Receiver>(&b, &s), E_RECEIVER_AUTH)
    }
}
#[test]
fn receiver_success_credits_once_and_echoes_exact_receipt() {
    let b = bound_receiver(0, 0);
    let s = receipt_spec(0, 0, 1, 100);
    let out = invoke::<Receiver>(&b, &s);
    assert_eq!(out.callback_result_body, s.body);
    let b = apply(&b, out.state_update.as_ref().unwrap());
    assert_eq!(b.len(), RECEIVER_BYTES);
    assert_eq!(u128_at(&b, 112), Ok(100));
    fails(invoke::<Receiver>(&b, &s), E_RECEIVER_ARGS)
}
#[test]
fn receiver_rejection_has_no_credit_effect() {
    let b = bound_receiver(0, 0);
    let out = invoke::<Receiver>(
        &b,
        &Spec { destination: address(2), method: ids::REJECT, body: vec![1], ..Spec::default() },
    );
    let b = apply(&b, out.state_update.as_ref().unwrap());
    fails(invoke::<Receiver>(&b, &receipt_spec(0, 0, 1, 100)), E_REJECT)
}
#[test]
fn receiver_rejects_unexpected_currency_and_overflow() {
    let b = bound_receiver(0, 0);
    fails(
        invoke::<Receiver>(&b, &Spec { extra: vec![(2, 1)], ..receipt_spec(0, 0, 1, 100) }),
        E_RECEIVER_ARGS,
    );
    let out = invoke::<Receiver>(&b, &receipt_spec(0, 0, 1, u128::MAX));
    let b = apply(&b, out.state_update.as_ref().unwrap());
    fails(invoke::<Receiver>(&b, &receipt_spec(0, 0, 2, 1)), E_RECEIVER_ARGS)
}

#[test]
fn generated_sequences_conserve_principal_and_preserve_earned_rights() {
    let mut seed = 0x54378291u64;
    for _ in 0..80 {
        let mut v = Book::funded();
        let mut paid = [[0u128; 2]; 3];
        for step in 0..180 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let i = (seed % 3) as usize;
            let kind = ((seed >> 8) % 2) as usize;
            let time = 100 + step;
            v.time = time;
            if (seed >> 16).is_multiple_of(4) && v.snapshot().progress[i].lanes[kind].pending > 0 {
                let amount = v.snapshot().progress[i].lanes[kind].pending;
                let success = seed & 1 != 0;
                assert_eq!(v.settle(i, kind, success).status_code, 0);
                if success {
                    paid[i][kind] += amount
                }
            } else {
                let actor = if seed & 7 == 0 {
                    STRANGER
                } else if (seed >> 24).is_multiple_of(3) {
                    OWNER
                } else {
                    BENEFICIARIES[i]
                };
                let method = [ids::CLAIM, ids::CANCEL, ids::REFUND, ids::STATUS]
                    [((seed >> 32) % 4) as usize];
                let body = if method == ids::STATUS { vec![] } else { vec![i as u8] };
                let before = v.state.clone();
                let out = v.call(actor, method, body, 0, time);
                if out.status_code != 0 {
                    assert_eq!(v.state, before);
                    assert!(out.outbound_intents.is_empty() && out.event_intents.is_empty())
                }
            }
            let s = v.snapshot();
            let mut accounted = 0;
            for (j, ledger) in paid.iter().enumerate() {
                assert_eq!(s.progress[j].claimed, ledger[0]);
                assert_eq!(s.progress[j].refunded, ledger[1]);
                let p = s.progress[j];
                let earned = if p.cancelled {
                    p.cap
                } else {
                    let cfg = s.config.streams[j];
                    let elapsed = if s.last_time < cfg.cliff {
                        0
                    } else {
                        s.last_time.min(cfg.end).saturating_sub(cfg.start)
                    };
                    let elapsed = if cfg.step > 0 && s.last_time < cfg.end {
                        elapsed / cfg.step * cfg.step
                    } else {
                        elapsed
                    };
                    cfg.total * u128::from(elapsed) / u128::from(cfg.end - cfg.start)
                };
                assert!(p.claimed + p.lanes[0].pending <= earned);
                if p.cancelled {
                    assert!(p.refunded + p.lanes[1].pending <= s.config.streams[j].total - earned)
                } else {
                    assert_eq!(p.refunded, 0)
                }
                accounted += p.claimed + p.refunded + p.lanes[0].pending + p.lanes[1].pending
            }
            assert_eq!(accounted + s.held().unwrap(), 2000);
        }
    }
}
