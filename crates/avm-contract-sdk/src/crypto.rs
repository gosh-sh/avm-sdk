//! Synchronous cryptography provided by the matching AVM runtime.
//!
//! These helpers require WASM execution. Native calls fail explicitly.

/// Computes the ordinary unkeyed 32-byte BLAKE3 hash, including for empty input.
pub fn blake3(data: &[u8]) -> [u8; 32] {
    hash(1, data)
}

/// Computes the 32-byte SHA-256 digest, including for empty input.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    hash(2, data)
}

fn hash(id: i32, data: &[u8]) -> [u8; 32] {
    let mut output = [0; 32];
    invoke(id, data, &mut output);
    output
}

/// Strictly verifies an Ed25519 signature over the supplied message directly.
///
/// No implicit prehash, context or Ed25519ph mode is used. Invalid keys,
/// small-order keys/R, noncanonical scalars and invalid signatures return false.
/// FUEL or memory exhaustion stops the invocation instead of returning false.
pub fn ed25519_verify(public_key: &[u8; 32], signature: &[u8; 64], message: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let length = 96_usize
            .checked_add(message.len())
            .filter(|length| u32::try_from(*length).is_ok())
            .unwrap_or_else(|| core::arch::wasm32::unreachable());
        let mut input = alloc::vec::Vec::with_capacity(length);
        input.extend_from_slice(public_key);
        input.extend_from_slice(signature);
        input.extend_from_slice(message);
        let mut output = [0];
        invoke(3, &input, &mut output);
        match output[0] {
            0 => false,
            1 => true,
            _ => core::arch::wasm32::unreachable(),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (public_key, signature, message);
        unsupported()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn unsupported() -> ! {
    panic!("AVM crypto helpers require WASM execution on a matching runtime")
}

#[cfg(not(target_arch = "wasm32"))]
fn invoke(_: i32, _: &[u8], _: &mut [u8]) {
    unsupported()
}

#[cfg(target_arch = "wasm32")]
use import::invoke;

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)]
mod import {
    #[link(wasm_import_module = "avm")]
    unsafe extern "C" {
        #[link_name = "precompile"]
        fn precompile(id: i32, in_ptr: i32, in_len: i32, out_ptr: i32) -> i32;
    }

    pub(super) fn invoke(id: i32, input: &[u8], output: &mut [u8]) {
        let output_len = match id {
            1 | 2 => 32,
            3 => 1,
            _ => core::arch::wasm32::unreachable(),
        };
        if output.len() != output_len || (id == 3 && input.len() < 96) {
            core::arch::wasm32::unreachable();
        }
        let length =
            u32::try_from(input.len()).unwrap_or_else(|_| core::arch::wasm32::unreachable());
        // SAFETY: these live shared/exclusive borrows are disjoint. Only the
        // fixed output width can be written by the matching synchronous host;
        // the host checks both complete ranges and never re-enters the guest.
        // wasm32 guest pointers and the checked length fit the registered ABI.
        let status = unsafe {
            precompile(
                id,
                input.as_ptr() as usize as i32,
                length as i32,
                output.as_mut_ptr() as usize as i32,
            )
        };
        if status != 0 {
            core::arch::wasm32::unreachable();
        }
    }
}
