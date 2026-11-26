#![no_std]
#![deny(unsafe_code)]

//! Guest-side runtime and canonical byte codecs for AVM core-WASM contracts.
//!
//! Optional crypto helpers use the matching AVM runtime. The export macros install the
//! required AVM entrypoints over a bounded arena in the module's own linear
//! memory. Small live input/output allocations use a 4 KiB static region;
//! larger ones take heap memory on demand. The standard library supplies only
//! the arena lock.

extern crate alloc;
extern crate std;

pub mod crypto;
mod input;
mod output;
mod owner_key;
#[cfg(target_arch = "wasm32")]
mod runtime;

pub use input::{
    decode_batch_input, decode_message_input, BatchInput, BatchMessageIter, CallbackHeader, Caller,
    CanonicalMessage, CurrencyAmountIter, CurrencyAmounts, DecodeError, DeployInput,
    ExternalMessage, InternalEventReadRequest, InternalEventReadRequestIter,
    InternalEventReadRequests, InternalMessage, MessageInput,
};
#[cfg(feature = "event-rings")]
pub use input::{
    decode_event_fields, decode_internal_event_results, InternalEventReadResult, ReadableEvent,
};
#[cfg(feature = "system-mint")]
pub use output::encode_mint_output;
#[cfg(feature = "event-rings")]
pub use output::EventReadEntry;
pub use output::{
    encode_batch_output, encode_event_fields, encode_message_output, BatchOutput, CallbackIntent,
    CodeUpgradeIntent, EncodeError, EventField, EventIntent, EventKind, EventValue, MessageOutput,
    OutboundIntent, StatePatch, StateUpdate,
};
pub use owner_key::{owner_key, set_owner_key, OWNER_KEY_BYTES};
#[cfg(all(target_arch = "wasm32", feature = "system-mint"))]
#[doc(hidden)]
pub use runtime::dispatch_mint_message;
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub use runtime::{dispatch_batch, dispatch_init, dispatch_message, GuestArena};

pub type DappId = [u8; 32];
pub type AccountId = [u8; 32];
pub type SelectedCodeId = [u8; 32];
pub type CodeHash = [u8; 32];
pub type AvmAmount = u128;

/// The VM-reserved method identifier for an account-local code upgrade.
pub const CODE_UPGRADE_METHOD_ID: u64 = 0xc2c6_c51b_c610_249d;

/// Arena capacity supplied by the contract builder from project measurements.
/// A WASM export macro without an explicit capacity requires this build input.
pub const DEFAULT_GUEST_ARENA_BYTES: usize = arena_build_input();

const fn arena_build_input() -> usize {
    let Some(value) = option_env!("AVM_GUEST_ARENA_BYTES") else {
        return 0;
    };
    let bytes = value.as_bytes();
    let (mut result, mut index) = (0_usize, 0);
    while index < bytes.len() {
        assert!(bytes[index] >= b'0' && bytes[index] <= b'9', "invalid arena build input");
        result = match result.checked_mul(10) {
            Some(value) => value,
            None => panic!("arena build input overflow"),
        };
        result = match result.checked_add((bytes[index] - b'0') as usize) {
            Some(value) => value,
            None => panic!("arena build input overflow"),
        };
        index += 1;
    }
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountAddress {
    pub dapp_id: DappId,
    pub account_id: AccountId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CurrencyAmount {
    pub currency_id: u32,
    pub amount: AvmAmount,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    pub block_lt: u64,
    pub block_time: u32,
    pub block_seq_no: u32,
    pub block_rand: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StateOutputMode {
    FullReplace = 0,
    DataPatchList = 1,
}

/// High-level contract entrypoints used by [`export_message_contract!`].
pub trait Contract {
    fn init(input: MessageInput<'_>) -> MessageOutput;

    fn process_message(input: MessageInput<'_>) -> MessageOutput;
}

/// AccountBatch fast path used by [`export_batch_contract!`].
pub trait BatchContract: Contract {
    fn process_batch(input: BatchInput<'_>) -> BatchOutput;
}

/// A system minter: `process_message` also returns
/// the currencies it mints, which the host accepts only from an account and
/// code that the network config designates. `init` never mints. The exported
/// `process_message` of [`export_mint_contract`] calls `process_mint_message`.
/// The SDK feature `system-mint` compiles it; an ordinary contract leaves the
/// feature off and builds without the mint surface.
#[cfg(feature = "system-mint")]
pub trait MintContract: Contract {
    fn process_mint_message(
        input: MessageInput<'_>,
    ) -> (MessageOutput, alloc::vec::Vec<CurrencyAmount>);
}

/// Exports the AVM guest surface of a [`MintContract`]: the surface of
/// [`export_message_contract`] whose `process_message` may return a mint
/// intent.
#[cfg(feature = "system-mint")]
#[macro_export]
macro_rules! export_mint_contract {
    ($contract:ty) => {
        $crate::export_mint_contract!($contract, arena_bytes = $crate::DEFAULT_GUEST_ARENA_BYTES);
    };
    ($contract:ty, arena_bytes = $arena_bytes:expr) => {
        $crate::export_message_contract!(@exports $contract, $arena_bytes, dispatch_mint_message);

        // The exports exist only on WASM; native builds keep the entrypoints in use.
        #[cfg(not(target_arch = "wasm32"))]
        const _: for<'a> fn($crate::MessageInput<'a>) -> $crate::MessageOutput =
            <$contract as $crate::Contract>::init;
        #[cfg(not(target_arch = "wasm32"))]
        const _: fn() = || {
            let _ = <$contract as $crate::MintContract>::process_mint_message;
        };
    };
}

/// Exports the exact MessageIsolated AVM guest surface.
///
/// The optional `arena_bytes = ...` form selects the bounded input/output arena
/// capacity. The linker-owned `memory` export is installed by
/// `avm-contract-build`.
#[macro_export]
macro_rules! export_message_contract {
    ($contract:ty) => {
        $crate::export_message_contract!(
            $contract,
            arena_bytes = $crate::DEFAULT_GUEST_ARENA_BYTES
        );
    };
    ($contract:ty, arena_bytes = $arena_bytes:expr) => {
        $crate::export_message_contract!(@exports $contract, $arena_bytes, dispatch_message);

        // The exports exist only on WASM; native builds keep the entrypoints in use.
        #[cfg(not(target_arch = "wasm32"))]
        const _: [for<'a> fn($crate::MessageInput<'a>) -> $crate::MessageOutput; 2] =
            [<$contract as $crate::Contract>::init, <$contract as $crate::Contract>::process_message];
    };
    // The guest exports. `process_message` calls `$dispatch`: `dispatch_message`,
    // or `dispatch_mint_message` for `export_mint_contract!`.
    (@exports $contract:ty, $arena_bytes:expr, $dispatch:ident) => {
        #[cfg(target_arch = "wasm32")]
        const _: () = assert!($arena_bytes > 0 && $arena_bytes <= u32::MAX as usize,
            "arena capacity is missing; measure contract tests, set build.arena_bytes or the macro arena_bytes parameter");
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __avm_guest_arena_bytes() -> u32 { $arena_bytes as u32 }
        #[cfg(target_arch = "wasm32")]
        static __AVM_GUEST_ARENA: $crate::GuestArena<{ $arena_bytes }> = $crate::GuestArena::new();

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn avm_guest_alloc(len: u32) -> u32 {
            __AVM_GUEST_ARENA.allocate(len)
        }

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn avm_guest_free(ptr: u32, len: u32) {
            assert!(__AVM_GUEST_ARENA.free(ptr, len));
        }

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn init(input_ptr: u32, input_len: u32) -> u64 {
            $crate::dispatch_init::<$contract, { $arena_bytes }>(
                &__AVM_GUEST_ARENA,
                input_ptr,
                input_len,
            )
        }

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn process_message(input_ptr: u32, input_len: u32) -> u64 {
            $crate::$dispatch::<$contract, { $arena_bytes }>(
                &__AVM_GUEST_ARENA,
                input_ptr,
                input_len,
            )
        }
    };
}

/// Exports the exact AccountBatch AVM guest surface, including the mandatory
/// isolated fallback.
#[macro_export]
macro_rules! export_batch_contract {
    ($contract:ty) => {
        $crate::export_batch_contract!($contract, arena_bytes = $crate::DEFAULT_GUEST_ARENA_BYTES);
    };
    ($contract:ty, arena_bytes = $arena_bytes:expr) => {
        $crate::export_message_contract!($contract, arena_bytes = $arena_bytes);

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn process_batch(input_ptr: u32, input_len: u32) -> u64 {
            $crate::dispatch_batch::<$contract, { $arena_bytes }>(
                &__AVM_GUEST_ARENA,
                input_ptr,
                input_len,
            )
        }

        #[cfg(not(target_arch = "wasm32"))]
        const _: for<'a> fn($crate::BatchInput<'a>) -> $crate::BatchOutput =
            <$contract as $crate::BatchContract>::process_batch;
    };
}
