# Contract memory maximum

A deployment build uses the peak linear memory measured by the contract's own
tests. Commit the resulting `avm-memory.json` next to `avm-contract.toml`.
The builder rounds the recorded peak up to a 64-KiB WASM page and never selects
a maximum below the recorded initial memory. It verifies the emitted memory
section and writes the selected maximum, measurement-file digest and artifact
purpose to `target/avm-build-provenance.txt`. Both clean builds use that value.

There is no fixed deployment default. Without a measurement or an explicit
setting, the build fails and explains how to produce one.

## Measure your tests

```bash
avm-contract-build build --manifest-path Cargo.toml --for-testing
export AVM_MEMORY_REPORT="$PWD/target/memory-runs.jsonl"
: > "$AVM_MEMORY_REPORT"
# Run every supported scenario, including the largest state and inputs.
# Deploy this test build into a fresh disposable local stand first.
avm-local-stand call --state demo.state --wasmtime-profile "$PROFILE" \
  --address "$ADDRESS" --method your.method --args-json '{}' \
  --identity user --check-memory
avm-contract-build record-memory --manifest-path Cargo.toml \
  --report "$AVM_MEMORY_REPORT"
avm-contract-build build --manifest-path Cargo.toml
git add avm-memory.json
```

The measuring build uses the existing admission cap when no maximum is explicit.
Its output directory contains `NOT-FOR-DEPLOYMENT`, its provenance says
`memory-test-only`, and stderr warns against deployment. Test builds retain the
usual artifact names so existing local test commands can consume them. Rebuild
without `--for-testing` before deployment; a successful deployment build clears
its own stale marker and records purpose `deployment`.

`AVM_MEMORY_REPORT` collects all synchronous stand invocations into a locked
JSONL file, preserving the original outcomes of expected negative tests and
commands without execution. Explicit `--check-memory` additionally makes the
command fail on rejected growth, incomplete capture or no observed invocation.
Use it on executing deploy, call or replay commands; quoting and inspection
do not run a guest.

`record-memory` selects the current tested WASM's CodeHash. Repeat `--code-hash`
to include other tested builds of the same contract. Use the final WASM after
optimization, include every tested variant and keep other contracts separate.
Absent observations, incomplete captures and rejected growth cannot produce a
successful measurement. The versioned project file records initial bytes,
peak bytes, test-call count and tested WASM hashes. It is reviewed project
metadata, not a proof that every future input will fit. Repeat measurements
after changing sources, dependencies, compiler settings or supported inputs.

## Understand the report

`memory_check.runs` contains CodeHash, entrypoint, initial bytes,
`declared_max_bytes`, `peak_bytes`, `near_limit` and `rejected_growths` for each
fresh invocation. Peak includes allocator, entrypoint and cleanup growth.
It measures WASM linear-memory capacity, not live Rust allocations or host RSS.
Linear memory cannot shrink within a fresh instance.

A warning appears at **80%** of the declared maximum. **Production data larger
than your tests can exhaust the declared memory: growth will be rejected and
the call can fail.** Test the largest supported state, inputs and batches,
including temporary allocations and output serialization. A small successful
test is insufficient coverage. Core-WASM `memory.grow` returns `-1` on rejected
growth; the local check fails even if the guest handled that sentinel. Its VM
result and actual stand state changes are preserved, so use disposable state.

## Set an explicit maximum

An explicit setting replaces the measurement:

```toml
[build]
max_memory_bytes = 4194304
```

Use a positive integer byte count, in multiples of 65536, within the runtime
admission cap and above the module's initial memory requirement. This does not
raise the runtime cap or FUEL budget. The builder accepts the explicit value
without reading an old measurement file. Changing the maximum changes the
WASM bytes and code identity; rebuild and publish a new code version for an
existing contract. Recheck FUEL funding for the active runtime profile.

## Reproduce the bundled examples

All 17 example contracts keep their own measured project file. Their complete
local scenario suites and direct WASM vectors can be measured again with
`scripts/tests/measure_example_memory.py`. Verify the deployment memory sections
and two-clean-build results with `scripts/tests/contract_memory.py`.
