# AVM SDK 0.6.0

- Deploy contracts whose deploy body exceeds 65536 bytes: a deploy that carries code may have a body of up to 2228224 bytes (`max_code_deploy_body_size`); every other message keeps the 65536-byte body limit. Its `init` input keeps the 1048576-byte input cap, so a deploy that carries more than about 1 MiB of code fails at `init`; see [runtime limits](runtime-limits.md#state-output-and-deploy-body-are-different-limits).
- Charge the Code Library publication fee of a deploy that carries code by the module's size and function count: 1000 SHELL for every started 40 KiB unit, with 256 functions included per unit; see [code publication fee](fuel-and-fees.md#code-publication-fee). Reusing a published selection still pays no publication fee.
- Declare the module's function count in the deploy body: the stand writes the number of entries of the WASM function section at body offset 286 (`function_count`), and `MessageInput::deploy()` accepts it. A deploy without code keeps it 0.
- Sign a deploy that carries code without its code bytes: its signatures cover the deploy with the WASM module omitted, under a separate hash domain, and still bind `code_hash`, `code_len` and `function_count`. Deploys without code keep their signing.
- Reject a `br_table` instruction with more than 65520 targets in code admission.
- Take the guest ABI arena's memory on demand. The configured arena capacity still bounds the simultaneously live input and output bytes, but it is no longer reserved in initial memory: an allocation that fits a 4-KiB static region stays there, and a larger one takes heap memory during the call that makes it. Each call pays only for the pages its own input, output and work use; see [contract memory](contract-memory.md).
- Decode the entrypoint input in place instead of copying it into the heap first, so a large input, such as an initialization message that carries the contract's own module, needs its pages once and no second copy.
- Keep the arena capacity checks, the two live allocations freed in LIFO order, exact-pointer frees, distinct output placement, the guest ABI, the Core-WASM V2 instruction set, operation prices, memory payment and the other runtime limits of SDK 0.5.1.

## Compatibility

- The stand configuration identity changes. Local stand state created with an earlier SDK is rejected with a configuration mismatch: create fresh local stand state for this bundle and keep earlier tools, profile and retained state together.
- Rebuild contracts with this release's guest crate before deploying them with code. An `init` built with an earlier guest crate rejects the non-zero `function_count` of such a deploy: its `input.deploy()` returns `NonZeroReserved`.
- A contract above 40 KiB pays more to publish, for example 2000 SHELL instead of 1000 for a 41441-byte module. Fund deploy addresses for the new fee.
- A deploy that carries code and was signed for an earlier release does not verify under this release.
- Rebuilding a contract changes its WASM, code hash, initial memory and FUEL use. Initial memory no longer includes the arena capacity, so a contract whose arena added initial pages starts each call with fewer pages after rebuilding. Existing deployed WASM is unaffected until it is rebuilt and upgraded.
- The guest crate now uses the Rust standard library for the arena lock. Contract crates that link the standard library, as the examples do, need no change; a `#![no_std]` contract crate with its own panic handler must link the standard library to use this guest crate.
- Existing `avm-memory.json` files keep their schema and are accepted by the builder, but a stack or arena measured with an earlier guest crate can be too small after rebuilding: built with their earlier records, bundled example calls trapped with a stack overflow or had a guest allocation rejected. Their new measurements changed the stack by -288 to +144 bytes and the arena by -456 to +1933 bytes. Delete the earlier record so that test builds use the bootstrap stack and arena, rerun the complete test capture and the stack and arena selection, and commit the new record before deployment.
- The guest crate remains version `0.1.1`. Contract WASM using the Core-WASM V2 instruction set remains admissible.
- The bundled profile's `memory_page_price`, `memory_class_maxima_bytes`, `signals_based_traps` and `pooling_linear_memory_keep_resident` are provisional development values: they can change before network acceptance, and a bundle with other values has a different profile identity.
- Linux x86-64-v3 and Python 3.11+ are required; see the bundle manifest for measured GLIBC dependencies. Examples are educational, and this SDK release does not establish production-network or performance acceptance.

# AVM SDK 0.5.1

- Run the local stand with the network execution FUEL limits: `max_message_execution_fuel = 1031871955` and `max_bucket_execution_fuel = 2063743910` instead of 10000000 and 100000000; see [runtime limits](runtime-limits.md#fuel-and-failure-diagnostics).
- Keep the Core-WASM V2 instruction set, admission rules, operation prices, memory payment and the other runtime limits of SDK 0.5.0.

## Compatibility

- The stand configuration identity changes. Local stand state created with SDK 0.5.0 is rejected with a configuration mismatch: create fresh local stand state for this bundle and keep earlier tools, profile and retained state together.
- A message reserve above 10 SHELL now buys up to 1031871955 FUEL instead of 10000000, and one `AccountBatch` invocation up to 2063743910 instead of 100000000. A call that ran out of FUEL at the old cap can complete and is charged for the FUEL it uses, at most its reserve.
- The guest crate remains version `0.1.1`. Contract WASM using the Core-WASM V2 instruction set remains admissible.
- The bundled profile's `memory_page_price`, `memory_class_maxima_bytes`, `signals_based_traps` and `pooling_linear_memory_keep_resident` are provisional development values: they can change before network acceptance, and a bundle with other values has a different profile identity.
- Linux x86-64-v3 and Python 3.11+ are required; see the bundle manifest for measured GLIBC dependencies. Examples are educational, and this SDK release does not establish production-network or performance acceptance.

# AVM SDK 0.5.0

- Run contracts on Wasmtime 48.0.8 LTS with Cranelift 0.135.8 and wasmparser 0.254.2, replacing Wasmtime 36.0.12, which lacks upstream security fixes.
- Build the native tools with Rust/Cargo 1.98.0; contract builds keep Rust/Cargo 1.99.1.
- Apply operation-specific FUEL prices and additional charges for requested memory bytes, table elements and memory-growth pages; see [the charging rules](fuel-and-fees.md#operation-prices-in-sdk-050).
- Keep the Core-WASM V2 instruction set, admission rules and configured runtime limits.
- Run the Revenue Hub guest ABI checks on Wasmtime 48.0.8.
- Produce byte-for-byte reproducible release bundles from identical build inputs.
- Record build paths in bundle provenance, license notices and the host commands' compiled-in source locations under fixed public roots instead of the build machine's directories; see [package contents](releases.md#package-contents-and-trust).
- Charge each paid call for the linear memory it ends with: its 64-KiB pages at the profile's `memory_page_price`, settled with compute FUEL in one rounding and outside every FUEL budget and cap; a paid reserve must also cover the initial memory before the call starts; see [memory payment](fuel-and-fees.md#memory-payment).
- Run contracts in ten pre-created memory classes selected by the declared maximum memory; a class changes neither FUEL nor the memory payment; see [memory classes](fuel-and-fees.md#memory-classes).
- Derive deployment stack, ABI arena and memory maximum from each contract's own tests, with explicit overrides, stack overflow traps and local peak/growth checks; see [contract memory](contract-memory.md).

<a id="compatibility"></a>

## Compatibility

- Rebuilding a contract now requires complete stack, arena and maximum-memory measurements in `avm-memory.json`, or explicit settings. Test the largest supported state and inputs before deployment; `--for-testing` artifacts are marked and must be rebuilt for deployment. Larger production inputs can exhaust the tested limits and stop a call.
- Use the guest SDK source supplied with this release when rebuilding: its export macros provide the resource metadata needed for the checked stack layout.

- The guest crate remains version `0.1.1`. Contract WASM using the Core-WASM V2 instruction set remains admissible.
- Paid calls also pay for memory: size each reserve for `fuel_used + pages_at_end * memory_page_price`. A call that would otherwise succeed with too small a reserve fails with status 56, returns no output and commits no effects; the whole reserve is charged. Local-free calls stay free.
- The bundled profile's `memory_page_price`, `memory_class_maxima_bytes`, `signals_based_traps` and `pooling_linear_memory_keep_resident` are provisional development values: they can change before network acceptance, and a bundle with other values has a different profile identity.
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
