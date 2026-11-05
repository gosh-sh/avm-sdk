# AVM SDK 0.5.0

- Run contracts on Wasmtime 48.0.8 LTS with Cranelift 0.135.8 and wasmparser 0.254.2, replacing Wasmtime 36.0.12, which lacks upstream security fixes.
- Build the native tools with Rust/Cargo 1.98.0; contract builds keep Rust/Cargo 1.99.1.
- Apply operation-specific FUEL prices and additional charges for requested memory bytes, table elements and memory-growth pages; see [the charging rules](fuel-and-fees.md#operation-prices-in-sdk-050).
- Keep the Core-WASM V2 instruction set, admission rules and configured runtime limits.
- Run the Revenue Hub guest ABI checks on Wasmtime 48.0.8.
- Produce byte-for-byte reproducible release bundles from identical build inputs.
- Derive deployment stack, ABI arena and memory maximum from each contract's own tests, with explicit overrides, stack overflow traps and local peak/growth checks; see [contract memory](contract-memory.md).

<a id="compatibility"></a>

## Compatibility

- Rebuilding a contract now requires complete stack, arena and maximum-memory measurements in `avm-memory.json`, or explicit settings. Test the largest supported state and inputs before deployment; `--for-testing` artifacts are marked and must be rebuilt for deployment. Larger production inputs can exhaust the tested limits and stop a call.
- Use the guest SDK source supplied with this release when rebuilding: its export macros provide the resource metadata needed for the checked stack layout.

- The guest crate remains version `0.1.1`. Contract WASM using the Core-WASM V2 instruction set remains admissible.
- Existing WASM can consume different FUEL with the new prices. Recheck initialization, message and batch calls against their configured budgets, including allocator and cleanup work. Calls that previously succeeded can exhaust FUEL; paid calls can burn different SHELL amounts and return different unused reserves.
- The profile records additional Wasmtime settings, so its identity, the configuration hash and prepared-cache namespaces change. Create fresh local stand state for the new bundle and keep earlier tools, profile and retained state together.
- Verify and install SDK 0.5.0 bundles with this release's scripts; earlier checkouts reject the new profile. These scripts still verify earlier published bundles.
- Linux x86-64-v3 and Python 3.11+ are required; see the bundle manifest for measured GLIBC dependencies. Examples are educational, and this SDK release does not establish production-network or performance acceptance.

# AVM SDK 0.4.0

- Use Rust/Cargo 1.99.1 for validated wide-arithmetic contract builds; native tools remain on 1.93.0.
- Enable Core-WASM V2 wide arithmetic with a matching runtime profile.
- Record wide-arithmetic compiler warnings in build provenance.
- Update Revenue Hub and Vesting Streams optimizers for wide-arithmetic WASM.
- Optimize Revenue Vault deployment size to fit the existing 65,536-byte limit.
- Update example checks and build/install instructions for matching tool bundles.

## Compatibility

- Guest builds require Rust/Cargo 1.99.1; the guest crate remains version `0.1.1`.
- Guests using the inherited instruction subset remain admissible. Rebuilding can change WASM, code hashes and FUEL use; profile/config identities and prepared-cache namespaces change. Keep tools, profile and retained data together, and create fresh local stand state for the new bundle.
- Linux x86-64-v3 and Python 3.11+ are required; see the bundle manifest for measured GLIBC dependencies. Examples are educational, and this SDK release does not establish production-network or performance acceptance.

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
