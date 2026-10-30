# Unreleased

Account balances and message transfers now use explicit currency entries in
`extra`. The separate `balance_main` and `value_main` fields and CLI
`--value-main` option are removed. Currency ID 0 is accepted like any other ID.
`fuel_fee_shells` keeps its existing meaning and fee rules.

This changes the guest API, message/account encodings and execution ABI.
Rebuild contracts with the matching SDK and recreate local stand data.

Constructor inputs now expose their external or internal message envelope,
including attached currency entries and authentication fields. CLI tooling can
inspect and decode ABI data, show raw and decoded account state, and quote the
deterministic deploy address before funding it.

Contract builds support relocated path dependencies. If a source file was
deleted or renamed after an earlier build, the builder rejects the stale
generated source tree and identifies the directory to remove before retrying.
It preserves the original sources and existing output artifacts on rejection.

Reinstalling an existing SDK release now checks executable permissions before
activating it. See [Getting Started](getting-started.md) for the CLI commands
and [release provenance](releases.md) for bundle verification.

# AVM SDK 0.2.0 / guest SDK 0.1.1

Contract results avoid an extra output copy, and the local stand now uses the
updated AVM runtime.

## What's Changed

- **Guest execution:** results encode directly into guest output memory;
  input copying is optimized without changing the public Rust API or wire format.
- **Runtime:** pooled allocation and deadline cancellation preserve fresh
  isolated contexts and FUEL. Message admission no longer truncates valid inputs
  by cumulative reserved FUEL; failed-batch charging is corrected.
- **Build and install:** builds enforce Rust/Cargo 1.93.0, rust-lld and a reviewed
  `Cargo.lock`. Installation verifies the complete package before activation.
- **Upgrades:** Contract A accepts your stand's owner key instead of the fixed
  test key, enabling owner-authorized upgrades.

## Get Started

The full bundle requires Linux x86-64-v3, glibc >=2.39, Python >=3.11, Bash,
`gh` and `jq`. Contract builds require exact Rust/Cargo 1.93.0, its
`wasm32-unknown-unknown` target and rust-lld, plus a reviewed `Cargo.lock`.
Offline builds also need cached dependencies. ARM, musl, macOS and Windows
binaries are not provided.

Follow [Getting Started](getting-started.md) to install and initialize the stand.
Then build from a writable checkout with the public owner key from `init.json`:

```bash
export AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY="$(jq -er .identities.package_owner init.json)"
avm-contract-build build --manifest-path examples/contract-a-v1/Cargo.toml
```

The key must be 64 lowercase hex digits. It is embedded at compile time; omitting
it keeps the fixed test owner. See [building a contract](getting-started.md#build-a-contract).

## Examples

- [contract-a-v1](../examples/README.md#contract-a-v1) and
  [contract-a-v2](../examples/README.md#contract-a-v2): owner-authorized upgrades.
- [contract-b](../examples/README.md#contract-b): internal messaging,
  ordinary-call callbacks and event emission/listing.
- [contract-batch-patch](../examples/README.md#contract-batch-patch): update a
  counter without replacing the full account state.

These are learning fixtures, not audited applications.

## Compatibility

The Rust guest API, ABI and ownership rules are unchanged. Rebuilding can change
code hashes and FUEL usage. Installation does not rebuild or upgrade contracts;
publication, selection and migration remain explicit.

**Breaking:** old local state is not migrated. Keep old tools, profile and state
together for replay. Initialize a fresh local stand, explicitly fund its derived
addresses and redeploy. This guidance applies only to local development and
does not authorize a production reset.

CompactV1 state trees change runtime roots/proofs; canonical `List(Data)`
EventLog encoding changes aggregate commitments. Profile/cache identities also
change. Use the shipped profile; do not mix profiles or compiled caches, or
edit state headers to force acceptance. The unchanged release's
[detailed runtime compatibility notes](https://github.com/gosh-sh/avm-sdk/blob/0748f4f0767a26da8bc363bcb6b5585539d6970e/docs/release-notes.md#runtime-compatibility-and-reset-guidance)
remain available.

**Known limitations:** guest internal-event reads are unsupported; event emission
and listing work. Deploy success callbacks remain unresolved in
[issue #13](https://github.com/gosh-sh/avm-sdk/issues/13). Ordinary-call callback
tests do not establish deploy callback support. This release provides no
production throughput, deadline or consensus-readiness guarantee.

The guest SDK retains its [MIT license](../LICENSE.md). Host tools retain their
existing terms in `licenses/Acki-Nacki-Node-LICENSE.md` in the
[release bundle](https://github.com/gosh-sh/avm-sdk/releases/tag/avm-dev-06e96ef71006-sdk-0748f4f0767a).
See [package contents and trust](releases.md#package-contents-and-trust) for
manifests, provenance and third-party notices.
