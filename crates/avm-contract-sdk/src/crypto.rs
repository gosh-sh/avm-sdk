//! Synchronous cryptography provided by the matching AVM runtime.
//!
//! These helpers require WASM execution. Native calls fail explicitly.
//!
//! Unsafe code: the private `import` module declares the `avm.precompile` host
//! import and makes its one call. The safe wrappers fix the id, pass borrowed
//! input and an exclusively borrowed output of the exact fixed width, and the
//! host checks both ranges before it reads or writes. The invariant, the reason
//! it holds and the tests that check it are listed in the SDK `docs/unsafe.md`.

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
    // SAFETY: this declaration matches the host import `avm.precompile` of the
    // crypto precompile ABI (docs/crypto.md): signature
    // `(i32, i32, i32, i32) -> i32`, and no effect other than reading the input
    // range and writing the output range it is given. Only `invoke` calls it,
    // under the conditions stated at that call.
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
        // SAFETY: `input` and `output` are live borrows for the whole call, so
        // the two ranges are disjoint and `output` is exclusively ours. The id is
        // 1, 2 or 3 and `output.len()` is its fixed width, checked above, so the
        // host writes at most `output.len()` bytes into `output` and nothing
        // else. The matching host (docs/crypto.md, docs/unsafe.md) validates
        // both complete ranges and
        // their non-overlap before any read or write, is synchronous, never
        // re-enters the guest and never grows memory during the call. wasm32
        // guest pointers and the checked u32 length fit the registered
        // `(i32, i32, i32, i32) -> i32` signature as unsigned bit patterns.
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
