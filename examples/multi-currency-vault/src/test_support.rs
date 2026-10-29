//! Host-level test support only (`#[cfg(test)]`, compiled into no normal
//! build). Hand-builds exact `ACMI` byte buffers and feeds them through the
//! SDK's own `avm_contract_sdk::decode_message_input` — `avm-contract-sdk`
//! exposes no higher-level constructor for `MessageInput` (`CurrencyAmounts`
//! and `CanonicalMessage` have no public constructor other than the wire
//! decoder itself), so this reproduces the exact, documented wire format
//! from `crates/avm-contract-sdk/src/input.rs` byte-for-byte, rather than
//! inventing a new API (Hard Rule 3).

// (No `#![cfg(test)]` here — `lib.rs` already declares
// `#[cfg(test)] mod test_support;`, and a second `cfg(test)` on this file
// itself would be a duplicate attribute.)
#![allow(clippy::too_many_arguments)] // test-only wire-format builders; one argument per wire field is clearer here than a builder struct for a handful of call sites

use avm_contract_sdk::decode_message_input;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::StateUpdate;

use crate::state::VaultState;

// Internal-message wire-layout constants, mirrored from
// `crates/avm-contract-sdk/src/input.rs` (private to that crate, so
// reproduced here for test construction only).
const INTERNAL_HEADER_LEN: usize = 192;
const EXTERNAL_HEADER_LEN: usize = 208;
const EXTERNAL_FLAG_SIGNED: u8 = 1;
const INTERNAL_FLAG_CALLBACK: u8 = 1;

pub struct Currency {
    pub currency_id: u32,
    pub amount: u128,
}

fn encode_extra(extra: &[Currency]) -> Vec<u8> {
    let mut sorted: Vec<&Currency> = extra.iter().collect();
    sorted.sort_by_key(|c| c.currency_id);
    let mut out = Vec::new();
    for currency in sorted {
        out.extend_from_slice(&currency.currency_id.to_le_bytes());
        out.extend_from_slice(&currency.amount.to_le_bytes());
    }
    out
}

/// Builds a minimal, flags=0 (no callback, no callback-request, no
/// event-reads) internal `CanonicalMessage` wire buffer.
pub fn internal_message_bytes(
    method_id: u64,
    source: ([u8; 32], [u8; 32]),
    destination: ([u8; 32], [u8; 32]),
    value_main: u128,
    extra: &[Currency],
    body: &[u8],
) -> Vec<u8> {
    let extra_bytes = encode_extra(extra);
    let mut header = vec![0u8; INTERNAL_HEADER_LEN];
    header[0] = 0; // tag: Internal
    header[1] = 0; // flags: none
    header[2..4].copy_from_slice(&(extra.len() as u16).to_le_bytes());
    header[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    header[8..16].copy_from_slice(&method_id.to_le_bytes());
    header[16..32].copy_from_slice(&value_main.to_le_bytes());
    header[32..48].copy_from_slice(&0u128.to_le_bytes()); // fuel_fee_shells
    header[48..56].copy_from_slice(&0u64.to_le_bytes()); // created_lt
    header[56..60].copy_from_slice(&0u32.to_le_bytes()); // created_at
                                                         // header[60..64] left zero (reserved)
    header[64..96].copy_from_slice(&source.0);
    header[96..128].copy_from_slice(&source.1);
    header[128..160].copy_from_slice(&destination.0);
    header[160..192].copy_from_slice(&destination.1);

    let mut out = header;
    out.extend_from_slice(&extra_bytes);
    out.extend_from_slice(body);
    out
}

/// Builds a minimal internal `CanonicalMessage` wire buffer with the
/// `CALLBACK` flag set, per `crates/avm-contract-sdk/src/input.rs`'s own
/// `decode_internal_message` rules (Gate 2a round 2 item 6): immediately
/// after the 192-byte header comes `CallbackHeader` —
/// `cause_msg_hash(32) + cause_address.dapp_id(32) +
/// cause_address.account_id(32) + status_code(4)` — before `extra`/`body`.
/// The decoder requires `cause_msg_hash != [0; 32]`, `cause_address` to be
/// non-zero, and **`cause_address == destination`** exactly; it also
/// requires `method_id != 0` whenever `status_code == 0`, and an empty
/// `body` whenever `status_code != 0` — this contract's own
/// `handler_method_id` is always a non-zero `withdrawal_callback_method_id`
/// value and `body` is always empty here, so both hold unconditionally.
pub fn callback_message_bytes(
    handler_method_id: u64,
    source: ([u8; 32], [u8; 32]),
    destination: ([u8; 32], [u8; 32]),
    cause_msg_hash: [u8; 32],
    status_code: u32,
) -> Vec<u8> {
    let mut header = vec![0u8; INTERNAL_HEADER_LEN];
    header[0] = 0; // tag: Internal
    header[1] = INTERNAL_FLAG_CALLBACK;
    header[2..4].copy_from_slice(&0u16.to_le_bytes()); // extra_count
    header[4..8].copy_from_slice(&0u32.to_le_bytes()); // body_len
    header[8..16].copy_from_slice(&handler_method_id.to_le_bytes());
    header[16..32].copy_from_slice(&0u128.to_le_bytes()); // value_main
    header[32..48].copy_from_slice(&0u128.to_le_bytes()); // fuel_fee_shells
    header[48..56].copy_from_slice(&0u64.to_le_bytes()); // created_lt
    header[56..60].copy_from_slice(&0u32.to_le_bytes()); // created_at
                                                         // header[60..64] left zero (reserved)
    header[64..96].copy_from_slice(&source.0);
    header[96..128].copy_from_slice(&source.1);
    header[128..160].copy_from_slice(&destination.0);
    header[160..192].copy_from_slice(&destination.1);

    let mut out = header;
    out.extend_from_slice(&cause_msg_hash);
    out.extend_from_slice(&destination.0); // cause_address must equal destination exactly
    out.extend_from_slice(&destination.1);
    out.extend_from_slice(&status_code.to_le_bytes());
    out
}

/// Builds a minimal external `CanonicalMessage` wire buffer. The signature
/// bytes are an arbitrary, fixed 64-byte placeholder — `decode_message_input`
/// only checks `signature_len == 64`, never cryptographic validity (that is
/// a host/runtime-level concern entirely outside this guest-side decoder).
pub fn external_message_bytes(
    method_id: u64,
    pubkey: [u8; 32],
    destination: ([u8; 32], [u8; 32]),
    value_main: u128,
    extra: &[Currency],
    body: &[u8],
) -> Vec<u8> {
    let extra_bytes = encode_extra(extra);
    let signature = [0xAB_u8; 64];
    let mut header = vec![0u8; EXTERNAL_HEADER_LEN];
    header[0] = 1; // tag: External
    header[1] = EXTERNAL_FLAG_SIGNED;
    header[2..4].copy_from_slice(&(extra.len() as u16).to_le_bytes());
    header[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    header[8..16].copy_from_slice(&method_id.to_le_bytes());
    header[16..32].copy_from_slice(&value_main.to_le_bytes());
    header[32..48].copy_from_slice(&0u128.to_le_bytes()); // fuel_fee_shells
    header[48..56].copy_from_slice(&0u64.to_le_bytes()); // nonce
    header[56..60].copy_from_slice(&0u32.to_le_bytes()); // created_at
    header[60..64].copy_from_slice(&0u32.to_le_bytes()); // expire_at
    header[64..96].copy_from_slice(&destination.0);
    header[96..128].copy_from_slice(&destination.1);
    header[128..160].copy_from_slice(&pubkey);
    header[160..162].copy_from_slice(&64u16.to_le_bytes()); // signature_len
                                                            // header[162..208] left zero (reserved)

    let mut out = header;
    out.extend_from_slice(&extra_bytes);
    out.extend_from_slice(body);
    out.extend_from_slice(&signature);
    out
}

fn push_length_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Wraps a canonical-message buffer (from `internal_message_bytes` or
/// `external_message_bytes`) into a full `ACMI` `MessageInput` buffer, with
/// the account-level fields (own balance, own `extra`, own state) supplied
/// directly — these are field group "(a)" (SPEC.md §1), independent of
/// whatever the message itself carries.
pub fn build_message_input(
    destination: ([u8; 32], [u8; 32]),
    caller_is_internal: bool,
    caller_internal_source: ([u8; 32], [u8; 32]),
    caller_external_pubkey: [u8; 32],
    account_balance_main: u128,
    account_extra: &[Currency],
    state: &[u8],
    message_bytes: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"ACMI");
    // ExecutionContext: block_lt(8) + block_time(4) + block_seq_no(4) + block_rand(32)
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 32]);
    out.push(u8::from(!caller_is_internal)); // caller_kind: 0 = Internal, 1 = External
    out.extend_from_slice(&[0u8; 7]); // reserved
    if caller_is_internal {
        out.extend_from_slice(&caller_internal_source.0);
        out.extend_from_slice(&caller_internal_source.1);
        out.extend_from_slice(&[0u8; 32]); // external_pubkey must be zero
    } else {
        out.extend_from_slice(&[0u8; 32]); // src_dapp_id must be zero
        out.extend_from_slice(&[0u8; 32]); // src_account_id must be zero
        out.extend_from_slice(&caller_external_pubkey);
    }
    out.extend_from_slice(&destination.0);
    out.extend_from_slice(&destination.1);
    out.extend_from_slice(&[0u8; 32]); // selected_code_id
    out.push(0); // required_state_output_mode: FullReplace
    out.extend_from_slice(&[0u8; 7]); // reserved
    out.extend_from_slice(&account_balance_main.to_le_bytes());
    out.extend_from_slice(&(account_extra.len() as u16).to_le_bytes());
    out.extend_from_slice(&encode_extra(account_extra));
    push_length_prefixed(&mut out, state);
    push_length_prefixed(&mut out, message_bytes);
    push_length_prefixed(&mut out, &[]); // internal_event_results
    out
}

/// Convenience: builds and decodes an internal-caller `MessageInput` in one
/// call. Panics (test-only) if the hand-built wire bytes fail to decode —
/// that would indicate a bug in this test helper itself, not in the
/// contract under test.
pub fn internal_input<'a>(
    bytes_out: &'a mut Vec<u8>,
    destination: ([u8; 32], [u8; 32]),
    source: ([u8; 32], [u8; 32]),
    method_id: u64,
    value_main: u128,
    extra: &[Currency],
    body: &[u8],
    account_balance_main: u128,
    account_extra: &[Currency],
    state: &[u8],
) -> MessageInput<'a> {
    let message = internal_message_bytes(method_id, source, destination, value_main, extra, body);
    *bytes_out = build_message_input(
        destination,
        true,
        source,
        [0; 32],
        account_balance_main,
        account_extra,
        state,
        &message,
    );
    decode_message_input(bytes_out).expect("test helper produced an undecodable ACMI buffer")
}

/// Convenience: builds and decodes an external-caller `MessageInput`.
pub fn external_input<'a>(
    bytes_out: &'a mut Vec<u8>,
    destination: ([u8; 32], [u8; 32]),
    pubkey: [u8; 32],
    method_id: u64,
    value_main: u128,
    extra: &[Currency],
    body: &[u8],
    account_balance_main: u128,
    account_extra: &[Currency],
    state: &[u8],
) -> MessageInput<'a> {
    let message = external_message_bytes(method_id, pubkey, destination, value_main, extra, body);
    *bytes_out = build_message_input(
        destination,
        false,
        ([0; 32], [0; 32]),
        pubkey,
        account_balance_main,
        account_extra,
        state,
        &message,
    );
    decode_message_input(bytes_out).expect("test helper produced an undecodable ACMI buffer")
}

/// Extracts the `VaultState` a successful `MessageOutput` committed (Gate
/// 2a round 3 item 2). This contract only ever constructs
/// `StateUpdate::FullReplace` (`success_with_state` in `handlers.rs`),
/// never `DataPatchList` — panics (test-only) if that ever isn't the case,
/// which would indicate a real bug in the handler under test, not in this
/// helper.
pub fn expect_full_replace_state(output: &MessageOutput) -> VaultState {
    let Some(StateUpdate::FullReplace(bytes)) = &output.state_update else {
        panic!(
            "expected a successful MessageOutput carrying StateUpdate::FullReplace, got: {:?}",
            output.state_update
        );
    };
    VaultState::parse(bytes).expect("a real state_update must itself parse")
}

/// Counting global allocator (Gate 2a round 3 item 3b, made thread-safe
/// in the "THEN Phase 3" item 1 follow-up) — wraps `std::alloc::System`,
/// the real allocator this crate would otherwise use (this crate is not
/// `#![no_std]`; `avm-contract-sdk` itself is, but its `extern crate
/// alloc` usage shares whatever single global allocator the final binary
/// registers), adding only a running byte counter. Reports *total bytes
/// ever requested from the allocator* (every `alloc`, and every `realloc`
/// growth — the default `GlobalAlloc::realloc` is implemented in terms of
/// `self.alloc(new_layout)`, so it is counted here too), not peak resident
/// usage or the `GuestArena`'s own `arena_bytes` figure (a separate, much
/// smaller, fixed-size buffer used only for the wire-format bytes
/// crossing the host/guest ABI boundary — see `REVIEW.md` for the full
/// distinction this item asked to confirm).
///
/// `#[global_allocator]` is necessarily process-wide once declared, but
/// the counter itself is `thread_local!`, not a shared `static` — `cargo
/// test` runs each test on its own thread by default, and each thread
/// gets its own independent counter starting at 0 (thread-locals
/// initialize per-thread, on first access from that thread), so
/// concurrently-running tests can no longer corrupt each other's
/// before/after snapshots. The tests that read it
/// (`tests::counting_allocator::*` in `lib.rs`) no longer need to be run
/// alone.
pub struct CountingAllocator;

std::thread_local! {
    static ALLOCATED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

// `lib.rs` relaxes `forbid(unsafe_code)` to `deny` under `#[cfg(test)]`
// specifically so this one, explicitly-justified `#[allow]` is possible:
// `GlobalAlloc` requires `unsafe impl`/`unsafe fn` by its own definition in
// `std` (there is no safe trait to implement instead), and every method
// body here does nothing but delegate straight to `std::alloc::System`
// (the real allocator this crate already uses) plus an ordinary
// thread-local counter increment — no raw-pointer arithmetic, no manual
// memory management of this crate's own devising. Test-only; compiled
// into no normal build.
#[allow(unsafe_code)]
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATED_BYTES.with(|cell| cell.set(cell.get() + layout.size()));
        std::alloc::System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        std::alloc::System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static COUNTING_ALLOCATOR: CountingAllocator = CountingAllocator;

/// Current running total of bytes ever requested from the allocator, on
/// *this* thread, since that thread's first allocation.
pub fn allocated_bytes() -> usize {
    ALLOCATED_BYTES.with(core::cell::Cell::get)
}
