//! State parsing, validation, and serialization (SPEC.md §2).
//!
//! The stored state blob is this contract's own wire format (not an
//! `avm-contract-sdk` format) — magic, reserved bytes, sort order, and
//! every length are validated defensively on every read, never assumed
//! from how this contract itself last wrote it (SPEC.md §2.2).

use crate::constants::CURRENCY_TAG_EXTRA;
use crate::constants::CURRENCY_TAG_MAIN;
use crate::constants::ERR_INVALID_CALLBACK;
use crate::constants::ERR_INVALID_STATE;
use crate::constants::ERR_LEDGER_AMOUNT_OVERFLOW;
use crate::constants::ERR_WITHDRAWAL_NOT_FOUND;
use crate::constants::HEADER_FLAG_DEPOSITS_PAUSED;
use crate::constants::HEADER_LEN;
use crate::constants::LEDGER_ENTRY_LEN;
use crate::constants::MAGIC;
// Gate 2b round 3 TASK H: `MAX_PENDING_PER_DEPOSITOR` is no longer used by
// `parse()` itself (the check that used it was removed — see that
// removal's own comment below); only this module's own tests still
// reference it, to confirm the removed check's shape explicitly.
#[cfg(test)]
use crate::constants::MAX_PENDING_PER_DEPOSITOR;
use crate::constants::MAX_PENDING_WITHDRAWALS;
use crate::constants::PENDING_RECORD_LEN;
use crate::constants::SHELL_CURRENCY_ID;
use crate::constants::STATE_VERSION;

// Gate 3c TASK 1: which specific defensive check inside `parse()` fired,
// for test-side coverage tallying only — see `parse_with_reason`'s own
// comment. Every variant here collapses to the identical, single
// `ERR_INVALID_STATE` code at the public `parse()` boundary; this type
// changes no production behavior and is never part of this contract's
// external API (`pub(crate)`, not `pub`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseFailureReason {
    BadMagic,
    BadVersion,
    /// Any header field too short to read, or a reserved/fixed-size
    /// per-row field too short inside an already-`chunks_exact`-sized
    /// chunk — the latter is defensive-only and not known to be
    /// reachable by any crafted input (SPEC.md §2.2's stated principle:
    /// validate defensively regardless of whether a gap is currently
    /// provable).
    TruncatedHeader,
    BadHeaderFlags,
    BadReservedBytes,
    BadBuildFlags,
    /// `ledger_count`/`pending_count` (or the lengths/offsets derived
    /// from them) overflow `usize` arithmetic.
    CountOverflow,
    /// The declared counts' implied total length doesn't match the
    /// buffer's actual length.
    LengthMismatch,
    TruncatedLedgerRow,
    /// `validate_currency_shape` rejected a ledger row — covers all
    /// three of its sub-cases (tag not 0/1, Main with nonzero id, or
    /// Extra with `currency_id == SHELL_CURRENCY_ID`) under one reason,
    /// matching the single check `parse()` itself performs; the
    /// Extra+SHELL sub-case specifically is also covered by the
    /// pre-existing `rejects_shell_ledger_row` unit test.
    BadLedgerCurrencyShape,
    ZeroLedgerAmount,
    UnsortedLedger,
    TruncatedPendingRow,
    /// Same three sub-cases as `BadLedgerCurrencyShape`, for a pending
    /// record instead of a ledger row.
    BadPendingCurrencyShape,
    ZeroPendingAmount,
    UnsortedPending,
    PendingOverCap,
    NextWithdrawalIdTooLow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LedgerKey {
    pub dapp_id: [u8; 32],
    pub account_id: [u8; 32],
    pub currency_tag: u8,
    pub currency_id: u32,
}

#[derive(Debug, Clone)]
pub struct LedgerEntry {
    pub key: LedgerKey,
    pub amount: u128,
}

#[derive(Debug, Clone)]
pub struct PendingWithdrawal {
    pub withdrawal_id: u32,
    pub depositor_dapp_id: [u8; 32],
    pub depositor_account_id: [u8; 32],
    pub currency_tag: u8,
    pub currency_id: u32,
    pub amount: u128,
}

#[derive(Debug, Clone)]
pub struct VaultState {
    pub owner_pubkey: [u8; 32],
    pub pending_owner_pubkey: [u8; 32],
    pub deposits_paused: bool,
    pub admin_seq: u32,
    pub next_withdrawal_id: u32,
    pub build_flags: u8,
    /// Strictly sorted ascending by `LedgerKey`; no duplicate keys, no
    /// zero-amount rows (Invariant I9).
    pub ledger: Vec<LedgerEntry>,
    /// Strictly sorted ascending by `withdrawal_id`; no duplicate ids.
    pub pending: Vec<PendingWithdrawal>,
}

impl VaultState {
    pub fn new_empty(owner_pubkey: [u8; 32], build_flags: u8) -> Self {
        Self {
            owner_pubkey,
            pending_owner_pubkey: [0; 32],
            deposits_paused: false,
            admin_seq: 0,
            next_withdrawal_id: 0,
            build_flags,
            ledger: Vec::new(),
            pending: Vec::new(),
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, u32> {
        Self::parse_with_reason(bytes).map_err(|_| ERR_INVALID_STATE)
    }

    // Gate 3c TASK 1: production behavior and the public `parse()` ABI
    // are UNCHANGED by this split — every failure still collapses to the
    // single `ERR_INVALID_STATE` code at the `parse()` boundary above,
    // exactly as before. This internal variant exists only so tests
    // (`lib.rs`'s fuzz suite) can tally which specific validation branch
    // actually fired, instead of only "Ok or Err" — the single-u32
    // public API carries no such information, and re-implementing
    // parse()'s own checks a second time just to classify failures from
    // the outside would risk drifting out of sync with the real logic.
    // `pub(crate)` only — never part of this contract's external API.
    pub(crate) fn parse_with_reason(bytes: &[u8]) -> Result<Self, ParseFailureReason> {
        use ParseFailureReason as R;
        let magic = bytes.get(0..4).ok_or(R::TruncatedHeader)?;
        if magic != MAGIC {
            return Err(R::BadMagic);
        }
        let version = *bytes.get(4).ok_or(R::TruncatedHeader)?;
        if version != STATE_VERSION {
            return Err(R::BadVersion);
        }
        let owner_pubkey: [u8; 32] = bytes
            .get(5..37)
            .ok_or(R::TruncatedHeader)?
            .try_into()
            .map_err(|_| R::TruncatedHeader)?;
        let pending_owner_pubkey: [u8; 32] = bytes
            .get(37..69)
            .ok_or(R::TruncatedHeader)?
            .try_into()
            .map_err(|_| R::TruncatedHeader)?;
        let flags = *bytes.get(69).ok_or(R::TruncatedHeader)?;
        if flags & !HEADER_FLAG_DEPOSITS_PAUSED != 0 {
            return Err(R::BadHeaderFlags);
        }
        let deposits_paused = flags & HEADER_FLAG_DEPOSITS_PAUSED != 0;
        let reserved = bytes.get(70..74).ok_or(R::TruncatedHeader)?;
        if reserved.iter().any(|byte| *byte != 0) {
            return Err(R::BadReservedBytes);
        }
        let admin_seq = read_u32(bytes, 74)?;
        let next_withdrawal_id = read_u32(bytes, 78)?;
        let ledger_count = read_u32(bytes, 82)? as usize;
        let pending_count = read_u32(bytes, 86)? as usize;
        let build_flags = *bytes.get(90).ok_or(R::TruncatedHeader)?;
        if build_flags & !1 != 0 {
            return Err(R::BadBuildFlags);
        }

        let ledger_bytes_len =
            ledger_count.checked_mul(LEDGER_ENTRY_LEN).ok_or(R::CountOverflow)?;
        let pending_bytes_len =
            pending_count.checked_mul(PENDING_RECORD_LEN).ok_or(R::CountOverflow)?;
        let ledger_start = HEADER_LEN;
        let ledger_end = ledger_start.checked_add(ledger_bytes_len).ok_or(R::CountOverflow)?;
        let pending_end = ledger_end.checked_add(pending_bytes_len).ok_or(R::CountOverflow)?;
        if pending_end != bytes.len() {
            return Err(R::LengthMismatch);
        }

        let ledger_bytes = bytes.get(ledger_start..ledger_end).ok_or(R::TruncatedLedgerRow)?;
        let mut ledger = Vec::with_capacity(ledger_count);
        let mut previous_key: Option<LedgerKey> = None;
        for chunk in ledger_bytes.chunks_exact(LEDGER_ENTRY_LEN) {
            let dapp_id: [u8; 32] = chunk
                .get(0..32)
                .ok_or(R::TruncatedLedgerRow)?
                .try_into()
                .map_err(|_| R::TruncatedLedgerRow)?;
            let account_id: [u8; 32] = chunk
                .get(32..64)
                .ok_or(R::TruncatedLedgerRow)?
                .try_into()
                .map_err(|_| R::TruncatedLedgerRow)?;
            let currency_tag = *chunk.get(64).ok_or(R::TruncatedLedgerRow)?;
            let currency_id = u32::from_le_bytes(
                chunk
                    .get(65..69)
                    .ok_or(R::TruncatedLedgerRow)?
                    .try_into()
                    .map_err(|_| R::TruncatedLedgerRow)?,
            );
            let amount = u128::from_le_bytes(
                chunk
                    .get(69..85)
                    .ok_or(R::TruncatedLedgerRow)?
                    .try_into()
                    .map_err(|_| R::TruncatedLedgerRow)?,
            );
            validate_currency_shape(currency_tag, currency_id)
                .map_err(|_| R::BadLedgerCurrencyShape)?;
            if amount == 0 {
                return Err(R::ZeroLedgerAmount);
            }
            let key = LedgerKey { dapp_id, account_id, currency_tag, currency_id };
            if let Some(previous) = previous_key {
                if key <= previous {
                    return Err(R::UnsortedLedger);
                }
            }
            previous_key = Some(key);
            ledger.push(LedgerEntry { key, amount });
        }

        let pending_bytes = bytes.get(ledger_end..pending_end).ok_or(R::TruncatedPendingRow)?;
        let mut pending = Vec::with_capacity(pending_count);
        let mut previous_id: Option<u32> = None;
        for chunk in pending_bytes.chunks_exact(PENDING_RECORD_LEN) {
            let withdrawal_id = u32::from_le_bytes(
                chunk
                    .get(0..4)
                    .ok_or(R::TruncatedPendingRow)?
                    .try_into()
                    .map_err(|_| R::TruncatedPendingRow)?,
            );
            let depositor_dapp_id: [u8; 32] = chunk
                .get(4..36)
                .ok_or(R::TruncatedPendingRow)?
                .try_into()
                .map_err(|_| R::TruncatedPendingRow)?;
            let depositor_account_id: [u8; 32] = chunk
                .get(36..68)
                .ok_or(R::TruncatedPendingRow)?
                .try_into()
                .map_err(|_| R::TruncatedPendingRow)?;
            let currency_tag = *chunk.get(68).ok_or(R::TruncatedPendingRow)?;
            let currency_id = u32::from_le_bytes(
                chunk
                    .get(69..73)
                    .ok_or(R::TruncatedPendingRow)?
                    .try_into()
                    .map_err(|_| R::TruncatedPendingRow)?,
            );
            let amount = u128::from_le_bytes(
                chunk
                    .get(73..89)
                    .ok_or(R::TruncatedPendingRow)?
                    .try_into()
                    .map_err(|_| R::TruncatedPendingRow)?,
            );
            validate_currency_shape(currency_tag, currency_id)
                .map_err(|_| R::BadPendingCurrencyShape)?;
            if amount == 0 {
                return Err(R::ZeroPendingAmount);
            }
            if let Some(previous) = previous_id {
                if withdrawal_id <= previous {
                    return Err(R::UnsortedPending);
                }
            }
            previous_id = Some(withdrawal_id);
            pending.push(PendingWithdrawal {
                withdrawal_id,
                depositor_dapp_id,
                depositor_account_id,
                currency_tag,
                currency_id,
                amount,
            });
        }

        // "THEN Phase 3" item 3: a stored state blob is never trusted to
        // already respect this contract's own caps/monotonicity rules
        // just because this contract's own code is what last wrote it
        // (SPEC.md §2.2's own stated defensive-parsing principle) — these
        // two checks close a real gap: `pending_count` alone was
        // bounds-checked only by the length-prefix arithmetic above, not
        // against `MAX_PENDING_WITHDRAWALS`, and `next_withdrawal_id` was
        // never checked against the pending ids it is responsible for
        // never colliding with (Invariant I12).
        if pending.len() > MAX_PENDING_WITHDRAWALS {
            return Err(R::PendingOverCap);
        }
        if let Some(last_pending_id) = previous_id {
            if next_withdrawal_id <= last_pending_id {
                return Err(R::NextWithdrawalIdTooLow);
            }
        }
        // Gate 2b round 3 TASK H: the per-depositor (`MAX_PENDING_PER_
        // DEPOSITOR`) cap check that used to live here — an O(n²)
        // pairwise comparison over every pending record — is REMOVED, not
        // linearized. Root-caused via a real-stand A/B test (never
        // committed spike copies: one with this loop, one without,
        // nothing else changed) to be the actual cause of P18/P19's
        // silently-stranded-withdrawal failure at worst-case state sizes:
        // removing exactly this loop, and nothing else, resolved all 8
        // previously-stuck configurations tested (docs/PLATFORM_FINDINGS.md
        // P20). A linear rewrite was also tried, twice (a `BTreeMap`
        // version and a sort-based `Vec` version) — both bloated the
        // compiled wasm enough to risk the same deploy-size limit this
        // investigation hit elsewhere (P20's `BodyTooLarge` finding),
        // which is itself part of why this check is dropped outright
        // rather than kept in a cheaper shape. This is safe to drop
        // because the invariant it enforced (at most
        // `MAX_PENDING_PER_DEPOSITOR` outstanding withdrawals per
        // depositor) is already enforced on every path that can ever
        // insert a pending record — `handlers::handle_withdraw`'s own
        // `pending_count_for_depositor(...) >= MAX_PENDING_PER_DEPOSITOR`
        // check, which runs before every insert — and no other method
        // ever writes to `pending` (`handle_callback` only ever removes).
        // `parse()`'s own stated defensive principle above (never trust a
        // blob just because this contract's own code wrote it) still
        // holds for the two checks retained: this one specifically
        // protected against this contract's own writer logic going wrong,
        // which is already independently guaranteed by `handle_withdraw`
        // itself on every insert, not by a redundant second check at read
        // time whose cost turned out to be large enough to strand real
        // withdrawals before it could ever matter.

        Ok(Self {
            owner_pubkey,
            pending_owner_pubkey,
            deposits_paused,
            admin_seq,
            next_withdrawal_id,
            build_flags,
            ledger,
            pending,
        })
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            HEADER_LEN
                + self.ledger.len() * LEDGER_ENTRY_LEN
                + self.pending.len() * PENDING_RECORD_LEN,
        );
        out.extend_from_slice(&MAGIC);
        out.push(STATE_VERSION);
        out.extend_from_slice(&self.owner_pubkey);
        out.extend_from_slice(&self.pending_owner_pubkey);
        out.push(if self.deposits_paused { HEADER_FLAG_DEPOSITS_PAUSED } else { 0 });
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&self.admin_seq.to_le_bytes());
        out.extend_from_slice(&self.next_withdrawal_id.to_le_bytes());
        out.extend_from_slice(&(self.ledger.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.pending.len() as u32).to_le_bytes());
        out.push(self.build_flags);
        debug_assert_eq!(out.len(), HEADER_LEN);

        for entry in &self.ledger {
            out.extend_from_slice(&entry.key.dapp_id);
            out.extend_from_slice(&entry.key.account_id);
            out.push(entry.key.currency_tag);
            out.extend_from_slice(&entry.key.currency_id.to_le_bytes());
            out.extend_from_slice(&entry.amount.to_le_bytes());
        }
        for record in &self.pending {
            out.extend_from_slice(&record.withdrawal_id.to_le_bytes());
            out.extend_from_slice(&record.depositor_dapp_id);
            out.extend_from_slice(&record.depositor_account_id);
            out.push(record.currency_tag);
            out.extend_from_slice(&record.currency_id.to_le_bytes());
            out.extend_from_slice(&record.amount.to_le_bytes());
        }
        out
    }

    /// Finds a ledger entry's current amount for `key`, if any (Invariant I9:
    /// a row is never stored with `amount == 0`, so "no entry" and "zero
    /// balance" are the same thing here).
    pub fn ledger_balance(&self, key: &LedgerKey) -> u128 {
        let Ok(index) = self.ledger.binary_search_by(|entry| entry.key.cmp(key)) else {
            return 0;
        };
        // `binary_search_by` only ever returns `Ok(index)` for an index
        // that genuinely exists in this same slice; `.get()` is used
        // anyway (never a bare `[index]`) so this can never panic even if
        // that invariant were ever somehow wrong.
        self.ledger.get(index).map_or(0, |entry| entry.amount)
    }

    /// Credits `amount` to the entry for `key`, creating a new row if none
    /// exists. Returns `true` if this created a genuinely new row (the
    /// caller is responsible for enforcing `MAX_LEDGER_ENTRIES` against new
    /// rows *before* calling this — this method only performs the mutation).
    /// Returns `Err(())` on `u128` overflow (checked arithmetic, SPEC.md's
    /// "checked arithmetic everywhere" requirement) — practically
    /// unreachable, but never silently wrapped.
    pub fn credit_ledger(&mut self, key: LedgerKey, amount: u128) -> Result<bool, ()> {
        match self.ledger.binary_search_by(|entry| entry.key.cmp(&key)) {
            Ok(index) => {
                let entry = self.ledger.get_mut(index).ok_or(())?;
                entry.amount = entry.amount.checked_add(amount).ok_or(())?;
                Ok(false)
            }
            Err(index) => {
                self.ledger.insert(index, LedgerEntry { key, amount });
                Ok(true)
            }
        }
    }

    /// Debits `amount` from the entry for `key`, removing the row entirely
    /// if it reaches exactly zero (Invariant I9). Returns `Err(())` if no
    /// entry exists or the balance is insufficient — callers must always
    /// check `ledger_balance` first; this is a defensive second check, not
    /// the primary one, and never panics either way.
    pub fn debit_ledger(&mut self, key: &LedgerKey, amount: u128) -> Result<(), ()> {
        let index = self.ledger.binary_search_by(|entry| entry.key.cmp(key)).map_err(|_| ())?;
        let entry = self.ledger.get(index).ok_or(())?;
        let remaining = entry.amount.checked_sub(amount).ok_or(())?;
        if remaining == 0 {
            self.ledger.remove(index);
        } else {
            let entry = self.ledger.get_mut(index).ok_or(())?;
            entry.amount = remaining;
        }
        Ok(())
    }

    // Gate 3e TASK F2: no longer called by production dispatch — the
    // callback fast path (`fast_callback`, below) locates and removes
    // its target pending record directly from the raw bytes, never via
    // this method. Kept `#[cfg(test)]`-only because the slow-path
    // `handlers::handle_callback` (the differential tests' own ground
    // truth) still uses it.
    #[cfg(test)]
    pub fn pending_index(&self, withdrawal_id: u32) -> Option<usize> {
        self.pending.binary_search_by(|record| record.withdrawal_id.cmp(&withdrawal_id)).ok()
    }

    /// Sums every outstanding pending-withdrawal record for `key`, via
    /// `checked_add` (Invariant I2). A pending withdrawal has already been
    /// *debited* from `ledger` (so it does not appear in `ledger_balance`),
    /// but it can still be *restored* back onto this same key later (on a
    /// callback with `status_code != 0`) — so this is the amount a future
    /// DEPOSIT's overflow precheck must also account for, not merely the
    /// ledger's own current (already-reduced) balance (Gate 2a round 2 item
    /// 2; SPEC.md §7 Invariant I2).
    pub fn pending_amount_for_key(&self, key: &LedgerKey) -> Result<u128, ()> {
        let mut total: u128 = 0;
        for record in &self.pending {
            if record.depositor_dapp_id == key.dapp_id
                && record.depositor_account_id == key.account_id
                && record.currency_tag == key.currency_tag
                && record.currency_id == key.currency_id
            {
                total = total.checked_add(record.amount).ok_or(())?;
            }
        }
        Ok(total)
    }

    pub fn pending_count_for_depositor(&self, dapp_id: &[u8; 32], account_id: &[u8; 32]) -> usize {
        self.pending
            .iter()
            .filter(|record| {
                &record.depositor_dapp_id == dapp_id && &record.depositor_account_id == account_id
            })
            .count()
    }

    /// Inserts a new pending record, kept sorted ascending by
    /// `withdrawal_id`. Callers must have already allocated a genuinely
    /// fresh, never-reused id (Invariant I12).
    pub fn insert_pending(&mut self, record: PendingWithdrawal) {
        let index = self
            .pending
            .binary_search_by(|existing| existing.withdrawal_id.cmp(&record.withdrawal_id))
            .unwrap_or_else(|insert_at| insert_at);
        self.pending.insert(index, record);
    }

    // Gate 3e TASK F2: same reasoning as `pending_index`, above.
    #[cfg(test)]
    pub fn remove_pending(&mut self, index: usize) -> PendingWithdrawal {
        self.pending.remove(index)
    }
}

// Gate 3e TASK F2: the callback fast path. Dispatch (`lib.rs`) routes
// every callback here directly from the raw state bytes, BEFORE any
// full `VaultState::parse` — `is_callback()` is a property of the wire
// message alone (SPEC.md §2's own header-is-separate-from-ledger
// layout; `avm_contract_sdk::InternalMessage::is_callback`), so no
// state access is needed to make that routing decision safely.
//
// What this validates — identical in content and error code to
// `parse_with_reason`: the header in full, the exact total length via
// checked arithmetic, and the ENTIRE pending section (per-record
// shape/nonzero-amount/strict-order, the global pending cap,
// `next_withdrawal_id` monotonicity). Any failure here returns
// `ERR_INVALID_STATE`, exactly as `parse()` would.
//
// What this deliberately does NOT validate: the ledger section's own
// content (currency shape, nonzero amounts, sort order) — only its
// declared byte length (already covered by the total-length check).
// Measured directly (Gate 3d TASK M5, Gate 3e TASK F1): decoding and
// validating 252 ledger rows on every callback was the single largest
// cost in the old, full-`parse()`-based callback path, and the removed
// per-depositor check's own real cost (measured, not estimated) already
// exceeded the entire measured callback headroom at that scale — this
// fast path exists specifically to buy back headroom without
// reinstating that check or shrinking either cap. SPEC.md §1 documents
// the accepted consequence: a callback no longer blocks on a corrupted
// ledger row; the very next `DEPOSIT`/`WITHDRAW` still goes through the
// full `parse()` and will catch it there, returning `ERR_INVALID_STATE`.
//
// For Settle (`status_code == 0`): the ledger bytes are copied through
// verbatim — never decoded, never re-validated, never re-encoded.
// For Restore (`status_code != 0`): the one row that needs updating is
// found by binary search over the raw 85-byte records, using a
// comparator that DECODES each candidate's key fields with the exact
// same field-by-field ordering `LedgerKey`'s own derived `Ord` uses
// (`fast_compare_ledger_row`, below) — raw little-endian byte
// comparison would disagree with that ordering for some currency ids
// (e.g. 256 vs 1) and was deliberately avoided. Restore still bypasses
// `MAX_LEDGER_ENTRIES` entirely on insert, exactly as `credit_ledger`
// already does on the slow path (SPEC.md §1/§2.4, Invariant I11).
impl VaultState {
    pub(crate) fn fast_callback(
        bytes: &[u8],
        withdrawal_id: u32,
        status_code: u32,
        caller_dapp_id: [u8; 32],
        caller_account_id: [u8; 32],
    ) -> Result<Vec<u8>, u32> {
        let magic = bytes.get(0..4).ok_or(ERR_INVALID_STATE)?;
        if magic != MAGIC {
            return Err(ERR_INVALID_STATE);
        }
        let version = *bytes.get(4).ok_or(ERR_INVALID_STATE)?;
        if version != STATE_VERSION {
            return Err(ERR_INVALID_STATE);
        }
        // owner_pubkey / pending_owner_pubkey: existence only matters
        // here (same as `parse_with_reason` — their *content* is never
        // validated by either path, only that the bytes exist).
        bytes.get(5..37).ok_or(ERR_INVALID_STATE)?;
        bytes.get(37..69).ok_or(ERR_INVALID_STATE)?;
        let flags = *bytes.get(69).ok_or(ERR_INVALID_STATE)?;
        if flags & !HEADER_FLAG_DEPOSITS_PAUSED != 0 {
            return Err(ERR_INVALID_STATE);
        }
        let reserved = bytes.get(70..74).ok_or(ERR_INVALID_STATE)?;
        if reserved.iter().any(|byte| *byte != 0) {
            return Err(ERR_INVALID_STATE);
        }
        let next_withdrawal_id = read_u32(bytes, 78).map_err(|_| ERR_INVALID_STATE)?;
        let ledger_count = read_u32(bytes, 82).map_err(|_| ERR_INVALID_STATE)? as usize;
        let pending_count = read_u32(bytes, 86).map_err(|_| ERR_INVALID_STATE)? as usize;
        let build_flags = *bytes.get(90).ok_or(ERR_INVALID_STATE)?;
        if build_flags & !1 != 0 {
            return Err(ERR_INVALID_STATE);
        }

        let ledger_bytes_len =
            ledger_count.checked_mul(LEDGER_ENTRY_LEN).ok_or(ERR_INVALID_STATE)?;
        let pending_bytes_len =
            pending_count.checked_mul(PENDING_RECORD_LEN).ok_or(ERR_INVALID_STATE)?;
        let ledger_start = HEADER_LEN;
        let ledger_end = ledger_start.checked_add(ledger_bytes_len).ok_or(ERR_INVALID_STATE)?;
        let pending_end = ledger_end.checked_add(pending_bytes_len).ok_or(ERR_INVALID_STATE)?;
        if pending_end != bytes.len() {
            return Err(ERR_INVALID_STATE);
        }
        let ledger_bytes = bytes.get(ledger_start..ledger_end).ok_or(ERR_INVALID_STATE)?;
        let pending_bytes = bytes.get(ledger_end..pending_end).ok_or(ERR_INVALID_STATE)?;

        // Pending section: fully validated (same checks as `parse()`),
        // locating the target record's byte range inline during the
        // same single pass — no separate decode/allocate step.
        struct PendingTarget {
            start: usize,
            end: usize,
            depositor_dapp_id: [u8; 32],
            depositor_account_id: [u8; 32],
            currency_tag: u8,
            currency_id: u32,
            amount: u128,
        }
        let mut previous_id: Option<u32> = None;
        let mut target: Option<PendingTarget> = None;
        for (row_index, chunk) in pending_bytes.chunks_exact(PENDING_RECORD_LEN).enumerate() {
            let wid = u32::from_le_bytes(
                chunk
                    .get(0..4)
                    .ok_or(ERR_INVALID_STATE)?
                    .try_into()
                    .map_err(|_| ERR_INVALID_STATE)?,
            );
            let dapp_id: [u8; 32] = chunk
                .get(4..36)
                .ok_or(ERR_INVALID_STATE)?
                .try_into()
                .map_err(|_| ERR_INVALID_STATE)?;
            let account_id: [u8; 32] = chunk
                .get(36..68)
                .ok_or(ERR_INVALID_STATE)?
                .try_into()
                .map_err(|_| ERR_INVALID_STATE)?;
            let tag = *chunk.get(68).ok_or(ERR_INVALID_STATE)?;
            let cid = u32::from_le_bytes(
                chunk
                    .get(69..73)
                    .ok_or(ERR_INVALID_STATE)?
                    .try_into()
                    .map_err(|_| ERR_INVALID_STATE)?,
            );
            let amount = u128::from_le_bytes(
                chunk
                    .get(73..89)
                    .ok_or(ERR_INVALID_STATE)?
                    .try_into()
                    .map_err(|_| ERR_INVALID_STATE)?,
            );
            validate_currency_shape(tag, cid).map_err(|_| ERR_INVALID_STATE)?;
            if amount == 0 {
                return Err(ERR_INVALID_STATE);
            }
            if let Some(prev) = previous_id {
                if wid <= prev {
                    return Err(ERR_INVALID_STATE);
                }
            }
            previous_id = Some(wid);
            if wid == withdrawal_id {
                let start = row_index * PENDING_RECORD_LEN;
                target = Some(PendingTarget {
                    start,
                    end: start + PENDING_RECORD_LEN,
                    depositor_dapp_id: dapp_id,
                    depositor_account_id: account_id,
                    currency_tag: tag,
                    currency_id: cid,
                    amount,
                });
            }
        }
        if pending_count > MAX_PENDING_WITHDRAWALS {
            return Err(ERR_INVALID_STATE);
        }
        if let Some(last) = previous_id {
            if next_withdrawal_id <= last {
                return Err(ERR_INVALID_STATE);
            }
        }

        let Some(PendingTarget {
            start: p_start,
            end: p_end,
            depositor_dapp_id: rec_dapp,
            depositor_account_id: rec_account,
            currency_tag: rec_tag,
            currency_id: rec_cid,
            amount: rec_amount,
        }) = target
        else {
            return Err(ERR_WITHDRAWAL_NOT_FOUND);
        };

        // Invariant I4b, same code (`ERR_INVALID_CALLBACK`) and same
        // position (after the pending lookup, matching `handle_callback`'s
        // own existing order) as the slow path.
        if rec_dapp != caller_dapp_id || rec_account != caller_account_id {
            return Err(ERR_INVALID_CALLBACK);
        }

        let mut new_pending = Vec::with_capacity(pending_bytes.len() - PENDING_RECORD_LEN);
        new_pending.extend_from_slice(pending_bytes.get(..p_start).ok_or(ERR_INVALID_STATE)?);
        new_pending.extend_from_slice(pending_bytes.get(p_end..).ok_or(ERR_INVALID_STATE)?);

        let (new_ledger_bytes, new_ledger_count): (Vec<u8>, usize) = if status_code == 0 {
            // Settle: the ledger is never touched, copied through
            // verbatim, byte-for-byte — never decoded, never
            // re-validated.
            (ledger_bytes.to_vec(), ledger_count)
        } else {
            // Restore bypasses MAX_LEDGER_ENTRIES entirely (same as the
            // slow path's `credit_ledger`) — no cap check on insert.
            let row_count = ledger_bytes.len() / LEDGER_ENTRY_LEN;
            let mut lo = 0usize;
            let mut hi = row_count;
            let mut found: Option<usize> = None;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let row_start = mid.checked_mul(LEDGER_ENTRY_LEN).ok_or(ERR_INVALID_STATE)?;
                let row_end = row_start.checked_add(LEDGER_ENTRY_LEN).ok_or(ERR_INVALID_STATE)?;
                let row = ledger_bytes.get(row_start..row_end).ok_or(ERR_INVALID_STATE)?;
                match fast_compare_ledger_row(row, &rec_dapp, &rec_account, rec_tag, rec_cid) {
                    std::cmp::Ordering::Equal => {
                        found = Some(mid);
                        break;
                    }
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
                }
            }
            match found {
                Some(idx) => {
                    let mut buf = ledger_bytes.to_vec();
                    let amount_off = idx * LEDGER_ENTRY_LEN + 69;
                    let existing = u128::from_le_bytes(
                        buf.get(amount_off..amount_off + 16)
                            .ok_or(ERR_INVALID_STATE)?
                            .try_into()
                            .map_err(|_| ERR_INVALID_STATE)?,
                    );
                    let updated =
                        existing.checked_add(rec_amount).ok_or(ERR_LEDGER_AMOUNT_OVERFLOW)?;
                    let slot = buf.get_mut(amount_off..amount_off + 16).ok_or(ERR_INVALID_STATE)?;
                    slot.copy_from_slice(&updated.to_le_bytes());
                    (buf, ledger_count)
                }
                None => {
                    // Not found: insert a brand-new row at the sorted
                    // position `lo` landed on — identical semantics to
                    // `credit_ledger`'s own `Err(index) => insert` arm.
                    let insert_at = lo;
                    let split = insert_at.checked_mul(LEDGER_ENTRY_LEN).ok_or(ERR_INVALID_STATE)?;
                    let mut buf = Vec::with_capacity(ledger_bytes.len() + LEDGER_ENTRY_LEN);
                    buf.extend_from_slice(ledger_bytes.get(..split).ok_or(ERR_INVALID_STATE)?);
                    buf.extend_from_slice(&rec_dapp);
                    buf.extend_from_slice(&rec_account);
                    buf.push(rec_tag);
                    buf.extend_from_slice(&rec_cid.to_le_bytes());
                    buf.extend_from_slice(&rec_amount.to_le_bytes());
                    buf.extend_from_slice(ledger_bytes.get(split..).ok_or(ERR_INVALID_STATE)?);
                    (buf, ledger_count + 1)
                }
            }
        };

        let new_pending_count = pending_count - 1;
        let mut out = Vec::with_capacity(HEADER_LEN + new_ledger_bytes.len() + new_pending.len());
        // magic .. next_withdrawal_id, unchanged.
        out.extend_from_slice(bytes.get(0..82).ok_or(ERR_INVALID_STATE)?);
        out.extend_from_slice(&(new_ledger_count as u32).to_le_bytes());
        out.extend_from_slice(&(new_pending_count as u32).to_le_bytes());
        out.push(build_flags);
        out.extend_from_slice(&new_ledger_bytes);
        out.extend_from_slice(&new_pending);
        Ok(out)
    }
}

/// Compares a raw 85-byte ledger row against a target key using the
/// SAME field-by-field ordering `LedgerKey`'s own derived `Ord` uses
/// (dapp_id, then account_id, then currency_tag, then currency_id) —
/// critically, `currency_id` is decoded to a `u32` and compared
/// NUMERICALLY, never compared as raw little-endian bytes: raw-byte
/// comparison would disagree with `LedgerKey`'s own ordering for any
/// pair of ids whose LE byte patterns are not already numerically
/// ordered (e.g. id 256, bytes `[00,01,00,00]`, vs id 1, bytes
/// `[01,00,00,00]`: lexicographic byte comparison puts 256 first;
/// numeric comparison puts 1 first — `LedgerKey` uses the latter).
fn fast_compare_ledger_row(
    row: &[u8],
    target_dapp: &[u8; 32],
    target_account: &[u8; 32],
    target_tag: u8,
    target_cid: u32,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let row_dapp = row.get(0..32).unwrap_or(&[]);
    match row_dapp.cmp(target_dapp.as_slice()) {
        Ordering::Equal => {}
        other => return other,
    }
    let row_account = row.get(32..64).unwrap_or(&[]);
    match row_account.cmp(target_account.as_slice()) {
        Ordering::Equal => {}
        other => return other,
    }
    let row_tag = row.get(64).copied().unwrap_or(0);
    match row_tag.cmp(&target_tag) {
        Ordering::Equal => {}
        other => return other,
    }
    let row_cid =
        row.get(65..69).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes).unwrap_or(0);
    row_cid.cmp(&target_cid)
}

fn validate_currency_shape(currency_tag: u8, currency_id: u32) -> Result<(), ()> {
    match currency_tag {
        CURRENCY_TAG_MAIN => {
            if currency_id != 0 {
                return Err(());
            }
        }
        CURRENCY_TAG_EXTRA => {
            if currency_id == SHELL_CURRENCY_ID {
                return Err(());
            }
        }
        _ => return Err(()),
    }
    Ok(())
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ParseFailureReason> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(ParseFailureReason::TruncatedHeader)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(tag: u8, id: u32) -> LedgerKey {
        LedgerKey { dapp_id: [1; 32], account_id: [2; 32], currency_tag: tag, currency_id: id }
    }

    #[test]
    fn round_trips_empty_state() {
        let state = VaultState::new_empty([9; 32], 0);
        let bytes = state.serialize();
        let parsed = VaultState::parse(&bytes).expect("parse");
        assert_eq!(parsed.owner_pubkey, [9; 32]);
        assert_eq!(parsed.ledger.len(), 0);
        assert_eq!(parsed.pending.len(), 0);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = VaultState::new_empty([0; 32], 0).serialize();
        bytes[0] = b'X';
        assert_eq!(VaultState::parse(&bytes).unwrap_err(), ERR_INVALID_STATE);
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = VaultState::new_empty([0; 32], 0).serialize();
        bytes.push(0);
        assert_eq!(VaultState::parse(&bytes).unwrap_err(), ERR_INVALID_STATE);
    }

    #[test]
    fn rejects_shell_ledger_row() {
        let mut state = VaultState::new_empty([0; 32], 0);
        state.ledger.push(LedgerEntry { key: key(CURRENCY_TAG_EXTRA, 2), amount: 1 });
        let bytes = state.serialize();
        assert_eq!(VaultState::parse(&bytes).unwrap_err(), ERR_INVALID_STATE);
    }

    #[test]
    fn rejects_zero_amount_row() {
        let mut state = VaultState::new_empty([0; 32], 0);
        state.ledger.push(LedgerEntry { key: key(CURRENCY_TAG_EXTRA, 7), amount: 0 });
        let bytes = state.serialize();
        assert_eq!(VaultState::parse(&bytes).unwrap_err(), ERR_INVALID_STATE);
    }

    #[test]
    fn rejects_unsorted_ledger() {
        let mut state = VaultState::new_empty([0; 32], 0);
        state.ledger.push(LedgerEntry { key: key(CURRENCY_TAG_EXTRA, 9), amount: 1 });
        state.ledger.push(LedgerEntry { key: key(CURRENCY_TAG_EXTRA, 7), amount: 1 });
        let bytes = state.serialize();
        assert_eq!(VaultState::parse(&bytes).unwrap_err(), ERR_INVALID_STATE);
    }

    /// Mutation-testing gap: every existing round-trip test uses
    /// `build_flags == 0`; none ever set bit 0 (`USING_DEMO_OWNER_KEY`'s
    /// mirror), so a mutant deleting the `!` in `build_flags & !1 != 0`
    /// (which would then reject bit 0 instead of accepting it) had nothing
    /// to catch it.
    #[test]
    fn round_trips_build_flags_one() {
        let state = VaultState::new_empty([9; 32], 1);
        let bytes = state.serialize();
        let parsed = VaultState::parse(&bytes).expect("parse");
        assert_eq!(parsed.build_flags, 1);
    }

    /// Companion to the above: any reserved bit (here, bit 1) set must
    /// still be rejected — confirms the check does more than always pass.
    #[test]
    fn rejects_build_flags_with_reserved_bits_set() {
        let state = VaultState::new_empty([9; 32], 2);
        let bytes = state.serialize();
        assert_eq!(VaultState::parse(&bytes).unwrap_err(), ERR_INVALID_STATE);
    }

    /// Mutation-testing gap: `pending_amount_for_key`'s matching `&&` chain
    /// (dapp_id/account_id/currency_tag/currency_id) had only ever been
    /// exercised with a single pending record in its fixtures, so `&&` vs
    /// `||` on any of those four comparisons was indistinguishable. This
    /// adds a second record that shares the SAME currency_tag/currency_id
    /// but a DIFFERENT depositor, confirming it is correctly excluded from
    /// the first depositor's total.
    #[test]
    fn pending_amount_for_key_excludes_records_with_partial_key_mismatch() {
        let mut state = VaultState::new_empty([0; 32], 0);
        let target_key = key(CURRENCY_TAG_EXTRA, 7);
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 1,
            depositor_dapp_id: target_key.dapp_id,
            depositor_account_id: target_key.account_id,
            currency_tag: target_key.currency_tag,
            currency_id: target_key.currency_id,
            amount: 100,
        });
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 2,
            depositor_dapp_id: target_key.dapp_id,
            depositor_account_id: [99; 32],
            currency_tag: target_key.currency_tag,
            currency_id: target_key.currency_id,
            amount: 9_999,
        });
        assert_eq!(state.pending_amount_for_key(&target_key).expect("sum"), 100);
    }

    /// "THEN Phase 3" item 3: a stored state blob claiming more pending
    /// records than `MAX_PENDING_WITHDRAWALS` is rejected on parse, not
    /// merely prevented by `insert_pending`'s own caller (`handle_withdraw`)
    /// at write time — a corrupted or hand-crafted blob bypassing that
    /// caller must still be caught here.
    #[test]
    fn rejects_pending_count_over_global_cap() {
        let mut state = VaultState::new_empty([0; 32], 0);
        for id in 1..=(MAX_PENDING_WITHDRAWALS as u32 + 1) {
            state.pending.push(PendingWithdrawal {
                withdrawal_id: id,
                depositor_dapp_id: [1; 32],
                depositor_account_id: [id as u8; 32],
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 1,
            });
        }
        state.next_withdrawal_id = MAX_PENDING_WITHDRAWALS as u32 + 2;
        let bytes = state.serialize();
        assert_eq!(VaultState::parse(&bytes).unwrap_err(), ERR_INVALID_STATE);
    }

    /// Gate 2b round 3 TASK H: `parse()` used to also reject a stored
    /// blob claiming more pending records for one depositor than
    /// `MAX_PENDING_PER_DEPOSITOR` — an O(n²) check, root-caused this
    /// round (real-stand A/B test, `docs/PLATFORM_FINDINGS.md` P20) as
    /// the actual cause of P18/P19's silently-stranded-withdrawal failure
    /// at worst-case state sizes, and removed. This test now documents
    /// the intentional new behavior: `parse()` no longer rejects this
    /// shape — the invariant it used to enforce here is still fully
    /// enforced on every path that can insert a pending record
    /// (`handlers::handle_withdraw`'s own real-time
    /// `pending_count_for_depositor` check), so a state exceeding the
    /// per-depositor cap can now only arise from this contract's own
    /// writer logic already having a bug, which this removed check could
    /// never have prevented either way (it only re-validated the output
    /// of the same logic that would have had to be wrong to produce it).
    #[test]
    fn parse_no_longer_enforces_per_depositor_cap() {
        let mut state = VaultState::new_empty([0; 32], 0);
        for id in 1..=(MAX_PENDING_PER_DEPOSITOR as u32 + 1) {
            state.pending.push(PendingWithdrawal {
                withdrawal_id: id,
                depositor_dapp_id: [1; 32],
                depositor_account_id: [2; 32],
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 1,
            });
        }
        state.next_withdrawal_id = MAX_PENDING_PER_DEPOSITOR as u32 + 2;
        let bytes = state.serialize();
        let parsed = VaultState::parse(&bytes).expect("no longer rejected");
        assert_eq!(parsed.pending.len(), MAX_PENDING_PER_DEPOSITOR + 1);
    }

    /// "THEN Phase 3" item 3: Invariant I12 — `next_withdrawal_id` must be
    /// strictly greater than every currently-outstanding pending id (not
    /// merely different from it), or a future allocation could collide
    /// with, or already be behind, a still-outstanding withdrawal.
    #[test]
    fn rejects_next_withdrawal_id_not_greater_than_any_pending_id() {
        let mut state = VaultState::new_empty([0; 32], 0);
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 5,
            depositor_dapp_id: [1; 32],
            depositor_account_id: [2; 32],
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 7,
            amount: 1,
        });
        state.next_withdrawal_id = 5; // equal to the outstanding id: must be rejected
        let bytes = state.serialize();
        assert_eq!(VaultState::parse(&bytes).unwrap_err(), ERR_INVALID_STATE);
    }

    #[test]
    fn credit_and_debit_round_trip() {
        let mut state = VaultState::new_empty([0; 32], 0);
        let k = key(CURRENCY_TAG_EXTRA, 7);
        assert_eq!(state.credit_ledger(k, 100), Ok(true));
        assert_eq!(state.ledger_balance(&k), 100);
        assert_eq!(state.credit_ledger(k, 50), Ok(false));
        assert_eq!(state.ledger_balance(&k), 150);
        state.debit_ledger(&k, 150).expect("debit");
        assert_eq!(state.ledger_balance(&k), 0);
        assert!(state.ledger.is_empty());
    }
}
