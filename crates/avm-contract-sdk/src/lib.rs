#![no_std]
#![forbid(unsafe_code)]

//! Guest-side runtime and canonical byte codecs for AVM core-WASM contracts.
//!
//! The crate deliberately has no host imports. The export macros install the
//! required AVM entrypoints over a bounded arena in the module's own linear
//! memory.

extern crate alloc;

mod input;
mod output;
mod runtime;

pub use input::{
    decode_batch_input, decode_message_input, BatchInput, BatchMessageIter, CallbackHeader, Caller,
    CanonicalMessage, CurrencyAmountIter, CurrencyAmounts, DecodeError, DeployInput,
    ExternalMessage, InternalEventReadRequest, InternalEventReadRequestIter,
    InternalEventReadRequests, InternalMessage, MessageInput,
};
pub use output::{
    encode_batch_output, encode_event_fields, encode_message_output, BatchOutput, CallbackIntent,
    CodeUpgradeIntent, EncodeError, EventField, EventIntent, EventKind, EventValue, MessageOutput,
    OutboundIntent, StatePatch, StateUpdate,
};
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
        #[cfg(target_arch = "wasm32")]
        const _: () = assert!($arena_bytes > 0 && $arena_bytes <= u32::MAX as usize,
            "arena capacity is missing; measure contract tests, set build.arena_bytes or the macro arena_bytes parameter");
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __avm_guest_arena_bytes() -> u32 { $arena_bytes as u32 }
        static __AVM_GUEST_ARENA: $crate::GuestArena<{ $arena_bytes }> = $crate::GuestArena::new();

        #[no_mangle]
        pub extern "C" fn avm_guest_alloc(len: u32) -> u32 {
            __AVM_GUEST_ARENA.allocate(len)
        }

        #[no_mangle]
        pub extern "C" fn avm_guest_free(ptr: u32, len: u32) {
            assert!(__AVM_GUEST_ARENA.free(ptr, len));
        }

        #[no_mangle]
        pub extern "C" fn init(input_ptr: u32, input_len: u32) -> u64 {
            $crate::dispatch_init::<$contract, { $arena_bytes }>(
                &__AVM_GUEST_ARENA,
                input_ptr,
                input_len,
            )
        }

        #[no_mangle]
        pub extern "C" fn process_message(input_ptr: u32, input_len: u32) -> u64 {
            $crate::dispatch_message::<$contract, { $arena_bytes }>(
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

        #[no_mangle]
        pub extern "C" fn process_batch(input_ptr: u32, input_len: u32) -> u64 {
            $crate::dispatch_batch::<$contract, { $arena_bytes }>(
                &__AVM_GUEST_ARENA,
                input_ptr,
                input_len,
            )
        }
    };
}
