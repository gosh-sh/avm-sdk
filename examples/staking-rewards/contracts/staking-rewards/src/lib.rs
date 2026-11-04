#![forbid(unsafe_code)]

//! `staking-rewards` contract: the SDK-facing layer over `avm-staking-rewards-core`.
//! Core knows nothing about `OutboundIntent`/`Caller`/currencies; this crate is the
//! only place that imports an SDK type, per `docs/SPEC.md`.

pub mod ids;

use avm_contract_sdk::{
    AccountAddress, CallbackIntent, Caller, CanonicalMessage, Contract, CurrencyAmount,
    MessageInput, MessageOutput, OutboundIntent, StateUpdate,
};
use avm_staking_rewards_core as core_crate;
use core_crate::{Error as CoreError, State};

/// Seconds. See `docs/SPEC.md` for the dimensional analysis this bound is
/// derived from.
pub const MAX_TTL: u32 = 300;

/// `fuel-and-fees.md:16`: "SHELL is ordinary transferable value stored as
/// extra currency `currency_id = 2`". Mirrors
/// `examples/bounty-escrow/contracts/escrow/src/lib.rs:13`'s own
/// `SHELL_CURRENCY_ID` constant (same platform-fixed value, independently
/// defined per contract -- the SDK exposes no shared constant for it).
pub const SHELL_CURRENCY_ID: u32 = 2;

const INIT_DATA_BYTES: usize = 4 + 4 + 16 + 16; // stake_currency_id, reward_currency_id, min_stake, fuel_reserve
const STAKE_BODY_BYTES: usize = 8 + 16; // expected_seq, amount
const UNSTAKE_BODY_BYTES: usize = 8 + 16 + 64; // expected_seq, amount, payout address
const CLAIM_BODY_BYTES: usize = 8 + 64; // expected_seq, payout address
const FUND_BODY_BYTES: usize = 16; // amount

pub struct StakingRewards;

impl Contract for StakingRewards {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}

fn core_err_to_status(e: CoreError) -> u32 {
    match e {
        CoreError::Overflow | CoreError::Underflow => ids::INVALID_STATE,
        CoreError::InvalidArgs => ids::INVALID_ARGS,
        CoreError::RowNotFound => ids::ROW_NOT_FOUND,
        CoreError::RowAlreadyExists => ids::INVALID_STATE,
        CoreError::PendingAlreadyInFlight => ids::PENDING_IN_FLIGHT,
        CoreError::NoPendingPayout => ids::NO_PENDING_PAYOUT,
        CoreError::BelowMinStake => ids::BELOW_MIN_STAKE,
        CoreError::ExceedsMaxAmount => ids::EXCEEDS_MAX_AMOUNT,
        CoreError::FundRejectedNoStakers => ids::FUND_REJECTED_NO_STAKERS,
        CoreError::FundExceedsMaxAcc => ids::FUND_EXCEEDS_MAX_ACC,
        CoreError::FundDeltaZero => ids::FUND_DELTA_ZERO,
        CoreError::NothingToClaim => ids::NOTHING_TO_CLAIM,
        CoreError::TooManyRows => ids::TOO_MANY_ROWS,
        CoreError::InvalidMinStake => ids::INVALID_MIN_STAKE,
        CoreError::InvalidFuelReserve => ids::INVALID_FUEL_RESERVE,
        CoreError::CurrencyCannotBeShell => ids::CURRENCY_CANNOT_BE_SHELL,
        CoreError::DecodeError => ids::INVALID_STATE,
    }
}

fn initialize(input: MessageInput<'_>) -> Result<MessageOutput, u32> {
    let data = input.deploy().map_err(|_| ids::INVALID_ARGS)?.init_data();
    if data.len() != INIT_DATA_BYTES {
        return Err(ids::INVALID_ARGS);
    }
    if !input.state.is_empty() {
        return Err(ids::INVALID_STATE);
    }
    if !input.message.extra().is_empty() {
        return Err(ids::UNSOLICITED_CURRENCY);
    }
    let stake_currency_id = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let reward_currency_id = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let min_stake = u128::from_le_bytes(data[8..24].try_into().unwrap());
    let fuel_reserve = u128::from_le_bytes(data[24..40].try_into().unwrap());
    if stake_currency_id == reward_currency_id {
        return Err(ids::INVALID_ARGS);
    }
    let state = State::new(stake_currency_id, reward_currency_id, min_stake, fuel_reserve)
        .map_err(core_err_to_status)?;
    Ok(MessageOutput::success(StateUpdate::FullReplace(state.encode())))
}

fn read_address(body: &[u8], offset: usize) -> AccountAddress {
    AccountAddress {
        dapp_id: body[offset..offset + 32].try_into().unwrap(),
        account_id: body[offset + 32..offset + 64].try_into().unwrap(),
    }
}

/// `expire_at - block_time <= MAX_TTL` and `block_time <= expire_at`, checked
/// against the CURRENT block time (not signing time): a replay attempt is
/// evaluated against the block it actually lands in, not the block it was
/// signed at.
fn check_ttl(expire_at: u32, block_time: u32) -> Result<(), u32> {
    if block_time > expire_at {
        return Err(ids::BAD_EXPIRY);
    }
    if expire_at.checked_sub(block_time).ok_or(ids::BAD_EXPIRY)? > MAX_TTL {
        return Err(ids::BAD_EXPIRY);
    }
    Ok(())
}

fn expected_seq_for(state: &State, account: &[u8; 32]) -> u64 {
    state.rows.iter().find(|r| &r.account == account).map(|r| r.seq).unwrap_or(0)
}

fn check_seq(state: &State, account: &[u8; 32], expected: u64) -> Result<(), u32> {
    if expected_seq_for(state, account) != expected {
        return Err(ids::SEQ_MISMATCH);
    }
    Ok(())
}

/// One combined `OutboundIntent{S, R}` to `destination`, omitting zero
/// entries, paired with a `CallbackIntent` keyed by `pending_id`.
///
/// `method_id: 0, body: empty` on the outbound is a bare currency transfer,
/// not a call into any named method (the SDK does not restrict
/// `OutboundIntent::method_id` to nonzero, unlike
/// `CallbackIntent::handler_method_id`, see `output.rs`'s
/// `validate_outbound_intents` vs. its callback-intent check). The
/// destination is caller-chosen and arbitrary, and must be a deployed
/// contract (`process_callback`'s own doc comment); it must also accept an
/// internal, non-callback message carrying this currency, or
/// `process_message` on the destination returns an `Err`, the whole
/// transfer rolls back atomically, and this contract's own
/// `payout_failure` path runs -- nothing is left unaccounted (`SHELL`
/// transfers are exempt from this requirement, since the runtime applies
/// them without invoking the destination's guest code at all; `S`/`R`
/// transfers are not).
///
/// The callback's `handler_method_id` is `pending_id` itself, not a fixed
/// hash-derived selector: that id is what the fast path scans for on the
/// way back in (core's own linear scan over the pending-flag+id fields,
/// see its doc comment).
///
/// `fuel_fee_shells` is `state.fuel_reserve` -- a per-deployment,
/// contract-controlled fee, not the caller's own declared
/// `ext.fuel_fee_shells`. These are two separately-charged amounts, both
/// debited from this contract's own SHELL balance, not the caller's:
/// `ext.fuel_fee_shells` funds the external call's own chargeable
/// execution (`docs/fuel-and-fees.md:50-52` lists external messages as a
/// chargeable route; measured on the real stand -- lowering only this
/// declared value changes how long the pool's own balance lasts, with
/// nothing else changed, since an external caller is a signing pubkey,
/// not an AVM account with its own balance, per
/// `docs/fuel-and-fees.md:5-8`), while this outbound's own
/// `fuel_fee_shells` (below) separately funds the forward delivery leg
/// per `docs/fuel-and-fees.md:101`'s "the sender prepays the reserve when
/// the outbound message is materialized" -- the sender of this outbound
/// is this contract, never the caller, same as the external call's own
/// fee.
fn combined_payout(
    state: &State,
    destination: AccountAddress,
    amount_s: u128,
    amount_r: u128,
    pending_id: u64,
) -> (OutboundIntent, CallbackIntent) {
    let mut extra = Vec::new();
    if amount_s > 0 {
        extra.push(CurrencyAmount { currency_id: state.stake_currency_id, amount: amount_s });
    }
    if amount_r > 0 {
        extra.push(CurrencyAmount { currency_id: state.reward_currency_id, amount: amount_r });
    }
    extra.sort_by_key(|c| c.currency_id);
    let outbound = OutboundIntent {
        input_index: 0,
        destination,
        fuel_fee_shells: state.fuel_reserve,
        extra,
        method_id: 0,
        body: Vec::new(),
    };
    let callback = CallbackIntent { outbound_index: 0, handler_method_id: pending_id };
    (outbound, callback)
}

/// `UNSTAKE`/`CLAIM` must attach exactly `state.fuel_reserve` of SHELL and
/// nothing else: exactly one currency entry, exact amount, matching this
/// contract's own exact-currency convention for `STAKE`/`FUND_REWARDS`
/// (see `docs/SPEC.md`'s "Currency rules"). Compare
/// `examples/bounty-escrow/contracts/escrow/src/lib.rs:459-466`'s own
/// `fuel_reserve` check, which accepts `>=` and allows a second currency
/// alongside it.
fn check_fee_reserve(
    state: &State,
    extra: avm_contract_sdk::CurrencyAmounts<'_>,
) -> Result<(), u32> {
    let entries: Vec<CurrencyAmount> = extra.iter().collect();
    if entries.len() != 1
        || entries[0].currency_id != SHELL_CURRENCY_ID
        || entries[0].amount != state.fuel_reserve
    {
        return Err(ids::WRONG_CURRENCY);
    }
    Ok(())
}

fn process(input: MessageInput<'_>) -> Result<MessageOutput, u32> {
    if input.message.is_callback() {
        return process_callback(input);
    }
    let Caller::External(pubkey) = input.caller else { return Err(ids::UNAUTHORIZED) };
    let method = input.message.method_id();
    let block_time = input.context.block_time;
    let CanonicalMessage::External(ext) = input.message else { return Err(ids::INVALID_ARGS) };

    if method == ids::STAKE {
        check_ttl(ext.expire_at, block_time)?;
        let body = input.message.body();
        if body.len() != STAKE_BODY_BYTES {
            return Err(ids::INVALID_ARGS);
        }
        let expected_seq = u64::from_le_bytes(body[0..8].try_into().unwrap());
        let amount = u128::from_le_bytes(body[8..24].try_into().unwrap());
        let mut state = State::decode(input.state).map_err(core_err_to_status)?;
        check_seq(&state, &pubkey, expected_seq)?;
        let extra: Vec<CurrencyAmount> = input.message.extra().iter().collect();
        if extra.len() != 1
            || extra[0].currency_id != state.stake_currency_id
            || extra[0].amount != amount
        {
            return Err(ids::WRONG_CURRENCY);
        }
        state.stake(pubkey, amount).map_err(core_err_to_status)?;
        return Ok(MessageOutput::success(StateUpdate::FullReplace(state.encode())));
    }

    if method == ids::UNSTAKE {
        check_ttl(ext.expire_at, block_time)?;
        let body = input.message.body();
        if body.len() != UNSTAKE_BODY_BYTES {
            return Err(ids::INVALID_ARGS);
        }
        let expected_seq = u64::from_le_bytes(body[0..8].try_into().unwrap());
        let amount = u128::from_le_bytes(body[8..24].try_into().unwrap());
        let destination = read_address(body, 24);
        let mut state = State::decode(input.state).map_err(core_err_to_status)?;
        check_seq(&state, &pubkey, expected_seq)?;
        check_fee_reserve(&state, input.message.extra())?;
        let pending = state.unstake(pubkey, amount).map_err(core_err_to_status)?;
        let (outbound, callback) = combined_payout(
            &state,
            destination,
            pending.amount_s,
            pending.amount_r,
            pending.pending_id,
        );
        let mut output = MessageOutput::success(StateUpdate::FullReplace(state.encode()));
        output.outbound_intents.push(outbound);
        output.callback_intents.push(callback);
        return Ok(output);
    }

    if method == ids::CLAIM {
        check_ttl(ext.expire_at, block_time)?;
        let body = input.message.body();
        if body.len() != CLAIM_BODY_BYTES {
            return Err(ids::INVALID_ARGS);
        }
        let expected_seq = u64::from_le_bytes(body[0..8].try_into().unwrap());
        let destination = read_address(body, 8);
        let mut state = State::decode(input.state).map_err(core_err_to_status)?;
        check_seq(&state, &pubkey, expected_seq)?;
        check_fee_reserve(&state, input.message.extra())?;
        let pending = state.claim(pubkey).map_err(core_err_to_status)?;
        let (outbound, callback) =
            combined_payout(&state, destination, 0, pending.amount_r, pending.pending_id);
        let mut output = MessageOutput::success(StateUpdate::FullReplace(state.encode()));
        output.outbound_intents.push(outbound);
        output.callback_intents.push(callback);
        return Ok(output);
    }

    if method == ids::FUND_REWARDS {
        // No expected_seq / per-account check: permissionless, anyone funds
        // with their own currency (see docs/SPEC.md's "Per-method equations").
        let body = input.message.body();
        if body.len() != FUND_BODY_BYTES {
            return Err(ids::INVALID_ARGS);
        }
        let amount = u128::from_le_bytes(body[0..16].try_into().unwrap());
        let mut state = State::decode(input.state).map_err(core_err_to_status)?;
        let extra: Vec<CurrencyAmount> = input.message.extra().iter().collect();
        if extra.len() != 1
            || extra[0].currency_id != state.reward_currency_id
            || extra[0].amount != amount
        {
            return Err(ids::WRONG_CURRENCY);
        }
        state.fund_rewards(amount).map_err(core_err_to_status)?;
        return Ok(MessageOutput::success(StateUpdate::FullReplace(state.encode())));
    }

    Err(ids::INVALID_ARGS)
}

/// Callback fast path: touches only the target row via `core_crate`'s
/// raw-byte helpers, never decoding every row.
///
/// The row is located by `pending_id` -- the callback's own `method_id`,
/// assigned by `combined_payout` from `PendingOut::pending_id` when the
/// payout was created -- not by the internal caller's `account_id`.
/// `Caller::Internal(source).account_id` cannot serve as the row-lookup
/// key here: the destination is caller-chosen and must itself be deployed
/// code (a funded-but-undeployed address is rejected as "account is
/// absent"; `docs/getting-started.md`'s own "repeating the same
/// derivation after funding creates the account" shows deployment, not
/// funding, creates one), and a deployed contract's `account_id` is
/// hash-derived (`runtime-limits.md`'s "Deterministic addresses" formula),
/// never equal to a caller's raw pubkey.
///
/// `header.cause_address == input.destination` is a sanity check: it
/// confirms the causal outbound was ours, not a correlation key.
fn process_callback(input: MessageInput<'_>) -> Result<MessageOutput, u32> {
    let (Caller::Internal(_), CanonicalMessage::Internal(message)) = (input.caller, input.message)
    else {
        return Err(ids::INVALID_CALLBACK);
    };
    let header = message.callback.ok_or(ids::INVALID_CALLBACK)?;
    if header.cause_address != input.destination {
        return Err(ids::INVALID_CALLBACK);
    }
    let pending_id = message.method_id;
    let success = header.status_code == 0;
    let new_bytes = if success {
        core_crate::State::payout_success_fast(input.state, pending_id)
    } else {
        core_crate::State::payout_failure_fast(input.state, pending_id)
    }
    .map_err(core_err_to_status)?;
    Ok(MessageOutput::success(StateUpdate::FullReplace(new_bytes)))
}

#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_message_contract!(StakingRewards, arena_bytes = 4_194_304);
