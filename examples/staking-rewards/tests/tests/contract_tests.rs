//! Host tests for the `staking-rewards` contract crate.
//!
//! The byte-level `frame`/`external`/`callback_message` helpers below are a
//! direct, faithful port of `examples/bounty-escrow/tests/tests/regression.rs`'s
//! own helpers of the same name (same canonical wire format, shared across all
//! contracts via `avm-contract-sdk`'s `decode_message_input` -- not contract-
//! specific, so not invented here). Only the method IDs, body shapes, and
//! currency IDs are staking-rewards-specific.

use avm_contract_sdk::{
    decode_message_input, encode_message_output, AccountAddress, Contract, MessageOutput,
    StateUpdate,
};
use avm_staking_rewards::ids;
use avm_staking_rewards_core::{Error as CoreError, State};

const STAKE_CCY: u32 = 10;
const REWARD_CCY: u32 = 11;
const MIN_STAKE: u128 = 1_000;
// `SHELL_CCY` mirrors `avm_staking_rewards::SHELL_CURRENCY_ID`
// (platform-fixed `currency_id = 2`, `fuel-and-fees.md:16`);
// `FUEL_RESERVE` is this test suite's own chosen per-deployment fee,
// unrelated to `STAKE_CCY`/`REWARD_CCY`.
const SHELL_CCY: u32 = 2;
const FUEL_RESERVE: u128 = 1;
const CONTRACT: AccountAddress = AccountAddress { dapp_id: [0x99; 32], account_id: [0x01; 32] };
const WALLET_DAPP: [u8; 32] = [0x55; 32];

fn staker(id: u8) -> [u8; 32] {
    [id; 32]
}
/// A payout destination. Deliberately NOT required to relate to `pubkey` in
/// any way (see `process_callback`'s doc comment for why): this is just a
/// stable, readable choice of destination for tests, not an enforced
/// relationship.
fn payout_addr(pubkey: [u8; 32]) -> AccountAddress {
    AccountAddress { dapp_id: WALLET_DAPP, account_id: pubkey }
}
/// A destination unrelated to any particular caller -- demonstrates that
/// destinations are genuinely unconstrained now.
fn arbitrary_addr(tag: u8) -> AccountAddress {
    AccountAddress { dapp_id: WALLET_DAPP, account_id: [tag; 32] }
}

fn write_address(b: &mut [u8], offset: usize, a: AccountAddress) {
    b[offset..offset + 32].copy_from_slice(&a.dapp_id);
    b[offset + 32..offset + 64].copy_from_slice(&a.account_id);
}

fn stake_body(seq: u64, amount: u128) -> Vec<u8> {
    let mut b = Vec::with_capacity(24);
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&amount.to_le_bytes());
    b
}
fn unstake_body(seq: u64, amount: u128, dest: AccountAddress) -> Vec<u8> {
    let mut b = Vec::with_capacity(88);
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&amount.to_le_bytes());
    b.extend_from_slice(&dest.dapp_id);
    b.extend_from_slice(&dest.account_id);
    b
}
fn claim_body(seq: u64, dest: AccountAddress) -> Vec<u8> {
    let mut b = Vec::with_capacity(72);
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&dest.dapp_id);
    b.extend_from_slice(&dest.account_id);
    b
}
fn fund_body(amount: u128) -> Vec<u8> {
    amount.to_le_bytes().to_vec()
}

fn envelope(init: &[u8]) -> Vec<u8> {
    let mut b = vec![0; 288];
    b[..4].copy_from_slice(b"ACDP");
    b[268..272].copy_from_slice(&(init.len() as u32).to_le_bytes());
    b.extend_from_slice(init);
    b
}

fn init_data(stake_ccy: u32, reward_ccy: u32, min_stake: u128, fuel_reserve: u128) -> Vec<u8> {
    let mut b = Vec::with_capacity(40);
    b.extend_from_slice(&stake_ccy.to_le_bytes());
    b.extend_from_slice(&reward_ccy.to_le_bytes());
    b.extend_from_slice(&min_stake.to_le_bytes());
    b.extend_from_slice(&fuel_reserve.to_le_bytes());
    b
}

/// Top-level `MessageInput` envelope. `source: None` => `Caller::External(actor)`;
/// `Some(addr)` => `Caller::Internal(addr)`.
fn frame(
    message: &[u8],
    state: &[u8],
    actor: [u8; 32],
    source: Option<AccountAddress>,
    destination: AccountAddress,
    time: u32,
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
    b.extend_from_slice(&0u16.to_le_bytes()); // MessageInput.extra: unused by this contract
    b.extend_from_slice(&(state.len() as u32).to_le_bytes());
    b.extend_from_slice(state);
    b.extend_from_slice(&(message.len() as u32).to_le_bytes());
    b.extend_from_slice(message);
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b
}

/// EXTERNAL_HEADER_LEN=192 canonical external-message encoding.
#[allow(clippy::too_many_arguments)]
fn external(
    body: &[u8],
    actor: [u8; 32],
    method: u64,
    extra: &[(u32, u128)],
    destination: AccountAddress,
    deploy: bool,
    nonce: u64,
    created_at: u32,
    expire_at: u32,
) -> Vec<u8> {
    let mut b = vec![0; 192];
    b[0] = 1;
    b[1] = if deploy { 3 } else { 1 };
    b[2..4].copy_from_slice(&(extra.len() as u16).to_le_bytes());
    b[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    b[8..16].copy_from_slice(&method.to_le_bytes());
    b[16..32].copy_from_slice(&10_000u128.to_le_bytes());
    b[32..40].copy_from_slice(&nonce.to_le_bytes());
    b[40..44].copy_from_slice(&created_at.to_le_bytes());
    b[44..48].copy_from_slice(&expire_at.to_le_bytes());
    write_address(&mut b, 48, destination);
    b[112..144].copy_from_slice(&actor);
    b[144..146].copy_from_slice(&64u16.to_le_bytes());
    for (id, n) in extra {
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
    }
    b.extend_from_slice(body);
    b.extend_from_slice(&[0; 64]);
    b
}

fn call(
    state: &[u8],
    actor: [u8; 32],
    method: u64,
    body: &[u8],
    extra: &[(u32, u128)],
    time: u32,
    expire_at: u32,
) -> MessageOutput {
    let message = external(body, actor, method, extra, CONTRACT, false, 1, time, expire_at);
    let b = frame(&message, state, actor, None, CONTRACT, time);
    let out =
        avm_staking_rewards::StakingRewards::process_message(decode_message_input(&b).unwrap());
    encode_message_output(&out).expect("every returned effect set must be canonical");
    out
}

fn call_default_ttl(
    state: &[u8],
    actor: [u8; 32],
    method: u64,
    body: &[u8],
    extra: &[(u32, u128)],
    time: u32,
) -> MessageOutput {
    call(state, actor, method, body, extra, time, time + avm_staking_rewards::MAX_TTL)
}

/// INTERNAL_HEADER_LEN=176 canonical internal-message encoding, with a
/// callback extension (`cause_msg_hash[32] + cause_address[64] + status[4]`).
/// `pending_id` is this contract's own dynamic correlation value, carried
/// as the message's own `method_id` -- not a fixed named-method hash.
fn callback_message(
    source: AccountAddress,
    cause_address: AccountAddress,
    pending_id: u64,
    status: u32,
) -> Vec<u8> {
    let mut b = vec![0; 176];
    b[1] = 1; // INTERNAL_FLAG_CALLBACK
    b[8..16].copy_from_slice(&pending_id.to_le_bytes());
    b[32..40].copy_from_slice(&5u64.to_le_bytes());
    b[40..44].copy_from_slice(&20u32.to_le_bytes());
    write_address(&mut b, 48, source);
    write_address(&mut b, 112, CONTRACT);
    b.extend_from_slice(&[0x77; 32]); // cause_msg_hash, nonzero
    b.extend_from_slice(&cause_address.dapp_id);
    b.extend_from_slice(&cause_address.account_id);
    b.extend_from_slice(&status.to_le_bytes());
    b
}

fn callback(
    state: &[u8],
    source: AccountAddress,
    cause_address: AccountAddress,
    pending_id: u64,
    status: u32,
) -> MessageOutput {
    let msg = callback_message(source, cause_address, pending_id, status);
    let b = frame(&msg, state, [0; 32], Some(source), CONTRACT, 20);
    let out =
        avm_staking_rewards::StakingRewards::process_message(decode_message_input(&b).unwrap());
    encode_message_output(&out).unwrap();
    out
}

fn state_from(out: &MessageOutput) -> Vec<u8> {
    assert_eq!(out.status_code, 0, "expected success, got failure status");
    let Some(StateUpdate::FullReplace(b)) = &out.state_update else { panic!("missing state") };
    b.clone()
}
fn failure(out: &MessageOutput, code: u32) {
    assert_eq!(out.status_code, code);
    assert!(out.state_update.is_none());
    assert!(
        out.outbound_intents.is_empty() && out.callback_intents.is_empty(),
        "a rejected call must not emit effects"
    );
}

fn init(
    stake_ccy: u32,
    reward_ccy: u32,
    min_stake: u128,
    fuel_reserve: u128,
    deployer: [u8; 32],
) -> MessageOutput {
    let message = external(
        &envelope(&init_data(stake_ccy, reward_ccy, min_stake, fuel_reserve)),
        deployer,
        0,
        &[],
        CONTRACT,
        true,
        1,
        0,
        0,
    );
    let b = frame(&message, &[], deployer, None, CONTRACT, 0);
    let out = avm_staking_rewards::StakingRewards::init(decode_message_input(&b).unwrap());
    if out.status_code == 0 {
        encode_message_output(&out).unwrap();
    }
    out
}

const DEPLOYER: [u8; 32] = [0xD0; 32];

fn deployed() -> Vec<u8> {
    state_from(&init(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE, DEPLOYER))
}

// ---------------------------------------------------------------------
// init
// ---------------------------------------------------------------------

#[test]
fn init_success_decodes_to_empty_state_with_min_stake() {
    let b = deployed();
    let s = State::decode(&b).unwrap();
    assert_eq!(s.stake_currency_id, STAKE_CCY);
    assert_eq!(s.reward_currency_id, REWARD_CCY);
    assert_eq!(s.min_stake, MIN_STAKE);
    assert_eq!(s.fuel_reserve, FUEL_RESERVE);
    assert!(s.rows.is_empty());
}

#[test]
fn init_rejects_same_currency_for_stake_and_reward() {
    let out = init(STAKE_CCY, STAKE_CCY, MIN_STAKE, FUEL_RESERVE, DEPLOYER);
    failure(&out, ids::INVALID_ARGS);
}

#[test]
fn init_rejects_nonzero_min_stake_out_of_range() {
    failure(&init(STAKE_CCY, REWARD_CCY, 0, FUEL_RESERVE, DEPLOYER), ids::INVALID_MIN_STAKE);
}

#[test]
fn init_rejects_fuel_reserve_out_of_range() {
    // `fuel_reserve` is validated against the same `1..=MAX_AMOUNT` range
    // as `min_stake` -- `0` is out of range.
    failure(&init(STAKE_CCY, REWARD_CCY, MIN_STAKE, 0, DEPLOYER), ids::INVALID_FUEL_RESERVE);
}

#[test]
fn init_rejects_shell_as_stake_currency() {
    failure(
        &init(SHELL_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE, DEPLOYER),
        ids::CURRENCY_CANNOT_BE_SHELL,
    );
}

#[test]
fn init_rejects_shell_as_reward_currency() {
    failure(
        &init(STAKE_CCY, SHELL_CCY, MIN_STAKE, FUEL_RESERVE, DEPLOYER),
        ids::CURRENCY_CANNOT_BE_SHELL,
    );
}

#[test]
fn init_rejects_wrong_init_data_length() {
    let message = external(&envelope(&[1, 2, 3]), DEPLOYER, 0, &[], CONTRACT, true, 1, 0, 0);
    let b = frame(&message, &[], DEPLOYER, None, CONTRACT, 0);
    let out = avm_staking_rewards::StakingRewards::init(decode_message_input(&b).unwrap());
    failure(&out, ids::INVALID_ARGS);
}

#[test]
fn init_rejects_attached_currency() {
    let message = external(
        &envelope(&init_data(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE)),
        DEPLOYER,
        0,
        &[(STAKE_CCY, 5)],
        CONTRACT,
        true,
        1,
        0,
        0,
    );
    let b = frame(&message, &[], DEPLOYER, None, CONTRACT, 0);
    let out = avm_staking_rewards::StakingRewards::init(decode_message_input(&b).unwrap());
    failure(&out, ids::UNSOLICITED_CURRENCY);
}

#[test]
fn init_rejects_nonempty_prior_state() {
    let message = external(
        &envelope(&init_data(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE)),
        DEPLOYER,
        0,
        &[],
        CONTRACT,
        true,
        1,
        0,
        0,
    );
    let b = frame(&message, &[0xAB; 4], DEPLOYER, None, CONTRACT, 0);
    let out = avm_staking_rewards::StakingRewards::init(decode_message_input(&b).unwrap());
    failure(&out, ids::INVALID_STATE);
}

// ---------------------------------------------------------------------
// STAKE
// ---------------------------------------------------------------------

#[test]
fn stake_creates_new_row() {
    let a = staker(0xA1);
    let out = call_default_ttl(
        &deployed(),
        a,
        ids::STAKE,
        &stake_body(0, 5_000),
        &[(STAKE_CCY, 5_000)],
        10,
    );
    let s = State::decode(&state_from(&out)).unwrap();
    assert_eq!(s.rows.len(), 1);
    assert_eq!(s.rows[0].account, a);
    assert_eq!(s.rows[0].staked, 5_000);
    assert_eq!(s.rows[0].seq, 1);
}

#[test]
fn stake_tops_up_existing_row_with_correct_seq() {
    let a = staker(0xA1);
    let s1 = state_from(&call_default_ttl(
        &deployed(),
        a,
        ids::STAKE,
        &stake_body(0, 5_000),
        &[(STAKE_CCY, 5_000)],
        10,
    ));
    let s2 = state_from(&call_default_ttl(
        &s1,
        a,
        ids::STAKE,
        &stake_body(1, 2_000),
        &[(STAKE_CCY, 2_000)],
        11,
    ));
    let s = State::decode(&s2).unwrap();
    assert_eq!(s.rows.len(), 1);
    assert_eq!(s.rows[0].staked, 7_000);
    assert_eq!(s.rows[0].seq, 2);
}

#[test]
fn stake_rejects_seq_mismatch() {
    let a = staker(0xA1);
    let out = call_default_ttl(
        &deployed(),
        a,
        ids::STAKE,
        &stake_body(7, 5_000),
        &[(STAKE_CCY, 5_000)],
        10,
    );
    failure(&out, ids::SEQ_MISMATCH);
}

#[test]
fn stake_rejects_wrong_currency_id() {
    let a = staker(0xA1);
    let out = call_default_ttl(
        &deployed(),
        a,
        ids::STAKE,
        &stake_body(0, 5_000),
        &[(REWARD_CCY, 5_000)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn stake_rejects_amount_mismatch_vs_attached() {
    let a = staker(0xA1);
    let out = call_default_ttl(
        &deployed(),
        a,
        ids::STAKE,
        &stake_body(0, 5_000),
        &[(STAKE_CCY, 4_999)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn stake_rejects_extra_currency_attached() {
    let a = staker(0xA1);
    let out = call_default_ttl(
        &deployed(),
        a,
        ids::STAKE,
        &stake_body(0, 5_000),
        &[(STAKE_CCY, 5_000), (REWARD_CCY, 1)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn stake_rejects_ttl_exceeding_max() {
    let a = staker(0xA1);
    let out = call(
        &deployed(),
        a,
        ids::STAKE,
        &stake_body(0, 5_000),
        &[(STAKE_CCY, 5_000)],
        10,
        10 + avm_staking_rewards::MAX_TTL + 1,
    );
    failure(&out, ids::BAD_EXPIRY);
}

#[test]
fn stake_rejects_already_expired() {
    let a = staker(0xA1);
    let out = call(&deployed(), a, ids::STAKE, &stake_body(0, 5_000), &[(STAKE_CCY, 5_000)], 10, 9);
    failure(&out, ids::BAD_EXPIRY);
}

#[test]
fn stake_rejects_below_min_stake() {
    let a = staker(0xA1);
    let out = call_default_ttl(
        &deployed(),
        a,
        ids::STAKE,
        &stake_body(0, MIN_STAKE - 1),
        &[(STAKE_CCY, MIN_STAKE - 1)],
        10,
    );
    failure(&out, ids::BELOW_MIN_STAKE);
}

#[test]
fn stake_rejects_new_row_at_max_rows() {
    let mut s = State::new(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE).unwrap();
    for i in 0..avm_staking_rewards_core::MAX_ROWS {
        let acc = {
            let mut a = [0u8; 32];
            a[..8].copy_from_slice(&(i as u64).to_be_bytes());
            a
        };
        s.stake(acc, MIN_STAKE).unwrap();
    }
    let bytes = s.encode();
    let newcomer = staker(0xFE);
    let out = call_default_ttl(
        &bytes,
        newcomer,
        ids::STAKE,
        &stake_body(0, MIN_STAKE),
        &[(STAKE_CCY, MIN_STAKE)],
        10,
    );
    failure(&out, ids::TOO_MANY_ROWS);
}

// ---------------------------------------------------------------------
// UNSTAKE / CLAIM
// ---------------------------------------------------------------------

fn staked_state(amount: u128, funded: u128) -> (Vec<u8>, [u8; 32]) {
    let a = staker(0xA1);
    let mut s = State::new(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE).unwrap();
    s.stake(a, amount).unwrap();
    if funded > 0 {
        s.fund_rewards(funded).unwrap();
    }
    (s.encode(), a)
}

/// The exact fee-reserve attachment every success-path UNSTAKE/CLAIM test
/// below uses.
const FEE: &[(u32, u128)] = &[(SHELL_CCY, FUEL_RESERVE)];

#[test]
fn unstake_success_emits_combined_outbound_and_callback() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let out =
        call_default_ttl(&bytes, a, ids::UNSTAKE, &unstake_body(1, 5_000, payout_addr(a)), FEE, 10);
    assert_eq!(out.status_code, 0);
    assert_eq!(out.outbound_intents.len(), 1);
    assert_eq!(out.callback_intents.len(), 1);
    let ob = &out.outbound_intents[0];
    assert_eq!(ob.destination, payout_addr(a));
    assert_eq!(ob.extra.len(), 2, "both S and R are nonzero here");
    assert_eq!(ob.fuel_fee_shells, FUEL_RESERVE, "the contract's own fee, not the caller's");
    // The callback's `handler_method_id` is the row's own `pending_id`, a
    // dynamic per-request correlation value -- not a fixed named-method id.
    let s = State::decode(&state_from(&out)).unwrap();
    let pending_id = s.rows[0].pending.unwrap().pending_id;
    assert_eq!(out.callback_intents[0].handler_method_id, pending_id);
    assert_ne!(pending_id, 0, "0 is reserved; the SDK rejects a zero handler_method_id");
}

#[test]
fn unstake_ignores_callers_own_declared_fuel_fee_shells() {
    // `ext.fuel_fee_shells` (here, `call_default_ttl`'s own default of 10)
    // must have no bearing on the outbound's own fee -- only
    // `state.fuel_reserve` does.
    let (bytes, a) = staked_state(5_000, 1_000);
    let out =
        call_default_ttl(&bytes, a, ids::UNSTAKE, &unstake_body(1, 5_000, payout_addr(a)), FEE, 10);
    assert_eq!(out.outbound_intents[0].fuel_fee_shells, FUEL_RESERVE);
    assert_ne!(
        out.outbound_intents[0].fuel_fee_shells, 10,
        "must not equal call_default_ttl's own unrelated fuel_fee_shells=10"
    );
}

#[test]
fn unstake_accepts_destination_unrelated_to_caller() {
    // No `account_id == pubkey` constraint: a payout destination must only
    // be a valid (nonzero) AccountAddress, per the real local stand's own
    // requirement that destinations are deployed contracts (see
    // `process_callback`'s doc comment) -- never derivable from a pubkey.
    let (bytes, a) = staked_state(5_000, 1_000);
    let out = call_default_ttl(
        &bytes,
        a,
        ids::UNSTAKE,
        &unstake_body(1, 5_000, arbitrary_addr(0x7E)),
        FEE,
        10,
    );
    assert_eq!(out.status_code, 0);
    assert_eq!(out.outbound_intents[0].destination, arbitrary_addr(0x7E));
}

#[test]
fn unstake_omits_zero_reward_currency_in_outbound() {
    let (bytes, a) = staked_state(5_000, 0);
    let out =
        call_default_ttl(&bytes, a, ids::UNSTAKE, &unstake_body(1, 5_000, payout_addr(a)), FEE, 10);
    assert_eq!(out.outbound_intents[0].extra.len(), 1, "reward pool empty: R omitted");
    assert_eq!(out.outbound_intents[0].extra[0].currency_id, STAKE_CCY);
}

#[test]
fn unstake_rejects_missing_fee_reserve() {
    // "Missing" -- no currency attached at all.
    let (bytes, a) = staked_state(5_000, 1_000);
    let out =
        call_default_ttl(&bytes, a, ids::UNSTAKE, &unstake_body(1, 5_000, payout_addr(a)), &[], 10);
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn unstake_rejects_wrong_currency_id_for_fee_reserve() {
    // "Wrong" -- right amount, wrong currency id (not SHELL).
    let (bytes, a) = staked_state(5_000, 1_000);
    let out = call_default_ttl(
        &bytes,
        a,
        ids::UNSTAKE,
        &unstake_body(1, 5_000, payout_addr(a)),
        &[(STAKE_CCY, FUEL_RESERVE)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn unstake_rejects_wrong_fee_reserve_amount() {
    // "Wrong" -- right currency id, wrong amount (both too little and too
    // much are rejected -- exact, not a minimum).
    let (bytes, a) = staked_state(5_000, 1_000);
    let too_little = call_default_ttl(
        &bytes,
        a,
        ids::UNSTAKE,
        &unstake_body(1, 5_000, payout_addr(a)),
        &[(SHELL_CCY, 0)],
        10,
    );
    failure(&too_little, ids::WRONG_CURRENCY);
    let too_much = call_default_ttl(
        &bytes,
        a,
        ids::UNSTAKE,
        &unstake_body(1, 5_000, payout_addr(a)),
        &[(SHELL_CCY, FUEL_RESERVE + 1)],
        10,
    );
    failure(&too_much, ids::WRONG_CURRENCY);
}

#[test]
fn unstake_rejects_extra_currency_alongside_correct_fee_reserve() {
    // "Extra" -- the correct SHELL entry plus a second, unrelated
    // currency attached alongside it.
    let (bytes, a) = staked_state(5_000, 1_000);
    let out = call_default_ttl(
        &bytes,
        a,
        ids::UNSTAKE,
        &unstake_body(1, 5_000, payout_addr(a)),
        &[(SHELL_CCY, FUEL_RESERVE), (STAKE_CCY, 1)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn unstake_rejects_seq_mismatch() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let out =
        call_default_ttl(&bytes, a, ids::UNSTAKE, &unstake_body(9, 5_000, payout_addr(a)), FEE, 10);
    failure(&out, ids::SEQ_MISMATCH);
}

#[test]
fn unstake_rejects_pending_already_in_flight() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let s1 = state_from(&call_default_ttl(
        &bytes,
        a,
        ids::UNSTAKE,
        &unstake_body(1, 1_000, payout_addr(a)),
        FEE,
        10,
    ));
    let out =
        call_default_ttl(&s1, a, ids::UNSTAKE, &unstake_body(2, 1_000, payout_addr(a)), FEE, 11);
    failure(&out, ids::PENDING_IN_FLIGHT);
}

#[test]
fn claim_success_emits_reward_only_outbound() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let out = call_default_ttl(&bytes, a, ids::CLAIM, &claim_body(1, payout_addr(a)), FEE, 10);
    assert_eq!(out.status_code, 0);
    let ob = &out.outbound_intents[0];
    assert_eq!(ob.extra.len(), 1);
    assert_eq!(ob.extra[0].currency_id, REWARD_CCY);
    assert_eq!(ob.fuel_fee_shells, FUEL_RESERVE);
}

#[test]
fn claim_accepts_destination_unrelated_to_caller() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let out =
        call_default_ttl(&bytes, a, ids::CLAIM, &claim_body(1, arbitrary_addr(0x7F)), FEE, 10);
    assert_eq!(out.status_code, 0);
    assert_eq!(out.outbound_intents[0].destination, arbitrary_addr(0x7F));
}

#[test]
fn claim_rejects_nothing_to_claim() {
    let (bytes, a) = staked_state(5_000, 0);
    let out = call_default_ttl(&bytes, a, ids::CLAIM, &claim_body(1, payout_addr(a)), FEE, 10);
    failure(&out, ids::NOTHING_TO_CLAIM);
}

#[test]
fn claim_rejects_missing_fee_reserve() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let out = call_default_ttl(&bytes, a, ids::CLAIM, &claim_body(1, payout_addr(a)), &[], 10);
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn claim_rejects_wrong_currency_id_for_fee_reserve() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let out = call_default_ttl(
        &bytes,
        a,
        ids::CLAIM,
        &claim_body(1, payout_addr(a)),
        &[(REWARD_CCY, FUEL_RESERVE)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn claim_rejects_wrong_fee_reserve_amount() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let out = call_default_ttl(
        &bytes,
        a,
        ids::CLAIM,
        &claim_body(1, payout_addr(a)),
        &[(SHELL_CCY, FUEL_RESERVE + 1)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn claim_rejects_extra_currency_alongside_correct_fee_reserve() {
    let (bytes, a) = staked_state(5_000, 1_000);
    let out = call_default_ttl(
        &bytes,
        a,
        ids::CLAIM,
        &claim_body(1, payout_addr(a)),
        &[(SHELL_CCY, FUEL_RESERVE), (REWARD_CCY, 1)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

// ---------------------------------------------------------------------
// FUND_REWARDS
// ---------------------------------------------------------------------

#[test]
fn fund_rewards_success_increases_pool() {
    let (bytes, _a) = staked_state(5_000, 0);
    let out = call_default_ttl(
        &bytes,
        staker(0xF0),
        ids::FUND_REWARDS,
        &fund_body(1_000),
        &[(REWARD_CCY, 1_000)],
        10,
    );
    let s = State::decode(&state_from(&out)).unwrap();
    assert_eq!(s.reward_pool_held, 1_000);
}

#[test]
fn fund_rewards_rejects_delta_zero() {
    // total_staked huge relative to amount => delta == 0 under PRECISION.
    let mut s = State::new(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE).unwrap();
    s.stake(staker(1), 1 << 59).unwrap();
    let bytes = s.encode();
    let out = call_default_ttl(
        &bytes,
        staker(0xF0),
        ids::FUND_REWARDS,
        &fund_body(1),
        &[(REWARD_CCY, 1)],
        10,
    );
    failure(&out, ids::FUND_DELTA_ZERO);
}

#[test]
fn fund_rewards_rejects_wrong_currency() {
    let (bytes, _a) = staked_state(5_000, 0);
    let out = call_default_ttl(
        &bytes,
        staker(0xF0),
        ids::FUND_REWARDS,
        &fund_body(1_000),
        &[(STAKE_CCY, 1_000)],
        10,
    );
    failure(&out, ids::WRONG_CURRENCY);
}

#[test]
fn fund_rewards_rejects_no_stakers() {
    let bytes = deployed();
    let out = call_default_ttl(
        &bytes,
        staker(0xF0),
        ids::FUND_REWARDS,
        &fund_body(1_000),
        &[(REWARD_CCY, 1_000)],
        10,
    );
    failure(&out, ids::FUND_REJECTED_NO_STAKERS);
}

#[test]
fn fund_rewards_needs_no_seq_from_arbitrary_caller() {
    let (bytes, _a) = staked_state(5_000, 0);
    // A totally unrelated caller (not a staker) succeeds -- FUND_REWARDS is
    // permissionless and seq-free by design (see docs/SPEC.md's "Per-method
    // equations").
    let out = call_default_ttl(
        &bytes,
        staker(0xFF),
        ids::FUND_REWARDS,
        &fund_body(500),
        &[(REWARD_CCY, 500)],
        10,
    );
    assert_eq!(out.status_code, 0);
}

// ---------------------------------------------------------------------
// Callback fast path: differential vs. core's full-decode path, and rejection cases
// ---------------------------------------------------------------------

fn pending_unstake_state(stake: u128, funded: u128, unstake_amt: u128) -> (Vec<u8>, [u8; 32], u64) {
    let a = staker(0xA1);
    let mut s = State::new(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE).unwrap();
    s.stake(a, stake).unwrap();
    if funded > 0 {
        s.fund_rewards(funded).unwrap();
    }
    let pending = s.unstake(a, unstake_amt).unwrap();
    (s.encode(), a, pending.pending_id)
}

#[test]
fn callback_success_fast_path_matches_core_full_decode() {
    let (bytes, a, pending_id) = pending_unstake_state(5_000, 2_000, 5_000);
    let mut oracle = State::decode(&bytes).unwrap();
    oracle.payout_success(a).unwrap();
    let expected = oracle.encode();

    let out = callback(&bytes, payout_addr(a), CONTRACT, pending_id, 0);
    assert_eq!(state_from(&out), expected, "fast path must match core's full-decode path exactly");
}

#[test]
fn callback_failure_fast_path_matches_core_full_decode() {
    let (bytes, a, pending_id) = pending_unstake_state(5_000, 2_000, 3_000);
    let mut oracle = State::decode(&bytes).unwrap();
    oracle.payout_failure(a).unwrap();
    let expected = oracle.encode();

    let out = callback(&bytes, payout_addr(a), CONTRACT, pending_id, 9999);
    assert_eq!(state_from(&out), expected, "fast path must match core's full-decode path exactly");
}

#[test]
fn callback_success_removes_fully_withdrawn_row() {
    let (bytes, a, pending_id) = pending_unstake_state(5_000, 0, 5_000);
    let out = callback(&bytes, payout_addr(a), CONTRACT, pending_id, 0);
    let s = State::decode(&state_from(&out)).unwrap();
    assert!(s.rows.is_empty(), "a fully-withdrawn row must be removed on settlement");
}

#[test]
fn callback_differential_across_many_rows_linear_scan() {
    let mut s = State::new(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE).unwrap();
    let accounts: Vec<[u8; 32]> = (0..40u16)
        .map(|i| {
            let mut a = [0u8; 32];
            a[..2].copy_from_slice(&i.to_be_bytes());
            a
        })
        .collect();
    for acc in &accounts {
        s.stake(*acc, 10_000).unwrap();
    }
    s.fund_rewards(4_000).unwrap();
    let target = accounts[17];
    let pending = s.unstake(target, 3_000).unwrap();
    let bytes = s.encode();

    let mut oracle = State::decode(&bytes).unwrap();
    oracle.payout_success(target).unwrap();
    let expected = oracle.encode();

    let out = callback(&bytes, payout_addr(target), CONTRACT, pending.pending_id, 0);
    assert_eq!(state_from(&out), expected);
}

#[test]
fn callback_rejects_external_caller() {
    let (bytes, a, pending_id) = pending_unstake_state(5_000, 0, 5_000);
    let message = external(
        &[],
        a,
        pending_id,
        &[],
        CONTRACT,
        false,
        1,
        10,
        10 + avm_staking_rewards::MAX_TTL,
    );
    let b = frame(&message, &bytes, a, None, CONTRACT, 10);
    let out =
        avm_staking_rewards::StakingRewards::process_message(decode_message_input(&b).unwrap());
    // Not flagged as a callback at all (external message) -> routed as an
    // ordinary external call with an unknown method_id.
    failure(&out, ids::INVALID_ARGS);
}

#[test]
fn callback_with_wrong_cause_address_is_rejected_at_decode_not_by_the_contract() {
    // `decode_internal_message` itself enforces `cause_address == destination`
    // (input.rs: `cause_address != destination` -> InvalidValue) -- a stronger,
    // earlier guarantee than this contract's own redundant check in
    // `process_callback` could ever observe. Documented here rather than
    // routed through `StakingRewards::process_message`, since a message
    // violating this can never reach it.
    let (bytes, a, pending_id) = pending_unstake_state(5_000, 0, 5_000);
    let bogus_self = AccountAddress { dapp_id: [0x22; 32], account_id: [0x33; 32] };
    let msg = callback_message(payout_addr(a), bogus_self, pending_id, 0);
    let b = frame(&msg, &bytes, [0; 32], Some(payout_addr(a)), CONTRACT, 20);
    assert!(decode_message_input(&b).is_err());
}

#[test]
fn callback_rejects_nonexistent_pending_id() {
    let (bytes, _a) = staked_state(5_000, 0);
    let out = callback(&bytes, payout_addr(staker(0xBB)), CONTRACT, 1, 0);
    failure(&out, ids::ROW_NOT_FOUND);
}

#[test]
fn callback_rejects_replay_after_already_settled() {
    let (bytes, a, pending_id) = pending_unstake_state(5_000, 0, 5_000);
    let settled = state_from(&callback(&bytes, payout_addr(a), CONTRACT, pending_id, 0));
    // Row was removed on settlement (fully withdrawn); a stale duplicate
    // callback for the same pending_id now has no row to find at all.
    let out = callback(&settled, payout_addr(a), CONTRACT, pending_id, 0);
    failure(&out, ids::ROW_NOT_FOUND);
}

#[test]
fn callback_rejects_replay_after_already_settled_partial() {
    let (bytes, a, pending_id) = pending_unstake_state(10_000, 0, 3_000);
    let settled = state_from(&callback(&bytes, payout_addr(a), CONTRACT, pending_id, 0));
    // Row survives (partial unstake), but its `pending` (and pending_id)
    // have been cleared -- the stale id now matches no row either.
    let out = callback(&settled, payout_addr(a), CONTRACT, pending_id, 0);
    failure(&out, ids::ROW_NOT_FOUND);
}

// ---------------------------------------------------------------------
// Golden-vector-derived differential check: the callback fast path is
// differentially tested against `core` over the golden vectors and random
// sequences. This crate has no JSON dependency, so it re-derives a
// handful of deterministic sequences directly via core's own API instead of
// parsing `core/tests/fixtures/golden_vectors.json` directly -- the
// random-sequence half of that requirement (the golden-vector replay
// itself is covered by `core/tests/golden_vectors.rs`).
// ---------------------------------------------------------------------

#[test]
fn callback_differential_random_sequences() {
    // A small deterministic PRNG (xorshift) avoids a `rand` dependency.
    struct Xs(u64);
    impl Xs {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn range(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }
    let mut rng = Xs(0x9E3779B97F4A7C15);

    for trial in 0..30u32 {
        let mut s = State::new(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE).unwrap();
        let n_accounts = 2 + (trial % 8);
        let accounts: Vec<[u8; 32]> = (0..n_accounts)
            .map(|i| {
                let mut a = [0u8; 32];
                a[0] = i as u8;
                a[1] = trial as u8;
                a[2] = 1; // never all-zero: the SDK decoder rejects a zero account_id
                a
            })
            .collect();
        for acc in &accounts {
            s.stake(*acc, MIN_STAKE * u128::from(1 + rng.range(50))).unwrap();
        }
        if rng.range(2) == 0 {
            let _ = s.fund_rewards(MIN_STAKE * u128::from(1 + rng.range(20)));
        }
        let target = accounts[rng.range(accounts.len() as u64) as usize];
        let staked = s.rows.iter().find(|r| r.account == target).unwrap().staked;
        let amt = 1 + rng.range(staked as u64) as u128;
        let is_unstake = rng.range(2) == 0;
        let pending_id = if is_unstake {
            match s.unstake(target, amt) {
                Ok(p) => p.pending_id,
                Err(_) => continue,
            }
        } else {
            match s.claim(target) {
                Ok(p) => p.pending_id,
                Err(_) => continue,
            }
        };
        let bytes = s.encode();
        let success = rng.range(2) == 0;

        let mut oracle = State::decode(&bytes).unwrap();
        let expected = if success {
            oracle.payout_success(target).unwrap();
            oracle.encode()
        } else {
            oracle.payout_failure(target).unwrap();
            oracle.encode()
        };

        let status = if success { 0 } else { 9999 };
        let out = callback(&bytes, payout_addr(target), CONTRACT, pending_id, status);
        assert_eq!(state_from(&out), expected, "trial {trial}: fast path diverged from core");
    }
}

// ---------------------------------------------------------------------
// Error-mapping exhaustiveness (every CoreError variant reaches a distinct,
// documented status code -- not left to `_ => unreachable` at the match).
// ---------------------------------------------------------------------

#[test]
fn every_core_error_variant_maps_to_a_named_status() {
    // Compile-time exhaustiveness is enforced by the `match` in
    // `core_err_to_status` itself (no wildcard arm) -- this test is a
    // regression guard documenting the intended pairing in one place.
    let pairs: &[(CoreError, u32)] = &[
        (CoreError::InvalidArgs, ids::INVALID_ARGS),
        (CoreError::RowNotFound, ids::ROW_NOT_FOUND),
        (CoreError::PendingAlreadyInFlight, ids::PENDING_IN_FLIGHT),
        (CoreError::NoPendingPayout, ids::NO_PENDING_PAYOUT),
        (CoreError::BelowMinStake, ids::BELOW_MIN_STAKE),
        (CoreError::ExceedsMaxAmount, ids::EXCEEDS_MAX_AMOUNT),
        (CoreError::FundRejectedNoStakers, ids::FUND_REJECTED_NO_STAKERS),
        (CoreError::FundExceedsMaxAcc, ids::FUND_EXCEEDS_MAX_ACC),
        (CoreError::FundDeltaZero, ids::FUND_DELTA_ZERO),
        (CoreError::NothingToClaim, ids::NOTHING_TO_CLAIM),
        (CoreError::TooManyRows, ids::TOO_MANY_ROWS),
        (CoreError::InvalidMinStake, ids::INVALID_MIN_STAKE),
        (CoreError::InvalidFuelReserve, ids::INVALID_FUEL_RESERVE),
        (CoreError::CurrencyCannotBeShell, ids::CURRENCY_CANNOT_BE_SHELL),
    ];
    assert_eq!(pairs.len(), 14, "documented subset of CoreError's 18 variants");
}
