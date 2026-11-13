# Contract Crypto Helpers

The `avm_contract_sdk::crypto` API provides synchronous BLAKE3, SHA-256 and
strict Ed25519 verification through the matching AVM runtime. It requires a
runtime that supports these helpers. Earlier runtimes reject contracts that
use them during admission. Contracts that do not call these helpers retain
their existing import-free build.

```rust
#![forbid(unsafe_code)]
use avm_contract_sdk::crypto::{blake3, sha256, ed25519_verify};

fn verify_payload(key: &[u8; 32], signature: &[u8; 64], payload: &[u8]) -> bool {
    let digest = sha256(payload);
    ed25519_verify(key, signature, &digest)
}

fn payload_commitment(payload: &[u8]) -> [u8; 32] {
    blake3(payload)
}
```

`blake3` is the ordinary unkeyed 32-byte BLAKE3 hash. `sha256` returns a
32-byte SHA-256 digest. Both accept empty input. `ed25519_verify` verifies
the supplied message directly, without an implicit hash, context or
Ed25519ph mode. Pass a digest explicitly only when the application protocol
requires signing that digest. The argument order is public key, signature,
message. Invalid signatures and invalid keys return `false`; strict checking
rejects small-order keys/R and noncanonical signature scalars.

The helpers consume the invocation's compute FUEL. Exhaustion stops the
invocation before native cryptographic work; it is not an invalid-signature
result. Normal memory limits, ABI output validation and payment still apply.
The verification wrapper packs its key, signature and message in guest memory;
account for that temporary allocation when measuring the contract's resources.

Run contract code through the SDK's WASM builder and local stand. Calling these
helpers in a native Rust process fails explicitly instead of substituting a
different implementation. Contract source does not need unsafe Rust.

The [signature verifier example](../examples/signature-verifier/README.md)
separates the node's external-call owner from the signer of an application
payload and uses all three helpers.

## PROVISIONAL Precompile Compute Prices

The active runtime charges `base_fuel + per_byte_fuel * input_bytes` before
native work. These are PROVISIONAL prices; final prices require worst-host
validator calibration and may change with a later runtime profile.

- BLAKE3: base 2,048 FUEL plus 8 FUEL per input byte.
- SHA-256: base 2,048 FUEL plus 4 FUEL per input byte.
- Strict Ed25519 verification: base 300,000 FUEL plus 12 FUEL per input byte.
  Its input includes the 32-byte public key and 64-byte signature, so use
  `input_bytes = 96 + message_bytes`.

The WASM call and wrapper instructions are additional metered compute.
These charges consume the existing invocation allowance; they do not replace
or increase it. Memory capacity payment, SHELL conversion and settlement stay
separate and unchanged. Insufficient compute allowance stops the call before
native cryptographic work; it is not a `false` verification result.
