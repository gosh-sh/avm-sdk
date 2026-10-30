use avm_bounty_escrow::*;
use avm_bounty_receiver::Receiver;
use avm_contract_sdk::{
    decode_message_input, encode_message_output, AccountAddress, Contract, MessageOutput,
    StateUpdate,
};

const OWNER: [u8; 32] = [0x41; 32];
const WORKER: [u8; 32] = [0x42; 32];
const STRANGER: [u8; 32] = [0x43; 32];
fn address(id: u8) -> AccountAddress {
    AccountAddress { dapp_id: [0x11; 32], account_id: [id; 32] }
}
fn config() -> Config {
    Config {
        owner: OWNER,
        worker: WORKER,
        refund_to: address(2),
        pay_to: address(3),
        total: 1000,
        deadline: 100,
        count: 3,
        amounts: [300, 400, 300, 0, 0, 0, 0, 0],
        fuel_reserve: 10000,
    }
}

fn envelope(init: &[u8]) -> Vec<u8> {
    let mut b = vec![0; 288];
    b[..4].copy_from_slice(b"ACDP");
    b[268..272].copy_from_slice(&(init.len() as u32).to_le_bytes());
    b.extend_from_slice(init);
    b
}
fn extras(b: &mut Vec<u8>, entries: &[(u32, u128)]) {
    b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (id, amount) in entries {
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&amount.to_le_bytes());
    }
}
fn frame(
    message: &[u8],
    state: &[u8],
    actor: [u8; 32],
    source: Option<AccountAddress>,
    destination: AccountAddress,
    time: u32,
    balance: u128,
) -> Vec<u8> {
    let mut b = Vec::from(*b"ACMI");
    b.extend_from_slice(&1u64.to_le_bytes());
    b.extend_from_slice(&time.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&[0x12; 32]);
    b.push(u8::from(source.is_none()));
    b.extend_from_slice(&[0; 7]);
    if let Some(a) = source {
        b.extend_from_slice(&a.dapp_id);
        b.extend_from_slice(&a.account_id);
        b.extend_from_slice(&[0; 32]);
    } else {
        b.extend_from_slice(&[0; 64]);
        b.extend_from_slice(&actor);
    }
    b.extend_from_slice(&destination.dapp_id);
    b.extend_from_slice(&destination.account_id);
    b.extend_from_slice(&[0x44; 32]);
    b.extend_from_slice(&[0; 8]);
    extras(&mut b, &[(PRINCIPAL_CURRENCY_ID, balance), (SHELL_CURRENCY_ID, 1000000)]);
    b.extend_from_slice(&(state.len() as u32).to_le_bytes());
    b.extend_from_slice(state);
    b.extend_from_slice(&(message.len() as u32).to_le_bytes());
    b.extend_from_slice(message);
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b
}
fn external(
    body: &[u8],
    actor: [u8; 32],
    method: u64,
    value: u128,
    destination: AccountAddress,
    deploy: bool,
    extra: &[(u32, u128)],
) -> Vec<u8> {
    let mut entries = Vec::new();
    if value != 0 {
        entries.push((PRINCIPAL_CURRENCY_ID, value));
    }
    entries.extend_from_slice(extra);
    let mut b = vec![0; 192];
    b[0] = 1;
    b[1] = if deploy { 3 } else { 1 };
    b[2..4].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    b[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    b[8..16].copy_from_slice(&method.to_le_bytes());
    b[16..32].copy_from_slice(&10000u128.to_le_bytes());
    b[32..40].copy_from_slice(&1u64.to_le_bytes());
    b[40..44].copy_from_slice(&1u32.to_le_bytes());
    b[44..48].copy_from_slice(&1000u32.to_le_bytes());
    write_address(&mut b, 48, destination);
    b[112..144].copy_from_slice(&actor);
    b[144..146].copy_from_slice(&64u16.to_le_bytes());
    for (id, n) in entries {
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
    }
    b.extend_from_slice(body);
    b.extend_from_slice(&[0; 64]);
    b
}
fn evaluate(
    state: &[u8],
    actor: [u8; 32],
    method: u64,
    body: &[u8],
    value: u128,
    time: u32,
) -> MessageOutput {
    let message = external(body, actor, method, value, address(1), false, &[]);
    let b = frame(&message, state, actor, None, address(1), time, 10000);
    let out = Escrow::process_message(decode_message_input(&b).unwrap());
    encode_message_output(&out).expect("every returned effect set must be canonical");
    out
}
fn state_from(out: &MessageOutput) -> Vec<u8> {
    assert_eq!(out.status_code, 0);
    let Some(StateUpdate::FullReplace(b)) = &out.state_update else { panic!("missing state") };
    State::decode(b).expect("escrow accounting invariant");
    b.clone()
}
fn failure(out: MessageOutput, code: u32) {
    assert_eq!(out.status_code, code);
    assert_eq!(out.state_update, None);
    assert!(
        out.outbound_intents.is_empty()
            && out.callback_intents.is_empty()
            && out.event_intents.is_empty()
    );
}
fn initialized() -> Vec<u8> {
    let message = external(&envelope(&config().encode()), OWNER, 0, 0, address(1), true, &[]);
    let b = frame(&message, &[], OWNER, None, address(1), 10, 0);
    state_from(&Escrow::init(decode_message_input(&b).unwrap()))
}
fn funded() -> Vec<u8> {
    state_from(&evaluate(&initialized(), OWNER, ids::FUND, &[], 1000, 11))
}
fn hash_args(index: u8, hash: u8) -> Vec<u8> {
    let mut b = vec![index];
    b.extend_from_slice(&[hash; 32]);
    b
}
fn approved(index: u8) -> Vec<u8> {
    let s = state_from(&evaluate(&funded(), WORKER, ids::SUBMIT, &hash_args(index, 0x51), 0, 12));
    state_from(&evaluate(&s, OWNER, ids::APPROVE, &hash_args(index, 0x51), 0, 13))
}
fn callback_message(
    source: AccountAddress,
    body: &[u8],
    status: u32,
    value: u128,
    hash: [u8; 32],
) -> Vec<u8> {
    let mut b = vec![0; 176];
    b[1] = 1;
    b[2..4].copy_from_slice(&u16::from(value != 0).to_le_bytes());
    b[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    b[8..16].copy_from_slice(&ids::CALLBACK.to_le_bytes());
    b[32..40].copy_from_slice(&5u64.to_le_bytes());
    b[40..44].copy_from_slice(&20u32.to_le_bytes());
    write_address(&mut b, 48, source);
    write_address(&mut b, 112, address(1));
    b.extend_from_slice(&hash);
    b.extend_from_slice(&address(1).dapp_id);
    b.extend_from_slice(&address(1).account_id);
    b.extend_from_slice(&status.to_le_bytes());
    if value != 0 {
        b.extend_from_slice(&PRINCIPAL_CURRENCY_ID.to_le_bytes());
        b.extend_from_slice(&value.to_le_bytes());
    }
    b.extend_from_slice(body);
    b
}
fn callback(
    state: &[u8],
    source: AccountAddress,
    body: &[u8],
    status: u32,
    value: u128,
    hash: [u8; 32],
) -> MessageOutput {
    let msg = callback_message(source, body, status, value, hash);
    let b = frame(&msg, state, [0; 32], Some(source), address(1), 20, 1000);
    let out = Escrow::process_message(decode_message_input(&b).unwrap());
    encode_message_output(&out).unwrap();
    out
}

#[test]
fn config_roundtrip_and_state_roundtrip() {
    assert_eq!(Config::decode(&config().encode()), Ok(config()));
    let s = initialized();
    assert_eq!(State::decode(&s).unwrap().encode(), s);
}
#[test]
fn init_requires_owner_signature() {
    let msg = external(&envelope(&config().encode()), STRANGER, 0, 0, address(1), true, &[]);
    let b = frame(&msg, &[], STRANGER, None, address(1), 10, 0);
    failure(Escrow::init(decode_message_input(&b).unwrap()), ERR_AUTH);
}
#[test]
fn config_rejects_zero_or_duplicate_identities() {
    for zero in [false, true] {
        let mut c = config();
        c.worker = if zero { [0; 32] } else { c.owner };
        assert_eq!(Config::decode(&c.encode()), Err(ERR_ARGS));
    }
}
#[test]
fn config_rejects_zero_or_alias_destinations() {
    let mut c = config();
    c.refund_to = c.pay_to;
    assert_eq!(Config::decode(&c.encode()), Err(ERR_ARGS));
    c.refund_to = address(0);
    assert_eq!(Config::decode(&c.encode()), Err(ERR_ARGS));
}
#[test]
fn config_rejects_invalid_count_and_amounts() {
    for count in [0, 9, 255] {
        let mut c = config();
        c.count = count;
        assert_eq!(Config::decode(&c.encode()), Err(ERR_ARGS));
    }
    let mut c = config();
    c.amounts[0] = 0;
    assert_eq!(Config::decode(&c.encode()), Err(ERR_ARGS));
}
#[test]
fn config_rejects_overflow_and_wrong_total() {
    let mut c = config();
    c.total += 1;
    assert_eq!(Config::decode(&c.encode()), Err(ERR_ARGS));
    c.total = u128::MAX;
    c.amounts[0] = u128::MAX;
    assert_eq!(Config::decode(&c.encode()), Err(ERR_ARGS));
}
#[test]
fn config_and_state_reject_truncation_without_panics() {
    let c = config().encode();
    for n in 0..c.len() {
        assert!(Config::decode(&c[..n]).is_err());
    }
    let s = initialized();
    for n in 0..s.len() {
        assert!(State::decode(&s[..n]).is_err());
    }
}
#[test]
fn deployment_envelope_lengths_are_checked() {
    let b = envelope(&config().encode());
    let extract = |body: &[u8]| {
        let message = external(body, OWNER, 0, 0, address(1), true, &[]);
        let bytes = frame(&message, &[], OWNER, None, address(1), 10, 0);
        decode_message_input(&bytes).unwrap().deploy().map(|d| d.init_data().to_vec())
    };
    assert_eq!(extract(&b).unwrap(), config().encode());
    for n in 0..b.len() {
        assert!(extract(&b[..n]).is_err());
    }
    let mut b = b;
    b.push(0);
    assert!(extract(&b).is_err());
}
#[test]
fn funding_is_exact_and_once() {
    let s = initialized();
    for value in [0, 999, 1001] {
        failure(evaluate(&s, OWNER, ids::FUND, &[], value, 11), ERR_VALUE);
    }
    let s = funded();
    failure(evaluate(&s, OWNER, ids::FUND, &[], 1000, 12), ERR_PHASE);
}
#[test]
fn funding_requires_owner_and_live_deadline() {
    failure(evaluate(&initialized(), STRANGER, ids::FUND, &[], 1000, 11), ERR_AUTH);
    failure(evaluate(&initialized(), OWNER, ids::FUND, &[], 1000, 100), ERR_DEADLINE);
}
#[test]
fn unfunded_contract_rejects_work_and_refund() {
    failure(evaluate(&initialized(), WORKER, ids::SUBMIT, &hash_args(0, 1), 0, 12), ERR_PHASE);
    failure(evaluate(&initialized(), OWNER, ids::REFUND, &[], 0, 101), ERR_PHASE);
}
#[test]
fn submit_requires_worker_and_nonzero_hash() {
    failure(evaluate(&funded(), OWNER, ids::SUBMIT, &hash_args(0, 1), 0, 12), ERR_AUTH);
    failure(evaluate(&funded(), WORKER, ids::SUBMIT, &hash_args(0, 0), 0, 12), ERR_ARGS);
}
#[test]
fn approving_an_old_revision_fails() {
    let s = state_from(&evaluate(&funded(), WORKER, ids::SUBMIT, &hash_args(0, 1), 0, 12));
    let s = state_from(&evaluate(&s, WORKER, ids::SUBMIT, &hash_args(0, 2), 0, 13));
    failure(evaluate(&s, OWNER, ids::APPROVE, &hash_args(0, 1), 0, 14), ERR_ARGS);
    assert_eq!(
        State::decode(&state_from(&evaluate(&s, OWNER, ids::APPROVE, &hash_args(0, 2), 0, 14)))
            .unwrap()
            .phases[0],
        APPROVED
    );
}
#[test]
fn approval_requires_owner_and_submission() {
    failure(evaluate(&funded(), OWNER, ids::APPROVE, &hash_args(0, 1), 0, 12), ERR_PHASE);
    let s = approved(0);
    failure(evaluate(&s, WORKER, ids::APPROVE, &hash_args(0, 0x51), 0, 14), ERR_AUTH);
}
#[test]
fn deadline_is_exclusive_for_submit_and_approve() {
    failure(evaluate(&funded(), WORKER, ids::SUBMIT, &hash_args(0, 1), 0, 100), ERR_DEADLINE);
    let s = approved(0);
    failure(evaluate(&s, OWNER, ids::APPROVE, &hash_args(0, 0x51), 0, 100), ERR_DEADLINE);
}
#[test]
fn pay_requires_approval_and_authorization() {
    failure(evaluate(&funded(), WORKER, ids::PAY, &[0], 0, 12), ERR_PHASE);
    failure(evaluate(&approved(0), STRANGER, ids::PAY, &[0], 0, 14), ERR_AUTH);
}
#[test]
fn pending_transfer_blocks_second_payment_and_refund() {
    let out = evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14);
    assert_eq!(out.outbound_intents.len(), 1);
    assert_eq!(out.outbound_intents[0].extra[0].amount, 300);
    assert_eq!(out.callback_intents.len(), 1);
    let s = state_from(&out);
    failure(evaluate(&s, WORKER, ids::PAY, &[0], 0, 15), ERR_PENDING);
    failure(evaluate(&s, OWNER, ids::REFUND, &[], 0, 101), ERR_PENDING);
}
#[test]
fn successful_callback_settles_once() {
    let out = evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14);
    let s = state_from(&out);
    let receipt = &out.outbound_intents[0].body;
    let s = state_from(&callback(&s, address(3), receipt, 0, 0, [0x61; 32]));
    let state = State::decode(&s).unwrap();
    assert_eq!((state.paid, state.pending.kind, state.phases[0]), (300, 0, PAID));
    failure(evaluate(&s, WORKER, ids::PAY, &[0], 0, 15), ERR_PHASE);
    failure(callback(&s, address(3), receipt, 0, 0, [0x61; 32]), ERR_CALLBACK);
}
#[test]
fn failed_transfer_returns_principal_and_can_retry() {
    let out = evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14);
    let s = state_from(&out);
    let s = state_from(&callback(&s, address(3), &[], 2102, 300, [0x62; 32]));
    let state = State::decode(&s).unwrap();
    assert_eq!((state.paid, state.phases[0], state.pending.kind), (0, APPROVED, 0));
    let out = evaluate(&s, WORKER, ids::PAY, &[0], 0, 15);
    assert_eq!(State::decode(&state_from(&out)).unwrap().pending.attempt, 2);
}
#[test]
fn failure_without_exact_return_cannot_unlock() {
    let s = state_from(&evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14));
    for value in [0, 299, 301] {
        failure(callback(&s, address(3), &[], 2102, value, [0x63; 32]), ERR_CALLBACK);
    }
}
#[test]
fn wrong_receiver_and_wrong_receipt_are_rejected() {
    let out = evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14);
    let s = state_from(&out);
    failure(
        callback(&s, address(2), &out.outbound_intents[0].body, 0, 0, [0x64; 32]),
        ERR_CALLBACK,
    );
    let mut receipt = out.outbound_intents[0].body.clone();
    receipt[8] += 1;
    failure(callback(&s, address(3), &receipt, 0, 0, [0x64; 32]), ERR_CALLBACK);
}
#[test]
fn old_failure_cannot_settle_a_new_attempt() {
    let s = state_from(&evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14));
    let s = state_from(&callback(&s, address(3), &[], 2102, 300, [0x65; 32]));
    let s = state_from(&evaluate(&s, WORKER, ids::PAY, &[0], 0, 15));
    failure(callback(&s, address(3), &[], 2102, 300, [0x65; 32]), ERR_CALLBACK);
}
#[test]
fn external_callback_impersonation_is_rejected() {
    failure(evaluate(&approved(0), OWNER, ids::CALLBACK, &[0; 32], 0, 14), ERR_ARGS);
}
#[test]
fn refund_excludes_approved_work_and_allows_late_payment() {
    let s = approved(1);
    failure(evaluate(&s, OWNER, ids::REFUND, &[], 0, 99), ERR_DEADLINE);
    let out = evaluate(&s, OWNER, ids::REFUND, &[], 0, 100);
    assert_eq!(out.outbound_intents[0].extra[0].amount, 600);
    let s = state_from(&out);
    let s = state_from(&callback(&s, address(2), &out.outbound_intents[0].body, 0, 0, [0x66; 32]));
    let st = State::decode(&s).unwrap();
    assert_eq!((st.refunded, st.phases[..3].to_vec()), (600, vec![REFUNDED, APPROVED, REFUNDED]));
    let out = evaluate(&s, WORKER, ids::PAY, &[1], 0, 101);
    let s = state_from(&out);
    let st = State::decode(&state_from(&callback(
        &s,
        address(3),
        &out.outbound_intents[0].body,
        0,
        0,
        [0x67; 32],
    )))
    .unwrap();
    assert_eq!(st.paid + st.refunded, st.funded);
}
#[test]
fn failed_refund_keeps_unapproved_balance_claimable() {
    let out = evaluate(&funded(), OWNER, ids::REFUND, &[], 0, 100);
    let s = state_from(&out);
    let s = state_from(&callback(&s, address(2), &[], 2102, 1000, [0x68; 32]));
    assert_eq!(State::decode(&s).unwrap().refunded, 0);
    assert_eq!(evaluate(&s, OWNER, ids::REFUND, &[], 0, 101).status_code, 0);
}
#[test]
fn attempt_budget_is_bounded_without_forgetting_old_receipts() {
    let mut s = approved(0);
    for i in 1..=MAX_ATTEMPTS {
        let out = evaluate(&s, WORKER, ids::PAY, &[0], 0, 14);
        s = state_from(&out);
        s = state_from(&callback(&s, address(3), &[], 2102, 300, [i as u8; 32]));
    }
    failure(evaluate(&s, WORKER, ids::PAY, &[0], 0, 15), ERR_LIMIT);
    assert_eq!(State::decode(&s).unwrap().receipts.len(), 64);
}
#[test]
fn corruption_of_accounting_or_pending_fields_fails_closed() {
    let s = funded();
    for offset in [376, 392, 408, 416, 417, 418, 424, 440, 448, 456, 712, 720] {
        let mut b = s.clone();
        b[offset] ^= 0x7f;
        assert!(State::decode(&b).is_err(), "offset {offset}");
        failure(evaluate(&b, OWNER, ids::STATUS, &[], 0, 12), ERR_STATE);
    }
}
#[test]
fn malformed_arguments_and_attached_value_have_no_effects() {
    for method in [ids::SUBMIT, ids::APPROVE, ids::PAY] {
        for len in [0, 2, 32, 34, 200] {
            let out = evaluate(
                &funded(),
                if method == ids::SUBMIT { WORKER } else { OWNER },
                method,
                &vec![255; len],
                0,
                12,
            );
            assert_ne!(out.status_code, 0);
            assert!(out.state_update.is_none());
        }
    }
    failure(evaluate(&funded(), OWNER, ids::STATUS, &[], 1, 12), ERR_VALUE);
}
#[test]
fn reserved_config_and_state_bytes_are_canonical() {
    let mut c = config().encode();
    c[213] = 1;
    assert_eq!(Config::decode(&c), Err(ERR_ARGS));
    let mut s = initialized();
    s[418] = 1;
    assert!(State::decode(&s).is_err());
}
#[test]
fn status_is_nonmutating_and_available_while_pending() {
    let s = state_from(&evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14));
    assert_eq!(state_from(&evaluate(&s, STRANGER, ids::STATUS, &[], 0, 15)), s);
}
#[test]
fn receiver_binding_is_owner_only_and_immutable() {
    let message = external(&envelope(&[]), OWNER, 0, 0, address(3), true, &[]);
    let b = frame(&message, &[], OWNER, None, address(3), 10, 0);
    let out = Receiver::init(decode_message_input(&b).unwrap());
    let Some(StateUpdate::FullReplace(s)) = out.state_update else { panic!() };
    let mut body = vec![0; 64];
    write_address(&mut body, 0, address(1));
    for (actor, expected) in [(STRANGER, 2100), (OWNER, 0)] {
        let msg = external(&body, actor, ids::BIND, 0, address(3), false, &[]);
        let b = frame(&msg, &s, actor, None, address(3), 11, 0);
        let out = Receiver::process_message(decode_message_input(&b).unwrap());
        assert_eq!(out.status_code, expected);
        if expected == 0 {
            let Some(StateUpdate::FullReplace(s2)) = out.state_update else { panic!() };
            let b = frame(&msg, &s2, actor, None, address(3), 12, 0);
            assert_eq!(
                Receiver::process_message(decode_message_input(&b).unwrap()).status_code,
                2101
            );
        }
    }
}

#[test]
fn payout_refuses_insolvent_principal_or_missing_fee_reserve() {
    let s = approved(0);
    let message = external(&[0], WORKER, ids::PAY, 0, address(1), false, &[]);
    let b = frame(&message, &s, WORKER, None, address(1), 14, 10000);
    let insolvent = frame(&message, &s, WORKER, None, address(1), 14, 999);
    failure(Escrow::process_message(decode_message_input(&insolvent).unwrap()), ERR_BALANCE);
    let mut b = b;
    // ACMI extra count260; principal entry262..282; SHELL amount286..302.
    b[286..302].copy_from_slice(&9999u128.to_le_bytes());
    failure(Escrow::process_message(decode_message_input(&b).unwrap()), ERR_BALANCE);
}

#[test]
fn only_shell_can_be_donated_to_execution_fees() {
    let state = initialized();
    for (extra, value, expected) in [
        (vec![(2, 10000)], 0, 0),
        (vec![(7, 10000)], 0, ERR_VALUE),
        (vec![(2, 10000)], 1, ERR_VALUE),
        (vec![], 0, ERR_VALUE),
    ] {
        let msg = external(&[], STRANGER, ids::TOPUP, value, address(1), false, &extra);
        let b = frame(&msg, &state, STRANGER, None, address(1), 12, 0);
        let out = Escrow::process_message(decode_message_input(&b).unwrap());
        assert_eq!(out.status_code, expected);
        if expected == 0 {
            assert_eq!(state_from(&out), state);
        }
    }
}

#[test]
fn ordinary_internal_message_cannot_impersonate_callback() {
    let s = state_from(&evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14));
    let receipt = payment_body(State::decode(&s).unwrap().pending);
    let mut msg = callback_message(address(3), &receipt, 0, 0, [0x71; 32]);
    msg[1] = 0;
    msg.drain(176..276); // Remove the native callback header from an ordinary message.
    let b = frame(&msg, &s, [0; 32], Some(address(3)), address(1), 20, 1000);
    failure(Escrow::process_message(decode_message_input(&b).unwrap()), ERR_AUTH);
}

#[test]
fn generated_action_sequences_preserve_principal_conservation() {
    // Deterministic sequence generator: tests illegal calls interleaved with
    // receipts; the accounting assertion is independent of the contract codec.
    let mut seed = 0x6513a09du64;
    for _ in 0..100 {
        let mut s = funded();
        for step in 0..80 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let index = (seed % 3) as u8;
            let actor = [OWNER, WORKER, STRANGER][((seed >> 8) % 3) as usize];
            let time = 12 + step;
            let (method, body) = match (seed >> 16) % 5 {
                0 => (ids::SUBMIT, hash_args(index, 0x51)),
                1 => (ids::APPROVE, hash_args(index, 0x51)),
                2 => (ids::PAY, vec![index]),
                3 => (ids::REFUND, vec![]),
                _ => (ids::STATUS, vec![]),
            };
            let out = evaluate(&s, actor, method, &body, 0, time);
            if out.status_code == 0 {
                s = state_from(&out);
                if !out.outbound_intents.is_empty() {
                    let st = State::decode(&s).unwrap();
                    let success = seed & 1 != 0;
                    let hash = [st.attempts as u8; 32];
                    let receipt =
                        if success { out.outbound_intents[0].body.clone() } else { vec![] };
                    s = state_from(&callback(
                        &s,
                        out.outbound_intents[0].destination,
                        &receipt,
                        if success { 0 } else { 2102 },
                        if success { 0 } else { st.pending.amount },
                        hash,
                    ));
                }
            } else {
                assert!(out.state_update.is_none() && out.outbound_intents.is_empty());
            }
            let st = State::decode(&s).unwrap();
            let retained: u128 = st.phases[..3]
                .iter()
                .enumerate()
                .filter(|(_, p)| !matches!(**p, PAID | REFUNDED))
                .map(|(i, _)| st.config.amounts[i])
                .sum();
            assert_eq!(st.paid + st.refunded + retained, 1000);
        }
    }
}

#[test]
fn funding_and_outbound_use_only_the_selected_principal_currency() {
    for entries in [vec![(2, 1000)], vec![(7, 1000)], vec![(0, 1000), (2, 1)]] {
        let msg = external(&[], OWNER, ids::FUND, 0, address(1), false, &entries);
        let bytes = frame(&msg, &initialized(), OWNER, None, address(1), 11, 1000);
        failure(Escrow::process_message(decode_message_input(&bytes).unwrap()), ERR_VALUE);
    }
    let out = evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14);
    assert_eq!(
        out.outbound_intents[0].extra,
        vec![avm_contract_sdk::CurrencyAmount { currency_id: PRINCIPAL_CURRENCY_ID, amount: 300 }]
    );
}

#[test]
fn callbacks_separate_exact_principal_from_optional_shell_refund() {
    let out = evaluate(&approved(0), WORKER, ids::PAY, &[0], 0, 14);
    let state = state_from(&out);
    for success in [false, true] {
        for currency in [SHELL_CURRENCY_ID, 7] {
            let body = if success { out.outbound_intents[0].body.as_slice() } else { &[] };
            let mut msg = callback_message(
                address(3),
                body,
                if success { 0 } else { 2102 },
                if success { 0 } else { 300 },
                [0x91; 32],
            );
            let count = u16::from(!success) + 1;
            msg[2..4].copy_from_slice(&count.to_le_bytes());
            let position = 276 + if success { 0 } else { 20 };
            let mut extra = currency.to_le_bytes().to_vec();
            extra.extend_from_slice(&17u128.to_le_bytes());
            msg.splice(position..position, extra);
            let bytes = frame(&msg, &state, [0; 32], Some(address(3)), address(1), 20, 1000);
            let result = Escrow::process_message(decode_message_input(&bytes).unwrap());
            if currency == SHELL_CURRENCY_ID {
                let s = State::decode(&state_from(&result)).unwrap();
                assert_eq!((s.pending.kind, s.paid), (0, if success { 300 } else { 0 }));
            } else {
                failure(result, ERR_CALLBACK);
            }
        }
    }
}
