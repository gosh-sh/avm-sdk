//! Method and callback handlers (SPEC.md §4/§5).

use avm_contract_sdk::AccountAddress;
use avm_contract_sdk::CallbackIntent;
use avm_contract_sdk::Caller;
use avm_contract_sdk::CanonicalMessage;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::OutboundIntent;
use avm_contract_sdk::StateUpdate;

use crate::constants::*;
use crate::state::LedgerKey;
use crate::state::PendingWithdrawal;
use crate::state::VaultState;

fn fail(code: u32) -> MessageOutput {
    MessageOutput::failure(code)
}

fn success_with_state(state: &VaultState) -> MessageOutput {
    MessageOutput::success(StateUpdate::FullReplace(state.serialize()))
}

/// The vault's own real SHELL holding — field group "(a)" (SPEC.md §1):
/// `input.extra`, the account's own current balance, never the inbound
/// message's own carried `extra`. An absent entry means exactly zero.
fn vault_shell_balance(input: &MessageInput<'_>) -> u128 {
    input
        .extra
        .iter()
        .find(|currency| currency.currency_id == SHELL_CURRENCY_ID)
        .map_or(0, |currency| currency.amount)
}

fn internal_source(input: &MessageInput<'_>) -> Option<AccountAddress> {
    match input.caller {
        Caller::Internal(source) => Some(source),
        Caller::External(_) => None,
    }
}

fn external_pubkey(input: &MessageInput<'_>) -> Option<[u8; 32]> {
    match input.caller {
        Caller::External(pubkey) => Some(pubkey),
        Caller::Internal(_) => None,
    }
}

// ---------------------------------------------------------------------
// DEPOSIT
// ---------------------------------------------------------------------

pub fn handle_deposit(input: &MessageInput<'_>, mut state: VaultState) -> MessageOutput {
    let Some(source) = internal_source(input) else {
        return fail(ERR_INVALID_CALLER);
    };
    if source.dapp_id != input.destination.dapp_id {
        return fail(ERR_CROSS_DAPP_CALLER_FORBIDDEN);
    }
    if state.deposits_paused {
        return fail(ERR_DEPOSITS_PAUSED);
    }
    // Atomic, all-or-nothing: reject the entire message if any extra entry
    // carries SHELL (error 1519), before crediting anything (SPEC.md §1/§4,
    // Invariant I10; no partial credit ever occurs).
    if input.message.extra().iter().any(|currency| currency.currency_id == SHELL_CURRENCY_ID) {
        return fail(ERR_SHELL_NOT_CUSTODIED);
    }

    // Two-phase: first compute every (key, amount) credit this message
    // requires, and how many would be genuinely new rows, so the whole
    // call can be rejected atomically (no partial credit) if the new-row
    // cap would be exceeded (Invariant I8; SPEC.md §2.4/§8 test 3).
    let mut credits: Vec<(LedgerKey, u128)> = Vec::new();
    let value_main = input.message.value_main();
    if value_main > 0 {
        credits.push((
            LedgerKey {
                dapp_id: source.dapp_id,
                account_id: source.account_id,
                currency_tag: CURRENCY_TAG_MAIN,
                currency_id: 0,
            },
            value_main,
        ));
    }
    for currency in input.message.extra().iter() {
        if currency.amount == 0 {
            continue;
        }
        credits.push((
            LedgerKey {
                dapp_id: source.dapp_id,
                account_id: source.account_id,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: currency.currency_id,
            },
            currency.amount,
        ));
    }
    if credits.is_empty() {
        // Nothing eligible carried — succeeds as a no-op (SPEC.md §4).
        return success_with_state(&state);
    }

    let mut new_row_count = 0usize;
    for (key, _) in &credits {
        if state.ledger.binary_search_by(|entry| entry.key.cmp(key)).is_err() {
            new_row_count += 1;
        }
    }
    // Only a genuinely new row can ever push the row count over the cap —
    // an all-existing-row deposit must succeed even if `ledger.len()`
    // already exceeds `MAX_LEDGER_ENTRIES` (reachable only via a restore,
    // which bypasses this cap entirely, SPEC.md §2.4/§7 I11), since it adds
    // no row at all (Gate 2a round 2 item 1).
    if new_row_count > 0 && state.ledger.len().saturating_add(new_row_count) > MAX_LEDGER_ENTRIES {
        return fail(ERR_LEDGER_CAPACITY_EXCEEDED);
    }

    // Single pass, mutating `state` directly as it goes (Gate 2a round 2
    // item 1 correction). Atomicity of a failure path does NOT come from
    // doing every credit's arithmetic before applying any of them — it
    // comes from Invariant I6 (SPEC.md §7): a failure's `MessageOutput`
    // carries `state_update: None` unconditionally (`fail` below never
    // takes `state` as an argument), and the SDK's own output validation
    // independently rejects any encoded output that pairs a non-zero
    // `status_code` with a state/effect change before it ever reaches the
    // runtime. So no matter how far this loop gets — including after
    // already mutating `state` for earlier, non-overflowing credits —
    // returning `fail(...)` on a later credit's overflow can never leak
    // any of those mutations onto the chain; the local `state` value this
    // function was holding is simply dropped along with everything else
    // in its stack frame once a `MessageOutput::failure` is returned.
    //
    // The one thing this loop still needs a genuine precheck for, rather
    // than relying purely on `credit_ledger`'s own internal `checked_add`,
    // is the pending-aware term (Gate 2a round 2 item 2; SPEC.md §7
    // Invariant I2): `credit_ledger` only ever checks
    // `existing_ledger_balance + amount`, with no concept of a same-key
    // pending withdrawal that could still be *restored* back onto it
    // later. Checking `existing + pending_amount_for_key + amount` here
    // first subsumes that narrower check — if the 3-term sum fits in
    // `u128`, the 2-term sum `credit_ledger` computes internally is
    // guaranteed to fit too, since `pending_amount_for_key` is never
    // negative — so the `credit_ledger` call below can never itself fail;
    // it would be entirely consistent with Invariant I6 to not check its
    // result at all, but it is still checked explicitly (never a bare
    // unwrap) purely as defense in depth.
    for (key, amount) in &credits {
        let existing = state.ledger_balance(key);
        let Ok(pending_sum) = state.pending_amount_for_key(key) else {
            return fail(ERR_LEDGER_AMOUNT_OVERFLOW);
        };
        let overflows = existing
            .checked_add(pending_sum)
            .and_then(|total| total.checked_add(*amount))
            .is_none();
        if overflows {
            return fail(ERR_LEDGER_AMOUNT_OVERFLOW);
        }
        if state.credit_ledger(*key, *amount).is_err() {
            return fail(ERR_LEDGER_AMOUNT_OVERFLOW);
        }
    }
    success_with_state(&state)
}

// ---------------------------------------------------------------------
// WITHDRAW
// ---------------------------------------------------------------------

const WITHDRAW_BODY_LEN: usize = 21;

pub fn handle_withdraw(input: &MessageInput<'_>, mut state: VaultState) -> MessageOutput {
    // Caller-type is checked first, strictly before body-shape validation
    // (Gate 2a round 2 item 4; SPEC.md §5, REVIEW.md entry 3) — "is this
    // message even allowed to invoke this method at all" does not depend
    // on anything decoded from the body, so there is no reason to parse an
    // untrusted body before confirming the caller is even the right kind.
    // This also keeps a wrong-caller-type request's rejection code (1500)
    // independent of both the body it happened to carry and the vault's
    // own SHELL level, superseding an earlier draft that checked body
    // shape first and, before that, one that checked Invariant I17 first.
    let Some(source) = internal_source(input) else {
        return fail(ERR_INVALID_CALLER);
    };

    let body = input.message.body();
    if body.len() != WITHDRAW_BODY_LEN {
        return fail(ERR_INVALID_BODY);
    }
    if input.message.value_main() != 0 {
        return fail(ERR_INVALID_BODY);
    }
    if !input.message.extra().is_empty() {
        return fail(ERR_INVALID_BODY);
    }
    let Some(currency_tag) = body.first().copied() else {
        return fail(ERR_INVALID_BODY);
    };
    let Some(currency_id) =
        body.get(1..5).and_then(|bytes| bytes.try_into().ok()).map(u32::from_le_bytes)
    else {
        return fail(ERR_INVALID_BODY);
    };
    let Some(amount) =
        body.get(5..21).and_then(|bytes| bytes.try_into().ok()).map(u128::from_le_bytes)
    else {
        return fail(ERR_INVALID_BODY);
    };
    if currency_tag != CURRENCY_TAG_MAIN && currency_tag != CURRENCY_TAG_EXTRA {
        return fail(ERR_INVALID_CURRENCY_SELECTOR);
    }
    if currency_tag == CURRENCY_TAG_MAIN && currency_id != 0 {
        return fail(ERR_INVALID_CURRENCY_SELECTOR);
    }

    // Invariant I17: the vault's own real SHELL holding must clear the
    // margin before anything else that could mutate state — checked
    // before the cross-dapp check, the amount check, the balance check,
    // and the pending-capacity check.
    if vault_shell_balance(input) < MIN_OPERATIONAL_SHELL {
        return fail(ERR_VAULT_UNDERFUNDED);
    }

    if source.dapp_id != input.destination.dapp_id {
        return fail(ERR_CROSS_DAPP_CALLER_FORBIDDEN);
    }
    if amount == 0 {
        return fail(ERR_INVALID_AMOUNT);
    }

    let key = LedgerKey {
        dapp_id: source.dapp_id,
        account_id: source.account_id,
        currency_tag,
        currency_id,
    };
    if state.ledger_balance(&key) < amount {
        return fail(ERR_INSUFFICIENT_BALANCE);
    }

    if state.pending.len() >= MAX_PENDING_WITHDRAWALS {
        return fail(ERR_PENDING_CAPACITY_EXCEEDED);
    }
    if state.pending_count_for_depositor(&source.dapp_id, &source.account_id)
        >= MAX_PENDING_PER_DEPOSITOR
    {
        return fail(ERR_PENDING_CAPACITY_EXCEEDED);
    }

    let Some(advanced_next_id) = state.next_withdrawal_id.checked_add(1) else {
        return fail(ERR_WITHDRAWAL_ID_EXHAUSTED);
    };
    let withdrawal_id = state.next_withdrawal_id;

    // checks-effects-interactions: debit and insert the pending record
    // before emitting the payout (SPEC.md §5).
    if state.debit_ledger(&key, amount).is_err() {
        // Unreachable given the ledger_balance check above; fails safely
        // rather than panicking if it were ever somehow wrong.
        return fail(ERR_INSUFFICIENT_BALANCE);
    }
    state.next_withdrawal_id = advanced_next_id;
    state.insert_pending(PendingWithdrawal {
        withdrawal_id,
        depositor_dapp_id: source.dapp_id,
        depositor_account_id: source.account_id,
        currency_tag,
        currency_id,
        amount,
    });

    let mut output = success_with_state(&state);
    output.outbound_intents = vec![OutboundIntent {
        input_index: 0,
        destination: source,
        value_main: if currency_tag == CURRENCY_TAG_MAIN { amount } else { 0 },
        fuel_fee_shells: 0,
        extra: if currency_tag == CURRENCY_TAG_EXTRA {
            vec![avm_contract_sdk::CurrencyAmount { currency_id, amount }]
        } else {
            Vec::new()
        },
        method_id: 0,
        body: Vec::new(),
    }];
    output.callback_intents = vec![CallbackIntent {
        outbound_index: 0,
        handler_method_id: withdrawal_callback_method_id(withdrawal_id),
    }];
    output
}

// ---------------------------------------------------------------------
// TOPUP
// ---------------------------------------------------------------------

pub fn handle_topup(input: &MessageInput<'_>, state: &VaultState) -> MessageOutput {
    if external_pubkey(input).is_none() {
        return fail(ERR_INVALID_CALLER);
    }
    // TOPUP has no declared ABI input type at all (`avm-contract.toml`
    // carries no `input_type_id` for `vault.topup`) — a non-empty body is
    // therefore always malformed for this method (Gate 2a round 2 item 5;
    // SPEC.md §4).
    if !input.message.body().is_empty() {
        return fail(ERR_INVALID_BODY);
    }
    if input.message.value_main() != 0 {
        return fail(ERR_TOPUP_REQUIRES_SHELL_ONLY);
    }
    let extra = input.message.extra();
    if extra.is_empty() {
        return fail(ERR_TOPUP_REQUIRES_SHELL_ONLY);
    }
    if extra.iter().any(|currency| currency.currency_id != SHELL_CURRENCY_ID) {
        return fail(ERR_TOPUP_REQUIRES_SHELL_ONLY);
    }
    // Invariant I15: never touches the ledger in any way, regardless of
    // what it currently holds; the carried SHELL becomes part of the
    // vault's own real account balance at the protocol level, not
    // anything this contract's own state tracks.
    success_with_state(state)
}

// ---------------------------------------------------------------------
// Admin: PROPOSE_OWNER / ACCEPT_OWNER / SET_PAUSE
// ---------------------------------------------------------------------

/// Caller-type (1500) and body-shape (1501) are distinct, separately
/// reported outcomes — checked in that order, caller-type first, so an
/// `Internal` caller sending any admin method id is always rejected as
/// `invalid_caller`, never `invalid_body`, regardless of what body it
/// happened to carry.
fn require_admin_caller(input: &MessageInput<'_>) -> Result<[u8; 32], u32> {
    external_pubkey(input).ok_or(ERR_INVALID_CALLER)
}

fn require_admin_body_shape(input: &MessageInput<'_>, expected_len: usize) -> Result<(), u32> {
    if input.message.value_main() != 0 || !input.message.extra().is_empty() {
        return Err(ERR_INVALID_BODY);
    }
    if input.message.body().len() != expected_len {
        return Err(ERR_INVALID_BODY);
    }
    Ok(())
}

fn read_expected_admin_seq(body: &[u8]) -> Option<u32> {
    body.get(0..4).and_then(|bytes| bytes.try_into().ok()).map(u32::from_le_bytes)
}

pub fn handle_propose_owner(input: &MessageInput<'_>, mut state: VaultState) -> MessageOutput {
    const BODY_LEN: usize = 36; // expected_admin_seq(4) + new_owner_pubkey(32)
    let pubkey = match require_admin_caller(input) {
        Ok(pubkey) => pubkey,
        Err(code) => return fail(code),
    };
    if let Err(code) = require_admin_body_shape(input, BODY_LEN) {
        return fail(code);
    }
    if pubkey != state.owner_pubkey {
        return fail(ERR_UNAUTHORIZED_OWNER);
    }
    let body = input.message.body();
    let Some(expected_admin_seq) = read_expected_admin_seq(body) else {
        return fail(ERR_INVALID_BODY);
    };
    let Some(new_owner_pubkey): Option<[u8; 32]> =
        body.get(4..36).and_then(|bytes| bytes.try_into().ok())
    else {
        return fail(ERR_INVALID_BODY);
    };
    if expected_admin_seq != state.admin_seq {
        return fail(ERR_STALE_ADMIN_SEQUENCE);
    }
    let Some(advanced) = state.admin_seq.checked_add(1) else {
        return fail(ERR_ADMIN_SEQUENCE_EXHAUSTED);
    };
    state.pending_owner_pubkey = new_owner_pubkey;
    state.admin_seq = advanced;
    let event = match admin_seq_event(advanced) {
        Ok(event) => event,
        Err(code) => return fail(code),
    };
    let mut output = success_with_state(&state);
    output.event_intents = vec![event];
    output
}

pub fn handle_accept_owner(input: &MessageInput<'_>, mut state: VaultState) -> MessageOutput {
    const BODY_LEN: usize = 4; // expected_admin_seq(4)
    let pubkey = match require_admin_caller(input) {
        Ok(pubkey) => pubkey,
        Err(code) => return fail(code),
    };
    if let Err(code) = require_admin_body_shape(input, BODY_LEN) {
        return fail(code);
    }
    if state.pending_owner_pubkey == [0; 32] || pubkey != state.pending_owner_pubkey {
        return fail(ERR_UNAUTHORIZED_PENDING_OWNER);
    }
    let Some(expected_admin_seq) = read_expected_admin_seq(input.message.body()) else {
        return fail(ERR_INVALID_BODY);
    };
    if expected_admin_seq != state.admin_seq {
        return fail(ERR_STALE_ADMIN_SEQUENCE);
    }
    let Some(advanced) = state.admin_seq.checked_add(1) else {
        return fail(ERR_ADMIN_SEQUENCE_EXHAUSTED);
    };
    state.owner_pubkey = state.pending_owner_pubkey;
    state.pending_owner_pubkey = [0; 32];
    state.admin_seq = advanced;
    let event = match admin_seq_event(advanced) {
        Ok(event) => event,
        Err(code) => return fail(code),
    };
    let mut output = success_with_state(&state);
    output.event_intents = vec![event];
    output
}

pub fn handle_set_pause(input: &MessageInput<'_>, mut state: VaultState) -> MessageOutput {
    const BODY_LEN: usize = 5; // expected_admin_seq(4) + flags(1)
    let pubkey = match require_admin_caller(input) {
        Ok(pubkey) => pubkey,
        Err(code) => return fail(code),
    };
    if let Err(code) = require_admin_body_shape(input, BODY_LEN) {
        return fail(code);
    }
    if pubkey != state.owner_pubkey {
        return fail(ERR_UNAUTHORIZED_OWNER);
    }
    let body = input.message.body();
    let Some(expected_admin_seq) = read_expected_admin_seq(body) else {
        return fail(ERR_INVALID_BODY);
    };
    let Some(supplied_flags) = body.get(4).copied() else {
        return fail(ERR_INVALID_BODY);
    };
    // Invariant I13 (SPEC.md): expected_admin_seq is checked before this
    // call has any other effect whatsoever — including, as here, any
    // further validation of the body's own content. Checked before the
    // reserved-pause-flag-bits check below (Phase 4 adversarial review,
    // finding 2 — this was the one admin method out of three with the
    // two checks in the wrong order; `handle_propose_owner` and
    // `handle_accept_owner` both already checked admin_seq first).
    if expected_admin_seq != state.admin_seq {
        return fail(ERR_STALE_ADMIN_SEQUENCE);
    }
    if supplied_flags & !HEADER_FLAG_DEPOSITS_PAUSED != 0 {
        return fail(ERR_INVALID_PAUSE_FLAGS);
    }
    let Some(advanced) = state.admin_seq.checked_add(1) else {
        return fail(ERR_ADMIN_SEQUENCE_EXHAUSTED);
    };
    state.deposits_paused = supplied_flags & HEADER_FLAG_DEPOSITS_PAUSED != 0;
    state.admin_seq = advanced;
    let event = match admin_seq_event(advanced) {
        Ok(event) => event,
        Err(code) => return fail(code),
    };
    let mut output = success_with_state(&state);
    output.event_intents = vec![event];
    output
}

fn admin_seq_event(admin_seq: u32) -> Result<avm_contract_sdk::EventIntent, u32> {
    // `encode_event_fields` can only fail on a field count overflowing
    // `u16` (impossible with exactly one field), a name/value length
    // overflowing `u16` (impossible for this fixed, short ASCII name and
    // an 8-byte `U64` value), or out-of-order field names (impossible with
    // exactly one field) — reading the SDK's own source confirms this is
    // provably unreachable for this fixed, single-field event. Handled
    // explicitly anyway (Gate 2a round 2 item 5): on this unreachable
    // error, fail safely with a dedicated error code (1530) rather than
    // silently emitting a default/empty `fields_binary` as part of an
    // apparently-successful admin action via `.unwrap_or_default()` — the
    // same "fail safely rather than mask" pattern this file already uses
    // for every other provably-unreachable-but-still-checked case.
    let fields_binary = avm_contract_sdk::encode_event_fields(&[avm_contract_sdk::EventField {
        name: String::from("admin_seq"),
        value: avm_contract_sdk::EventValue::U64(u64::from(admin_seq)),
    }])
    .map_err(|_| ERR_EVENT_ENCODING_FAILED)?;
    Ok(avm_contract_sdk::EventIntent {
        input_index: 0,
        kind: avm_contract_sdk::EventKind::External,
        event_class: ADMIN_EVENT_CLASS,
        event_name: String::from("vault.admin_seq"),
        topics: Vec::new(),
        fields_binary,
    })
}

const ADMIN_EVENT_CLASS: u64 = 0x4d43_5654_4144_4d4e; // "MCVTADMN"-ish, arbitrary fixed tag

// ---------------------------------------------------------------------
// Callback (settle or restore a pending withdrawal)
// ---------------------------------------------------------------------

// Gate 3e TASK F2: no longer reached by production dispatch (`lib.rs`
// routes every callback to `handle_callback_fast`, below, directly).
// Kept `#[cfg(test)]`-only as the differential tests' own ground
// truth, not left behind as dead production code.
#[cfg(test)]
pub fn handle_callback(input: &MessageInput<'_>, mut state: VaultState) -> MessageOutput {
    let CanonicalMessage::Internal(message) = input.message else {
        return fail(ERR_INVALID_CALLBACK);
    };
    let Some(callback) = message.callback else {
        return fail(ERR_INVALID_CALLBACK);
    };
    let Caller::Internal(caller_source) = input.caller else {
        return fail(ERR_INVALID_CALLBACK);
    };

    // Gate 2a round 2 item 5: reject outright if this callback's own
    // method_id does not carry this contract's own CALLBACK_MAGIC_TAG in
    // its upper 32 bits, rather than blindly treating its lower 32 bits as
    // a withdrawal id. Dispatch already routes here strictly on the
    // wire-level callback flag, never on method_id matching alone
    // (Invariant I4c) — this is a second, independent check on the
    // method_id's own shape, not a replacement for that dispatch gate.
    let method_id = input.message.method_id();
    if !method_id_is_withdrawal_callback(method_id) {
        return fail(ERR_INVALID_CALLBACK);
    }
    let withdrawal_id = withdrawal_id_from_callback_method_id(method_id);
    let Some(index) = state.pending_index(withdrawal_id) else {
        return fail(ERR_WITHDRAWAL_NOT_FOUND);
    };
    let Some(pending_record) = state.pending.get(index) else {
        // Unreachable: `pending_index` only ever returns an index it just
        // found via binary search on this same vec. Fails safely rather
        // than panicking if it were ever somehow wrong.
        return fail(ERR_WITHDRAWAL_NOT_FOUND);
    };

    // Invariant I4b: only the pending record's own depositor may settle or
    // restore it.
    if caller_source.dapp_id != pending_record.depositor_dapp_id
        || caller_source.account_id != pending_record.depositor_account_id
    {
        return fail(ERR_INVALID_CALLBACK);
    }

    let record = state.remove_pending(index);
    if callback.status_code != 0 {
        // Restore bypasses MAX_LEDGER_ENTRIES entirely (SPEC.md §1/§2.4) —
        // unconditional, no check on the callback's own carried value
        // (Invariant I11; SPEC.md §1's A8 reasoning).
        let key = LedgerKey {
            dapp_id: record.depositor_dapp_id,
            account_id: record.depositor_account_id,
            currency_tag: record.currency_tag,
            currency_id: record.currency_id,
        };
        if state.credit_ledger(key, record.amount).is_err() {
            // Practically unreachable (would require a u128 overflow on
            // restore, which Invariant I2's deposit-time precheck is
            // specifically designed to make impossible — SPEC.md §7);
            // fails safely rather than panicking. Uses the same code a
            // DEPOSIT-time ledger overflow uses (1529,
            // `ledger_amount_overflow`), not 1513 (`invalid_callback`,
            // documented exclusively as a caller/depositor mismatch,
            // SPEC.md §6) — Phase 4 adversarial review, finding 3: this
            // used to return 1513, contradicting both 1513's own
            // documented, exclusive meaning and SPEC.md §5's claim that a
            // caller mismatch is "the only way this branch ever fails
            // once a matching record is found."
            return fail(ERR_LEDGER_AMOUNT_OVERFLOW);
        }
    }
    // status_code == 0: Settled — funds already genuinely delivered;
    // nothing further to credit.
    success_with_state(&state)
}

/// Gate 3e TASK F2: the real dispatch path for every callback (`lib.rs`
/// routes here directly, before any full `VaultState::parse` — see
/// `state.rs`'s `fast_callback` for what it validates and what it
/// deliberately does not, and `SPEC.md` §1 for why that is acceptable).
/// `handle_callback`, above, is kept unchanged and is no longer reached
/// from production dispatch — it is retained as the differential
/// tests' own ground truth (`lib.rs`'s `tests::fast_path_differential`
/// module calls it directly), not as dead code left behind by
/// accident.
pub(crate) fn handle_callback_fast(input: &MessageInput<'_>) -> MessageOutput {
    let CanonicalMessage::Internal(message) = input.message else {
        return fail(ERR_INVALID_CALLBACK);
    };
    let Some(callback) = message.callback else {
        return fail(ERR_INVALID_CALLBACK);
    };
    let Caller::Internal(caller_source) = input.caller else {
        return fail(ERR_INVALID_CALLBACK);
    };
    // Same check, same code, same position as the slow path (Gate 2a
    // round 2 item 5): reject outright if this callback's own
    // method_id does not carry CALLBACK_MAGIC_TAG, before treating its
    // lower 32 bits as a withdrawal id.
    let method_id = input.message.method_id();
    if !method_id_is_withdrawal_callback(method_id) {
        return fail(ERR_INVALID_CALLBACK);
    }
    let withdrawal_id = withdrawal_id_from_callback_method_id(method_id);

    match VaultState::fast_callback(
        input.state,
        withdrawal_id,
        callback.status_code,
        caller_source.dapp_id,
        caller_source.account_id,
    ) {
        Ok(new_state) => MessageOutput::success(StateUpdate::FullReplace(new_state)),
        Err(code) => fail(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Currency;
    use crate::test_support::{self};

    const DAPP: [u8; 32] = [1; 32];
    const DEPOSITOR: [u8; 32] = [2; 32];

    /// Seeds `count` distinct, single-unit filler ledger rows (each a
    /// different `dapp_id`, so each is a genuinely separate key, never
    /// colliding with `DAPP`/`DEPOSITOR`-keyed rows this test module uses
    /// elsewhere) starting at index `starting_at`.
    fn seed_filler_rows(state: &mut VaultState, count: usize, starting_at: u32) {
        for i in 0..count {
            let mut filler_dapp = [0u8; 32];
            filler_dapp[28..32].copy_from_slice(&(starting_at + i as u32).to_be_bytes());
            state
                .credit_ledger(
                    LedgerKey {
                        dapp_id: filler_dapp,
                        account_id: [0; 32],
                        currency_tag: CURRENCY_TAG_EXTRA,
                        currency_id: 1,
                    },
                    1,
                )
                .expect("seed filler row");
        }
    }

    #[test]
    fn deposit_overflow_is_atomic_across_multiple_currencies() {
        // Gate 2a round 2 item 1 correction: atomicity here does NOT come
        // from checking every credit before applying any of them — it
        // comes from Invariant I6 (a failure's `MessageOutput` carries no
        // `state_update` at all, enforced independently by the SDK's own
        // output validation). This test seeds the near-`u128::MAX` row at
        // the LATER-processed id (9) specifically so that `handle_deposit`
        // genuinely mutates its local `state` for the first credit (id 7)
        // *before* discovering the second credit (id 9) overflows — i.e.
        // this exercises the case where some in-progress mutation really
        // has happened by the time `fail` is returned, and confirms that
        // mutation still never surfaces in the returned output.
        let mut state = VaultState::new_empty([9; 32], 0);
        state
            .credit_ledger(
                LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 9,
                },
                u128::MAX - 5,
            )
            .expect("seed credit");

        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            DEPOSIT_METHOD_ID,
            0,
            &[Currency { currency_id: 7, amount: 50 }, Currency { currency_id: 9, amount: 10 }],
            &[],
            0,
            &[],
            &state.serialize(),
        );

        let output = handle_deposit(&input, state.clone());
        assert_eq!(output.status_code, ERR_LEDGER_AMOUNT_OVERFLOW);
        // This is the entire check: I6 guarantees that `state_update: None`
        // is, by itself, proof that nothing this call did is ever
        // observable anywhere — re-parsing the *original* `state` variable
        // (never passed to `handle_deposit` by mutable reference; only a
        // `.clone()` was) would be vacuous, since that variable was never
        // going to reflect anything the call did regardless of whether
        // the call was correct or buggy.
        assert!(output.state_update.is_none(), "a failure must carry no state update (I6)");
    }

    #[test]
    fn deposit_overflow_precheck_accounts_for_pending_amount_too() {
        // Gate 2a round 2 item 2: the existing ledger balance alone has
        // headroom for this deposit, but existing + the same key's
        // outstanding pending amount (which could still be restored back
        // onto this key later, on a callback with status_code != 0) does
        // not. Without accounting for pending here, this DEPOSIT would
        // wrongly succeed now and only overflow later, at restore time
        // (where it would merely fail safely rather than panic, but the
        // funds would already be unrestorable — exactly the gap item 2
        // closes).
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        state.credit_ledger(key, 100).expect("seed small existing balance");
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 1,
            depositor_dapp_id: DAPP,
            depositor_account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
            amount: u128::MAX - 150,
        });
        state.next_withdrawal_id = 2;

        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            DEPOSIT_METHOD_ID,
            0,
            &[Currency { currency_id: 7, amount: 60 }],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_deposit(&input, state.clone());
        assert_eq!(output.status_code, ERR_LEDGER_AMOUNT_OVERFLOW);
        // I6 alone is the complete check (Gate 2a round 3 item 1) —
        // re-parsing `state` itself would be vacuous, since it was only
        // ever passed to `handle_deposit` via `.clone()`.
        assert!(output.state_update.is_none(), "a failure must carry no state update (I6)");
    }

    #[test]
    fn deposit_existing_row_succeeds_even_when_ledger_already_over_cap() {
        // Gate 2a round 2 item 1: ledger at 221 total rows (220 fillers +
        // the depositor's own existing row) — reachable in practice only
        // via a restore bypassing MAX_LEDGER_ENTRIES (SPEC.md §2.4/§7
        // I11), constructed directly here. Crediting the SAME existing
        // key must still succeed: it adds no new row, so the
        // `new_row_count > 0` guard must not reject it merely because
        // `ledger.len()` already exceeds the cap.
        let mut state = VaultState::new_empty([9; 32], 0);
        state
            .credit_ledger(
                LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 50,
                },
                1000,
            )
            .expect("seed depositor row");
        seed_filler_rows(&mut state, 220, 0);
        assert_eq!(state.ledger.len(), 221);

        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            DEPOSIT_METHOD_ID,
            0,
            &[Currency { currency_id: 50, amount: 1 }],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_deposit(&input, state.clone());
        assert_eq!(output.status_code, 0);
        // Gate 2a round 3 item 2: assert the resulting committed state, not
        // merely the status code — still 301 rows (no new row added), and
        // the existing row's balance genuinely increased.
        let resulting = test_support::expect_full_replace_state(&output);
        assert_eq!(resulting.ledger.len(), 221, "crediting an existing row adds no row");
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 50,
        };
        assert_eq!(resulting.ledger_balance(&key), 1001);
    }

    #[test]
    fn deposit_new_row_fails_when_ledger_already_over_cap() {
        let mut state = VaultState::new_empty([9; 32], 0);
        state
            .credit_ledger(
                LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 50,
                },
                1000,
            )
            .expect("seed depositor row");
        seed_filler_rows(&mut state, 220, 0);
        assert_eq!(state.ledger.len(), 221);

        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            DEPOSIT_METHOD_ID,
            0,
            &[Currency { currency_id: 60, amount: 1 }],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_deposit(&input, state.clone());
        assert_eq!(output.status_code, ERR_LEDGER_CAPACITY_EXCEEDED);
        assert!(output.state_update.is_none(), "a failure must carry no state update (I6)");
    }

    #[test]
    fn deposit_new_row_fails_exactly_at_cap() {
        let mut state = VaultState::new_empty([9; 32], 0);
        seed_filler_rows(&mut state, 220, 0);
        assert_eq!(state.ledger.len(), 220);

        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            DEPOSIT_METHOD_ID,
            0,
            &[Currency { currency_id: 60, amount: 1 }],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_deposit(&input, state.clone());
        assert_eq!(output.status_code, ERR_LEDGER_CAPACITY_EXCEEDED);
        assert!(output.state_update.is_none(), "a failure must carry no state update (I6)");
    }

    #[test]
    fn deposit_one_new_row_ok_two_new_rows_fail_atomically_near_cap() {
        // At 219 rows: a single-new-row deposit succeeds (-> 220, not
        // over); a message requiring TWO new rows in the SAME call fails
        // atomically (-> 221, over) with neither new row applied.
        let mut ok_state = VaultState::new_empty([9; 32], 0);
        seed_filler_rows(&mut ok_state, 219, 0);
        assert_eq!(ok_state.ledger.len(), 219);
        let mut bytes_ok = Vec::new();
        let input_ok = test_support::internal_input(
            &mut bytes_ok,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            DEPOSIT_METHOD_ID,
            0,
            &[Currency { currency_id: 60, amount: 1 }],
            &[],
            0,
            &[],
            &ok_state.serialize(),
        );
        let output_ok = handle_deposit(&input_ok, ok_state.clone());
        assert_eq!(output_ok.status_code, 0);
        // Gate 2a round 3 item 2: assert the resulting committed state —
        // exactly 220 rows now (the one new row applied), with the new
        // row holding exactly the deposited amount.
        let resulting_ok = test_support::expect_full_replace_state(&output_ok);
        assert_eq!(resulting_ok.ledger.len(), 220);
        let new_key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 60,
        };
        assert_eq!(resulting_ok.ledger_balance(&new_key), 1);

        let mut fail_state = VaultState::new_empty([9; 32], 0);
        seed_filler_rows(&mut fail_state, 219, 0);
        let mut bytes_fail = Vec::new();
        // value_main > 0 (a new Main row) + one extra entry (a new Extra
        // row) = two new rows in one message.
        let input_fail = test_support::internal_input(
            &mut bytes_fail,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            DEPOSIT_METHOD_ID,
            10,
            &[Currency { currency_id: 60, amount: 1 }],
            &[],
            0,
            &[],
            &fail_state.serialize(),
        );
        let output_fail = handle_deposit(&input_fail, fail_state.clone());
        assert_eq!(output_fail.status_code, ERR_LEDGER_CAPACITY_EXCEEDED);
        // I6 alone is the complete check here (Gate 2a round 3 item 1) —
        // re-parsing `fail_state` itself would be vacuous, since it was
        // only ever passed to `handle_deposit` via `.clone()`.
        assert!(output_fail.state_update.is_none(), "a failure must carry no state update (I6)");
    }

    /// Gate 2a round 3 item 4: TOPUP has no declared ABI input type at all
    /// (`avm-contract.toml` carries no `input_type_id` for `vault.topup`),
    /// so a non-empty body is always malformed.
    #[test]
    fn topup_non_empty_body_is_rejected() {
        let state = VaultState::new_empty([9; 32], 0);
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            [3; 32],
            TOPUP_METHOD_ID,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: 100 }],
            &[0xAA],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_topup(&input, &state);
        assert_eq!(output.status_code, ERR_INVALID_BODY);
        assert!(output.state_update.is_none());
    }

    /// Gate 2a round 3 item 4: a genuine callback-flagged message whose
    /// `method_id` does not carry `CALLBACK_MAGIC_TAG` in its upper 32
    /// bits must be rejected outright (1513) — chosen deliberately so its
    /// LOWER 32 bits collide with a real outstanding withdrawal_id (1),
    /// which is exactly the scenario the magic-tag check exists to catch:
    /// without it, this would be indistinguishable from a genuine
    /// withdrawal-1 callback and would incorrectly settle/restore it.
    #[test]
    fn callback_with_wrong_magic_tag_is_rejected() {
        let mut state = VaultState::new_empty([9; 32], 0);
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 1,
            depositor_dapp_id: DAPP,
            depositor_account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
            amount: 50,
        });
        state.next_withdrawal_id = 2;

        let message_bytes = test_support::callback_message_bytes(
            DEPOSIT_METHOD_ID, /* upper 32 bits are NORMAL_METHOD_TAG, not CALLBACK_MAGIC_TAG; lower 32 bits == 1 */
            (DAPP, DEPOSITOR),
            (DAPP, DAPP),
            [0xCD; 32],
            0,
        );
        let acmi_bytes = test_support::build_message_input(
            (DAPP, DAPP),
            true,
            (DAPP, DEPOSITOR),
            [0; 32],
            0,
            &[],
            &state.serialize(),
            &message_bytes,
        );
        let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
            .expect("real ACMI buffer must decode");
        let output = handle_callback(&input, state.clone());
        assert_eq!(output.status_code, ERR_INVALID_CALLBACK);
        assert!(output.state_update.is_none());
    }

    /// Gate 2a round 3 item 4: Invariant I4b — a callback whose `method_id`
    /// correctly names an outstanding withdrawal, but whose caller is not
    /// that withdrawal's own depositor, must be rejected (1513).
    #[test]
    fn callback_from_non_depositor_caller_is_rejected() {
        let mut state = VaultState::new_empty([9; 32], 0);
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 1,
            depositor_dapp_id: DAPP,
            depositor_account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
            amount: 50,
        });
        state.next_withdrawal_id = 2;

        const IMPOSTOR: [u8; 32] = [99; 32];
        let message_bytes = test_support::callback_message_bytes(
            withdrawal_callback_method_id(1),
            (DAPP, IMPOSTOR),
            (DAPP, DAPP),
            [0xCD; 32],
            0,
        );
        let acmi_bytes = test_support::build_message_input(
            (DAPP, DAPP),
            true,
            (DAPP, IMPOSTOR),
            [0; 32],
            0,
            &[],
            &state.serialize(),
            &message_bytes,
        );
        let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
            .expect("real ACMI buffer must decode");
        let output = handle_callback(&input, state.clone());
        assert_eq!(output.status_code, ERR_INVALID_CALLBACK);
        assert!(output.state_update.is_none());
    }

    const OWNER: [u8; 32] = [7; 32];

    /// "THEN Phase 3" item 2: Invariant I4d — the payout `OutboundIntent`
    /// and its `CallbackIntent` carry exactly what SPEC.md §5 says they
    /// must: destination == the requester's own address, the correct
    /// amount in whichever of value_main/extra matches the requested
    /// currency, `fuel_fee_shells == 0`, `method_id == 0`, an empty body,
    /// and a `CallbackIntent` naming this exact withdrawal_id.
    #[test]
    fn withdraw_outbound_and_callback_intent_contents_match_i4d() {
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        state.credit_ledger(key, 1000).expect("seed");

        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&400u128.to_le_bytes());

        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output = handle_withdraw(&input, state.clone());
        assert_eq!(output.status_code, 0);

        assert_eq!(output.outbound_intents.len(), 1);
        let outbound = &output.outbound_intents[0];
        assert_eq!(outbound.input_index, 0);
        assert_eq!(outbound.destination.dapp_id, DAPP, "I4d: destination == requester");
        assert_eq!(outbound.destination.account_id, DEPOSITOR, "I4d: destination == requester");
        assert_eq!(outbound.value_main, 0, "requested currency was Extra, not Main");
        assert_eq!(
            outbound.extra,
            vec![avm_contract_sdk::CurrencyAmount { currency_id: 7, amount: 400 }]
        );
        assert_eq!(outbound.fuel_fee_shells, 0);
        assert_eq!(outbound.method_id, 0, "a plain value transfer, never a method call");
        assert!(outbound.body.is_empty());

        assert_eq!(output.callback_intents.len(), 1);
        let callback = &output.callback_intents[0];
        assert_eq!(callback.outbound_index, 0);
        assert_eq!(callback.handler_method_id, withdrawal_callback_method_id(0));
    }

    /// "THEN Phase 3" item 2: the Settled path (`status_code == 0` on the
    /// callback) removes the pending record and credits nothing back —
    /// the funds were already genuinely delivered.
    #[test]
    fn callback_settled_removes_pending_without_crediting_back() {
        let mut state = VaultState::new_empty([9; 32], 0);
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 5,
            depositor_dapp_id: DAPP,
            depositor_account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
            amount: 300,
        });
        state.next_withdrawal_id = 6;
        let pending_len_before = state.pending.len();

        let message_bytes = test_support::callback_message_bytes(
            withdrawal_callback_method_id(5),
            (DAPP, DEPOSITOR),
            (DAPP, DAPP),
            [0xCD; 32],
            0,
        );
        let acmi_bytes = test_support::build_message_input(
            (DAPP, DAPP),
            true,
            (DAPP, DEPOSITOR),
            [0; 32],
            0,
            &[],
            &state.serialize(),
            &message_bytes,
        );
        let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
            .expect("real ACMI buffer must decode");
        let output = handle_callback(&input, state.clone());
        assert_eq!(output.status_code, 0);
        let resulting = test_support::expect_full_replace_state(&output);
        assert_eq!(resulting.pending.len(), pending_len_before - 1);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        assert_eq!(resulting.ledger_balance(&key), 0, "settled must never credit back");
    }

    /// "THEN Phase 3" item 2: admin success paths, plus admin_seq replay
    /// (1524) — using the stale, already-consumed `expected_admin_seq`
    /// again against the post-success state must be rejected.
    #[test]
    fn propose_owner_success_then_replay_is_rejected() {
        let state = VaultState::new_empty(OWNER, 0);
        let new_owner = [8u8; 32];
        let mut body = Vec::with_capacity(36);
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&new_owner);

        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            OWNER,
            PROPOSE_OWNER_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_propose_owner(&input, state.clone());
        assert_eq!(output.status_code, 0);
        let resulting = test_support::expect_full_replace_state(&output);
        assert_eq!(resulting.pending_owner_pubkey, new_owner);
        assert_eq!(resulting.admin_seq, 1);
        assert_eq!(output.event_intents.len(), 1);

        // Replay: the exact same body (expected_admin_seq == 0) is now
        // stale against the post-success state (admin_seq == 1).
        let mut bytes2 = Vec::new();
        let input2 = test_support::external_input(
            &mut bytes2,
            (DAPP, DAPP),
            OWNER,
            PROPOSE_OWNER_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[],
            &resulting.serialize(),
        );
        let output2 = handle_propose_owner(&input2, resulting.clone());
        assert_eq!(output2.status_code, ERR_STALE_ADMIN_SEQUENCE);
        assert!(output2.state_update.is_none());
    }

    #[test]
    fn accept_owner_success() {
        let mut state = VaultState::new_empty(OWNER, 0);
        let new_owner = [8u8; 32];
        state.pending_owner_pubkey = new_owner;
        let mut body = Vec::with_capacity(4);
        body.extend_from_slice(&0u32.to_le_bytes());
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            new_owner,
            ACCEPT_OWNER_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_accept_owner(&input, state.clone());
        assert_eq!(output.status_code, 0);
        let resulting = test_support::expect_full_replace_state(&output);
        assert_eq!(resulting.owner_pubkey, new_owner);
        assert_eq!(resulting.pending_owner_pubkey, [0; 32]);
        assert_eq!(resulting.admin_seq, 1);
    }

    #[test]
    fn set_pause_success() {
        let state = VaultState::new_empty(OWNER, 0);
        let mut body = Vec::with_capacity(5);
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(HEADER_FLAG_DEPOSITS_PAUSED);
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            OWNER,
            SET_PAUSE_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_set_pause(&input, state.clone());
        assert_eq!(output.status_code, 0);
        let resulting = test_support::expect_full_replace_state(&output);
        assert!(resulting.deposits_paused);
        assert_eq!(resulting.admin_seq, 1);
    }

    /// Phase 4 adversarial review, finding 2: Invariant I13 (SPEC.md)
    /// requires `expected_admin_seq` to be checked "before that call has
    /// any other effect whatsoever," and SPEC.md §4 lists the
    /// stale-sequence check (1524) ahead of the reserved-pause-flag-bits
    /// check (1516) for `SET_PAUSE`. `handle_set_pause` had the two in the
    /// opposite order: a call that is both stale AND carries a reserved
    /// flag bit was reported as 1516, not 1524 — the one admin method out
    /// of three with this ordering bug (`handle_propose_owner` and
    /// `handle_accept_owner` both check `admin_seq` immediately after
    /// decoding the body, with no other body-content check in between).
    #[test]
    fn set_pause_stale_sequence_is_reported_before_reserved_flag_bits() {
        let mut state = VaultState::new_empty(OWNER, 0);
        state.admin_seq = 5;
        let mut body = Vec::with_capacity(5);
        body.extend_from_slice(&0u32.to_le_bytes()); // stale: real admin_seq is 5
        body.push(0x02); // and a reserved bit is set (bit 0 is the only valid one)
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            OWNER,
            SET_PAUSE_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_set_pause(&input, state.clone());
        assert_eq!(
            output.status_code, ERR_STALE_ADMIN_SEQUENCE,
            "a stale admin_seq must be reported before the reserved-flag-bits check, \
             matching Invariant I13 and SPEC.md §4's stated order for every admin method"
        );
        assert!(output.state_update.is_none());
    }

    /// Mutation-testing gap: every other admin-method test sends a
    /// correctly-shaped body with `value_main == 0` and empty `extra`
    /// (success paths), or a wrong-LENGTH body (targeting the length
    /// check). None sends a correct-length body paired with a non-zero
    /// `value_main` — so `require_admin_body_shape`'s own
    /// `value_main() != 0 || !extra().is_empty()` guard had no test that
    /// would fail if it were weakened (e.g. `||` to `&&`) or removed.
    #[test]
    fn admin_body_shape_rejects_nonzero_value_main() {
        let state = VaultState::new_empty(OWNER, 0);
        let mut body = Vec::with_capacity(5);
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(HEADER_FLAG_DEPOSITS_PAUSED);
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            OWNER,
            SET_PAUSE_METHOD_ID,
            1,
            &[],
            &body,
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_set_pause(&input, state.clone());
        assert_eq!(output.status_code, ERR_INVALID_BODY);
        assert!(output.state_update.is_none());
    }

    /// Companion to the above: a correct-length body with an empty
    /// `value_main` but a non-empty `extra` must also be rejected — the
    /// other half of the `||` that a mutant could weaken to `&&`.
    #[test]
    fn admin_body_shape_rejects_nonempty_extra() {
        let state = VaultState::new_empty(OWNER, 0);
        let mut body = Vec::with_capacity(5);
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(HEADER_FLAG_DEPOSITS_PAUSED);
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            OWNER,
            SET_PAUSE_METHOD_ID,
            0,
            &[Currency { currency_id: 7, amount: 1 }],
            &body,
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_set_pause(&input, state.clone());
        assert_eq!(output.status_code, ERR_INVALID_BODY);
        assert!(output.state_update.is_none());
    }

    /// Mutation-testing gap: `handle_accept_owner`'s
    /// `state.pending_owner_pubkey == [0; 32] || pubkey != state.pending_owner_pubkey`
    /// guard — every existing accept-owner test uses the matching caller
    /// against a genuinely-pending owner (the success path). No test sent
    /// a WRONG caller while a real `pending_owner_pubkey` was set, which is
    /// exactly the case where `||` weakened to `&&` would wrongly accept
    /// (first operand false, second true: `&&` evaluates to false and lets
    /// the mismatched caller through).
    #[test]
    fn accept_owner_rejects_mismatched_caller_when_pending_owner_set() {
        let mut state = VaultState::new_empty(OWNER, 0);
        let real_pending = [8u8; 32];
        let wrong_caller = [9u8; 32];
        state.pending_owner_pubkey = real_pending;
        let mut body = Vec::with_capacity(4);
        body.extend_from_slice(&0u32.to_le_bytes());
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            wrong_caller,
            ACCEPT_OWNER_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_accept_owner(&input, state.clone());
        assert_eq!(output.status_code, ERR_UNAUTHORIZED_PENDING_OWNER);
        assert!(output.state_update.is_none());
    }

    /// Mutation-testing gap: `pending_count_for_depositor`'s comparisons
    /// (`replace == with !=`, `replace usize return with 0/1`) had no test
    /// that drives the cap via REAL sequential `handle_withdraw` calls —
    /// only pre-built fixture states existed. This fills
    /// `MAX_PENDING_PER_DEPOSITOR` for one depositor one real withdrawal at
    /// a time and confirms the next one is rejected (1509).
    #[test]
    fn pending_withdrawals_cap_enforced_via_real_sequential_calls() {
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        state.credit_ledger(key, 100_000).expect("seed");
        for _ in 0..MAX_PENDING_PER_DEPOSITOR {
            let mut body = Vec::with_capacity(21);
            body.push(CURRENCY_TAG_EXTRA);
            body.extend_from_slice(&7u32.to_le_bytes());
            body.extend_from_slice(&1u128.to_le_bytes());
            let mut bytes = Vec::new();
            let input = test_support::internal_input(
                &mut bytes,
                (DAPP, DEPOSITOR),
                (DAPP, DEPOSITOR),
                WITHDRAW_METHOD_ID,
                0,
                &[],
                &body,
                0,
                &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
                &state.serialize(),
            );
            let output = handle_withdraw(&input, state.clone());
            assert_eq!(output.status_code, 0);
            state = test_support::expect_full_replace_state(&output);
        }
        assert_eq!(state.pending_count_for_depositor(&DAPP, &DEPOSITOR), MAX_PENDING_PER_DEPOSITOR);

        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&1u128.to_le_bytes());
        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output = handle_withdraw(&input, state.clone());
        assert_eq!(output.status_code, ERR_PENDING_CAPACITY_EXCEEDED);
        assert!(output.state_update.is_none());
    }

    /// SPEC.md §8 test 12 ("a depositor whose own payout handler
    /// re-withdraws on every single payout, attempting to recurse without
    /// bound"): the chain must always terminate at one of exactly two
    /// points — `ERR_PENDING_CAPACITY_EXCEEDED` (the previous test, above)
    /// or `ERR_INSUFFICIENT_BALANCE`, whichever bound is reached first.
    /// This test drives the *other* branch: a balance small enough to run
    /// out strictly before `MAX_PENDING_PER_DEPOSITOR` real sequential
    /// `handle_withdraw` calls are reached, confirming the cap is not the
    /// only thing that can stop the recursion — the ledger balance check
    /// (`WITHDRAW`'s existing, already-tested `insufficient_balance` path)
    /// independently bounds it too, in whichever order the two limits are
    /// actually reached. As SPEC.md itself notes for this test, this closes
    /// it analytically (real sequential handler calls demonstrating both
    /// termination points) rather than via a dedicated real-stand
    /// recursive-depositor-fixture probe, which SPEC.md explicitly treats
    /// as sufficient here ("follows analytically from Invariant I8 combined
    /// with probe P4's confirmed ordering, rather than requiring a fifth
    /// dedicated probe contract").
    #[test]
    fn pending_withdrawal_recursion_terminates_via_insufficient_balance_before_cap() {
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        // Exactly 2 units available: two withdrawals of 1 each exhaust the
        // balance to zero while leaving pending count at 2, strictly below
        // MAX_PENDING_PER_DEPOSITOR (4) — the balance check must be reached
        // before the cap check on the third attempt.
        state.credit_ledger(key, 2).expect("seed");
        for _ in 0..2 {
            let mut body = Vec::with_capacity(21);
            body.push(CURRENCY_TAG_EXTRA);
            body.extend_from_slice(&7u32.to_le_bytes());
            body.extend_from_slice(&1u128.to_le_bytes());
            let mut bytes = Vec::new();
            let input = test_support::internal_input(
                &mut bytes,
                (DAPP, DEPOSITOR),
                (DAPP, DEPOSITOR),
                WITHDRAW_METHOD_ID,
                0,
                &[],
                &body,
                0,
                &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
                &state.serialize(),
            );
            let output = handle_withdraw(&input, state.clone());
            assert_eq!(output.status_code, 0);
            state = test_support::expect_full_replace_state(&output);
        }
        assert_eq!(state.pending_count_for_depositor(&DAPP, &DEPOSITOR), 2);

        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&1u128.to_le_bytes());
        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output = handle_withdraw(&input, state.clone());
        assert_eq!(output.status_code, ERR_INSUFFICIENT_BALANCE);
        assert!(output.state_update.is_none());
        // The cap was never the binding constraint here: pending count
        // stayed at 2 (asserted above), well below MAX_PENDING_PER_DEPOSITOR
        // (4) — confirming the balance check, not the cap, terminated this
        // chain.
    }

    /// "THEN Phase 3" item 2: Invariant I17 boundary — exactly
    /// `MIN_OPERATIONAL_SHELL` passes; one below fails (1528).
    #[test]
    fn i17_boundary_exactly_min_operational_shell_passes_one_below_fails() {
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        state.credit_ledger(key, 1000).expect("seed");
        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&100u128.to_le_bytes());

        let mut bytes_ok = Vec::new();
        let input_ok = test_support::internal_input(
            &mut bytes_ok,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output_ok = handle_withdraw(&input_ok, state.clone());
        assert_eq!(output_ok.status_code, 0, "exactly MIN_OPERATIONAL_SHELL must pass (>=, not >)");

        let mut bytes_fail = Vec::new();
        let input_fail = test_support::internal_input(
            &mut bytes_fail,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL - 1 }],
            &state.serialize(),
        );
        let output_fail = handle_withdraw(&input_fail, state.clone());
        assert_eq!(output_fail.status_code, ERR_VAULT_UNDERFUNDED);
        assert!(output_fail.state_update.is_none());
    }

    /// "THEN Phase 3" item 2: withdrawing `currency_id == 2` (SHELL) via
    /// the `Extra` selector always hits `insufficient_balance` (1505),
    /// never anything else — no ledger row for SHELL can ever exist
    /// (`state.rs`'s own parse-time/deposit-time rejection), so its
    /// tracked balance is always exactly 0 (SPEC.md §6's own note on this).
    #[test]
    fn withdraw_shell_currency_id_always_insufficient_balance() {
        let state = VaultState::new_empty([9; 32], 0);
        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&SHELL_CURRENCY_ID.to_le_bytes());
        body.extend_from_slice(&1u128.to_le_bytes());
        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output = handle_withdraw(&input, state.clone());
        assert_eq!(output.status_code, ERR_INSUFFICIENT_BALANCE);
        assert!(output.state_update.is_none());
    }

    /// Mutation-testing gap: every other withdraw test in this file uses
    /// `CURRENCY_TAG_EXTRA`; none ever exercises the `CURRENCY_TAG_MAIN`
    /// selector, so the "is this tag Main or Extra, and does Main require
    /// `currency_id == 0`" checks (`handle_withdraw`'s first two guards)
    /// had no test that could fail if either were weakened or removed.
    #[test]
    fn withdraw_via_currency_tag_main_succeeds() {
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_MAIN,
            currency_id: 0,
        };
        state.credit_ledger(key, 500).expect("seed");
        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_MAIN);
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&200u128.to_le_bytes());
        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output = handle_withdraw(&input, state.clone());
        assert_eq!(output.status_code, 0);
        assert_eq!(output.outbound_intents.len(), 1);
        let outbound = &output.outbound_intents[0];
        assert_eq!(outbound.value_main, 200, "Main-tagged withdrawal pays out via value_main");
        assert!(outbound.extra.is_empty());
        let resulting = test_support::expect_full_replace_state(&output);
        assert_eq!(resulting.ledger_balance(&key), 300);
    }

    /// "THEN Phase 3" item 2: a reentrancy chain — a second WITHDRAW
    /// attempt for the same depositor/currency, fired against the state
    /// the FIRST call's own success already produced (modeling what a
    /// malicious receiving contract firing a nested message off the back
    /// of its own payout would actually see). Checks-effects-interactions
    /// means the debit is committed into the returned state before any
    /// outbound intent is even constructed, so this "reentrant" second
    /// attempt can only ever see the already-debited balance — there is
    /// no window in which it could observe the pre-debit state.
    #[test]
    fn reentrant_double_withdraw_second_attempt_sees_debited_balance() {
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        state.credit_ledger(key, 500).expect("seed");
        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&500u128.to_le_bytes());

        let mut bytes1 = Vec::new();
        let input1 = test_support::internal_input(
            &mut bytes1,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output1 = handle_withdraw(&input1, state.clone());
        assert_eq!(output1.status_code, 0);
        let resulting1 = test_support::expect_full_replace_state(&output1);
        assert_eq!(resulting1.ledger_balance(&key), 0);

        let mut bytes2 = Vec::new();
        let input2 = test_support::internal_input(
            &mut bytes2,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &resulting1.serialize(),
        );
        let output2 = handle_withdraw(&input2, resulting1.clone());
        assert_eq!(output2.status_code, ERR_INSUFFICIENT_BALANCE);
        assert!(output2.state_update.is_none());
    }

    /// SPEC.md §8 test 3: restore bypasses `MAX_LEDGER_ENTRIES`, tested at
    /// the exact boundary — fill to exactly 220 (including the victim),
    /// have the victim withdraw their entire balance (debiting to 0,
    /// removing their row via I9 -> 219), have a brand-new depositor's
    /// deposit bring it back to exactly 220 (a genuinely new row, not a
    /// restore), then deliver a failure callback for the victim's
    /// withdrawal and confirm the restore succeeds anyway, recreating
    /// their row as a 221st.
    #[test]
    fn restore_bypasses_ledger_cap_at_exact_boundary() {
        const VICTIM: [u8; 32] = [50; 32];
        const NEW_DEPOSITOR: [u8; 32] = [60; 32];
        let victim_key = LedgerKey {
            dapp_id: DAPP,
            account_id: VICTIM,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        let mut state = VaultState::new_empty([9; 32], 0);
        state.credit_ledger(victim_key, 500).expect("seed victim");
        seed_filler_rows(&mut state, 219, 0);
        assert_eq!(state.ledger.len(), 220);

        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&500u128.to_le_bytes());
        let mut bytes1 = Vec::new();
        let input1 = test_support::internal_input(
            &mut bytes1,
            (DAPP, VICTIM),
            (DAPP, VICTIM),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output1 = handle_withdraw(&input1, state.clone());
        assert_eq!(output1.status_code, 0);
        let after_withdraw = test_support::expect_full_replace_state(&output1);
        assert_eq!(after_withdraw.ledger.len(), 219, "victim's zeroed row must be removed (I9)");
        assert_eq!(after_withdraw.pending.len(), 1);
        let withdrawal_id = after_withdraw.pending[0].withdrawal_id;

        let mut bytes2 = Vec::new();
        let input2 = test_support::internal_input(
            &mut bytes2,
            (DAPP, NEW_DEPOSITOR),
            (DAPP, NEW_DEPOSITOR),
            DEPOSIT_METHOD_ID,
            0,
            &[Currency { currency_id: 99, amount: 1 }],
            &[],
            0,
            &[],
            &after_withdraw.serialize(),
        );
        let output2 = handle_deposit(&input2, after_withdraw.clone());
        assert_eq!(output2.status_code, 0, "new deposit must succeed at 219 -> 220");
        let after_new_deposit = test_support::expect_full_replace_state(&output2);
        assert_eq!(after_new_deposit.ledger.len(), 220);

        let message_bytes = test_support::callback_message_bytes(
            withdrawal_callback_method_id(withdrawal_id),
            (DAPP, VICTIM),
            (DAPP, DAPP),
            [0xCD; 32],
            1,
        );
        let acmi_bytes = test_support::build_message_input(
            (DAPP, DAPP),
            true,
            (DAPP, VICTIM),
            [0; 32],
            0,
            &[],
            &after_new_deposit.serialize(),
            &message_bytes,
        );
        let input3 = avm_contract_sdk::decode_message_input(&acmi_bytes)
            .expect("real ACMI buffer must decode");
        let output3 = handle_callback(&input3, after_new_deposit.clone());
        assert_eq!(
            output3.status_code, 0,
            "restore must never be rejected for ledger_capacity_exceeded"
        );
        let after_restore = test_support::expect_full_replace_state(&output3);
        assert_eq!(
            after_restore.ledger.len(),
            221,
            "restore recreates the victim's row as a 221st row"
        );
        assert_eq!(after_restore.ledger_balance(&victim_key), 500);
    }

    /// Phase 4 adversarial review, finding 1: the byte-densest state this
    /// contract's own rules can actually reach is **252 ledger rows
    /// together with 32 outstanding pending withdrawals** (24,359 bytes),
    /// not 220 rows + 32 pending (21,639 bytes), which SPEC.md §2.4,
    /// docs/THREAT_MODEL.md §5.9, this module's own
    /// `restore_bypasses_ledger_cap_at_exact_boundary` framing, and
    /// `constants.rs`'s `MIN_OPERATIONAL_SHELL` derivation all previously
    /// took as the true worst case. The reason 252+32 was dismissed (SPEC
    /// §1: "reaching 252 ledger rows requires every pending withdrawal to
    /// have already resolved") is incomplete: once 252 rows exist with zero
    /// pending, a *partial* withdrawal (not draining a row to exactly zero)
    /// creates a new pending record without removing its row — Invariant
    /// I9 only ever removes a row that a debit brings to exactly zero
    /// (`state.rs::debit_ledger`), and `handle_withdraw` never checks
    /// `MAX_LEDGER_ENTRIES` at all (only the two pending caps), since
    /// withdraw never adds a row. So all 32 global pending slots can be
    /// refilled on top of 252 still-intact rows.
    #[test]
    fn true_worst_case_state_is_252_rows_plus_32_pending() {
        const CURRENCIES: [u32; 4] = [101, 102, 103, 104];

        fn big_depositor(index: u8) -> [u8; 32] {
            let mut account = [0u8; 32];
            account[0] = 0xD0;
            account[31] = index;
            account
        }
        fn filler_depositor(index: u32) -> [u8; 32] {
            let mut account = [0u8; 32];
            account[0] = 0xE0;
            account[28..32].copy_from_slice(&index.to_be_bytes());
            account
        }
        fn do_withdraw(
            state: &VaultState,
            account: [u8; 32],
            currency_id: u32,
            amount: u128,
        ) -> VaultState {
            let mut body = Vec::with_capacity(21);
            body.push(CURRENCY_TAG_EXTRA);
            body.extend_from_slice(&currency_id.to_le_bytes());
            body.extend_from_slice(&amount.to_le_bytes());
            let mut bytes = Vec::new();
            let input = test_support::internal_input(
                &mut bytes,
                (DAPP, account),
                (DAPP, account),
                WITHDRAW_METHOD_ID,
                0,
                &[],
                &body,
                0,
                &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
                &state.serialize(),
            );
            let output = handle_withdraw(&input, state.clone());
            assert_eq!(output.status_code, 0, "withdraw must succeed");
            test_support::expect_full_replace_state(&output)
        }
        fn do_deposit(
            state: &VaultState,
            account: [u8; 32],
            currency_id: u32,
            amount: u128,
        ) -> VaultState {
            let mut bytes = Vec::new();
            let input = test_support::internal_input(
                &mut bytes,
                (DAPP, account),
                (DAPP, account),
                DEPOSIT_METHOD_ID,
                0,
                &[Currency { currency_id, amount }],
                &[],
                0,
                &[],
                &state.serialize(),
            );
            let output = handle_deposit(&input, state.clone());
            assert_eq!(output.status_code, 0, "new-row deposit must succeed below the cap");
            test_support::expect_full_replace_state(&output)
        }
        fn do_restore(state: &VaultState, record: &PendingWithdrawal) -> VaultState {
            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(record.withdrawal_id),
                (DAPP, record.depositor_account_id),
                (DAPP, DAPP),
                [0xCD; 32],
                1, // status_code != 0 -> restore
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, record.depositor_account_id),
                [0; 32],
                0,
                &[],
                &state.serialize(),
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let output = handle_callback(&input, state.clone());
            assert_eq!(output.status_code, 0, "restore must bypass MAX_LEDGER_ENTRIES");
            test_support::expect_full_replace_state(&output)
        }

        // --- Setup: ledger exactly at MAX_LEDGER_ENTRIES (220): 32 rows
        // that will be fully drained, plus 188 unrelated filler rows. ---
        let mut state = VaultState::new_empty([9; 32], 0);
        for index in 0..8u8 {
            for currency_id in CURRENCIES {
                state
                    .credit_ledger(
                        LedgerKey {
                            dapp_id: DAPP,
                            account_id: big_depositor(index),
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id,
                        },
                        10,
                    )
                    .expect("seed depositor row");
            }
        }
        seed_filler_rows(&mut state, 188, 0); // 32 + 188 = 220
        assert_eq!(state.ledger.len(), MAX_LEDGER_ENTRIES);

        // --- Step 1: fully drain all 32 of those rows -> I9 removes each
        // row; 32 pending records now outstanding (4 per depositor). ---
        for index in 0..8u8 {
            for currency_id in CURRENCIES {
                state = do_withdraw(&state, big_depositor(index), currency_id, 10);
            }
        }
        assert_eq!(state.ledger.len(), 188, "every fully-drained row was removed (I9)");
        assert_eq!(state.pending.len(), MAX_PENDING_WITHDRAWALS);

        // --- Step 2: 32 unrelated new depositors fill the freed capacity
        // back up to exactly MAX_LEDGER_ENTRIES with genuinely new rows. ---
        for index in 0..32u32 {
            state = do_deposit(&state, filler_depositor(index), 200, 1);
        }
        assert_eq!(state.ledger.len(), MAX_LEDGER_ENTRIES);

        // --- Step 3: all 32 pending withdrawals fail; restores bypass the
        // cap (SPEC.md §1/§2.4) and recreate all 32 removed rows -> 252. ---
        for record in state.pending.clone() {
            state = do_restore(&state, &record);
        }
        assert_eq!(state.ledger.len(), MAX_LEDGER_ENTRIES + MAX_PENDING_WITHDRAWALS); // 252
        assert_eq!(state.pending.len(), 0);

        // --- Step 4: the step SPEC.md previously assumed was impossible.
        // Each of the 8 depositors makes 4 PARTIAL withdrawals (1 of the
        // 10 restored units), refilling all 32 pending slots WITHOUT
        // removing a single row. ---
        for index in 0..8u8 {
            for currency_id in CURRENCIES {
                state = do_withdraw(&state, big_depositor(index), currency_id, 1);
            }
        }
        assert_eq!(state.ledger.len(), 252, "a partial withdrawal never removes its row");
        assert_eq!(state.pending.len(), MAX_PENDING_WITHDRAWALS);

        let worst_case_bytes = state.serialize().len();
        assert_eq!(
            worst_case_bytes,
            HEADER_LEN + 252 * LEDGER_ENTRY_LEN + MAX_PENDING_WITHDRAWALS * PENDING_RECORD_LEN
        );
        assert_eq!(worst_case_bytes, 24_359);

        // MIN_OPERATIONAL_SHELL is derived (constants.rs, SPEC.md §1) as
        // worst_case_bytes x 1.000 SHELL/byte/s x 60s. Against the
        // genuinely reachable worst case that is 24,359 x 60 = 1,461,540 —
        // the shipped 1,300,000 (derived from the previously-believed
        // worst case of 21,639 bytes) is short of its own stated policy.
        assert!(
            MIN_OPERATIONAL_SHELL >= worst_case_bytes as u128 * 60,
            "MIN_OPERATIONAL_SHELL ({MIN_OPERATIONAL_SHELL}) does not cover its own \
             stated 60-second rent window at the genuinely reachable worst-case state \
             size ({worst_case_bytes} bytes, requiring {}) — SPEC.md §2.4's previously \
             documented worst case of 21,639 bytes (220 rows + 32 pending) was not the \
             maximum: 252 rows + 32 pending is reachable, because a partial withdrawal \
             creates a pending record without removing its row",
            worst_case_bytes as u128 * 60
        );
    }

    /// SPEC.md §8 test 7: a DEPOSIT carrying SHELL alongside an
    /// otherwise-legitimate currency is rejected whole — none of the
    /// other, otherwise-valid currencies are partially credited.
    #[test]
    fn deposit_with_shell_and_other_currencies_rejected_atomically() {
        let state = VaultState::new_empty([9; 32], 0);
        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            DEPOSIT_METHOD_ID,
            1000,
            &[
                Currency { currency_id: 7, amount: 50 },
                Currency { currency_id: SHELL_CURRENCY_ID, amount: 1 },
            ],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_deposit(&input, state.clone());
        assert_eq!(output.status_code, ERR_SHELL_NOT_CUSTODIED);
        assert!(output.state_update.is_none());
    }

    /// SPEC.md §8 test 8: `TOPUP_METHOD_ID` rejects a non-zero
    /// `value_main`, a mixed (non-all-SHELL) `extra`, and an empty
    /// `extra`; succeeds with all-SHELL `extra` even while paused (I15).
    #[test]
    fn topup_rejects_nonzero_value_main() {
        let state = VaultState::new_empty([9; 32], 0);
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            [3; 32],
            TOPUP_METHOD_ID,
            100,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: 50 }],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_topup(&input, &state);
        assert_eq!(output.status_code, ERR_TOPUP_REQUIRES_SHELL_ONLY);
    }

    #[test]
    fn topup_rejects_mixed_currency() {
        let state = VaultState::new_empty([9; 32], 0);
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            [3; 32],
            TOPUP_METHOD_ID,
            0,
            &[
                Currency { currency_id: SHELL_CURRENCY_ID, amount: 50 },
                Currency { currency_id: 7, amount: 1 },
            ],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_topup(&input, &state);
        assert_eq!(output.status_code, ERR_TOPUP_REQUIRES_SHELL_ONLY);
    }

    #[test]
    fn topup_rejects_empty_extra() {
        let state = VaultState::new_empty([9; 32], 0);
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            [3; 32],
            TOPUP_METHOD_ID,
            0,
            &[],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_topup(&input, &state);
        assert_eq!(output.status_code, ERR_TOPUP_REQUIRES_SHELL_ONLY);
    }

    #[test]
    fn topup_succeeds_all_shell_while_paused() {
        let mut state = VaultState::new_empty([9; 32], 0);
        state.deposits_paused = true;
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            [3; 32],
            TOPUP_METHOD_ID,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: 50 }],
            &[],
            0,
            &[],
            &state.serialize(),
        );
        let output = handle_topup(&input, &state);
        assert_eq!(output.status_code, 0, "Invariant I15: TOPUP succeeds even while paused");
    }

    /// SPEC.md §8 test 15: `TOPUP_METHOD_ID` never reads, iterates, or
    /// touches the ledger in any way (Invariant I15), regardless of what
    /// it currently holds for other depositors and currencies.
    #[test]
    fn topup_never_touches_existing_ledger_contents() {
        let mut state = VaultState::new_empty([9; 32], 0);
        state
            .credit_ledger(
                LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR,
                    currency_tag: CURRENCY_TAG_MAIN,
                    currency_id: 0,
                },
                111,
            )
            .expect("seed1");
        state
            .credit_ledger(
                LedgerKey {
                    dapp_id: DAPP,
                    account_id: [51; 32],
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 7,
                },
                222,
            )
            .expect("seed2");
        state
            .credit_ledger(
                LedgerKey {
                    dapp_id: DAPP,
                    account_id: [52; 32],
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 9,
                },
                333,
            )
            .expect("seed3");
        let before_bytes = state.serialize();

        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, DAPP),
            [3; 32],
            TOPUP_METHOD_ID,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: 999 }],
            &[],
            0,
            &[],
            &before_bytes,
        );
        let output = handle_topup(&input, &state);
        assert_eq!(output.status_code, 0);
        let resulting = test_support::expect_full_replace_state(&output);
        assert_eq!(
            resulting.serialize(),
            before_bytes,
            "TOPUP must leave every ledger row byte-for-byte unchanged (I15)"
        );
    }

    /// SPEC.md §8 test 9: both callback outcomes resolve exactly as they
    /// would unpaused (Invariant I14) — settle normally, restore
    /// normally — while deposits are currently paused.
    #[test]
    fn callback_resolves_normally_while_deposits_paused() {
        const DEPOSITOR_B: [u8; 32] = [51; 32];
        let mut state = VaultState::new_empty([9; 32], 0);
        state.deposits_paused = true;
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 1,
            depositor_dapp_id: DAPP,
            depositor_account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
            amount: 100,
        });
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 2,
            depositor_dapp_id: DAPP,
            depositor_account_id: DEPOSITOR_B,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 9,
            amount: 200,
        });
        state.next_withdrawal_id = 3;

        let msg1 = test_support::callback_message_bytes(
            withdrawal_callback_method_id(1),
            (DAPP, DEPOSITOR),
            (DAPP, DAPP),
            [0xCD; 32],
            0,
        );
        let acmi1 = test_support::build_message_input(
            (DAPP, DAPP),
            true,
            (DAPP, DEPOSITOR),
            [0; 32],
            0,
            &[],
            &state.serialize(),
            &msg1,
        );
        let input1 =
            avm_contract_sdk::decode_message_input(&acmi1).expect("real ACMI buffer must decode");
        let output1 = handle_callback(&input1, state.clone());
        assert_eq!(output1.status_code, 0);
        let resulting1 = test_support::expect_full_replace_state(&output1);
        assert_eq!(resulting1.pending.len(), 1);
        assert!(resulting1.deposits_paused, "pause state itself untouched by callback resolution");

        let msg2 = test_support::callback_message_bytes(
            withdrawal_callback_method_id(2),
            (DAPP, DEPOSITOR_B),
            (DAPP, DAPP),
            [0xCD; 32],
            1,
        );
        let acmi2 = test_support::build_message_input(
            (DAPP, DAPP),
            true,
            (DAPP, DEPOSITOR_B),
            [0; 32],
            0,
            &[],
            &state.serialize(),
            &msg2,
        );
        let input2 =
            avm_contract_sdk::decode_message_input(&acmi2).expect("real ACMI buffer must decode");
        let output2 = handle_callback(&input2, state.clone());
        assert_eq!(output2.status_code, 0);
        let resulting2 = test_support::expect_full_replace_state(&output2);
        assert_eq!(resulting2.pending.len(), 1);
        let key_b = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR_B,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 9,
        };
        assert_eq!(
            resulting2.ledger_balance(&key_b),
            200,
            "restore credits normally even while paused"
        );
    }

    /// SPEC.md §8 test 10, first bullet: pause then unpause, then replay
    /// the original (now long-stale) pause call — must fail
    /// (`stale_admin_sequence`), leaving the currently-unpaused state
    /// intact.
    #[test]
    fn pause_unpause_replay_sequence() {
        let state0 = VaultState::new_empty(OWNER, 0);
        let mut pause_body = Vec::with_capacity(5);
        pause_body.extend_from_slice(&0u32.to_le_bytes());
        pause_body.push(HEADER_FLAG_DEPOSITS_PAUSED);
        let mut b1 = Vec::new();
        let input1 = test_support::external_input(
            &mut b1,
            (DAPP, DAPP),
            OWNER,
            SET_PAUSE_METHOD_ID,
            0,
            &[],
            &pause_body,
            0,
            &[],
            &state0.serialize(),
        );
        let output1 = handle_set_pause(&input1, state0.clone());
        assert_eq!(output1.status_code, 0);
        let state1 = test_support::expect_full_replace_state(&output1);
        assert!(state1.deposits_paused);
        assert_eq!(state1.admin_seq, 1);

        let mut unpause_body = Vec::with_capacity(5);
        unpause_body.extend_from_slice(&1u32.to_le_bytes());
        unpause_body.push(0);
        let mut b2 = Vec::new();
        let input2 = test_support::external_input(
            &mut b2,
            (DAPP, DAPP),
            OWNER,
            SET_PAUSE_METHOD_ID,
            0,
            &[],
            &unpause_body,
            0,
            &[],
            &state1.serialize(),
        );
        let output2 = handle_set_pause(&input2, state1.clone());
        assert_eq!(output2.status_code, 0);
        let state2 = test_support::expect_full_replace_state(&output2);
        assert!(!state2.deposits_paused);
        assert_eq!(state2.admin_seq, 2);

        let mut b3 = Vec::new();
        let input3 = test_support::external_input(
            &mut b3,
            (DAPP, DAPP),
            OWNER,
            SET_PAUSE_METHOD_ID,
            0,
            &[],
            &pause_body,
            0,
            &[],
            &state2.serialize(),
        );
        let output3 = handle_set_pause(&input3, state2.clone());
        assert_eq!(output3.status_code, ERR_STALE_ADMIN_SEQUENCE);
        assert!(output3.state_update.is_none());
    }

    /// SPEC.md §8 test 10, second bullet: owner rotation A→B→A, then
    /// replay A's very first propose-B call from the start of the
    /// sequence — must fail regardless of how many other admin calls
    /// have happened since.
    #[test]
    fn owner_rotation_a_to_b_to_a_then_replay_original_propose_fails() {
        const A: [u8; 32] = OWNER;
        const B: [u8; 32] = [8; 32];
        let state0 = VaultState::new_empty(A, 0);

        let mut body_propose_b = Vec::with_capacity(36);
        body_propose_b.extend_from_slice(&0u32.to_le_bytes());
        body_propose_b.extend_from_slice(&B);
        let mut b1 = Vec::new();
        let input1 = test_support::external_input(
            &mut b1,
            (DAPP, DAPP),
            A,
            PROPOSE_OWNER_METHOD_ID,
            0,
            &[],
            &body_propose_b,
            0,
            &[],
            &state0.serialize(),
        );
        let output1 = handle_propose_owner(&input1, state0.clone());
        assert_eq!(output1.status_code, 0);
        let state1 = test_support::expect_full_replace_state(&output1);

        let mut body_accept = Vec::with_capacity(4);
        body_accept.extend_from_slice(&1u32.to_le_bytes());
        let mut b2 = Vec::new();
        let input2 = test_support::external_input(
            &mut b2,
            (DAPP, DAPP),
            B,
            ACCEPT_OWNER_METHOD_ID,
            0,
            &[],
            &body_accept,
            0,
            &[],
            &state1.serialize(),
        );
        let output2 = handle_accept_owner(&input2, state1.clone());
        assert_eq!(output2.status_code, 0);
        let state2 = test_support::expect_full_replace_state(&output2);
        assert_eq!(state2.owner_pubkey, B);

        let mut body_propose_a = Vec::with_capacity(36);
        body_propose_a.extend_from_slice(&2u32.to_le_bytes());
        body_propose_a.extend_from_slice(&A);
        let mut b3 = Vec::new();
        let input3 = test_support::external_input(
            &mut b3,
            (DAPP, DAPP),
            B,
            PROPOSE_OWNER_METHOD_ID,
            0,
            &[],
            &body_propose_a,
            0,
            &[],
            &state2.serialize(),
        );
        let output3 = handle_propose_owner(&input3, state2.clone());
        assert_eq!(output3.status_code, 0);
        let state3 = test_support::expect_full_replace_state(&output3);

        let mut body_accept2 = Vec::with_capacity(4);
        body_accept2.extend_from_slice(&3u32.to_le_bytes());
        let mut b4 = Vec::new();
        let input4 = test_support::external_input(
            &mut b4,
            (DAPP, DAPP),
            A,
            ACCEPT_OWNER_METHOD_ID,
            0,
            &[],
            &body_accept2,
            0,
            &[],
            &state3.serialize(),
        );
        let output4 = handle_accept_owner(&input4, state3.clone());
        assert_eq!(output4.status_code, 0);
        let state4 = test_support::expect_full_replace_state(&output4);
        assert_eq!(state4.owner_pubkey, A);
        assert_eq!(state4.admin_seq, 4);

        let mut b5 = Vec::new();
        let input5 = test_support::external_input(
            &mut b5,
            (DAPP, DAPP),
            A,
            PROPOSE_OWNER_METHOD_ID,
            0,
            &[],
            &body_propose_b,
            0,
            &[],
            &state4.serialize(),
        );
        let output5 = handle_propose_owner(&input5, state4.clone());
        assert_eq!(output5.status_code, ERR_STALE_ADMIN_SEQUENCE);
        assert!(output5.state_update.is_none());
    }

    /// SPEC.md §8 test 14, third case: `input.extra` with no entry for
    /// `currency_id == 2` at all (absent, not an explicit zero) must be
    /// indistinguishable from an explicit `0` — fails `vault_underfunded`
    /// identically, confirming "entry absent" is never special-cased as
    /// "check inapplicable."
    #[test]
    fn i17_absent_shell_entry_is_treated_as_zero() {
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        state.credit_ledger(key, 1000).expect("seed");
        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&100u128.to_le_bytes());
        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &body,
            0,
            &[], // no SHELL entry at all, not even a zero-amount one
            &state.serialize(),
        );
        let output = handle_withdraw(&input, state.clone());
        assert_eq!(output.status_code, ERR_VAULT_UNDERFUNDED);
        assert!(output.state_update.is_none());
    }

    /// SPEC.md §8 test 16: a `WITHDRAW` call whose `extra` includes
    /// SHELL (alongside an otherwise well-formed body) is rejected as
    /// `invalid_body` (1501), not silently ignored — the existing
    /// empty-`extra` requirement already covers this structurally.
    #[test]
    fn withdraw_carrying_shell_in_extra_is_rejected_as_invalid_body() {
        let mut state = VaultState::new_empty([9; 32], 0);
        let key = LedgerKey {
            dapp_id: DAPP,
            account_id: DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
        };
        state.credit_ledger(key, 1000).expect("seed");
        let mut body = Vec::with_capacity(21);
        body.push(CURRENCY_TAG_EXTRA);
        body.extend_from_slice(&7u32.to_le_bytes());
        body.extend_from_slice(&100u128.to_le_bytes());
        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, DEPOSITOR),
            (DAPP, DEPOSITOR),
            WITHDRAW_METHOD_ID,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: 1 }],
            &body,
            0,
            &[Currency { currency_id: SHELL_CURRENCY_ID, amount: MIN_OPERATIONAL_SHELL }],
            &state.serialize(),
        );
        let output = handle_withdraw(&input, state.clone());
        assert_eq!(output.status_code, ERR_INVALID_BODY);
        assert!(output.state_update.is_none());
    }

    /// SPEC.md §8 test 17: a permanently-dropped callback's own pending
    /// record is never touched, cleared, or confused when a separate,
    /// unrelated withdrawal resolves normally — the direct regression
    /// test for Invariant I3's "at most once" guarantee holding even in
    /// the presence of an unrelated, permanently-unresolved record in
    /// the same `pending` list.
    #[test]
    fn stuck_pending_withdrawal_never_confused_with_a_separate_normal_one() {
        const STUCK_DEPOSITOR: [u8; 32] = [61; 32];
        const NORMAL_DEPOSITOR: [u8; 32] = [62; 32];
        let mut state = VaultState::new_empty([9; 32], 0);
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 1,
            depositor_dapp_id: DAPP,
            depositor_account_id: STUCK_DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
            amount: 500,
        });
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 2,
            depositor_dapp_id: DAPP,
            depositor_account_id: NORMAL_DEPOSITOR,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 9,
            amount: 300,
        });
        state.next_withdrawal_id = 3;
        // withdrawal_id 1 ("stuck") never receives any callback at all —
        // simulated simply by never calling handle_callback for it.

        let msg = test_support::callback_message_bytes(
            withdrawal_callback_method_id(2),
            (DAPP, NORMAL_DEPOSITOR),
            (DAPP, DAPP),
            [0xCD; 32],
            0,
        );
        let acmi = test_support::build_message_input(
            (DAPP, DAPP),
            true,
            (DAPP, NORMAL_DEPOSITOR),
            [0; 32],
            0,
            &[],
            &state.serialize(),
            &msg,
        );
        let input =
            avm_contract_sdk::decode_message_input(&acmi).expect("real ACMI buffer must decode");
        let output = handle_callback(&input, state.clone());
        assert_eq!(output.status_code, 0);
        let resulting = test_support::expect_full_replace_state(&output);
        assert_eq!(resulting.pending.len(), 1, "only id=2 resolved; id=1 remains");
        assert_eq!(
            resulting.pending[0].withdrawal_id, 1,
            "the stuck record is untouched and still present"
        );
        assert_eq!(resulting.pending[0].depositor_account_id, STUCK_DEPOSITOR);
        assert_eq!(
            resulting.pending[0].amount, 500,
            "stuck record's own fields are byte-for-byte unchanged"
        );
    }
}
