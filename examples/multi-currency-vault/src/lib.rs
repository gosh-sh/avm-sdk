// `forbid` (not merely `deny`) on production code — this contract's own
// guest logic processing untrusted message/state bytes must never contain
// `unsafe`, full stop, with no local override possible anywhere in this
// crate. Relaxed to `deny` under `#[cfg(test)]` only (Gate 2a round 3 item
// 3b): implementing `std::alloc::GlobalAlloc` for the counting allocator
// `test_support::CountingAllocator` uses requires `unsafe impl`/`unsafe
// fn` by that trait's own definition in `std` — there is no safe way to
// satisfy it, and `forbid` cannot be locally overridden even for test-only
// code, so `deny` (which a scoped, justified `#[allow(unsafe_code)]` CAN
// override) is used for the test configuration specifically. No
// production/non-test code path is affected by this relaxation.
#![cfg_attr(not(test), forbid(unsafe_code))]
#![cfg_attr(test, deny(unsafe_code))]
// Denied on production code only (this contract's own guest logic
// processing untrusted message/state bytes); freely allowed under
// `#[cfg(test)]`, where host-level test helpers legitimately use
// `.expect()` on values the test itself just constructed, not untrusted
// input (Gate 2a review round 1, item 6).
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)
)]

//! Multi-Currency Vault — see `SPEC.md` for the full specification this
//! implements, `docs/THREAT_MODEL.md` for the adversarial analysis, and
//! `docs/PLATFORM_FINDINGS.md` for the empirical platform spike both are
//! grounded in. Nothing here claims to be "secure" or "audited" — see
//! `REVIEW.md` for what was tested, what was not, and residual risks.

mod constants;
mod handlers;
mod state;
#[cfg(test)]
mod test_support;

use avm_contract_sdk::Contract;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::StateUpdate;
use avm_contract_sdk::CODE_UPGRADE_METHOD_ID;
use constants::*;
use state::VaultState;

struct MultiCurrencyVault;

impl Contract for MultiCurrencyVault {
    fn init(_input: MessageInput<'_>) -> MessageOutput {
        let build_flags = if USING_DEMO_OWNER_KEY { BUILD_FLAG_DEMO_OWNER_KEY } else { 0 };
        let state = VaultState::new_empty(VAULT_OWNER_PUBKEY, build_flags);
        MessageOutput::success(StateUpdate::FullReplace(state.serialize()))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        // Dispatch order (SPEC.md §3/§4, this project's Hard Rules):
        // 1. CODE_UPGRADE_METHOD_ID check, before any other dispatch.
        // 2. The wire-level callback flag, never method_id matching alone
        //    (Invariant I4c) — checked before any named-method comparison.
        // 3. Named methods.
        // 4. Fallthrough: unrecognized_method (1002).
        if input.message.method_id() == CODE_UPGRADE_METHOD_ID {
            return MessageOutput::failure(ERR_UPGRADE_NOT_SUPPORTED);
        }

        // Gate 3e TASK F2: a callback is routed to the fast path BEFORE
        // any full `VaultState::parse` — the wire-level callback flag
        // (`is_callback()`) is a property of the message alone, never
        // the state, so this reordering changes nothing about which
        // messages are recognized as callbacks (Invariant I4c still
        // holds: dispatch gates on this same flag, never on method_id
        // matching alone). See `SPEC.md` §1 for why skipping full
        // ledger validation on this path specifically is acceptable.
        if input.message.is_callback() {
            return handlers::handle_callback_fast(&input);
        }

        let state = match VaultState::parse(input.state) {
            Ok(state) => state,
            Err(code) => return MessageOutput::failure(code),
        };

        match input.message.method_id() {
            DEPOSIT_METHOD_ID => handlers::handle_deposit(&input, state),
            WITHDRAW_METHOD_ID => handlers::handle_withdraw(&input, state),
            TOPUP_METHOD_ID => handlers::handle_topup(&input, &state),
            PROPOSE_OWNER_METHOD_ID => handlers::handle_propose_owner(&input, state),
            ACCEPT_OWNER_METHOD_ID => handlers::handle_accept_owner(&input, state),
            SET_PAUSE_METHOD_ID => handlers::handle_set_pause(&input, state),
            _ => MessageOutput::failure(ERR_UNRECOGNIZED_METHOD),
        }
    }
}

avm_contract_sdk::export_message_contract!(MultiCurrencyVault, arena_bytes = 262_144);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LedgerKey;
    use crate::state::PendingWithdrawal;
    use crate::test_support;

    const DAPP: [u8; 32] = [1; 32];
    const CALLER: [u8; 32] = [2; 32];
    const EXTERNAL_PUBKEY: [u8; 32] = [3; 32];

    fn empty_state() -> Vec<u8> {
        VaultState::new_empty([9; 32], 0).serialize()
    }

    /// Builds a `VaultState` with `filler_count` distinct, single-unit
    /// ledger rows (each a different `dapp_id`, so each is a genuinely
    /// separate key) plus one real row for `(DAPP, CALLER)` at
    /// `currency_id = 1` seeded with `depositor_amount` — used by the Gate
    /// 2a round 2 item 3 arena-headroom tests below to construct the
    /// largest ledger this contract's own rules can ever produce, without
    /// having to actually replay the long real sequence of DEPOSIT/
    /// WITHDRAW/callback calls that would otherwise be needed to reach it.
    fn ledger_with_fillers(filler_count: usize, depositor_amount: u128) -> VaultState {
        let mut state = VaultState::new_empty([9; 32], 0);
        state
            .credit_ledger(
                LedgerKey {
                    dapp_id: DAPP,
                    account_id: CALLER,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 1,
                },
                depositor_amount,
            )
            .expect("seed depositor row");
        for i in 0..filler_count {
            let mut filler_dapp = [0u8; 32];
            filler_dapp[28..32].copy_from_slice(&(i as u32).to_be_bytes());
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
        state
    }

    /// Gate 2a round 2 item 3: real combined input+output wire-byte sizes
    /// at this contract's own worst-case state sizes, measured against
    /// `avm_contract_sdk::GuestArena<262_144>` — the exact fixed-size,
    /// bump-allocated arena `lib.rs`'s `export_message_contract!(...,
    /// arena_bytes = 262_144)` installs in the real compiled `.wasm`
    /// module, used to hold the live input AND output allocations
    /// *simultaneously* (`crates/avm-contract-sdk/src/runtime.rs`'s own
    /// doc comment: "AVM invokes one entrypoint on a fresh instance, then
    /// frees output followed by input" — a fixed LIFO ownership rule,
    /// meaning `input_len + output_len` together, not either alone, is
    /// what must fit within `N`). `input_len` is measured as the literal
    /// byte length of the `ACMI` buffer `test_support`'s wire-format
    /// builders produce — the exact bytes a real host would write across
    /// this boundary. `output_len` is measured via
    /// `avm_contract_sdk::encode_message_output`, the SDK's own real,
    /// exported encoder (the same routine `GuestArena::write_output` calls
    /// internally) — not a reimplementation. This measures the real
    /// arena-sizing arithmetic, which is pure byte-length math independent
    /// of target architecture; it does not execute inside an actual
    /// wasm32 sandbox, since driving the real compiled `.wasm`'s raw guest
    /// ABI (how a host writes bytes into guest memory before calling
    /// `process_message`) is not documented anywhere in this checkout and
    /// reimplementing it blind would risk inventing undocumented behavior
    /// (Hard Rule 3) — see `REVIEW.md` for this scope decision in full.
    mod arena_headroom {
        use super::*;

        const ARENA_BYTES: usize = 262_144;

        fn report(label: &str, input_len: usize, output_len: usize) {
            let combined = input_len + output_len;
            let headroom = ARENA_BYTES as isize - combined as isize;
            println!(
                "[arena-headroom] {label}: input_len={input_len} output_len={output_len} \
                 combined={combined} arena_bytes={ARENA_BYTES} headroom={headroom}"
            );
            assert!(
                headroom >= 0,
                "{label}: combined input+output ({combined}) exceeds arena_bytes \
                 ({ARENA_BYTES}) by {} bytes — raise arena_bytes in lib.rs's \
                 export_message_contract! call",
                -headroom
            );
        }

        /// 252 ledger rows (`MAX_LEDGER_ENTRIES` (220) + `MAX_PENDING_WITHDRAWALS`
        /// (32) — the maximum row count this contract's own rules can ever
        /// produce, reached only via 32 restores bypassing the cap on top
        /// of an already-full ledger; SPEC.md §2.4/§7 I11) — crediting the
        /// one existing depositor row (not a new one, since a new row
        /// would correctly be rejected at 1508 per item 1's fix). Figures
        /// updated from 232/220/12 to 252/220/32 — Gate 2b round 3 TASK H,
        /// `docs/PLATFORM_FINDINGS.md` P20/P21 (root cause confirmed and
        /// fixed; pending cap restored to its architectural value).
        #[test]
        fn deposit_at_252_row_worst_case() {
            let state = ledger_with_fillers(251, 1_000_000);
            assert_eq!(state.ledger.len(), 252);
            let state_bytes = state.serialize();

            // Built directly via the lower-level wire-builders (not the
            // `*_input` convenience wrappers): those tie their returned
            // `MessageInput<'a>` lifetime to the SAME `'a` as their
            // `&'a mut Vec<u8>` out-param, which the borrow checker then
            // treats as a live mutable borrow for as long as `input` is
            // used — incompatible with also reading the buffer's own
            // `.len()` for this test's measurement. Building the message
            // bytes, wrapping them into a full ACMI buffer, and decoding
            // that owned, immutable buffer directly sidesteps this with
            // no change in what is actually being tested.
            let message_bytes = test_support::internal_message_bytes(
                DEPOSIT_METHOD_ID,
                (DAPP, CALLER),
                (DAPP, DAPP),
                0,
                &[test_support::Currency { currency_id: 1, amount: 500 }],
                &[],
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &state_bytes,
                &message_bytes,
            );
            let input_len = acmi_bytes.len();
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, 0, "existing-row DEPOSIT must succeed at 252 rows");
            // Gate 2a round 3 item 2: assert the resulting committed
            // state, not merely the status code.
            let resulting = test_support::expect_full_replace_state(&output);
            assert_eq!(resulting.ledger.len(), 252, "crediting an existing row adds no row");
            let key = LedgerKey {
                dapp_id: DAPP,
                account_id: CALLER,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 1,
            };
            assert_eq!(resulting.ledger_balance(&key), 1_000_500);
            let output_len = avm_contract_sdk::encode_message_output(&output)
                .expect("real output must encode")
                .len();
            report("DEPOSIT at 252 ledger rows", input_len, output_len);
        }

        /// Same 252-row worst case, withdrawing part of the existing
        /// depositor row.
        #[test]
        fn withdraw_at_252_row_worst_case() {
            let state = ledger_with_fillers(251, 1_000_000);
            assert_eq!(state.ledger.len(), 252);
            let state_bytes = state.serialize();

            let mut body = Vec::with_capacity(21);
            body.push(CURRENCY_TAG_EXTRA);
            body.extend_from_slice(&1u32.to_le_bytes());
            body.extend_from_slice(&500u128.to_le_bytes());

            let message_bytes = test_support::internal_message_bytes(
                WITHDRAW_METHOD_ID,
                (DAPP, CALLER),
                (DAPP, DAPP),
                0,
                &[],
                &body,
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[test_support::Currency {
                    currency_id: SHELL_CURRENCY_ID,
                    amount: MIN_OPERATIONAL_SHELL,
                }],
                &state_bytes,
                &message_bytes,
            );
            let input_len = acmi_bytes.len();
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, 0, "WITHDRAW must succeed at 252 ledger rows");
            // Gate 2a round 3 item 2: assert the resulting committed
            // state — the debited row's balance decreased by exactly the
            // withdrawn amount, and a pending record was inserted.
            let resulting = test_support::expect_full_replace_state(&output);
            let key = LedgerKey {
                dapp_id: DAPP,
                account_id: CALLER,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 1,
            };
            assert_eq!(resulting.ledger_balance(&key), 1_000_000 - 500);
            assert_eq!(resulting.pending.len(), 1);
            assert_eq!(resulting.pending[0].amount, 500);
            let output_len = avm_contract_sdk::encode_message_output(&output)
                .expect("real output must encode")
                .len();
            report("WITHDRAW at 252 ledger rows", input_len, output_len);
        }

        /// The true byte-densest reachable state is `MAX_LEDGER_ENTRIES +
        /// MAX_PENDING_WITHDRAWALS` (252 ledger rows) *together with*
        /// `MAX_PENDING_WITHDRAWALS` (32) pending records outstanding at
        /// once (91 + 252*85 + 32*89 = 24,359 bytes) — **corrected at
        /// Phase 4 (fresh-context adversarial review, finding 1)** from an
        /// earlier, smaller fixture (220 rows + 32 pending, 21,639 bytes)
        /// that reasoned reaching 252 rows requires every pending
        /// withdrawal to have already resolved via restore. That reasoning
        /// was incomplete: once 252 rows exist, a *partial* withdrawal
        /// opens a new pending record without removing its row (Invariant
        /// I9 only removes a row a debit brings to exactly zero), so all
        /// 32 pending slots can be refilled on top of all 252 rows — see
        /// `src/handlers.rs`'s
        /// `true_worst_case_state_is_252_rows_plus_32_pending` for the
        /// real-handler-call proof this is reachable; this test, like its
        /// predecessor, constructs the equivalent state directly (not via
        /// handler calls) purely to measure arena/allocator headroom at
        /// it. Figures before that were updated from 232/220/12 to
        /// 252/220/32 — Gate 2b round 3 TASK H, `docs/
        /// PLATFORM_FINDINGS.md` P20/P21: this exact shape (max ledger
        /// rows together with max pending) was real-stand-verified to
        /// resolve cleanly after `state.rs`'s O(n²) per-depositor check
        /// (the actual P18/P19 cause) was removed; this host-level test
        /// exercises this contract's own callback-handling logic directly
        /// and was never what P18/P19 found broken in the first place (the
        /// real platform's own delivery/fuel behavior for an
        /// auto-generated callback at scale, which no host-level call can
        /// reproduce — see REVIEW.md).
        #[test]
        fn callback_at_max_ledger_and_max_pending_worst_case() {
            let mut state = ledger_with_fillers(251, 1_000_000);
            for id in 1..=32u32 {
                // Gate 2b round 3 TASK H: state.rs's own parse-time
                // per-depositor validation was removed this round (the
                // confirmed P18/P19 cause) — this fixture still respects
                // MAX_PENDING_PER_DEPOSITOR itself, though, since
                // handle_withdraw's own real-time check (not parse's)
                // still enforces it on every real insert, and this test
                // exercises handle_callback directly, not handle_withdraw
                // — distribute across 8 distinct depositor accounts, 4
                // each: CALLER itself (ids 1-4, so withdrawal_id 1 — the
                // one this test actually resolves — still belongs to
                // CALLER) plus 7 filler accounts (ids 5-32).
                let depositor_account = if id <= 4 {
                    CALLER
                } else {
                    let mut account = [0u8; 32];
                    account[31] = ((id - 5) / 4) as u8 + 1;
                    account
                };
                state.pending.push(PendingWithdrawal {
                    withdrawal_id: id,
                    depositor_dapp_id: DAPP,
                    depositor_account_id: depositor_account,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 1,
                    amount: 100,
                });
            }
            state.next_withdrawal_id = 33;
            assert_eq!(state.ledger.len(), 252);
            assert_eq!(state.pending.len(), 32);
            let state_bytes = state.serialize();
            assert_eq!(state_bytes.len(), 24_359, "the true worst-case byte count");

            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, CALLER),
                (DAPP, DAPP),
                [0xCD; 32],
                1, // status_code != 0: restore
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &state_bytes,
                &message_bytes,
            );
            let input_len = acmi_bytes.len();
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, 0, "callback restore must succeed at 252+32");
            // Gate 2a round 3 item 2: assert the resulting committed
            // state — the restored amount was credited back, and exactly
            // one pending record (the resolved one) is gone.
            let resulting = test_support::expect_full_replace_state(&output);
            assert_eq!(resulting.pending.len(), 31);
            let key = LedgerKey {
                dapp_id: DAPP,
                account_id: CALLER,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 1,
            };
            assert_eq!(resulting.ledger_balance(&key), 1_000_000 + 100);
            let output_len = avm_contract_sdk::encode_message_output(&output)
                .expect("real output must encode")
                .len();
            report("CALLBACK (restore) at 252 ledger rows + 32 pending", input_len, output_len);
        }
    }

    /// Gate 2a round 3 item 3b: total allocator traffic during
    /// `process_message` at this contract's own worst-case state sizes,
    /// via `test_support::CountingAllocator` (a thin wrapper around the
    /// real `std::alloc::System` this crate otherwise uses, not a
    /// separate/fake allocator) — a different, complementary measurement
    /// from `arena_headroom`'s wire-byte sizes: this counts every byte
    /// the ordinary heap allocator sees (`Vec<LedgerEntry>`,
    /// `Vec<(LedgerKey, u128)>` credit buffers, the serialized state
    /// buffer, etc.), which `arena_bytes` has no bearing on at all (see
    /// `REVIEW.md` for the full distinction). The counter is
    /// `thread_local!` ("THEN Phase 3" item 1 follow-up), so these tests
    /// no longer need to be run alone — `cargo test` gives each test its
    /// own thread by default, and each thread's counter starts fresh.
    mod counting_allocator {
        use super::*;

        /// Re-measured at Gate 3b TASK R, at the TRUE worst case (252
        /// ledger rows + 32 pending — `MAX_LEDGER_ENTRIES` (220) +
        /// `MAX_PENDING_WITHDRAWALS` (32), reachable via 32 restores on
        /// top of a full 220-row ledger, then 32 partial withdrawals
        /// refilling all 32 pending slots without removing a row; see
        /// `handlers.rs::true_worst_case_state_is_252_rows_plus_32_pending`
        /// and `SPEC.md` §1 for the full derivation — not the 220-row
        /// figure an earlier version of this comment quoted, which
        /// predates that correction):
        /// - DEPOSIT at 252 rows: 46,087 bytes
        /// - WITHDRAW at 252 rows: 46,384 bytes
        /// - CALLBACK (Restore) at 252 rows + 32 pending: 51,534 bytes
        /// - CALLBACK (Settle) at 252 rows + 32 pending: 51,534 bytes
        ///   (identical to Restore at this scale — crediting the ledger
        ///   back on Restore adds no measurable allocator traffic beyond
        ///   what removing the pending record itself already costs)
        ///
        /// The highest of these (51,534 bytes, either callback outcome)
        /// is what the budget below is actually sized against. The
        /// budget (150,000) is roughly **2.9×** that figure — generous
        /// enough not to be flaky but still a real regression guard: a
        /// future change that balloons allocator traffic at worst-case
        /// state (e.g. reintroducing a full ledger clone) would trip it.
        /// (An earlier version of this comment, predating the 220→252
        /// worst-case correction, quoted ~45.7–46.4 KB and a ~3.2×
        /// margin — those numbers came from the DEPOSIT/WITHDRAW figures
        /// only, which this round confirms are still correct at 252 rows,
        /// but omitted the callback measurement entirely, which turns
        /// out to be the actual highest figure and the real basis for
        /// this budget's margin.)
        const ALLOCATOR_BUDGET_BYTES: usize = 150_000;

        fn assert_within_budget(label: &str, bytes: usize) {
            println!(
                "[counting-allocator] {label}: {bytes} bytes allocated (budget {ALLOCATOR_BUDGET_BYTES}, headroom {})",
                ALLOCATOR_BUDGET_BYTES as isize - bytes as isize
            );
            assert!(
                bytes <= ALLOCATOR_BUDGET_BYTES,
                "{label}: {bytes} bytes allocated exceeds the {ALLOCATOR_BUDGET_BYTES}-byte budget"
            );
        }

        #[test]
        fn deposit_at_252_row_worst_case() {
            let state = ledger_with_fillers(251, 1_000_000);
            let state_bytes = state.serialize();
            let message_bytes = test_support::internal_message_bytes(
                DEPOSIT_METHOD_ID,
                (DAPP, CALLER),
                (DAPP, DAPP),
                0,
                &[test_support::Currency { currency_id: 1, amount: 500 }],
                &[],
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &state_bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let before = test_support::allocated_bytes();
            let output = MultiCurrencyVault::process_message(input);
            let after = test_support::allocated_bytes();
            assert_eq!(output.status_code, 0);
            assert_within_budget("DEPOSIT at 252 ledger rows", after - before);
        }

        #[test]
        fn withdraw_at_252_row_worst_case() {
            let state = ledger_with_fillers(251, 1_000_000);
            let state_bytes = state.serialize();
            let mut body = Vec::with_capacity(21);
            body.push(CURRENCY_TAG_EXTRA);
            body.extend_from_slice(&1u32.to_le_bytes());
            body.extend_from_slice(&500u128.to_le_bytes());
            let message_bytes = test_support::internal_message_bytes(
                WITHDRAW_METHOD_ID,
                (DAPP, CALLER),
                (DAPP, DAPP),
                0,
                &[],
                &body,
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[test_support::Currency {
                    currency_id: SHELL_CURRENCY_ID,
                    amount: MIN_OPERATIONAL_SHELL,
                }],
                &state_bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let before = test_support::allocated_bytes();
            let output = MultiCurrencyVault::process_message(input);
            let after = test_support::allocated_bytes();
            assert_eq!(output.status_code, 0);
            assert_within_budget("WITHDRAW at 252 ledger rows", after - before);
        }

        #[test]
        fn callback_at_max_ledger_and_max_pending_worst_case() {
            // Phase 4 (fresh-context adversarial review, finding 1): 252
            // rows (not 220), see the arena_headroom module's identical
            // fixture above for the full explanation.
            let mut state = ledger_with_fillers(251, 1_000_000);
            for id in 1..=32u32 {
                // Gate 2b round 3 TASK H: see the arena_headroom module's
                // identical fixture above for the full explanation —
                // distribute across 8 distinct depositor accounts, 4
                // each: CALLER itself (ids 1-4) plus 7 filler accounts
                // (ids 5-32).
                let depositor_account = if id <= 4 {
                    CALLER
                } else {
                    let mut account = [0u8; 32];
                    account[31] = ((id - 5) / 4) as u8 + 1;
                    account
                };
                state.pending.push(PendingWithdrawal {
                    withdrawal_id: id,
                    depositor_dapp_id: DAPP,
                    depositor_account_id: depositor_account,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 1,
                    amount: 100,
                });
            }
            state.next_withdrawal_id = 33;
            let state_bytes = state.serialize();
            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, CALLER),
                (DAPP, DAPP),
                [0xCD; 32],
                1,
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &state_bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let before = test_support::allocated_bytes();
            let output = MultiCurrencyVault::process_message(input);
            let after = test_support::allocated_bytes();
            assert_eq!(output.status_code, 0);
            assert_within_budget(
                "CALLBACK (restore) at 252 ledger rows + 32 pending",
                after - before,
            );
        }

        /// Gate 3b TASK R: the test above exercises Restore
        /// (`status_code != 0`) at the true worst case; this one
        /// exercises the other callback outcome, Settle
        /// (`status_code == 0`), at the identical 252-row + 32-pending
        /// fixture, and — going beyond an allocator-budget check —
        /// asserts the resulting state byte-for-byte against an
        /// independently constructed expected state: the ledger
        /// untouched (Settle credits nothing back) and exactly one
        /// pending record (`withdrawal_id` 1) removed, leaving 31.
        #[test]
        fn settle_at_max_ledger_and_max_pending_worst_case() {
            let mut state = ledger_with_fillers(251, 1_000_000);
            for id in 1..=32u32 {
                let depositor_account = if id <= 4 {
                    CALLER
                } else {
                    let mut account = [0u8; 32];
                    account[31] = ((id - 5) / 4) as u8 + 1;
                    account
                };
                state.pending.push(PendingWithdrawal {
                    withdrawal_id: id,
                    depositor_dapp_id: DAPP,
                    depositor_account_id: depositor_account,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 1,
                    amount: 100,
                });
            }
            state.next_withdrawal_id = 33;
            let state_bytes = state.serialize();

            let mut expected_state = state.clone();
            expected_state.pending.retain(|record| record.withdrawal_id != 1);
            assert_eq!(expected_state.pending.len(), 31, "exactly one of 32 pending removed");
            assert_eq!(
                expected_state.ledger.len(),
                state.ledger.len(),
                "Settle must never touch the ledger"
            );
            let expected_bytes = expected_state.serialize();

            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, CALLER),
                (DAPP, DAPP),
                [0xCD; 32],
                0, // status_code == 0 -> Settle
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &state_bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let before = test_support::allocated_bytes();
            let output = MultiCurrencyVault::process_message(input);
            let after = test_support::allocated_bytes();
            assert_eq!(output.status_code, 0);
            let actual_state = test_support::expect_full_replace_state(&output);
            assert_eq!(
                actual_state.serialize(),
                expected_bytes,
                "Settle at 252 rows + 32 pending must produce exactly the expected \
                 state byte-for-byte: ledger unchanged, pending 32 -> 31"
            );
            assert_within_budget("SETTLE at 252 ledger rows + 32 pending", after - before);
        }
    }

    /// Dispatch routes purely by `method_id`; each handler itself checks
    /// caller type. This pins that an `External` caller sending either of
    /// the two `Internal`-only method ids is rejected with `invalid_caller`
    /// (1500) — not merely "falls through to the handler and happens to
    /// work," but a specific, asserted outcome.
    #[test]
    fn external_caller_sending_internal_only_ids_is_rejected() {
        for method_id in [DEPOSIT_METHOD_ID, WITHDRAW_METHOD_ID] {
            let mut bytes = Vec::new();
            let input = test_support::external_input(
                &mut bytes,
                (DAPP, CALLER),
                EXTERNAL_PUBKEY,
                method_id,
                0,
                &[],
                &[0; 21], // long enough to pass WITHDRAW's body-length gate too
                0,
                &[],
                &empty_state(),
            );
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(
                output.status_code, ERR_INVALID_CALLER,
                "method_id {method_id:#x} should be rejected as invalid_caller for an External caller"
            );
            assert!(output.state_update.is_none());
        }
    }

    /// Mirror case: an `Internal` caller sending any of the four
    /// `External`-only method ids is rejected with `invalid_caller` (1500).
    #[test]
    fn internal_caller_sending_external_only_ids_is_rejected() {
        for method_id in
            [TOPUP_METHOD_ID, PROPOSE_OWNER_METHOD_ID, ACCEPT_OWNER_METHOD_ID, SET_PAUSE_METHOD_ID]
        {
            let mut bytes = Vec::new();
            let input = test_support::internal_input(
                &mut bytes,
                (DAPP, CALLER),
                (DAPP, CALLER),
                method_id,
                0,
                &[],
                &[0; 36], // long enough to pass every admin method's body-length gate too
                0,
                &[],
                &empty_state(),
            );
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(
                output.status_code, ERR_INVALID_CALLER,
                "method_id {method_id:#x} should be rejected as invalid_caller for an Internal caller"
            );
            assert!(output.state_update.is_none());
        }
    }

    /// Gate 2a round 2 item 4: caller-type is checked strictly before
    /// body-shape in `handle_withdraw` — an `External` caller sending
    /// `WITHDRAW_METHOD_ID` with an obviously wrong-length body must still
    /// get 1500 (`invalid_caller`), never 1501 (`invalid_body`).
    #[test]
    fn external_caller_sending_withdraw_with_malformed_body_is_rejected_as_invalid_caller() {
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, CALLER),
            EXTERNAL_PUBKEY,
            WITHDRAW_METHOD_ID,
            0,
            &[],
            &[0xAA; 3], // deliberately the wrong length (WITHDRAW_BODY_LEN == 21)
            0,
            &[],
            &empty_state(),
        );
        let output = MultiCurrencyVault::process_message(input);
        assert_eq!(output.status_code, ERR_INVALID_CALLER);
        assert!(output.state_update.is_none());
    }

    #[test]
    fn unknown_method_id_is_unrecognized() {
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, CALLER),
            EXTERNAL_PUBKEY,
            0x1234_5678_9abc_def0,
            0,
            &[],
            &[],
            0,
            &[],
            &empty_state(),
        );
        let output = MultiCurrencyVault::process_message(input);
        assert_eq!(output.status_code, ERR_UNRECOGNIZED_METHOD);
        assert!(output.state_update.is_none());
    }

    #[test]
    fn code_upgrade_method_id_is_rejected_before_any_other_dispatch() {
        let mut bytes = Vec::new();
        let input = test_support::external_input(
            &mut bytes,
            (DAPP, CALLER),
            EXTERNAL_PUBKEY,
            avm_contract_sdk::CODE_UPGRADE_METHOD_ID,
            0,
            &[],
            &[],
            0,
            &[],
            &empty_state(),
        );
        let output = MultiCurrencyVault::process_message(input);
        assert_eq!(output.status_code, ERR_UPGRADE_NOT_SUPPORTED);
        assert!(output.state_update.is_none());
    }

    /// SPEC.md §8 test 1 — Invariant I4c's direct regression test: a
    /// plain internal message (the wire-level `CALLBACK` flag is **not**
    /// set) whose `method_id` happens to numerically collide with
    /// `withdrawal_callback_method_id(id)` for a real, currently-pending
    /// `id` must still fail as `unrecognized_method` (1002) — dispatch
    /// gates on the wire flag, never on `method_id` matching alone, so
    /// this never reaches settlement logic regardless of body content.
    #[test]
    fn plain_message_with_callback_shaped_method_id_is_unrecognized() {
        let mut state = VaultState::new_empty([9; 32], 0);
        state.pending.push(PendingWithdrawal {
            withdrawal_id: 7,
            depositor_dapp_id: DAPP,
            depositor_account_id: CALLER,
            currency_tag: CURRENCY_TAG_EXTRA,
            currency_id: 1,
            amount: 50,
        });
        state.next_withdrawal_id = 8;
        let state_bytes = state.serialize();

        let mut bytes = Vec::new();
        let input = test_support::internal_input(
            &mut bytes,
            (DAPP, CALLER),
            (DAPP, CALLER),
            withdrawal_callback_method_id(7),
            0,
            &[],
            &[],
            0,
            &[],
            &state_bytes,
        );
        let output = MultiCurrencyVault::process_message(input);
        assert_eq!(output.status_code, ERR_UNRECOGNIZED_METHOD);
        assert!(output.state_update.is_none());
    }

    /// SPEC.md §9's model-based simulator strategy. Each test here
    /// targets exactly one of §9's five named scenarios, driven through
    /// the real `MultiCurrencyVault::process_message` dispatch (not a
    /// direct handler call), since what §9 asks to exercise is genuinely
    /// a property of how this contract behaves under an adversarial
    /// *environment* (duplicate/dropped/retained delivery), not merely
    /// one handler function in isolation.
    mod simulator {
        use super::*;

        /// §9, bullet 1 — the "retain" branch, labeled explicitly
        /// undefended. Demonstrates Invariant I1 genuinely breaking, not
        /// merely asserted in prose: this contract's `handle_callback`
        /// restores unconditionally on any non-zero `status_code`
        /// (Invariant I11), with no way to know — and no code path to
        /// check — whether the real platform actually refunded the
        /// failed payout's value or retained it at the destination. This
        /// spike's own probes only ever observed "never debited in the
        /// first place" (refund) on the one route actually reachable; the
        /// *retain* branch this test forces is the one this contract has
        /// never been shown to be safe against. The carried-value
        /// verification gate that would once have caught exactly this
        /// case was deliberately removed from this design (SPEC.md §1),
        /// precisely because the real platform never exhibited this
        /// behavior on that one reachable route (`docs/
        /// PLATFORM_FINDINGS.md` probe A8) — this test asserts the gap
        /// exists, not that anything catches it.
        #[test]
        fn retain_branch_breaks_invariant_i1_by_design_no_defense_exists() {
            let mut state = VaultState::new_empty([9; 32], 0);
            let key = LedgerKey {
                dapp_id: DAPP,
                account_id: CALLER,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
            };
            state.credit_ledger(key, 1000).expect("seed");
            // Invariant I1 holds at the start: the vault's simulated real
            // account balance for currency 7 equals what the ledger
            // claims (field group "(a)" vs. the ledger's own bookkeeping).
            let mut real_balance: u128 = 1000;
            assert_eq!(state.ledger_balance(&key), real_balance);

            let mut body = Vec::with_capacity(21);
            body.push(CURRENCY_TAG_EXTRA);
            body.extend_from_slice(&7u32.to_le_bytes());
            body.extend_from_slice(&1000u128.to_le_bytes());
            let mut bytes = Vec::new();
            let input = test_support::internal_input(
                &mut bytes,
                (DAPP, CALLER),
                (DAPP, CALLER),
                WITHDRAW_METHOD_ID,
                0,
                &[],
                &body,
                0,
                &[test_support::Currency {
                    currency_id: SHELL_CURRENCY_ID,
                    amount: MIN_OPERATIONAL_SHELL,
                }],
                &state.serialize(),
            );
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, 0);
            let after_withdraw = test_support::expect_full_replace_state(&output);
            assert_eq!(after_withdraw.ledger_balance(&key), 0);
            assert_eq!(after_withdraw.pending.len(), 1);
            // Modeling what a real OutboundIntent delivery does: the
            // vault's real account balance genuinely decreases when the
            // payout is actually sent, regardless of what happens to it
            // at the destination afterward.
            real_balance -= 1000;

            // The simulated environment's retain branch: delivery to the
            // destination fails, but — unlike every outcome this
            // project's own probes ever directly observed — the carried
            // value is NOT refunded to the vault; it stays with the
            // now-failed destination. `real_balance` therefore does NOT
            // go back up, even though this contract's own callback
            // handler is about to restore the ledger unconditionally.
            let withdrawal_id = after_withdraw.pending[0].withdrawal_id;
            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(withdrawal_id),
                (DAPP, CALLER),
                (DAPP, DAPP),
                [0xCD; 32],
                1, // failure: restore
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &after_withdraw.serialize(),
                &message_bytes,
            );
            let callback_input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let callback_output = MultiCurrencyVault::process_message(callback_input);
            assert_eq!(callback_output.status_code, 0);
            let after_restore = test_support::expect_full_replace_state(&callback_output);
            assert_eq!(
                after_restore.ledger_balance(&key),
                1000,
                "this contract's restore is unconditional, exactly as documented"
            );

            // Invariant I1: sum(ledger) + sum(pending) for this currency,
            // compared against what the vault's REAL account balance
            // actually is under the retain branch.
            let claimed_total = after_restore.ledger_balance(&key)
                + after_restore
                    .pending
                    .iter()
                    .filter(|p| p.currency_tag == CURRENCY_TAG_EXTRA && p.currency_id == 7)
                    .map(|p| p.amount)
                    .sum::<u128>();
            assert_eq!(claimed_total, 1000);
            assert_eq!(real_balance, 0);
            assert!(
                claimed_total > real_balance,
                "Invariant I1 is genuinely broken under the retain branch — this \
                 is the explicitly undefended gap SPEC.md §9 names, demonstrated \
                 directly rather than merely asserted: this contract's \
                 bookkeeping now claims {claimed_total} of this currency that it \
                 does not, in real terms, actually hold ({real_balance})."
            );
        }

        /// §9, bullet 3 — dropped callbacks accumulate to the global
        /// cap, blocking even a depositor with no stuck withdrawal of
        /// their own.
        #[test]
        fn dropped_callbacks_accumulate_to_the_global_cap_blocking_even_unrelated_depositors() {
            let mut state = VaultState::new_empty([9; 32], 0);
            for i in 0..MAX_PENDING_WITHDRAWALS as u32 {
                let depositor = {
                    let mut account = [0u8; 32];
                    account[31] = i as u8 + 1;
                    account
                };
                let key = LedgerKey {
                    dapp_id: DAPP,
                    account_id: depositor,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 7,
                };
                state.credit_ledger(key, 100).expect("seed");
                let mut body = Vec::with_capacity(21);
                body.push(CURRENCY_TAG_EXTRA);
                body.extend_from_slice(&7u32.to_le_bytes());
                body.extend_from_slice(&50u128.to_le_bytes());
                let mut bytes = Vec::new();
                let input = test_support::internal_input(
                    &mut bytes,
                    (DAPP, depositor),
                    (DAPP, depositor),
                    WITHDRAW_METHOD_ID,
                    0,
                    &[],
                    &body,
                    0,
                    &[test_support::Currency {
                        currency_id: SHELL_CURRENCY_ID,
                        amount: MIN_OPERATIONAL_SHELL,
                    }],
                    &state.serialize(),
                );
                let output = MultiCurrencyVault::process_message(input);
                assert_eq!(
                    output.status_code, 0,
                    "withdrawal {i} must succeed before getting stuck"
                );
                state = test_support::expect_full_replace_state(&output);
                // No callback of any kind is ever delivered for this
                // withdrawal_id — modeling either stuck route
                // (THREAT_MODEL.md §"L") — so it accumulates permanently.
            }
            assert_eq!(state.pending.len(), MAX_PENDING_WITHDRAWALS);

            // A fresh depositor, with no stuck withdrawal of their own,
            // attempts their own first withdrawal.
            const FRESH: [u8; 32] = [99; 32];
            let fresh_key = LedgerKey {
                dapp_id: DAPP,
                account_id: FRESH,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 9,
            };
            state.credit_ledger(fresh_key, 500).expect("seed fresh depositor");
            let mut body = Vec::with_capacity(21);
            body.push(CURRENCY_TAG_EXTRA);
            body.extend_from_slice(&9u32.to_le_bytes());
            body.extend_from_slice(&10u128.to_le_bytes());
            let mut bytes = Vec::new();
            let input = test_support::internal_input(
                &mut bytes,
                (DAPP, FRESH),
                (DAPP, FRESH),
                WITHDRAW_METHOD_ID,
                0,
                &[],
                &body,
                0,
                &[test_support::Currency {
                    currency_id: SHELL_CURRENCY_ID,
                    amount: MIN_OPERATIONAL_SHELL,
                }],
                &state.serialize(),
            );
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, ERR_PENDING_CAPACITY_EXCEEDED);
            assert!(output.state_update.is_none());
            assert_eq!(
                state.pending.len(),
                MAX_PENDING_WITHDRAWALS,
                "the stuck records remain untouched"
            );
        }

        /// §9, bullet 4 — a duplicated callback's second delivery is
        /// absorbed as `withdrawal_not_found` (1510), never a
        /// double-credit or double-restore.
        #[test]
        fn duplicated_callback_second_delivery_is_absorbed_as_not_found() {
            let mut state = VaultState::new_empty([9; 32], 0);
            state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: CALLER,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 100,
            });
            state.next_withdrawal_id = 2;

            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, CALLER),
                (DAPP, DAPP),
                [0xCD; 32],
                0,
            );
            let acmi1 = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &state.serialize(),
                &message_bytes,
            );
            let input1 = avm_contract_sdk::decode_message_input(&acmi1)
                .expect("real ACMI buffer must decode");
            let output1 = MultiCurrencyVault::process_message(input1);
            assert_eq!(output1.status_code, 0);
            let after_first = test_support::expect_full_replace_state(&output1);
            assert_eq!(after_first.pending.len(), 0);

            let acmi2 = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &after_first.serialize(),
                &message_bytes,
            );
            let input2 = avm_contract_sdk::decode_message_input(&acmi2)
                .expect("real ACMI buffer must decode");
            let output2 = MultiCurrencyVault::process_message(input2);
            assert_eq!(
                output2.status_code, ERR_WITHDRAWAL_NOT_FOUND,
                "second delivery is absorbed, never a double-credit/restore"
            );
            assert!(output2.state_update.is_none());
        }

        /// §9, bullet 5 — a duplicated *original* `DEPOSIT` request
        /// double-credits, exactly matching the trust assumption named
        /// in `docs/THREAT_MODEL.md` §3: this contract relies on the
        /// platform's own exactly-once-delivery guarantee and has no way
        /// to distinguish a replay from a genuine second deposit of the
        /// same amount. Documented here as a named, expected consequence,
        /// not discovered as a silent surprise.
        #[test]
        fn duplicated_deposit_request_double_credits_as_a_named_trust_assumption() {
            let state = VaultState::new_empty([9; 32], 0);
            let key = LedgerKey {
                dapp_id: DAPP,
                account_id: CALLER,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
            };
            let mut bytes = Vec::new();
            let input = test_support::internal_input(
                &mut bytes,
                (DAPP, CALLER),
                (DAPP, CALLER),
                DEPOSIT_METHOD_ID,
                0,
                &[test_support::Currency { currency_id: 7, amount: 500 }],
                &[],
                0,
                &[],
                &state.serialize(),
            );
            let output1 = MultiCurrencyVault::process_message(input);
            assert_eq!(output1.status_code, 0);
            let after_first = test_support::expect_full_replace_state(&output1);
            assert_eq!(after_first.ledger_balance(&key), 500);

            // The exact same message, delivered a second time.
            let mut bytes2 = Vec::new();
            let input2 = test_support::internal_input(
                &mut bytes2,
                (DAPP, CALLER),
                (DAPP, CALLER),
                DEPOSIT_METHOD_ID,
                0,
                &[test_support::Currency { currency_id: 7, amount: 500 }],
                &[],
                0,
                &[],
                &after_first.serialize(),
            );
            let output2 = MultiCurrencyVault::process_message(input2);
            assert_eq!(
                output2.status_code, 0,
                "this contract has no way to distinguish a replay from a genuine second deposit"
            );
            let after_second = test_support::expect_full_replace_state(&output2);
            assert_eq!(
                after_second.ledger_balance(&key),
                1000,
                "double-crediting is the documented, named consequence of this trust assumption"
            );
        }

        /// §9, bullet 6 — this contract's own callback-handling logic
        /// never *deliberately* returns failure once the depositor-match
        /// check (I4b) passes, with exactly one practically-unreachable
        /// exception (a `u128` overflow on restore); demonstrates
        /// concretely what is lost when even that one case is forced:
        /// since the real platform never redelivers a callback it has
        /// already sent once (the same exactly-once-delivery trust
        /// assumption applies to callbacks too), a failure here
        /// permanently strands the withdrawal — the same category of
        /// loss as a dropped callback, just triggered by this contract's
        /// own defensive path instead of the platform ever dropping it.
        #[test]
        fn failing_callback_handler_permanently_strands_the_withdrawal() {
            let mut state = VaultState::new_empty([9; 32], 0);
            let key = LedgerKey {
                dapp_id: DAPP,
                account_id: CALLER,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
            };
            state.credit_ledger(key, u128::MAX - 50).expect("seed near-max balance");
            state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: CALLER,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 100,
            });
            state.next_withdrawal_id = 2;

            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, CALLER),
                (DAPP, DAPP),
                [0xCD; 32],
                1, // restore -> credit_ledger will overflow against the near-max seed
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, CALLER),
                [0; 32],
                0,
                &[],
                &state.serialize(),
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(
                output.status_code, ERR_LEDGER_AMOUNT_OVERFLOW,
                "the one practically-unreachable overflow case this handler can still hit \
                 (Phase 4 adversarial review, finding 3: this used to incorrectly return \
                 ERR_INVALID_CALLBACK, the same code a genuine caller/depositor mismatch \
                 uses, contradicting that code's own documented, exclusive meaning)"
            );
            // Invariant I6: this failure carries no state update at all —
            // the pending-record removal this function's own local
            // `state` value performed before discovering the overflow is
            // simply discarded, never committed anywhere.
            assert!(output.state_update.is_none());
        }
    }

    /// SPEC.md §9's "property/fuzz testing of random, valid operation
    /// sequences checked against Invariants I1 through I17 after every
    /// single step." No property-testing crate (e.g. `proptest`) is a
    /// dependency of this project — adding one would be its own scope
    /// decision, not taken here (`REVIEW.md`) — so this is a small,
    /// hand-rolled, deterministic PRNG (seeded per run, so any failure
    /// is exactly reproducible from the printed seed) driving real
    /// `DEPOSIT`/`WITHDRAW`/callback calls through the real
    /// `MultiCurrencyVault::process_message` dispatch. Gate 3e TASK
    /// F2(a): every callback step across all 200 seeds also runs the
    /// slow path (`handlers::handle_callback`) on the identical
    /// pre-callback state and asserts its encoded output matches the
    /// real (fast-path) dispatch exactly — this is this project's
    /// differential test for the callback fast path, not a separate,
    /// smaller sample.
    mod property {
        use std::collections::HashMap;

        use super::*;

        struct Lcg(u64);
        impl Lcg {
            fn next(&mut self) -> u64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                self.0
            }

            fn below(&mut self, bound: u64) -> u64 {
                self.next() % bound.max(1)
            }
        }

        const DEPOSITORS: [[u8; 32]; 3] = [[70; 32], [71; 32], [72; 32]];
        const CURRENCIES: [u32; 2] = [7, 9];

        /// After every step, the state this contract's own code just
        /// produced must always be able to re-parse itself — exercising
        /// I9, ledger/pending sortedness, the pending caps, and I12's
        /// `next_withdrawal_id` monotonicity structurally, via
        /// `state.rs`'s own defensive checks, not re-derived by hand.
        fn assert_structurally_valid(state: &VaultState) -> VaultState {
            let bytes = state.serialize();
            VaultState::parse(&bytes).unwrap_or_else(|code| {
                panic!(
                    "a state this contract's own code just produced failed to \
                     re-parse itself: error {code}"
                )
            })
        }

        #[test]
        fn random_operation_sequences_preserve_structural_and_i1_invariants() {
            for seed in 0..200u64 {
                let mut rng = Lcg(seed.wrapping_add(1));
                let mut state = VaultState::new_empty([9; 32], 0);
                let mut cumulative_deposits: HashMap<(usize, u32), u128> = HashMap::new();

                for _step in 0..25u32 {
                    let depositor_index = rng.below(DEPOSITORS.len() as u64) as usize;
                    let depositor = DEPOSITORS[depositor_index];
                    let currency_id = CURRENCIES[rng.below(CURRENCIES.len() as u64) as usize];
                    let key = LedgerKey {
                        dapp_id: DAPP,
                        account_id: depositor,
                        currency_tag: CURRENCY_TAG_EXTRA,
                        currency_id,
                    };

                    let action = rng.below(3);
                    if action == 0 || state.pending.is_empty() {
                        let amount = 1 + rng.below(500) as u128;
                        let mut bytes = Vec::new();
                        let input = test_support::internal_input(
                            &mut bytes,
                            (DAPP, depositor),
                            (DAPP, depositor),
                            DEPOSIT_METHOD_ID,
                            0,
                            &[test_support::Currency { currency_id, amount }],
                            &[],
                            0,
                            &[],
                            &state.serialize(),
                        );
                        let output = MultiCurrencyVault::process_message(input);
                        if output.status_code == 0 {
                            state = test_support::expect_full_replace_state(&output);
                            *cumulative_deposits
                                .entry((depositor_index, currency_id))
                                .or_insert(0) += amount;
                        }
                    } else if action == 1 {
                        let balance = state.ledger_balance(&key);
                        if balance > 0 {
                            let balance_u64 = balance as u64;
                            let amount = (1 + rng.below(balance_u64)).min(balance_u64) as u128;
                            let mut body = Vec::with_capacity(21);
                            body.push(CURRENCY_TAG_EXTRA);
                            body.extend_from_slice(&currency_id.to_le_bytes());
                            body.extend_from_slice(&amount.to_le_bytes());
                            let mut bytes = Vec::new();
                            let input = test_support::internal_input(
                                &mut bytes,
                                (DAPP, depositor),
                                (DAPP, depositor),
                                WITHDRAW_METHOD_ID,
                                0,
                                &[],
                                &body,
                                0,
                                &[test_support::Currency {
                                    currency_id: SHELL_CURRENCY_ID,
                                    amount: MIN_OPERATIONAL_SHELL,
                                }],
                                &state.serialize(),
                            );
                            let output = MultiCurrencyVault::process_message(input);
                            if output.status_code == 0 {
                                state = test_support::expect_full_replace_state(&output);
                            }
                        }
                    } else {
                        let idx = rng.below(state.pending.len() as u64) as usize;
                        let record = state.pending[idx].clone();
                        let settle = rng.below(2) == 0;
                        let message_bytes = test_support::callback_message_bytes(
                            withdrawal_callback_method_id(record.withdrawal_id),
                            (DAPP, record.depositor_account_id),
                            (DAPP, DAPP),
                            [0xCD; 32],
                            u32::from(!settle),
                        );
                        let acmi = test_support::build_message_input(
                            (DAPP, DAPP),
                            true,
                            (DAPP, record.depositor_account_id),
                            [0; 32],
                            0,
                            &[],
                            &state.serialize(),
                            &message_bytes,
                        );
                        let input = avm_contract_sdk::decode_message_input(&acmi)
                            .expect("real ACMI buffer must decode");

                        // Gate 3e TASK F2(a): differential check against the
                        // slow path (`handlers::handle_callback`, now
                        // `#[cfg(test)]`-only) on the SAME pre-callback
                        // state, for all 200 seeds' worth of real callback
                        // steps — not a separate, smaller sample.
                        let pre_state =
                            VaultState::parse(&state.serialize()).expect("pre-state parses");
                        let slow_output = handlers::handle_callback(&input, pre_state);

                        let output = MultiCurrencyVault::process_message(input);
                        assert_eq!(
                            output.status_code, 0,
                            "seed {seed}: a well-formed callback for a real pending \
                             record must always succeed"
                        );
                        assert_eq!(
                            output.status_code, slow_output.status_code,
                            "seed {seed}: fast and slow callback paths disagree on status_code"
                        );
                        let fast_bytes = avm_contract_sdk::encode_message_output(&output)
                            .expect("fast output encodes");
                        let slow_bytes = avm_contract_sdk::encode_message_output(&slow_output)
                            .expect("slow output encodes");
                        assert_eq!(
                            fast_bytes, slow_bytes,
                            "seed {seed}: fast and slow callback paths produced different \
                             encoded output"
                        );
                        state = test_support::expect_full_replace_state(&output);
                    }

                    state = assert_structurally_valid(&state);

                    // Invariant I1, internal-consistency form: what this
                    // contract's own bookkeeping currently claims
                    // (ledger + still-outstanding pending) for any key
                    // can never exceed what was ever genuinely deposited
                    // for that key — restores only ever move value
                    // between ledger and pending, never create it, and
                    // settles only ever remove it, never duplicate it.
                    for (&(depositor_index, currency_id), &deposited) in &cumulative_deposits {
                        let key = LedgerKey {
                            dapp_id: DAPP,
                            account_id: DEPOSITORS[depositor_index],
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id,
                        };
                        let pending_sum = state
                            .pending_amount_for_key(&key)
                            .expect("no overflow across this test's small amounts");
                        let claimed = state.ledger_balance(&key) + pending_sum;
                        assert!(
                            claimed <= deposited,
                            "seed {seed}: Invariant I1 (internal consistency) violated \
                             for depositor {depositor_index} currency {currency_id}: \
                             claimed {claimed} > ever-deposited {deposited}"
                        );
                    }
                }
            }
        }
    }

    /// Phase 5: a fuzz-style run of `VaultState::parse` on random and
    /// mutated bytes. `parse` is this contract's one true untrusted-input
    /// boundary — every other entrypoint receives a state that was either
    /// just produced by this contract's own `serialize()` or decoded by
    /// `avm-contract-sdk`'s own ACMI/ABI layer before `parse` ever sees
    /// it — so this is the single place a malformed or adversarial state
    /// buffer could reach this contract's code. `parse` is expected to
    /// return `Err` freely (SPEC.md §7's defensive checks exist
    /// specifically so it can), but it must never panic: a panic inside
    /// guest code is a hard trap, strictly worse than a clean error
    /// return (§5.7's residual-risk framing). Uses a seeded `Lcg` (fixed
    /// seed, reproducible) rather than any external fuzzing crate, since
    /// this project added no new dependencies this engagement.
    mod fuzz_parse {
        use std::collections::BTreeMap;

        use super::*;
        use crate::state::LedgerEntry;
        use crate::state::ParseFailureReason;

        struct Lcg(u64);
        impl Lcg {
            fn next(&mut self) -> u64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                self.0
            }

            fn below(&mut self, bound: u64) -> u64 {
                self.next() % bound.max(1)
            }

            fn byte(&mut self) -> u8 {
                (self.next() & 0xFF) as u8
            }

            fn u128_val(&mut self) -> u128 {
                (u128::from(self.next()) << 64) | u128::from(self.next())
            }

            /// A genuine 50/50 decision. `below(2)` (`next() % 2`) is
            /// NOT one: this LCG's multiplier and increment are both
            /// odd, so mod 2 its state transition is `state + 1`, and
            /// `next()`'s lowest bit deterministically ALTERNATES every
            /// single call regardless of seed — a textbook LCG weakness
            /// (low-order bits of a power-of-2-modulus LCG have short
            /// period; bit 0 has period exactly 2). Discovered this
            /// round (Gate 3c TASK 1) when a `below(2)`-gated 50/50
            /// branch choice in the header-aware fuzzer came out ~2.3%
            /// instead of ~50% — confirmed root cause via direct
            /// instrumentation. Bit 31 is far enough from the degenerate
            /// bit 0 that its own period (2^32) vastly exceeds this
            /// module's iteration counts, so it behaves effectively
            /// randomly at this test's scale, unlike bit 0.
            fn coin(&mut self) -> bool {
                (self.next() >> 31) & 1 == 0
            }
        }

        fn reason_name(reason: ParseFailureReason) -> String {
            format!("{reason:?}")
        }

        const ITERATIONS: u64 = 20_000;

        /// Pure random bytes at random lengths, from zero up to somewhat
        /// past this contract's own largest-ever-measured worst-case
        /// state (24,359 bytes, §2.4) — covers truncated headers, random
        /// magic/flags bytes, random ledger/pending counts paired with
        /// wrong-length trailing data, and oversized buffers.
        #[test]
        fn parse_never_panics_on_random_bytes() {
            let mut rng = Lcg(0x5EED_F250_1F02_2001);
            for seed in 0..ITERATIONS {
                let len = rng.below(30_000) as usize;
                let mut buf = Vec::with_capacity(len);
                for _ in 0..len {
                    buf.push(rng.byte());
                }
                let result = std::panic::catch_unwind(|| VaultState::parse(&buf));
                assert!(
                    result.is_ok(),
                    "seed {seed}: VaultState::parse panicked on a random {len}-byte buffer \
                     (an Err return was always an acceptable outcome; a panic is not)"
                );
            }
        }

        /// Bit- and byte-level mutations of a real, genuinely
        /// `serialize()`-produced buffer — closer to what an adversary
        /// starting from a known-valid state and flipping bits would
        /// produce than pure random bytes are, and more likely to land
        /// just past a length boundary (e.g. `ledger_count` off by one
        /// from the buffer's real remaining length) than uniform random
        /// bytes would.
        #[test]
        fn parse_never_panics_on_mutated_valid_state() {
            let mut seed_state = VaultState::new_empty([3; 32], 0);
            // currency_id starts at 10, not 0, specifically to avoid
            // colliding with SHELL_CURRENCY_ID (2) — CURRENCY_TAG_EXTRA
            // paired with currency_id 2 is correctly rejected by
            // `validate_currency_shape` (SPEC.md §7 I10), so this range
            // must skip it to build a genuinely valid template.
            for i in 0..40u32 {
                seed_state
                    .credit_ledger(
                        LedgerKey {
                            dapp_id: DAPP,
                            account_id: [i as u8; 32],
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id: i + 10,
                        },
                        1000 + u128::from(i),
                    )
                    .expect("seed row");
            }
            seed_state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: [0; 32],
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 10,
                amount: 500,
            });
            seed_state.next_withdrawal_id = 2;
            let template = seed_state.serialize();
            if let Err(code) = VaultState::parse(&template) {
                panic!("template itself must be valid, got error code {code}");
            }

            let mut rng = Lcg(0xA11A_C0DE_F00D_BA5E);
            for seed in 0..ITERATIONS {
                let mut buf = template.clone();
                // Each iteration applies a fresh, independent set of
                // mutations starting from the clean template, not a
                // cumulative chain — so one iteration's mutation can never
                // make a later iteration's starting point already-invalid
                // in a way that would mask what that later iteration was
                // actually testing.
                let mutation_count = 1 + rng.below(8) as usize;
                for _ in 0..mutation_count {
                    match rng.below(3) {
                        0 => {
                            // Flip a single random bit.
                            let index = rng.below(buf.len() as u64) as usize;
                            let bit = rng.below(8) as u8;
                            buf[index] ^= 1 << bit;
                        }
                        1 => {
                            // Overwrite a random byte with a random value.
                            let index = rng.below(buf.len() as u64) as usize;
                            buf[index] = rng.byte();
                        }
                        _ => {
                            // Truncate or extend the buffer itself, to
                            // exercise length/count mismatches directly.
                            let delta = 1 + rng.below(16) as usize;
                            if rng.below(2) == 0 && buf.len() > delta {
                                buf.truncate(buf.len() - delta);
                            } else {
                                for _ in 0..delta {
                                    buf.push(rng.byte());
                                }
                            }
                        }
                    }
                }
                let result = std::panic::catch_unwind(|| VaultState::parse(&buf));
                assert!(
                    result.is_ok(),
                    "seed {seed}: VaultState::parse panicked on a mutated valid-state buffer \
                     ({mutation_count} mutation(s) applied, final length {}) — \
                     an Err return was always an acceptable outcome; a panic is not",
                    buf.len()
                );
            }
        }

        /// Gate 3c TASK 1(c): confirms the random-mutation strategy above
        /// genuinely reaches six specific, named `parse()` rejection
        /// branches — not just "some `Err` or other" — by tallying which
        /// `ParseFailureReason` the SAME mutation loop produces, then
        /// closing whatever gap remains with small, deterministic,
        /// targeted cases (not more random search). Two of the six are
        /// structurally out of reach for ANY mutation of this round's
        /// template: it carries exactly one pending record, so there is
        /// no second record for "pending id order" (`UnsortedPending`)
        /// to ever compare against, and reaching "pending over cap"
        /// (`PendingOverCap`) needs 33 genuinely valid, sorted pending
        /// rows, not a one-or-two-byte perturbation of a 1-record
        /// template — no amount of random mutation finds either one.
        /// The targeted cases below are added for all six anyway (not
        /// only the two proven-unreachable ones), so this test's
        /// guarantee never depends on how lucky the fixed seed happened
        /// to be for the other four.
        #[test]
        fn parse_never_panics_on_mutated_valid_state_reaches_every_named_branch() {
            let mut seed_state = VaultState::new_empty([3; 32], 0);
            for i in 0..40u32 {
                seed_state
                    .credit_ledger(
                        LedgerKey {
                            dapp_id: DAPP,
                            account_id: [i as u8; 32],
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id: i + 10,
                        },
                        1000 + u128::from(i),
                    )
                    .expect("seed row");
            }
            seed_state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: [0; 32],
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 10,
                amount: 500,
            });
            seed_state.next_withdrawal_id = 2;
            let template = seed_state.serialize();

            let mut rng = Lcg(0xA11A_C0DE_F00D_BA5E);
            let mut ok_count = 0u64;
            let mut reason_counts: BTreeMap<String, u64> = BTreeMap::new();

            for _ in 0..ITERATIONS {
                let mut buf = template.clone();
                let mutation_count = 1 + rng.below(8) as usize;
                for _ in 0..mutation_count {
                    match rng.below(3) {
                        0 => {
                            let index = rng.below(buf.len() as u64) as usize;
                            let bit = rng.below(8) as u8;
                            buf[index] ^= 1 << bit;
                        }
                        1 => {
                            let index = rng.below(buf.len() as u64) as usize;
                            buf[index] = rng.byte();
                        }
                        _ => {
                            let delta = 1 + rng.below(16) as usize;
                            if rng.coin() && buf.len() > delta {
                                buf.truncate(buf.len() - delta);
                            } else {
                                for _ in 0..delta {
                                    buf.push(rng.byte());
                                }
                            }
                        }
                    }
                }
                match VaultState::parse_with_reason(&buf) {
                    Ok(_) => ok_count += 1,
                    Err(reason) => {
                        *reason_counts.entry(reason_name(reason)).or_insert(0) += 1;
                    }
                }
            }

            // --- Targeted cases: deterministic, not random. Each starts
            // from a small, hand-built, otherwise-valid state and applies
            // exactly the change needed to hit one named branch, mirroring
            // the existing dedicated unit tests in `state.rs` (reused
            // scenarios, not new ones, where a matching unit test already
            // exists) so there are two independent confirmations of the
            // same check, not two different claims about it. ---
            let targeted: [(&'static str, Vec<u8>); 6] = [
                ("UnsortedLedger", {
                    // Mirrors `state::tests::rejects_unsorted_ledger`:
                    // two rows pushed in descending currency_id order.
                    let mut s = VaultState::new_empty([0; 32], 0);
                    s.ledger.push(LedgerEntry {
                        key: LedgerKey {
                            dapp_id: DAPP,
                            account_id: [2; 32],
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id: 9,
                        },
                        amount: 1,
                    });
                    s.ledger.push(LedgerEntry {
                        key: LedgerKey {
                            dapp_id: DAPP,
                            account_id: [2; 32],
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id: 7,
                        },
                        amount: 1,
                    });
                    s.serialize()
                }),
                ("ZeroLedgerAmount", {
                    // Mirrors `state::tests::rejects_zero_amount_row`.
                    let mut s = VaultState::new_empty([0; 32], 0);
                    s.ledger.push(LedgerEntry {
                        key: LedgerKey {
                            dapp_id: DAPP,
                            account_id: [2; 32],
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id: 7,
                        },
                        amount: 0,
                    });
                    s.serialize()
                }),
                ("BadLedgerCurrencyShape", {
                    // Mirrors `state::tests::rejects_shell_ledger_row`:
                    // CURRENCY_TAG_EXTRA paired with SHELL_CURRENCY_ID.
                    let mut s = VaultState::new_empty([0; 32], 0);
                    s.ledger.push(LedgerEntry {
                        key: LedgerKey {
                            dapp_id: DAPP,
                            account_id: [2; 32],
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id: SHELL_CURRENCY_ID,
                        },
                        amount: 1,
                    });
                    s.serialize()
                }),
                ("UnsortedPending", {
                    // No existing unit test covers this specific branch
                    // (the pre-existing suite only has the ledger-side
                    // `rejects_unsorted_ledger`) — two pending records,
                    // withdrawal_id descending.
                    let mut s = VaultState::new_empty([0; 32], 0);
                    s.pending.push(PendingWithdrawal {
                        withdrawal_id: 5,
                        depositor_dapp_id: DAPP,
                        depositor_account_id: [1; 32],
                        currency_tag: CURRENCY_TAG_EXTRA,
                        currency_id: 10,
                        amount: 1,
                    });
                    s.pending.push(PendingWithdrawal {
                        withdrawal_id: 3,
                        depositor_dapp_id: DAPP,
                        depositor_account_id: [2; 32],
                        currency_tag: CURRENCY_TAG_EXTRA,
                        currency_id: 11,
                        amount: 1,
                    });
                    s.next_withdrawal_id = 10;
                    s.serialize()
                }),
                ("PendingOverCap", {
                    // Mirrors `state::tests::rejects_pending_count_over_global_cap`:
                    // MAX_PENDING_WITHDRAWALS (32) + 1 = 33 valid, sorted
                    // pending records.
                    let mut s = VaultState::new_empty([0; 32], 0);
                    for id in 1..=(MAX_PENDING_WITHDRAWALS as u32 + 1) {
                        s.pending.push(PendingWithdrawal {
                            withdrawal_id: id,
                            depositor_dapp_id: DAPP,
                            depositor_account_id: [id as u8; 32],
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id: 10,
                            amount: 1,
                        });
                    }
                    s.next_withdrawal_id = MAX_PENDING_WITHDRAWALS as u32 + 2;
                    s.serialize()
                }),
                ("NextWithdrawalIdTooLow", {
                    // Mirrors
                    // `state::tests::rejects_next_withdrawal_id_not_greater_than_any_pending_id`.
                    let mut s = VaultState::new_empty([0; 32], 0);
                    s.pending.push(PendingWithdrawal {
                        withdrawal_id: 5,
                        depositor_dapp_id: [1; 32],
                        depositor_account_id: [2; 32],
                        currency_tag: CURRENCY_TAG_EXTRA,
                        currency_id: 7,
                        amount: 1,
                    });
                    s.next_withdrawal_id = 5;
                    s.serialize()
                }),
            ];

            for (expected, buf) in targeted {
                match VaultState::parse_with_reason(&buf) {
                    Ok(_) => panic!(
                        "targeted case for {expected:?} was expected to fail parse(), \
                         but it parsed Ok — the fixture itself is wrong"
                    ),
                    Err(reason) => {
                        let actual = reason_name(reason);
                        assert_eq!(
                            actual, expected,
                            "targeted case meant to hit {expected:?} instead hit {actual:?} — \
                             the fixture doesn't exercise the branch it claims to"
                        );
                        *reason_counts.entry(actual).or_insert(0) += 1;
                    }
                }
            }

            println!(
                "[fuzz coverage] mutated_valid_state: ok={ok_count} \
                 reasons(random+targeted)={reason_counts:?}"
            );

            for required in [
                "UnsortedLedger",
                "ZeroLedgerAmount",
                "BadLedgerCurrencyShape",
                "UnsortedPending",
                "PendingOverCap",
                "NextWithdrawalIdTooLow",
            ] {
                assert!(
                    reason_counts.contains_key(required),
                    "named branch {required:?} was never reached by either the random \
                     mutation loop or the targeted cases"
                );
            }
        }

        /// One ledger row's 85 raw bytes, content biased (not uniform)
        /// so the matching branch of the test below can actually reach
        /// `parse()`'s per-row logic instead of being filtered out at
        /// the very first check almost every time: `currency_tag` is a
        /// valid value (0 or 1) 80% of the time (fully random, mostly
        /// invalid, the other 20%, to still exercise `BadLedgerCurrencyShape`
        /// via a bad tag specifically); `currency_id` for a valid Extra
        /// tag is `SHELL_CURRENCY_ID` 10% of the time (the other
        /// sub-case of the same reason); `amount` is zero 1-in-16 times.
        fn gen_ledger_row(rng: &mut Lcg) -> [u8; LEDGER_ENTRY_LEN] {
            let mut row = [0u8; LEDGER_ENTRY_LEN];
            for byte in row.iter_mut().take(64) {
                *byte = rng.byte();
            }
            let tag: u8 = if rng.below(10) < 8 {
                if rng.coin() {
                    0
                } else {
                    1
                }
            } else {
                rng.byte()
            };
            row[64] = tag;
            let id: u32 = if tag == 0 {
                if rng.below(10) < 8 {
                    0
                } else {
                    rng.next() as u32
                }
            } else if tag == 1 {
                if rng.below(10) < 1 {
                    SHELL_CURRENCY_ID
                } else {
                    rng.next() as u32
                }
            } else {
                rng.next() as u32
            };
            row[65..69].copy_from_slice(&id.to_le_bytes());
            let amount: u128 = if rng.below(16) == 0 { 0 } else { rng.u128_val().max(1) };
            row[69..85].copy_from_slice(&amount.to_le_bytes());
            row
        }

        /// Same bias strategy as `gen_ledger_row`, for an 89-byte
        /// pending record with the given `withdrawal_id` already
        /// decided by the caller (so it can control sort order across
        /// a run of records independently of this function).
        fn gen_pending_row(rng: &mut Lcg, withdrawal_id: u32) -> [u8; PENDING_RECORD_LEN] {
            let mut row = [0u8; PENDING_RECORD_LEN];
            row[0..4].copy_from_slice(&withdrawal_id.to_le_bytes());
            for byte in row.iter_mut().take(68).skip(4) {
                *byte = rng.byte();
            }
            let tag: u8 = if rng.below(10) < 8 {
                if rng.coin() {
                    0
                } else {
                    1
                }
            } else {
                rng.byte()
            };
            row[68] = tag;
            let id: u32 = if tag == 0 {
                if rng.below(10) < 8 {
                    0
                } else {
                    rng.next() as u32
                }
            } else if tag == 1 {
                if rng.below(10) < 1 {
                    SHELL_CURRENCY_ID
                } else {
                    rng.next() as u32
                }
            } else {
                rng.next() as u32
            };
            row[69..73].copy_from_slice(&id.to_le_bytes());
            let amount: u128 = if rng.below(16) == 0 { 0 } else { rng.u128_val().max(1) };
            row[73..89].copy_from_slice(&amount.to_le_bytes());
            row
        }

        /// Gate 3b TASK P (corrected at Gate 3c TASK 1): the original
        /// version of this test drew fully-random `u32` declared
        /// `ledger_count`/`pending_count` values, then separately
        /// CLAMPED the body size to 300 rows / 50 pending when sizing
        /// the "matching" body — so the body's real length almost never
        /// actually matched the (unclamped, typically enormous) declared
        /// counts, and REVIEW.md entry 47's claim that "about half" of
        /// iterations get a genuinely consistent body was false: nearly
        /// every iteration fell through to `LengthMismatch`, the same
        /// single reason, regardless of which branch it took. Fixed
        /// here, test-first (confirmed by running it before fixing it —
        /// see the Gate 3c REVIEW entry for the failing numbers): the
        /// matching branch now draws genuinely SMALL counts directly
        /// (`ledger_count` biased toward 0-3 but ranging up to 300,
        /// `pending_count` similarly up to 32 plus a few above it) and
        /// builds the body from exactly that many rows via
        /// `gen_ledger_row`/`gen_pending_row` above, so the lengths are
        /// correct by construction, every time — not by chance. The
        /// mismatched branch (large/random declared counts paired with
        /// an unrelated random-length body) is kept as-is, preserving
        /// its own coverage of the length-check/overflow path.
        #[test]
        fn parse_never_panics_on_header_aware_random_bytes() {
            let mut rng = Lcg(0xC0FF_EE15_600D_F00D);
            let mut ok_count = 0u64;
            let mut reason_counts: BTreeMap<String, u64> = BTreeMap::new();
            let mut passed_length_check = 0u64;

            for seed in 0..ITERATIONS {
                let matching = rng.coin();

                let (ledger_count, pending_count, ledger_bytes, pending_bytes, next_withdrawal_id) =
                    if matching {
                        let ledger_count: u32 =
                            if rng.coin() { rng.below(4) as u32 } else { rng.below(300) as u32 };
                        let pending_count: u32 =
                            if rng.coin() { rng.below(4) as u32 } else { rng.below(40) as u32 };

                        let mut ledger_bytes =
                            Vec::with_capacity(ledger_count as usize * LEDGER_ENTRY_LEN);
                        for _ in 0..ledger_count {
                            ledger_bytes.extend_from_slice(&gen_ledger_row(&mut rng));
                        }

                        let mut pending_bytes =
                            Vec::with_capacity(pending_count as usize * PENDING_RECORD_LEN);
                        let mut last_id: Option<u32> = None;
                        let mut cur: u32 = 1 + rng.below(1000) as u32;
                        for _ in 0..pending_count {
                            pending_bytes.extend_from_slice(&gen_pending_row(&mut rng, cur));
                            last_id = Some(cur);
                            // 70% ascending (lets multi-record states
                            // sometimes stay sorted), 30% a fresh random
                            // value (routinely breaks sort order).
                            cur = if rng.below(10) < 7 {
                                cur.wrapping_add(1 + rng.below(5) as u32)
                            } else {
                                rng.next() as u32
                            };
                        }
                        let next_withdrawal_id = match last_id {
                            // 50/50: equal to the last id (invalid) vs.
                            // genuinely greater (valid), so both sides of
                            // this specific check get real coverage too.
                            Some(id) if rng.coin() => id,
                            Some(id) => id.wrapping_add(1 + rng.below(5) as u32),
                            None => rng.below(1000) as u32,
                        };
                        (
                            ledger_count,
                            pending_count,
                            ledger_bytes,
                            pending_bytes,
                            next_withdrawal_id,
                        )
                    } else {
                        // Mismatched branch, unchanged in spirit from the
                        // original: fully random (including huge) declared
                        // counts, paired with an unrelated random-length
                        // body — keeps exercising the length-check and
                        // (on a 32-bit `usize` target, not this 64-bit
                        // host) overflow paths.
                        let ledger_count = rng.next() as u32;
                        let pending_count = rng.next() as u32;
                        let body_len = rng.below(20_000) as usize;
                        let mut body = Vec::with_capacity(body_len);
                        for _ in 0..body_len {
                            body.push(rng.byte());
                        }
                        (ledger_count, pending_count, body, Vec::new(), rng.next() as u32)
                    };

                let mut buf =
                    Vec::with_capacity(HEADER_LEN + ledger_bytes.len() + pending_bytes.len());
                buf.extend_from_slice(&MAGIC);
                buf.push(STATE_VERSION);
                for _ in 0..32 {
                    buf.push(rng.byte());
                }
                for _ in 0..32 {
                    buf.push(rng.byte());
                }
                buf.push(if rng.coin() { 0 } else { HEADER_FLAG_DEPOSITS_PAUSED });
                buf.extend_from_slice(&[0u8; 4]);
                buf.extend_from_slice(&(rng.next() as u32).to_le_bytes()); // admin_seq
                buf.extend_from_slice(&next_withdrawal_id.to_le_bytes());
                buf.extend_from_slice(&ledger_count.to_le_bytes());
                buf.extend_from_slice(&pending_count.to_le_bytes());
                buf.push(if rng.coin() { 0 } else { 1 });
                assert_eq!(
                    buf.len(),
                    HEADER_LEN,
                    "header construction must stay in sync with HEADER_LEN"
                );
                buf.extend_from_slice(&ledger_bytes);
                buf.extend_from_slice(&pending_bytes);

                let caught = std::panic::catch_unwind(|| VaultState::parse_with_reason(&buf));
                let outcome = match caught {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        panic!(
                            "seed {seed}: VaultState::parse panicked on a header-aware random \
                             buffer (matching={matching}, ledger_count={ledger_count}, \
                             pending_count={pending_count}, total length {}) — an Err return \
                             was always an acceptable outcome; a panic is not",
                            buf.len()
                        );
                    }
                };
                match outcome {
                    Ok(_) => {
                        ok_count += 1;
                        passed_length_check += 1;
                    }
                    Err(reason) => {
                        let name = reason_name(reason);
                        if name != "LengthMismatch" && name != "CountOverflow" {
                            passed_length_check += 1;
                        }
                        *reason_counts.entry(name).or_insert(0) += 1;
                    }
                }
            }

            let passed_pct = 100.0 * passed_length_check as f64 / ITERATIONS as f64;
            println!(
                "[fuzz coverage] header_aware: ok={ok_count} \
                 passed_length_check={passed_length_check}/{ITERATIONS} ({passed_pct:.1}%) \
                 reasons={reason_counts:?}"
            );

            assert!(
                passed_length_check * 4 >= ITERATIONS,
                "only {passed_pct:.1}% of iterations got past the length check \
                 (need >= 25%) — reasons observed: {reason_counts:?}"
            );
            assert!(
                reason_counts.len() >= 5,
                "only {} distinct Err reasons occurred (need >= 5): {reason_counts:?}",
                reason_counts.len()
            );
        }
    }

    /// Gate 3e TASK F2: the callback fast path's own dedicated test
    /// suite, beyond the 200-seed differential already folded into
    /// `tests::property` (above). Every scenario here either compares
    /// the fast path (`handlers::handle_callback_fast`, reached via the
    /// real `MultiCurrencyVault::process_message` dispatch) against the
    /// slow path (`handlers::handle_callback`, `#[cfg(test)]`-only) on
    /// the identical input, or exercises a behavior specific to what
    /// the fast path does and does not validate.
    mod fast_path {
        use super::*;

        const DEPOSITOR_A: [u8; 32] = [61; 32];
        const DEPOSITOR_B: [u8; 32] = [62; 32];

        /// Runs ONE callback (`withdrawal_id`, `status_code`) against
        /// `state_bytes` through both the real dispatch (fast path) and
        /// the slow path directly, asserting their encoded outputs are
        /// byte-identical. Returns the fast path's own output for
        /// further assertions by the caller.
        fn assert_paths_agree(
            state_bytes: &[u8],
            withdrawal_id: u32,
            depositor: [u8; 32],
            status_code: u32,
        ) -> MessageOutput {
            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(withdrawal_id),
                (DAPP, depositor),
                (DAPP, DAPP),
                [0xCD; 32],
                status_code,
            );
            let acmi_bytes = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, depositor),
                [0; 32],
                0,
                &[],
                state_bytes,
                &message_bytes,
            );
            let fast_input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let slow_input = avm_contract_sdk::decode_message_input(&acmi_bytes)
                .expect("real ACMI buffer must decode");
            let pre_state = VaultState::parse(state_bytes).expect("pre-state parses");
            let slow_output = handlers::handle_callback(&slow_input, pre_state);
            let fast_output = MultiCurrencyVault::process_message(fast_input);
            assert_eq!(
                fast_output.status_code, slow_output.status_code,
                "fast/slow status_code mismatch"
            );
            let fast_bytes =
                avm_contract_sdk::encode_message_output(&fast_output).expect("fast encodes");
            let slow_bytes =
                avm_contract_sdk::encode_message_output(&slow_output).expect("slow encodes");
            assert_eq!(fast_bytes, slow_bytes, "fast/slow encoded output mismatch");
            fast_output
        }

        type LedgerFixtureRow = (([u8; 32], [u8; 32]), u32, u128);

        fn seed_state_with_pending(
            ledger: &[LedgerFixtureRow],
            pending_id: u32,
            pending_depositor: [u8; 32],
            pending_currency_id: u32,
            pending_amount: u128,
        ) -> VaultState {
            let mut state = VaultState::new_empty([9; 32], 0);
            for &((dapp, account), currency_id, amount) in ledger {
                state
                    .credit_ledger(
                        LedgerKey {
                            dapp_id: dapp,
                            account_id: account,
                            currency_tag: CURRENCY_TAG_EXTRA,
                            currency_id,
                        },
                        amount,
                    )
                    .expect("seed ledger row");
            }
            state.pending.push(PendingWithdrawal {
                withdrawal_id: pending_id,
                depositor_dapp_id: DAPP,
                depositor_account_id: pending_depositor,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: pending_currency_id,
                amount: pending_amount,
            });
            state.next_withdrawal_id = pending_id + 1;
            state
        }

        /// F2(a): every hand-built callback scenario this suite covers,
        /// run through both paths.
        #[test]
        fn fixtures_match_slow_path() {
            // Settle on a 1-row ledger.
            let state =
                seed_state_with_pending(&[((DAPP, DEPOSITOR_A), 7, 1000)], 1, DEPOSITOR_A, 7, 100);
            let output = assert_paths_agree(&state.serialize(), 1, DEPOSITOR_A, 0);
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(result.pending.len(), 0);
            assert_eq!(
                result.ledger_balance(&LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR_A,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 7
                }),
                1000
            );

            // Restore adding to an existing row.
            let state =
                seed_state_with_pending(&[((DAPP, DEPOSITOR_A), 7, 900)], 1, DEPOSITOR_A, 7, 100);
            let output = assert_paths_agree(&state.serialize(), 1, DEPOSITOR_A, 1);
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(
                result.ledger_balance(&LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR_A,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 7
                }),
                1000
            );

            // Restore into a fully-drained (removed) row -- must insert.
            let mut state = VaultState::new_empty([9; 32], 0);
            let key = LedgerKey {
                dapp_id: DAPP,
                account_id: DEPOSITOR_A,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
            };
            state.credit_ledger(key, 100).expect("seed");
            state.debit_ledger(&key, 100).expect("drain");
            assert!(state.ledger.is_empty());
            state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: DEPOSITOR_A,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 250,
            });
            state.next_withdrawal_id = 2;
            let output = assert_paths_agree(&state.serialize(), 1, DEPOSITOR_A, 1);
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(result.ledger.len(), 1);
            assert_eq!(result.ledger_balance(&key), 250);
        }

        /// F2(b): the ordering trap. A ledger with rows at currency ids
        /// 1, 255, 256, and 257 (all for the SAME depositor, so only
        /// `currency_id` distinguishes them); restoring into each must
        /// land the fast path on the identical row the slow path would
        /// use. Raw little-endian byte comparison of ids 1 vs 256 would
        /// disagree with numeric comparison (`LedgerKey`'s own derived
        /// `Ord`); this is exactly the case that would catch it.
        #[test]
        fn restore_ordering_matches_slow_path_for_currency_ids_1_255_256_257() {
            for target_id in [1u32, 255, 256, 257] {
                let mut state = VaultState::new_empty([9; 32], 0);
                for id in [1u32, 255, 256, 257] {
                    if id != target_id {
                        state
                            .credit_ledger(
                                LedgerKey {
                                    dapp_id: DAPP,
                                    account_id: DEPOSITOR_A,
                                    currency_tag: CURRENCY_TAG_EXTRA,
                                    currency_id: id,
                                },
                                1,
                            )
                            .expect("seed filler");
                    }
                }
                state.pending.push(PendingWithdrawal {
                    withdrawal_id: 1,
                    depositor_dapp_id: DAPP,
                    depositor_account_id: DEPOSITOR_A,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: target_id,
                    amount: 500,
                });
                state.next_withdrawal_id = 2;
                let output = assert_paths_agree(&state.serialize(), 1, DEPOSITOR_A, 1);
                let result = test_support::expect_full_replace_state(&output);
                assert_eq!(
                    result.ledger_balance(&LedgerKey {
                        dapp_id: DAPP,
                        account_id: DEPOSITOR_A,
                        currency_tag: CURRENCY_TAG_EXTRA,
                        currency_id: target_id
                    }),
                    500,
                    "target_id={target_id}"
                );
            }
        }

        /// F2(c): restore into an existing row (add), into a removed
        /// row (insert), and an overflow case (1529) — all at the true
        /// 252-row worst case, matching the slow path exactly.
        #[test]
        fn restore_at_252_rows_insert_add_and_overflow() {
            fn ledger_with_fillers(filler_count: usize) -> VaultState {
                let mut state = VaultState::new_empty([9; 32], 0);
                for i in 0..filler_count {
                    let mut filler_dapp = [0u8; 32];
                    filler_dapp[28..32].copy_from_slice(&(i as u32).to_be_bytes());
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
                state
            }

            // (add) 251 fillers (never colliding with DAPP/DEPOSITOR_A)
            // + DEPOSITOR_A's own real row = 252, restoring into the
            // existing real row.
            let mut state = ledger_with_fillers(251);
            state
                .credit_ledger(
                    LedgerKey {
                        dapp_id: DAPP,
                        account_id: DEPOSITOR_A,
                        currency_tag: CURRENCY_TAG_EXTRA,
                        currency_id: 7,
                    },
                    900,
                )
                .expect("seed real row");
            state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: DEPOSITOR_A,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 50,
            });
            state.next_withdrawal_id = 2;
            assert_eq!(state.ledger.len(), 252);
            let output = assert_paths_agree(&state.serialize(), 1, DEPOSITOR_A, 1);
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(result.ledger.len(), 252, "add must not create a new row");
            assert_eq!(
                result.ledger_balance(&LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR_A,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 7
                }),
                950
            );

            // (insert) 251 fillers + a pending record for a brand-new
            // 252nd key -- restore must insert it, bypassing any cap.
            let mut state = ledger_with_fillers(251);
            state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: DEPOSITOR_A,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 999,
                amount: 50,
            });
            state.next_withdrawal_id = 2;
            assert_eq!(state.ledger.len(), 251);
            let output = assert_paths_agree(&state.serialize(), 1, DEPOSITOR_A, 1);
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(result.ledger.len(), 252, "insert must create exactly one new row");

            // (overflow) a near-u128::MAX row restored into -> 1529,
            // both paths, no state update either way.
            let mut state = VaultState::new_empty([9; 32], 0);
            let key = LedgerKey {
                dapp_id: DAPP,
                account_id: DEPOSITOR_A,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
            };
            state.credit_ledger(key, u128::MAX - 50).expect("seed near-max");
            state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: DEPOSITOR_A,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 100,
            });
            state.next_withdrawal_id = 2;
            let output = assert_paths_agree(&state.serialize(), 1, DEPOSITOR_A, 1);
            assert_eq!(output.status_code, ERR_LEDGER_AMOUNT_OVERFLOW);
            assert!(output.state_update.is_none());
        }

        /// F2(d): unknown id, wrong caller, wrong magic, and a
        /// paused-deposits callback still resolving — all via the REAL
        /// dispatch (the fast path specifically, not the slow path
        /// directly, which is what this task calls for).
        #[test]
        fn fast_path_rejects_unknown_wrong_caller_wrong_magic_and_resolves_while_paused() {
            let state = seed_state_with_pending(&[], 1, DEPOSITOR_A, 7, 100);
            let bytes = state.serialize();

            // Unknown withdrawal id.
            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(999),
                (DAPP, DEPOSITOR_A),
                (DAPP, DAPP),
                [0xCD; 32],
                0,
            );
            let acmi = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, DEPOSITOR_A),
                [0; 32],
                0,
                &[],
                &bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi).expect("decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, ERR_WITHDRAWAL_NOT_FOUND);

            // Wrong caller (not the pending record's own depositor).
            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, DEPOSITOR_B),
                (DAPP, DAPP),
                [0xCD; 32],
                0,
            );
            let acmi = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, DEPOSITOR_B),
                [0; 32],
                0,
                &[],
                &bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi).expect("decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, ERR_INVALID_CALLBACK);

            // Wrong magic (method_id's upper 32 bits are NORMAL_METHOD_TAG,
            // not CALLBACK_MAGIC_TAG).
            let message_bytes = test_support::callback_message_bytes(
                DEPOSIT_METHOD_ID,
                (DAPP, DEPOSITOR_A),
                (DAPP, DAPP),
                [0xCD; 32],
                0,
            );
            let acmi = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, DEPOSITOR_A),
                [0; 32],
                0,
                &[],
                &bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi).expect("decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, ERR_INVALID_CALLBACK);

            // Paused deposits: callback still resolves (deposits-paused
            // only ever gates DEPOSIT, never a callback).
            let mut paused_state = seed_state_with_pending(&[], 1, DEPOSITOR_A, 7, 100);
            paused_state.deposits_paused = true;
            let paused_bytes = paused_state.serialize();
            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, DEPOSITOR_A),
                (DAPP, DAPP),
                [0xCD; 32],
                0,
            );
            let acmi = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, DEPOSITOR_A),
                [0; 32],
                0,
                &[],
                &paused_bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi).expect("decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(
                output.status_code, 0,
                "a callback must resolve regardless of deposits_paused"
            );
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(result.pending.len(), 0);
        }

        /// F2(e): a corrupted ledger row (invalid currency_tag) never
        /// blocks a Settle through the fast path — the accepted,
        /// documented consequence of skipping ledger-content validation
        /// on this path (SPEC.md §1). The very next DEPOSIT/WITHDRAW,
        /// which still goes through the full `parse()`, DOES catch it.
        #[test]
        fn corrupted_ledger_content_settle_succeeds_then_next_op_rejects() {
            let state =
                seed_state_with_pending(&[((DAPP, DEPOSITOR_A), 7, 1000)], 1, DEPOSITOR_A, 7, 100);
            let mut bytes = state.serialize();
            // Corrupt the one ledger row's currency_tag (offset
            // HEADER_LEN + 64) to a value that is neither 0 (Main) nor
            // 1 (Extra).
            bytes[HEADER_LEN + 64] = 0xFF;
            assert!(
                VaultState::parse(&bytes).is_err(),
                "the corrupted bytes must fail full parse()"
            );

            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, DEPOSITOR_A),
                (DAPP, DAPP),
                [0xCD; 32],
                0,
            );
            let acmi = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, DEPOSITOR_A),
                [0; 32],
                0,
                &[],
                &bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi).expect("decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(
                output.status_code, 0,
                "Settle must succeed despite the corrupted ledger row"
            );
            // Not `test_support::expect_full_replace_state` here: it
            // asserts the result parses cleanly, which this corrupted
            // state deliberately does not (the documented consequence
            // under test) -- pull the raw bytes out directly instead.
            let Some(StateUpdate::FullReplace(settled_bytes)) = output.state_update else {
                panic!("expected a FullReplace state update");
            };
            assert!(
                VaultState::parse(&settled_bytes).is_err(),
                "the Settle output must still carry the same corruption, unmodified"
            );

            // The next real operation on this same (still-corrupted)
            // state goes through full parse() and is rejected.
            let dep_body_bytes_dummy = test_support::internal_message_bytes(
                DEPOSIT_METHOD_ID,
                (DAPP, DEPOSITOR_A),
                (DAPP, DAPP),
                0,
                &[test_support::Currency { currency_id: 7, amount: 1 }],
                &[],
            );
            let next_acmi = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, DEPOSITOR_A),
                [0; 32],
                0,
                &[],
                &settled_bytes,
                &dep_body_bytes_dummy,
            );
            let next_input = avm_contract_sdk::decode_message_input(&next_acmi).expect("decode");
            let next_output = MultiCurrencyVault::process_message(next_input);
            assert_eq!(
                next_output.status_code, ERR_INVALID_STATE,
                "the very next DEPOSIT/WITHDRAW must reject the still-corrupted ledger"
            );
        }

        /// F2(f): random and header-aware random bytes fed directly
        /// into the fast path must never panic — mirrors
        /// `tests::fuzz_parse`'s own two strategies, targeting
        /// `VaultState::fast_callback` instead of `parse_with_reason`.
        #[test]
        fn fast_path_never_panics_on_random_bytes() {
            struct Lcg(u64);
            impl Lcg {
                fn next(&mut self) -> u64 {
                    self.0 = self
                        .0
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    self.0
                }

                fn below(&mut self, bound: u64) -> u64 {
                    self.next() % bound.max(1)
                }

                fn byte(&mut self) -> u8 {
                    (self.next() & 0xFF) as u8
                }
            }
            let mut rng = Lcg(0xFA57_7A74_0000_0001);
            for seed in 0..5_000u64 {
                let len = rng.below(3_000) as usize;
                let mut buf = Vec::with_capacity(len);
                for _ in 0..len {
                    buf.push(rng.byte());
                }
                let wid = rng.next() as u32;
                let status = rng.next() as u32;
                let dapp = [rng.byte(); 32];
                let account = [rng.byte(); 32];
                let result = std::panic::catch_unwind(|| {
                    VaultState::fast_callback(&buf, wid, status, dapp, account)
                });
                assert!(result.is_ok(), "seed {seed}: fast_callback panicked on random bytes");
            }
        }

        #[test]
        fn fast_path_never_panics_on_header_aware_random_bytes() {
            struct Lcg(u64);
            impl Lcg {
                fn next(&mut self) -> u64 {
                    self.0 = self
                        .0
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    self.0
                }

                fn below(&mut self, bound: u64) -> u64 {
                    self.next() % bound.max(1)
                }

                fn byte(&mut self) -> u8 {
                    (self.next() & 0xFF) as u8
                }
            }
            let mut rng = Lcg(0xFA57_7A74_0000_0002);
            for seed in 0..5_000u64 {
                let mut buf = Vec::new();
                buf.extend_from_slice(&MAGIC);
                buf.push(STATE_VERSION);
                for _ in 0..32 {
                    buf.push(rng.byte());
                }
                for _ in 0..32 {
                    buf.push(rng.byte());
                }
                buf.push(if rng.below(2) == 0 { 0 } else { HEADER_FLAG_DEPOSITS_PAUSED });
                buf.extend_from_slice(&[0u8; 4]);
                buf.extend_from_slice(&(rng.next() as u32).to_le_bytes());
                buf.extend_from_slice(&(rng.next() as u32).to_le_bytes());
                let ledger_count = rng.below(400) as u32;
                let pending_count = rng.below(60) as u32;
                buf.extend_from_slice(&ledger_count.to_le_bytes());
                buf.extend_from_slice(&pending_count.to_le_bytes());
                buf.push(if rng.below(2) == 0 { 0 } else { 1 });
                for _ in 0..(ledger_count as usize * LEDGER_ENTRY_LEN) {
                    buf.push(rng.byte());
                }
                for _ in 0..(pending_count as usize * PENDING_RECORD_LEN) {
                    buf.push(rng.byte());
                }
                let wid = rng.next() as u32;
                let status = rng.next() as u32;
                let dapp = [rng.byte(); 32];
                let account = [rng.byte(); 32];
                let result = std::panic::catch_unwind(|| {
                    VaultState::fast_callback(&buf, wid, status, dapp, account)
                });
                assert!(
                    result.is_ok(),
                    "seed {seed}: fast_callback panicked on header-aware random bytes"
                );
            }
        }

        /// Mutation-testing gap found by a fresh `cargo mutants` run this
        /// round: every fixture above uses `build_flags == 0`, so a mutant
        /// deleting the `!` in `fast_callback`'s own
        /// `build_flags & !1 != 0` check (its own copy of the same check
        /// `round_trips_build_flags_one` already covers for `parse()`)
        /// had nothing to catch it. Dispatches a real callback through
        /// the fast path with build_flags == 1 (must resolve, matching
        /// the slow path) and build_flags == 2, a reserved bit (must
        /// still reject), confirming the fast path's own check does the
        /// same job as parse()'s rather than a vacuous one.
        #[test]
        fn fast_path_accepts_build_flag_one_and_rejects_reserved_bit() {
            let mut state =
                seed_state_with_pending(&[((DAPP, DEPOSITOR_A), 7, 1000)], 1, DEPOSITOR_A, 7, 100);
            state.build_flags = 1;
            let output = assert_paths_agree(&state.serialize(), 1, DEPOSITOR_A, 0);
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(result.build_flags, 1);
            assert_eq!(result.pending.len(), 0);

            let mut bad_state =
                seed_state_with_pending(&[((DAPP, DEPOSITOR_A), 7, 1000)], 1, DEPOSITOR_A, 7, 100);
            bad_state.build_flags = 2;
            let bytes = bad_state.serialize();
            let message_bytes = test_support::callback_message_bytes(
                withdrawal_callback_method_id(1),
                (DAPP, DEPOSITOR_A),
                (DAPP, DAPP),
                [0xCD; 32],
                0,
            );
            let acmi = test_support::build_message_input(
                (DAPP, DAPP),
                true,
                (DAPP, DEPOSITOR_A),
                [0; 32],
                0,
                &[],
                &bytes,
                &message_bytes,
            );
            let input = avm_contract_sdk::decode_message_input(&acmi).expect("decode");
            let output = MultiCurrencyVault::process_message(input);
            assert_eq!(output.status_code, ERR_INVALID_STATE);
        }

        /// Mutation-testing gap: every fixture above has exactly one
        /// pending record (row_index == 0), so a mutant replacing `*`
        /// with `/` in `fast_callback`'s
        /// `let start = row_index * PENDING_RECORD_LEN;` (only
        /// observable once row_index >= 1, since `0 * N == 0 / N`) had
        /// nothing to catch it. Two pending records (ids 1 and 2); the
        /// callback resolves id 2, which sits at row_index == 1, for
        /// both Settle and Restore.
        #[test]
        fn fast_path_resolves_pending_record_at_nonzero_row_index() {
            let mut state = VaultState::new_empty([9; 32], 0);
            state.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: DEPOSITOR_A,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 5,
                amount: 10,
            });
            state.pending.push(PendingWithdrawal {
                withdrawal_id: 2,
                depositor_dapp_id: DAPP,
                depositor_account_id: DEPOSITOR_B,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 100,
            });
            state
                .credit_ledger(
                    LedgerKey {
                        dapp_id: DAPP,
                        account_id: DEPOSITOR_B,
                        currency_tag: CURRENCY_TAG_EXTRA,
                        currency_id: 7,
                    },
                    1000,
                )
                .expect("seed real row");
            state.next_withdrawal_id = 3;
            let output = assert_paths_agree(&state.serialize(), 2, DEPOSITOR_B, 0);
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(result.pending.len(), 1, "only the id-2 record must be removed");
            assert_eq!(
                result.pending[0].withdrawal_id, 1,
                "the id-1 record must survive untouched"
            );
            assert_eq!(
                result.ledger_balance(&LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR_B,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 7
                }),
                1000
            );

            let mut state2 = VaultState::new_empty([9; 32], 0);
            state2.pending.push(PendingWithdrawal {
                withdrawal_id: 1,
                depositor_dapp_id: DAPP,
                depositor_account_id: DEPOSITOR_A,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 5,
                amount: 10,
            });
            state2.pending.push(PendingWithdrawal {
                withdrawal_id: 2,
                depositor_dapp_id: DAPP,
                depositor_account_id: DEPOSITOR_B,
                currency_tag: CURRENCY_TAG_EXTRA,
                currency_id: 7,
                amount: 100,
            });
            state2.next_withdrawal_id = 3;
            let output = assert_paths_agree(&state2.serialize(), 2, DEPOSITOR_B, 1);
            let result = test_support::expect_full_replace_state(&output);
            assert_eq!(result.pending.len(), 1, "only the id-2 record must be removed");
            assert_eq!(
                result.pending[0].withdrawal_id, 1,
                "the id-1 record must survive untouched"
            );
            assert_eq!(
                result.ledger_balance(&LedgerKey {
                    dapp_id: DAPP,
                    account_id: DEPOSITOR_B,
                    currency_tag: CURRENCY_TAG_EXTRA,
                    currency_id: 7
                }),
                100,
                "restore must insert a new row for the id-2 record's key"
            );
        }
    }
}
