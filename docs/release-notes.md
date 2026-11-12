# AVM SDK 0.6.1

- Use relative schedules in `vesting-streams`: start, cliff and end are offsets from the first execution, zero start is allowed, late deployment and funding are accepted, and cancellation after end preserves the full earned amount with no refund entitlement. The `time` event field now reports elapsed seconds from the first execution. State grows from 3592 to 3600 bytes; the new schema id is `252aa2507344852d6703cbe2d53e2020230641e4c6938a93e3ca2f5599af63a5`.
- Use a relative deadline in `bounty-escrow`, measured from its first execution. Deployment and exact one-time funding are accepted after the offset; late submission and approval acknowledge expiry without creating approved rights, and unapproved funds become refundable. Zero offset is valid. Late submission and approval emit `escrow.expired`; status 2005 is returned only for a refund before the offset. State grows from 2864 to 2872 bytes; the new schema id is `611387b8821a94e71fe7a21553d177ef902715f238624b464ba94ec81999b8a7`.
- Store Revenue Hub governance times as wide elapsed offsets from the first execution. A zero governance delay is allowed; expired votes and executions succeed with `hub.expired` and apply no vote or action. Expiry status 3013 is reserved and is no longer returned by votes or executions. Proposal times are now 8 bytes each, expanding the proposal record from 48 to 64 bytes and moving member records from byte 2504 to byte 2520. Vault state grows from 4168 to 4192 bytes; the new schema id is `9b29bb574f1cbe12d9904f2e1c40ea1fade8285ef49566d274b5195ce6a9335f`.
- Deploy a module above 1 MiB, up to `max_code_size`, on the network and on the local stand: the input of a deploy that carries code is bounded only by its own `max_code_deploy_input_bytes` (3276800 bytes). SDK 0.6.0 also applied the general 1048576-byte input limit and rejected such deploys; every other input keeps that general limit.
- The `init` of a deploy that carries code receives the whole deploy message, module included, so the contract's `arena_bytes` must hold that input and the `init` output together. Measure both deploy forms, a deploy that carries the code and a deploy from a committed selection, with production-sized `init_data`, `salt`, `ui_name` and `deploy_meta`, and remeasure `avm-memory.json` after any source, dependency, toolchain or supported-input change; see [contract memory](contract-memory.md).
- Keep a deploy's `salt`, `ui_name` and all `deploy_meta` field names and values within 235 bytes together: the mandatory `avm.account.init` event must fit `max_event_value_bytes` (500 bytes), and a larger deploy fails its `init` with status 44 (`VM_ABI_OUTPUT_MALFORMED`) on the network and on the local stand. Put larger deploy data into `init_data`.
- Admit an external deploy, on the network and on the local stand, only when the funding already committed at its address covers its whole pre-execution charge: its `fuel_fee_shells`, the account initialization fee and, for a deploy that introduces a new selection, the Code Library publication fee; see [code publication fee](fuel-and-fees.md#code-publication-fee). A zero `fuel_fee_shells` is not admitted. Both rules also hold for a deploy of already published code that would complete at once. A refused deploy is not charged and its nonce stays unused, so a funded and paid deploy can reuse it; the reason is `cannot pay the external: InsufficientBalance` for insufficient funding and `cannot pay the external: RouteRequiresFee` for a zero fee.
- Check on the local stand that an external call or deploy can be paid where the network checks it, after the owner key and before the time, nonce and signature checks, and refuse it with the network's reason: an unpaid call now reports `cannot pay the external: RouteRequiresFee` instead of `local call charge failed: RouteRequiresFee`.
- Commit on the local stand, as on the network, an external call that reached WASM execution and then failed: its charge and its spent nonce are committed and none of its effects are kept; see [runtime limits](runtime-limits.md#fuel-and-failure-diagnostics).
- AccountBatch execution is not supported by the node yet; a message to such code ends with status 21. The local stand ends it the same way, and the bundled `contract-batch-patch` example and the Revenue Hub audit ledgers are now message-isolated contracts.

## Compatibility

- Rebuild and redeploy copies of `vesting-streams`, `bounty-escrow` and the Revenue Hub vault to use the new relative schedules and state schemas. Existing deployments keep their old WASM and behavior; no migration handler is provided. Constructor schedule values are durations rather than calendar timestamps.
- Fund the address of every external deploy for its whole pre-execution charge, including the publication fee of a new selection, and give the deploy a non-zero `fuel_fee_shells`: such deploys that SDK 0.6.0 admitted are now refused.
- Scripts that match the local stand's refusal reason for an unpaid external call must expect the network's reason.
- Code built with `execution_mode = "account_batch"` still deploys and initializes, but every message to it ends with status 21. Rebuild copies of the earlier `contract-batch-patch` or Revenue Hub audit examples from this release's message-isolated sources and redeploy them.
- The bundled profile's `memory_page_price`, `memory_class_maxima_bytes`, `signals_based_traps` and `pooling_linear_memory_keep_resident` are provisional development values: they can change before network acceptance, and a bundle with other values has a different profile identity.
- Linux x86-64-v3 and Python 3.11+ are required; see the bundle manifest for measured GLIBC dependencies. Examples are educational, and this SDK release does not establish production-network or performance acceptance.

# AVM SDK 0.6.0

- Authorize external calls by the owner key that a contract keeps in its own data: declare `owner_key_offset` in `[descriptor]` of `avm-contract.toml`, and the builder writes it as a trailing `u32` of the code descriptor. Only that key signs an external call; any other key, or an account without a key, is rejected with `AccountKeyNotAllowed` before any charge; see [owner key and external calls](owner-key-and-external-calls.md).
- Write, read and rotate the key with the guest helpers `set_owner_key`, `owner_key` and `OWNER_KEY_BYTES`. Deploy installs no key; by the SDK convention `init` writes the deploy's `owner_id`, and a rotated key authorizes calls from the next block.
- Reject value on external messages with `ExternalValueNotAllowed`. Fund an account by an internal message or, on the local stand, with `fund-user --extra-currencies`.
- Call and deploy contracts on a network with `avm-local-stand network-call`, `network-deploy` and `network-status`: they sign external messages with key files, send them to a node's `POST /v2/avm/messages`, follow its redirect to the destination's producer and wait for the outcome, such as `consumed` or `MESSAGE_REJECTED` with `AccountKeyNotAllowed` or `NonceReplay`; see [the CLI reference](cli-reference.md#network-client).
- Add the owner wallet example, which turns one signed external call into one internal message with value and an optional callback, the way other people reach a contract.
- Deploy contracts whose deploy body exceeds 65536 bytes: a deploy that carries code may have a body of up to 3145728 bytes (`max_code_deploy_body_size`) and a module of up to 3145440 bytes (`max_code_size`); every other message keeps the 65536-byte body limit. Its `init` input has its own 3276800-byte cap (`max_code_deploy_input_bytes`); see [runtime limits](runtime-limits.md#state-output-and-deploy-body-are-different-limits).
- On the network, a deploy that publishes new code completes in two phases: the block producer compiles the module within 120 seconds before its bytes enter a block, and `init` runs 120 seconds after the publishing block is final. A deploy over committed code, by an external or an internal message, completes at once; see [contract deploy](contract-deploy.md).
- Charge the Code Library publication fee of a deploy that carries code by the module's size and function count: 1000 SHELL for every started 40 KiB unit, with 256 functions included per unit; see [code publication fee](fuel-and-fees.md#code-publication-fee). Reusing a published selection still pays no publication fee.
- Declare the module's function count in the deploy body: the stand writes the number of entries of the WASM function section at body offset 286 (`function_count`), and `MessageInput::deploy()` accepts it. A deploy without code keeps it 0.
- Sign a deploy that carries code without its code bytes: its signatures cover the deploy with the WASM module omitted, under a separate hash domain, and still bind `code_hash`, `code_len` and `function_count`. Deploys without code keep their signing.
- Reject a `br_table` instruction with more than 65520 targets in code admission.
- Take the guest ABI arena's memory on demand. The configured arena capacity still bounds the simultaneously live input and output bytes, but it is no longer reserved in initial memory: an allocation that fits a 4-KiB static region stays there, and a larger one takes heap memory during the call that makes it. Each call pays only for the pages its own input, output and work use; see [contract memory](contract-memory.md).
- Decode the entrypoint input in place instead of copying it into the heap first, so a large input, such as an initialization message that carries the contract's own module, needs its pages once and no second copy.
- Keep the arena capacity checks, the two live allocations freed in LIFO order, exact-pointer frees, distinct output placement, the guest ABI, the Core-WASM V2 instruction set, operation prices, memory payment and the other runtime limits of SDK 0.5.1.

## Compatibility

- The stand configuration identity changes. Local stand state created with an earlier SDK is rejected with a configuration mismatch: create fresh local stand state for this bundle and keep earlier tools, profile and retained state together.
- An external call to an account without an owner key is rejected. Declare `owner_key_offset`, write the key in `init` and keep it in every later state update in every contract that takes external calls, then rebuild, remeasure `avm-memory.json` and redeploy it; the bundled examples do so. Send value through internal messages instead of external calls.
- Rebuild contracts with this release's guest crate before deploying them with code. An `init` built with an earlier guest crate rejects the non-zero `function_count` of such a deploy: its `input.deploy()` returns `NonZeroReserved`.
- A contract above 40 KiB pays more to publish, for example 2000 SHELL instead of 1000 for a 41441-byte module. Fund deploy addresses for the new fee.
- A deploy that carries code and was signed for an earlier release does not verify under this release.
- Rebuilding a contract changes its WASM, code hash, initial memory and FUEL use. Initial memory no longer includes the arena capacity, so a contract whose arena added initial pages starts each call with fewer pages after rebuilding. Existing deployed WASM is unaffected until it is rebuilt and upgraded.
- The guest crate now uses the Rust standard library for the arena lock. Contract crates that link the standard library, as the examples do, need no change; a `#![no_std]` contract crate with its own panic handler must link the standard library to use this guest crate.
- Existing `avm-memory.json` files keep their schema and are accepted by the builder, but a stack or arena measured with an earlier guest crate can be too small after rebuilding: built with their earlier records, bundled example calls trapped with a stack overflow or had a guest allocation rejected. From SDK 0.5.1 to this release, which also changed the examples' own code, their records changed the stack by -256 to +5696 bytes and the arena by -40 to +4030 bytes. Delete the earlier record so that test builds use the bootstrap stack and arena, rerun the complete test capture and the stack and arena selection, and commit the new record before deployment.
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
