# Signature Verifier

This contract verifies a trusted signer's Ed25519 signature over
`SHA-256(payload)` and stores `BLAKE3(payload)` on success. Its state is the
external-call owner's public key (32 bytes), the trusted payload signer's key
(32 bytes) and the last accepted payload digest (32 bytes). These two keys have
different roles: a valid external-call signature does not replace the
application payload signature.

The constructor accepts the trusted signer's 32-byte public key as ABI `bytes`.
The deploy's `owner_id` supplies the external-call owner. `crypto.verify`
accepts ABI `bytes` containing `signature[64] || payload`. Invalid signatures
return contract status 1801 without replacing state. Malformed arguments return
1800, malformed state 1802 and an unsupported method 1803.

Build with the matching SDK builder and Rust/Cargo 1.99.1:

```bash
avm-contract-build build --manifest-path examples/signature-verifier/Cargo.toml
```

Use the documented [local stand workflow](../../docs/getting-started.md) with
this example's artifacts and method. ABI byte values are hexadecimal strings
prefixed with `0x`. Calls use the package-owner identity when that identity was
the deployment owner. Inspect the canonical contract output status as well as
the CLI result: a completed call can contain an application-level failure.

The manifest supplies explicit resources for the bounded example test corpus.
They are not defaults for arbitrary bridge workloads. Measure the actual
production states, input sizes and signature-validation paths before choosing
deployment limits. The source retains `#![forbid(unsafe_code)]` and adds no
guest cryptography dependency.
