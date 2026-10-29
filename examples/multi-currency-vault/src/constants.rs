//! All constants for the multi-currency vault, in one module per SPEC.md.
//!
//! Method ids: `DEPOSIT_METHOD_ID`/`WITHDRAW_METHOD_ID` follow SPEC.md §3's
//! `NORMAL_METHOD_TAG<<32|N` formula exactly (both `Internal`-caller-only,
//! so the wire `method_id` is a literal `u64` another contract's own
//! `OutboundIntent` chooses directly — no ABI name lookup is ever involved).
//! The four externally-invoked methods use the real host-computed values
//! discovered via `spike/methodid-probe/` (never committed) instead of that
//! formula, per SPEC.md §3's erratum and `REVIEW.md` entries 1/5 — confirmed
//! deterministic and dependent only on the declared ABI method name string,
//! not on this contract's own ACAB/descriptor bytes. No SDK-side helper to
//! compute this hash exists anywhere in this checkout (searched exhaustively
//! — `REVIEW.md` entry 5); these four values are pinned against the
//! installed release **`avm-dev-06e96ef71006-sdk-0748f4f0767a`**
//! (`ackinacki_commit 06e96ef71006cca020575cbf35e6b82cc82a324e`, `sdk_commit
//! 0748f4f0767a26da8bc363bcb6b5585539d6970e`, `guest_version 0.1.1`) and
//! must be re-verified (same technique: `spike/methodid-probe/`) if that
//! installed release ever changes — see the `pinned_external_method_ids`
//! test below and `docs/PLATFORM_FINDINGS.md`'s "Phase 2 implementation"
//! addendum (P10/P11) for the full discovery record.

/// Tag for this contract's own internal-only method ids (`DEPOSIT`/`WITHDRAW`).
const NORMAL_METHOD_TAG: u64 = 0x4D43_5654; // ASCII "MCVT"
/// Tag for withdrawal-callback correlation method ids.
const CALLBACK_MAGIC_TAG: u64 = 0x5744_4C43; // ASCII "WDLC"

pub const DEPOSIT_METHOD_ID: u64 = (NORMAL_METHOD_TAG << 32) | 1;
pub const WITHDRAW_METHOD_ID: u64 = (NORMAL_METHOD_TAG << 32) | 2;
// Method ids 3, 4, and 8 are permanently retired (TRANSFER, the time-based
// reclaim method, and the permissionless VERSION probe respectively — none
// exist in this contract, per SPEC.md §1/§3) and are never reassigned.

/// Discovered host-computed `method_id` for ABI name `"vault.topup"`.
pub const TOPUP_METHOD_ID: u64 = 0xc0aa_c68a_6cd1_7e18;
/// Discovered host-computed `method_id` for ABI name `"vault.propose_owner"`.
pub const PROPOSE_OWNER_METHOD_ID: u64 = 0xfc41_83aa_b166_1acf;
/// Discovered host-computed `method_id` for ABI name `"vault.accept_owner"`.
pub const ACCEPT_OWNER_METHOD_ID: u64 = 0x8fc0_3d2a_5e71_b6df;
/// Discovered host-computed `method_id` for ABI name `"vault.set_pause"`.
pub const SET_PAUSE_METHOD_ID: u64 = 0x33c0_aeb4_3f53_60d5;

/// This withdrawal's outbound payout is delivered with this as its own
/// `method_id` on the way back, letting this contract recover which pending
/// withdrawal a callback belongs to purely from that value (SPEC.md §3).
pub const fn withdrawal_callback_method_id(withdrawal_id: u32) -> u64 {
    (CALLBACK_MAGIC_TAG << 32) | withdrawal_id as u64
}

/// True iff `method_id`'s upper 32 bits mark it as one of this contract's
/// own requested withdrawal callbacks (Gate 2a round 2 item 5) — every
/// callback this contract ever requests uses `withdrawal_callback_method_id`
/// as its `handler_method_id`, which always sets this same tag; a genuine
/// callback (the wire-level flag this contract dispatches on, never
/// `method_id` matching alone — Invariant I4c) whose `method_id` somehow
/// does not carry this tag is rejected rather than having its lower 32 bits
/// blindly treated as a withdrawal id.
pub const fn method_id_is_withdrawal_callback(method_id: u64) -> bool {
    (method_id >> 32) == CALLBACK_MAGIC_TAG
}

/// Recovers the withdrawal id a genuine callback's own `method_id` encodes.
/// Callers must have already checked `input.message.is_callback()`; a
/// non-matching lower-32-bits pattern cannot occur for a genuine callback
/// to this contract, since every callback this contract ever requests uses
/// `withdrawal_callback_method_id` as its `handler_method_id` — but even if
/// it somehow did, the resulting id simply fails to find a pending record
/// and is rejected as `WITHDRAWAL_NOT_FOUND`, never panicking.
pub const fn withdrawal_id_from_callback_method_id(method_id: u64) -> u32 {
    (method_id & 0xFFFF_FFFF) as u32
}

/// SHELL is the AVM protocol currency, exposed at `extra[currency_id = 2]`
/// (`docs/fuel-and-fees.md`; confirmed throughout `docs/PLATFORM_FINDINGS.md`).
pub const SHELL_CURRENCY_ID: u32 = 2;

/// Currency tag for a ledger/pending record's `Main` (`value_main`) slot.
pub const CURRENCY_TAG_MAIN: u8 = 0;
/// Currency tag for a ledger/pending record's `Extra(currency_id)` slot.
pub const CURRENCY_TAG_EXTRA: u8 = 1;

/// Bounds genuinely new deposit rows only — restores bypass this cap
/// entirely (SPEC.md §1/§2.4). Reduced from 300 (Gate 2b round 2, TASK A)
/// as a stated margin of caution while root cause was still unconfirmed;
/// Gate 2b round 3 TASK F/G confirmed by real-stand A/B test that ledger
/// row count was never the actual driver of P18/P19's stuck-withdrawal
/// failure (`docs/PLATFORM_FINDINGS.md` P20/P21) — `MAX_PENDING_WITHDRAWALS`
/// was. Left at 220 rather than restored toward 300: this round's
/// instructions only called for restoring the pending cap if the evidence
/// supported it, and a smaller worst-case state size helps the separate,
/// newly-measured `MIN_OPERATIONAL_SHELL` margin problem (P21's TASK G3)
/// rather than working against it.
pub const MAX_LEDGER_ENTRIES: usize = 220;
/// Global cap on simultaneously outstanding pending withdrawals, shared
/// across every depositor (SPEC.md §2.4). Reduced to 12 at Gate 2b round 2
/// (TASK A) as an empirical margin of caution while root cause was
/// unconfirmed; **restored to 32 at Gate 2b round 3 (TASK F/G)** after the
/// real root cause — `state.rs` `parse()`'s own O(n²) per-depositor
/// check — was found via a real-stand A/B test (remove exactly that loop,
/// nothing else, and all 8 previously-stuck configurations resolve;
/// `docs/PLATFORM_FINDINGS.md` P20) and removed (see that check's own
/// removal comment in `state.rs`). With the check removed, pending = 32 —
/// this contract's own architectural ceiling (`MAX_PENDING_PER_DEPOSITOR`
/// × up to 8 depositor groups) — was directly verified to resolve cleanly
/// at both 220 and 300 ledger rows, for both Settle and Restore, with no
/// failing point found anywhere in the range tested (P21's TASK G2). No
/// margin caveat applies here the way it did for the 12-value: that value
/// was deliberately conservative *because* root cause was unconfirmed;
/// this value is justified by removing the actual cause, not by sitting a
/// safe distance from an unexplained failure.
pub const MAX_PENDING_WITHDRAWALS: usize = 32;
/// Per-depositor cap on simultaneously outstanding pending withdrawals
/// (SPEC.md §2.4).
pub const MAX_PENDING_PER_DEPOSITOR: usize = 4;
/// Heuristic SHELL margin `WITHDRAW_METHOD_ID` requires the vault's own
/// real balance to clear before it does anything else (Invariant I17).
/// **Re-derived again at Phase 4 (fresh-context adversarial review,
/// finding 1)**: the true byte-densest state this contract's own rules
/// can reach is **252 ledger rows together with 32 outstanding pending
/// withdrawals**, not 220 rows + 32 pending — a *partial* withdrawal
/// creates a pending record without removing its row (Invariant I9 only
/// removes a row a debit brings to exactly zero), and `handle_withdraw`
/// never checks `MAX_LEDGER_ENTRIES` (only the two pending caps), so all
/// 32 pending slots can be refilled on top of 252 still-intact rows once
/// 32 restores have bypassed the ledger cap. `91 + 252×85 + 32×89 =
/// 24,359` bytes (same 60-second margin-window policy, same
/// exactly-measured 1.000 SHELL/byte/second rate) —
/// `24,359 × 1.000 × 60 = 1,461,540`, rounded up to `1,500,000`. Proven
/// reachable by `handlers.rs`'s own
/// `true_worst_case_state_is_252_rows_plus_32_pending` test, which failed
/// against the prior `1,300,000` value before this change. Gate 2b round
/// 3's `21,639`/`1,300,000` figures are now historical, superseded by
/// this derivation. **Gate 2b round 3 TASK G3 found the 60-second framing
/// itself optimistic in practice** (tested against the then-current,
/// smaller value): a vault funded with exactly that value, or even 3× it,
/// did not survive real setup overhead plus 60 seconds of elapsed time
/// before a withdrawal attempt in real-stand testing
/// (`docs/PLATFORM_FINDINGS.md` P21) — this constant is a
/// withdrawal-admission *gate*, never an operating balance a real
/// deployment should treat as sufficient cushion; see SPEC.md §1 for the
/// full caveat.
pub const MIN_OPERATIONAL_SHELL: u128 = 1_500_000;

// --- State layout byte offsets/lengths (SPEC.md §2) ---

/// Fixed header length (SPEC.md §2.1).
pub const HEADER_LEN: usize = 91;
/// Exact length of one ledger entry record (SPEC.md §2.2).
pub const LEDGER_ENTRY_LEN: usize = 85;
/// Exact length of one pending-withdrawal record (SPEC.md §2.3).
pub const PENDING_RECORD_LEN: usize = 89;

pub const MAGIC: [u8; 4] = *b"MCVT";
pub const STATE_VERSION: u8 = 1;

/// Build-flags bit 0: this build is using the demo owner fallback key.
pub const BUILD_FLAG_DEMO_OWNER_KEY: u8 = 1;
/// Header flags bit 0: deposits paused.
pub const HEADER_FLAG_DEPOSITS_PAUSED: u8 = 1;

// --- Error codes (SPEC.md §6, all >= 1000 per the SDK's reserved range) ---

pub const ERR_UNRECOGNIZED_METHOD: u32 = 1002;
pub const ERR_INVALID_CALLER: u32 = 1500;
pub const ERR_INVALID_BODY: u32 = 1501;
pub const ERR_INVALID_STATE: u32 = 1502;
pub const ERR_INVALID_CURRENCY_SELECTOR: u32 = 1503;
pub const ERR_INVALID_AMOUNT: u32 = 1504;
pub const ERR_INSUFFICIENT_BALANCE: u32 = 1505;
pub const ERR_DEPOSITS_PAUSED: u32 = 1506;
pub const ERR_LEDGER_CAPACITY_EXCEEDED: u32 = 1508;
pub const ERR_PENDING_CAPACITY_EXCEEDED: u32 = 1509;
pub const ERR_WITHDRAWAL_NOT_FOUND: u32 = 1510;
pub const ERR_INVALID_CALLBACK: u32 = 1513;
pub const ERR_UNAUTHORIZED_OWNER: u32 = 1514;
pub const ERR_UNAUTHORIZED_PENDING_OWNER: u32 = 1515;
pub const ERR_INVALID_PAUSE_FLAGS: u32 = 1516;
pub const ERR_UPGRADE_NOT_SUPPORTED: u32 = 1518;
pub const ERR_SHELL_NOT_CUSTODIED: u32 = 1519;
pub const ERR_CROSS_DAPP_CALLER_FORBIDDEN: u32 = 1520;
pub const ERR_WITHDRAWAL_ID_EXHAUSTED: u32 = 1522;
pub const ERR_STALE_ADMIN_SEQUENCE: u32 = 1524;
pub const ERR_TOPUP_REQUIRES_SHELL_ONLY: u32 = 1526;
pub const ERR_ADMIN_SEQUENCE_EXHAUSTED: u32 = 1527;
pub const ERR_VAULT_UNDERFUNDED: u32 = 1528;
pub const ERR_LEDGER_AMOUNT_OVERFLOW: u32 = 1529;
pub const ERR_EVENT_ENCODING_FAILED: u32 = 1530;

/// Fixed, publicly-known demo fallback owner pubkey, used only when
/// `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` is unset at compile time. An arbitrary
/// placeholder public key, in the same spirit as `contract-a-v1`'s own
/// `FIXTURE_PACKAGE_OWNER_PUBKEY` — a public key is not a secret, and this
/// one's corresponding private key, if any exists at all, is explicitly
/// unverified and not confirmed safe (SPEC.md §1/§10). Never use this in a
/// real deployment.
const FIXTURE_VAULT_OWNER_PUBKEY: [u8; 32] = [
    0x2e, 0x7d, 0x41, 0xaf, 0x93, 0xc6, 0x5a, 0x18, 0x0b, 0xe4, 0xf7, 0x2c, 0x9d, 0x55, 0xa1, 0x3e,
    0x66, 0x4b, 0x8f, 0x02, 0xd9, 0x1c, 0x5e, 0xb0, 0x7a, 0x3d, 0xc8, 0x44, 0x1f, 0xe2, 0x6b, 0x90,
];

/// The compile-time-resolved owner pubkey: `AVM_EXAMPLE_VAULT_OWNER_PUBKEY`
/// (64 lowercase hex digits) if set at build time, following
/// `contract-a-v1`'s own `option_env!` pattern; `FIXTURE_VAULT_OWNER_PUBKEY`
/// otherwise. This must build with the stock `avm-contract-build build
/// --manifest-path ...` (no extra CLI flags accepted, confirmed
/// `docs/PLATFORM_FINDINGS.md` A10), so a plain environment variable is used
/// rather than a Cargo feature, which that tool has no way to pass through.
pub const VAULT_OWNER_PUBKEY: [u8; 32] = resolve_owner_pubkey();
/// Mirrors whether the build above fell back to the demo key — written into
/// `build_flags` bit 0 at `init()` (SPEC.md §2.1).
pub const USING_DEMO_OWNER_KEY: bool = option_env!("AVM_EXAMPLE_VAULT_OWNER_PUBKEY").is_none();

// Gate 2b round 2 TASK B: the hex-decoding logic itself is factored out
// into `decode_hex_pubkey`, a plain `const fn` taking `&str` — callable
// (and tested) at ordinary runtime with a literal argument, not only
// reachable via `option_env!` at compile time. `resolve_owner_pubkey`
// stays a thin `option_env!` dispatch with no decoding logic of its own,
// so a nibble/offset bug in the decoder is now directly testable rather
// than exercised only by a build that happens to set the env var.
const fn resolve_owner_pubkey() -> [u8; 32] {
    let Some(value) = option_env!("AVM_EXAMPLE_VAULT_OWNER_PUBKEY") else {
        return FIXTURE_VAULT_OWNER_PUBKEY;
    };
    decode_hex_pubkey(value)
}

// `const fn` so it remains usable to initialize `VAULT_OWNER_PUBKEY` at
// compile time via `resolve_owner_pubkey` above, but — unlike when this
// logic lived inline in `resolve_owner_pubkey` — it is also directly
// callable from an ordinary `#[test]` with a literal `&str`, at runtime,
// where a defect shows up as a failing assertion instead of only as a
// build panic nobody runs in CI unless `AVM_EXAMPLE_VAULT_OWNER_PUBKEY`
// happens to be set. An out-of-bounds index here (which cannot actually
// occur: the preceding `assert!` already fixes `hex.len() == 64`, and
// the loop never reaches `index * 2 + 1 >= 64`) would panic at whichever
// context this runs in — a build failure at compile time via
// `resolve_owner_pubkey`, an ordinary runtime panic if called directly —
// never silently on any message/state input this contract processes.
// `const fn` bodies also cannot use iterator-based bounds-checked
// alternatives like `.get()` in a `while` loop the way runtime code can,
// so the direct indexing here is the idiomatic const-context form,
// matching `contract-a-v1`'s own identical `package_owner_pubkey`
// pattern.
#[allow(clippy::indexing_slicing)]
const fn decode_hex_pubkey(hex: &str) -> [u8; 32] {
    let bytes = hex.as_bytes();
    assert!(bytes.len() == 64, "AVM_EXAMPLE_VAULT_OWNER_PUBKEY must be 64 lowercase hex digits");
    let mut key = [0u8; 32];
    let mut index = 0;
    while index < key.len() {
        key[index] = (hex_digit(bytes[index * 2]) << 4) | hex_digit(bytes[index * 2 + 1]);
        index += 1;
    }
    key
}

// `hex_digit` is a `const fn`, reachable from `decode_hex_pubkey` above
// (itself reachable both from `resolve_owner_pubkey`, at compile time via
// `option_env!`, and directly from this crate's own `#[test]`s, at
// runtime, since Gate 2b round 2 TASK B factored the decoder out of
// `resolve_owner_pubkey`). On the compile-time path (building with
// `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` set to malformed hex), a `panic!`
// reached here is a *build failure*
// (`cargo build`/`avm-contract-build` refusing to compile), not a
// runtime panic on any message/state input this contract ever
// processes — so the crate-level `deny(clippy::panic)` (`lib.rs`) is
// explicitly allowed here, with this comment as the justification
// clippy itself cannot verify. On the test path, the same `panic!` is an
// ordinary, directly-observable test failure (see
// `decode_hex_pubkey_rejects_wrong_length` and
// `decode_hex_pubkey_rejects_non_hex_character` in the `tests` module
// below) — this is also the same pattern `contract-a-v1`'s own
// `hex_digit`/`package_owner_pubkey` already uses for the compile-time
// path.
#[allow(clippy::panic)]
const fn hex_digit(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => panic!("AVM_EXAMPLE_VAULT_OWNER_PUBKEY must be 64 lowercase hex digits"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the four externally-invoked method ids to the exact values
    /// `spike/methodid-probe/` discovered against installed release
    /// `avm-dev-06e96ef71006-sdk-0748f4f0767a` (`sdk_commit
    /// 0748f4f0767a26da8bc363bcb6b5585539d6970e`). This test is tautological
    /// against the constants above by construction — its purpose is to be
    /// the single, obviously-discoverable place that breaks loudly (via a
    /// failed `cargo test`, not a silent drift) if anyone ever edits these
    /// constants without re-running the discovery probe against whatever
    /// SDK release is installed at the time, and to carry the release
    /// citation `docs/PLATFORM_FINDINGS.md` and `REVIEW.md` both point back
    /// to. If the installed `avm-sdk` release ever changes, re-run
    /// `spike/methodid-probe/` first and update both this test and the
    /// constants above together, never one without the other.
    #[test]
    fn pinned_external_method_ids() {
        assert_eq!(TOPUP_METHOD_ID, 0xc0aa_c68a_6cd1_7e18, "ABI name \"vault.topup\"");
        assert_eq!(
            PROPOSE_OWNER_METHOD_ID, 0xfc41_83aa_b166_1acf,
            "ABI name \"vault.propose_owner\""
        );
        assert_eq!(
            ACCEPT_OWNER_METHOD_ID, 0x8fc0_3d2a_5e71_b6df,
            "ABI name \"vault.accept_owner\""
        );
        assert_eq!(SET_PAUSE_METHOD_ID, 0x33c0_aeb4_3f53_60d5, "ABI name \"vault.set_pause\"");
    }

    /// Pins `DEPOSIT_METHOD_ID`/`WITHDRAW_METHOD_ID` to the
    /// `NORMAL_METHOD_TAG<<32|N` formula exactly — these are genuinely
    /// formula-derived (not host-hash-derived; `Internal`-caller-only, so
    /// no ABI name lookup is ever involved), unlike the four above.
    #[test]
    fn pinned_internal_method_ids_match_formula() {
        assert_eq!(DEPOSIT_METHOD_ID, (0x4D43_5654u64 << 32) | 1);
        assert_eq!(WITHDRAW_METHOD_ID, (0x4D43_5654u64 << 32) | 2);
    }

    /// `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` is never set in a test build, so
    /// `resolve_owner_pubkey()`'s `option_env!` branch must take the `None`
    /// arm and resolve to `FIXTURE_VAULT_OWNER_PUBKEY` exactly. This pins
    /// that compile-time-resolved value directly: a mutant that changes
    /// which arm is taken, or what either arm returns, changes the
    /// compiled constant itself (this is `const fn`, evaluated once at
    /// compile time under these exact, fixed build conditions), which this
    /// assertion catches even though no test ever calls `resolve_owner_pubkey`
    /// at runtime.
    #[test]
    fn vault_owner_pubkey_resolves_to_fixture_when_env_var_unset() {
        assert_eq!(VAULT_OWNER_PUBKEY, FIXTURE_VAULT_OWNER_PUBKEY);
    }

    /// Gate 2b round 2 TASK B: `decode_hex_pubkey` is now a plain `const
    /// fn` taking `&str`, directly callable here with a literal — not
    /// reachable only through `resolve_owner_pubkey`'s `option_env!`
    /// branch, which no test build ever takes. A known 64-hex-digit
    /// vector, checked byte-by-byte: a nibble-order or byte-offset bug
    /// (e.g. swapping the two `hex_digit` calls, or indexing `bytes[index]`
    /// instead of `bytes[index * 2]`) would silently produce a *different*,
    /// still-well-formed 32-byte key rather than panicking — exactly the
    /// kind of defect that builds fine with a wrong key and was previously
    /// invisible to this test suite (see REVIEW.md's correction to entry
    /// 22's "surfaces as a build panic" claim, which was wrong for this
    /// specific class of bug).
    #[test]
    fn decode_hex_pubkey_known_vector_matches_every_byte() {
        // Gate 2b round 3 TASK I: every byte of this 32-byte vector is
        // distinct (0x00..=0x1f, sequential) rather than a repeating
        // 16-byte pattern — a repeating vector cannot catch a bug that
        // shuffles bytes by a multiple of the repeat period (e.g. an
        // index computed mod 16 instead of mod 32), since the shuffled
        // output would be byte-identical to the correct one. A
        // non-repeating vector catches any such transposition.
        let decoded =
            decode_hex_pubkey("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let expected: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        assert_eq!(decoded, expected);
    }

    /// A 63-character (one short) input must panic rather than silently
    /// decode a truncated or shifted key — the same `assert!` that, at
    /// compile time via `resolve_owner_pubkey`, would make this a build
    /// failure instead.
    #[test]
    #[should_panic(expected = "64 lowercase hex digits")]
    fn decode_hex_pubkey_rejects_wrong_length() {
        let _ =
            decode_hex_pubkey("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde");
    }

    /// A non-hex character (here, `'g'`) must panic via `hex_digit`'s own
    /// unreachable match arm, not silently decode to some arbitrary byte.
    #[test]
    #[should_panic(expected = "64 lowercase hex digits")]
    fn decode_hex_pubkey_rejects_non_hex_character() {
        let _ =
            decode_hex_pubkey("g123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    }
}
