mod support;
use avm_revenue_core::{vault::*, *};
use avm_revenue_vault::RevenueVault;
use support::*;

#[test]
fn config_and_state_are_canonical() {
    let c = config();
    assert_eq!(Config::decode(&c.encode()), Ok(c));
    let v = Vault::new();
    assert_eq!(v.snapshot().encode(), v.state);
}

#[test]
fn sixteen_beneficiaries_can_hold_and_settle_full_u128_revenue_concurrently() {
    let mut c = config();
    c.count = 16;
    for (i, m) in c.members.iter_mut().enumerate() {
        *m = Member { key: [0x61 + i as u8; 32], destination: address(2 + i as u8), weight: 625 };
    }
    let spec = Spec { deploy: true, body: envelope(&c.encode()), ..Spec::default() };
    let state = initialize::<RevenueVault>(&spec);
    let mut v = Vault { state, time: 10, lt: 10 };
    v.deposit(u128::MAX);
    for i in 0..16 {
        assert_eq!(v.call(c.members[i].key, ids::CLAIM, vec![i as u8], 0).status_code, 0);
    }
    assert_eq!(v.snapshot().held(), Ok(15));
    for i in (0..16).rev() {
        assert_eq!(v.settle(i, true).status_code, 0);
    }
    assert_eq!(v.snapshot().paid, u128::MAX - 15);
    assert_eq!(v.snapshot().dust(), Ok(15));
    assert_eq!(v.snapshot().pending(), Ok(0));
}

#[test]
fn initialization_rejects_an_untested_cross_dapp_destination() {
    let mut c = config();
    c.members[0].destination.dapp_id = [0x22; 32];
    let spec = Spec { deploy: true, body: envelope(&c.encode()), ..Spec::default() };
    let bytes = frame(&[], &spec);
    fails(&<RevenueVault as avm_contract_sdk::Contract>::init(input(&bytes)), E_ARGS);
}
#[test]
fn shareholder_weights_must_sum_to_10000() {
    let mut c = config();
    c.members[0].weight += 1;
    assert_eq!(Config::decode(&c.encode()), Err(E_ARGS));
}
#[test]
fn duplicate_signers_are_rejected() {
    let mut c = config();
    c.signers[1] = c.signers[0];
    assert_eq!(Config::decode(&c.encode()), Err(E_ARGS));
}
#[test]
fn duplicate_recipient_keys_are_rejected() {
    let mut c = config();
    c.members[1].key = c.members[0].key;
    assert_eq!(Config::decode(&c.encode()), Err(E_ARGS));
}
#[test]
fn duplicate_destinations_are_rejected() {
    let mut c = config();
    c.members[1].destination = c.members[0].destination;
    assert_eq!(Config::decode(&c.encode()), Err(E_ARGS));
}
#[test]
fn invalid_member_count_or_unused_entries_are_rejected() {
    for n in [0, 17, 255] {
        let mut c = config();
        c.count = n;
        assert!(Config::decode(&c.encode()).is_err());
    }
    let mut c = config();
    c.members[5].weight = 1;
    assert!(Config::decode(&c.encode()).is_err());
}
#[test]
fn zero_keys_addresses_weights_and_reserves_are_rejected() {
    for kind in 0..4 {
        let mut c = config();
        match kind {
            0 => c.members[0].key = [0; 32],
            1 => c.members[0].destination = zero_address(),
            2 => c.members[0].weight = 0,
            _ => c.initial_fuel = 0,
        }
        assert!(Config::decode(&c.encode()).is_err());
    }
}
#[test]
fn entitlement_matches_integer_reference_for_small_values() {
    for total in 0..1000 {
        for weight in [1, 100, 3333, 5000, 9999, 10000] {
            assert_eq!(entitlement(total, weight), Ok(total * u128::from(weight) / 10000));
        }
    }
}
#[test]
fn entitlement_handles_u128_max_without_multiplication_overflow() {
    for w in [1, 3333, 5000, 9999, 10000] {
        let n = entitlement(u128::MAX, w).unwrap();
        assert_eq!(
            n,
            (u128::MAX / 10000) * u128::from(w) + (u128::MAX % 10000) * u128::from(w) / 10000
        );
    }
    assert_eq!(entitlement(u128::MAX, 10000), Ok(u128::MAX));
}
#[test]
fn full_u128_deposit_preserves_all_member_obligations() {
    let mut s = State::new(config());
    s.deposit(u128::MAX).unwrap();
    s.validate().unwrap();
    assert!(s.dust().unwrap() < 3);
    assert_eq!(State::decode(&s.encode()).unwrap().received, u128::MAX);
}
#[test]
fn deposit_overflow_is_atomic() {
    let mut s = State::new(config());
    s.deposit(u128::MAX).unwrap();
    let before = s.clone();
    assert_eq!(s.deposit(1), Err(E_OVERFLOW));
    assert_eq!(s, before);
}
#[test]
fn fragmentation_cannot_change_distribution() {
    let mut a = State::new(config());
    a.deposit(10_007).unwrap();
    let mut b = State::new(config());
    for _ in 0..10_007 {
        b.deposit(1).unwrap();
    }
    assert_eq!(a.members, b.members);
    assert_eq!(a.dust(), b.dust());
}
#[test]
fn carried_rounding_dust_becomes_claimable() {
    let mut v = Vault::new();
    v.deposit(10007);
    assert_eq!(v.snapshot().dust(), Ok(1));
    v.deposit(3);
    assert_eq!(v.snapshot().dust(), Ok(0));
    assert_eq!(
        v.snapshot().members[..3].iter().map(|m| m.claimable).collect::<Vec<_>>(),
        vec![5005, 3003, 2002]
    );
}
#[test]
fn principal_state_corruption_fails_closed() {
    let mut v = Vault::new();
    v.deposit(1000);
    for offset in [1792, 1808, 1840, 1841, 1896, 1912, 1928, 1944, 1960] {
        let mut s = v.state.clone();
        s[offset] ^= 0x7f;
        assert!(State::decode(&s).is_err(), "{offset}");
        fails(&invoke::<RevenueVault>(&s, &Spec::default()), E_STATE);
    }
}
#[test]
fn config_and_state_truncations_do_not_panic() {
    let c = config().encode();
    for n in 0..c.len() {
        assert!(Config::decode(&c[..n]).is_err());
    }
    let v = Vault::new();
    for n in 0..v.state.len() {
        assert!(State::decode(&v.state[..n]).is_err());
    }
}
#[test]
fn init_rejects_non_manager() {
    let s = Spec {
        actor: STRANGER,
        deploy: true,
        body: envelope(&config().encode()),
        ..Spec::default()
    };
    let b = frame(&[], &s);
    fails(&<RevenueVault as avm_contract_sdk::Contract>::init(input(&b)), E_AUTH);
}
#[test]
fn deposit_requires_value_and_accepts_only_shell_as_extra() {
    let v = Vault::new();
    for extra in [vec![(7, 5)], vec![(2, 5), (7, 6)]] {
        let s = Spec { method: ids::DEPOSIT, value: 1, extra, ..Spec::default() };
        fails(&invoke::<RevenueVault>(&v.state, &s), E_VALUE);
    }
    let mut v = v;
    fails(&v.call(STRANGER, ids::DEPOSIT, vec![], 0), E_VALUE);
}
#[test]
fn claims_pay_fixed_destinations_and_clear_only_after_receipt() {
    let mut v = Vault::new();
    v.deposit(1000);
    let out = v.claim(0);
    assert_eq!(out.status_code, 0);
    assert_eq!(out.outbound_intents[0].destination, address(2));
    assert_eq!(v.snapshot().paid, 0);
    assert_eq!(v.snapshot().members[0].pending, 500);
    assert_eq!(v.settle(0, true).status_code, 0);
    assert_eq!(v.snapshot().paid, 500);
}
#[test]
fn wrong_member_signer_cannot_claim() {
    let mut v = Vault::new();
    v.deposit(1000);
    fails(&v.call(MEMBER_KEYS[1], ids::CLAIM, vec![0], 0), E_AUTH);
}
#[test]
fn duplicate_claim_is_blocked_while_pending() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    fails(&v.claim(0), E_BUSY);
}
#[test]
fn other_members_can_claim_while_one_transfer_is_pending() {
    let mut v = Vault::new();
    v.deposit(1000);
    assert_eq!(v.claim(0).status_code, 0);
    assert_eq!(v.claim(1).status_code, 0);
    assert_eq!(v.claim(2).status_code, 0);
    assert_eq!(v.snapshot().pending(), Ok(1000));
    for i in [2, 0, 1] {
        assert_eq!(v.settle(i, true).status_code, 0);
    }
    assert_eq!(v.snapshot().paid, 1000);
}
#[test]
fn new_revenue_accrues_during_pending_payment() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    v.deposit(1000);
    assert_eq!(v.snapshot().members[0].claimable, 500);
    assert_eq!(v.settle(0, true).status_code, 0);
    assert_eq!(v.snapshot().members[0].paid, 500);
    assert_eq!(v.snapshot().members[0].claimable, 500);
}
#[test]
fn failed_delivery_restores_claimable_without_marking_paid() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    assert_eq!(v.settle(0, false).status_code, 0);
    assert_eq!(v.snapshot().members[0].claimable, 500);
    assert_eq!(v.snapshot().paid, 0);
    assert_eq!(v.claim(0).status_code, 0);
    assert_eq!(v.settle(0, true).status_code, 0);
}
#[test]
fn more_than_64_failures_remain_retryable() {
    let mut v = Vault::new();
    v.deposit(1000);
    for _ in 0..100 {
        assert_eq!(v.claim(0).status_code, 0);
        assert_eq!(v.settle(0, false).status_code, 0);
    }
    assert_eq!(v.snapshot().claim_nonce, 100);
    assert_eq!(v.claim(0).status_code, 0);
    assert_eq!(v.settle(0, true).status_code, 0);
    assert_eq!(v.snapshot().paid, 500);
}
#[test]
fn failure_must_return_exact_principal() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    for n in [0, 499, 501] {
        let mut s = v.receipt_spec(0, false);
        s.value = n;
        fails(&invoke::<RevenueVault>(&v.state, &s), E_CALLBACK);
    }
}
#[test]
fn callback_cannot_come_from_another_recipient() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    let mut s = v.receipt_spec(0, true);
    s.source = Some(address(3));
    fails(&invoke::<RevenueVault>(&v.state, &s), E_CALLBACK);
}
#[test]
fn callback_must_match_the_active_nonce_and_amount() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    let mut s = v.receipt_spec(0, true);
    s.body[1] += 1;
    fails(&invoke::<RevenueVault>(&v.state, &s), E_CALLBACK);
    s.body = payment_body(0, 1, 499);
    fails(&invoke::<RevenueVault>(&v.state, &s), E_CALLBACK);
}
#[test]
fn old_failure_receipt_cannot_unlock_later_attempt() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    let old = v.receipt_spec(0, false);
    let out = invoke::<RevenueVault>(&v.state, &old);
    v.accept(&out);
    v.claim(0);
    fails(&invoke::<RevenueVault>(&v.state, &old), E_CALLBACK);
}
#[test]
fn out_of_order_callback_lt_is_rejected_even_with_new_hash() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    let mut s = v.receipt_spec(0, false);
    s.created_lt = v.snapshot().members[0].after_lt;
    s.callback = Some((3102, [0x95; 32]));
    fails(&invoke::<RevenueVault>(&v.state, &s), E_CALLBACK);
}
#[test]
fn ordinary_internal_message_cannot_impersonate_callback() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    let mut s = v.receipt_spec(0, true);
    s.callback = None;
    fails(&invoke::<RevenueVault>(&v.state, &s), E_AUTH);
}
#[test]
fn duplicate_success_is_rejected_after_settlement() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    let s = v.receipt_spec(0, true);
    let out = invoke::<RevenueVault>(&v.state, &s);
    v.accept(&out);
    fails(&invoke::<RevenueVault>(&v.state, &s), E_CALLBACK);
}
#[test]
fn payout_refuses_missing_operating_shell_or_insolvent_principal() {
    let mut v = Vault::new();
    v.deposit(1000);
    let mut s = Spec {
        actor: MEMBER_KEYS[0],
        method: ids::CLAIM,
        body: vec![0],
        shells: 9999,
        ..Spec::default()
    };
    fails(&invoke::<RevenueVault>(&v.state, &s), E_BALANCE);
    s.shells = 1_000_000;
    s.balance = 999;
    fails(&invoke::<RevenueVault>(&v.state, &s), E_BALANCE);
}
#[test]
fn nonce_exhaustion_fails_without_effects() {
    let mut v = Vault::new();
    v.deposit(1000);
    let mut s = v.snapshot();
    s.claim_nonce = u64::MAX;
    v.state = s.encode();
    fails(&v.claim(0), E_OVERFLOW);
}
#[test]
fn status_remains_available_while_paused_and_pending() {
    let mut v = Vault::new();
    v.deposit(1000);
    v.claim(0);
    v.governance(PAUSE, 0);
    let before = v.state.clone();
    assert_eq!(v.call(STRANGER, ids::STATUS, vec![], 0).status_code, 0);
    assert_eq!(before, v.state);
    assert_eq!(v.settle(0, true).status_code, 0);
}
#[test]
fn varied_deposits_and_claim_orders_preserve_conservation() {
    let mut seed = 19u64;
    for _ in 0..30 {
        let mut v = Vault::new();
        for _ in 0..150 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if seed & 3 == 0 {
                v.deposit(u128::from(seed % 101 + 1));
            } else {
                let i = (seed % 3) as usize;
                let out = v.claim(i);
                if out.status_code == 0 {
                    assert_eq!(v.settle(i, seed & 4 != 0).status_code, 0);
                } else {
                    assert_eq!(out.status_code, E_EMPTY);
                }
            }
            let s = v.snapshot();
            let owed: u128 = s.members.iter().map(|m| m.claimable + m.pending).sum();
            assert_eq!(s.received, s.paid + owed + s.dust().unwrap());
        }
    }
}

#[test]
fn selected_principal_currency_is_distinct_from_shell_and_foreign_assets() {
    let v = Vault::new();
    for extra in [vec![(SHELL_CURRENCY_ID, 1000)], vec![(7, 1000)]] {
        fails(
            &invoke::<RevenueVault>(
                &v.state,
                &Spec { method: ids::DEPOSIT, extra, ..Spec::default() },
            ),
            E_VALUE,
        );
    }
    let out = invoke::<RevenueVault>(
        &v.state,
        &Spec {
            method: ids::DEPOSIT,
            value: 1000,
            extra: vec![(SHELL_CURRENCY_ID, 50)],
            ..Spec::default()
        },
    );
    assert_eq!(out.status_code, 0);
    let mut v = Vault::new();
    v.deposit(1000);
    let out = v.claim(0);
    assert_eq!(
        out.outbound_intents[0].extra,
        vec![avm_contract_sdk::CurrencyAmount { currency_id: PRINCIPAL_CURRENCY_ID, amount: 500 }]
    );
}

#[test]
fn callback_optional_shell_refund_cannot_replace_or_add_principal() {
    for success in [false, true] {
        for currency in [SHELL_CURRENCY_ID, 7] {
            let mut v = Vault::new();
            v.deposit(1000);
            v.claim(0);
            let mut spec = v.receipt_spec(0, success);
            spec.extra = vec![(currency, 17)];
            let out = invoke::<RevenueVault>(&v.state, &spec);
            if currency == SHELL_CURRENCY_ID {
                assert_eq!(out.status_code, 0);
            } else {
                fails(&out, E_CALLBACK);
            }
        }
    }
}
