# AVM SDK 0.3.0

## Changed

- **Currencies:** explicit ECC balances/transfers; removed ambiguous main-currency fields; enabled currency ID `0`.
- **Constructors:** expose deployment data, salt and identities directly to `init`.
- **Bindings:** generate typed dispatch and method/error IDs instead of handwritten ABI glue.
- **CLI:** inspect ABI/account state and calculate deploy addresses before funding.
- **Deploy callbacks:** support internal-deploy success, failed-init settlement and rejection notifications.
- **Diagnostics:** report constructor errors, invalid timestamps and used/available FUEL.
- **Build/install:** fix relocated dependencies, reject stale generated sources and validate executable permissions.
- **Examples:** add bounty escrow, Revenue Hub, vesting, token and factory-child workflows.
- **Revenue Hub checks:** replace Node.js with Rust/Wasmtime.

<a id="compatibility"></a>

## Upgrade

- **Breaking:** encodings, ABI and profiles changed; rebuild contracts and create fresh local stand state.
- Match SDK/profile bytes; guest crate version `0.1.1` alone does not establish compatibility.
- Linux x86-64-v3 bundle; glibc ≥2.39, Python ≥3.11. Builds require Rust/Cargo 1.93.0, WASM target and rust-lld.
- Bash, `gh` and `jq` are required; offline builds need a reviewed `Cargo.lock` and cached dependencies.
- Keep 0.2.0 tools/profile/state together for replay. Examples are educational, not audited applications.
- **Getting started:** [installation and contract builds](getting-started.md), [package verification](releases.md).
- **Fees and limits:** `fuel_fee_shells` rules are unchanged; see [fees](fuel-and-fees.md) and [runtime limits](runtime-limits.md).
- **Credits:** @h2so4nackl-code — [#9](https://github.com/gosh-sh/avm-sdk/pull/9), [#10](https://github.com/gosh-sh/avm-sdk/pull/10), [#11](https://github.com/gosh-sh/avm-sdk/pull/11), [#25](https://github.com/gosh-sh/avm-sdk/pull/25).
- **Credits:** @alandocarmo — [#12](https://github.com/gosh-sh/avm-sdk/pull/12), [#26](https://github.com/gosh-sh/avm-sdk/pull/26), [callback reproducer](../examples/deploy-callback-repro).

## AVM SDK 0.2.0 / guest SDK 0.1.1

- **Guest execution:** direct guest-output encoding and faster input copying; public Rust API and wire format stayed unchanged.
- **Runtime:** pooled allocation and deadlines preserve fresh contexts and FUEL; admission and failed-batch charging were corrected.
- **Build/install:** pinned Rust/Cargo 1.93.0, rust-lld and reviewed `Cargo.lock`; complete package verification before activation.
- **Upgrades:** Contract A embeds the stand owner key at build time for [owner-authorized upgrades](getting-started.md#build-a-contract).
- **Compatibility:** CompactV1 roots/proofs, `List(Data)` EventLog commitments and profile/cache identities changed; use matching tools/profile/state and fresh local state. See [0.2 compatibility guidance](https://github.com/gosh-sh/avm-sdk/blob/0748f4f0767a26da8bc363bcb6b5585539d6970e/docs/release-notes.md#runtime-compatibility-and-reset-guidance); this is not production-reset guidance.
- **0.2-only limits:** no internal-event reads or deploy-success callbacks; ordinary callbacks and event emission/listing worked. No production throughput, deadline or consensus-readiness guarantee.

## Licenses

- Guest SDK: [MIT](../LICENSE.md). Host-tool terms and third-party notices remain in the [verified release bundles](releases.md#package-contents-and-trust).
