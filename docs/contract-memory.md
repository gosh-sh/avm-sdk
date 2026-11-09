# Contract memory, stack and ABI arena

A deployment build reads `avm-memory.json` next to `avm-contract.toml`. Commit
this project file with your source. It records the stack and ABI arena required
by the contract's own tests, their peak linear memory, test count and tested
WASM hashes. No fixed stack, arena or linear-memory default is used for deployment.
Missing measurements require explicit settings or a complete test measurement.

The stack is the smallest passing size on a **16-byte ABI alignment** grid.
The arena holds the simultaneously live raw ABI input and output allocations;
its measured capacity is their largest combined byte length, without rounding.
The capacity bounds the live bytes; it is not reserved memory. An allocation
that fits the remaining space of a 4-KiB static region stays in initial memory.
A larger one takes heap memory during the call that makes it, so each call pays
only for the pages its own input, output and work use. The linker reserves no
extra initial heap page. The total memory maximum is the larger of initial
memory and the test peak, rounded up to a **64-KiB WASM page**. Stack and arena
are not rounded to memory pages.

## Measure your contract's complete tests

The capture wrapper runs your existing test command, preserving its assertions
and outcomes. That command must build and execute the contract through
`AVM_CONTRACT_BUILD` and `AVM_LOCAL_STAND`, or the corresponding executables on
`PATH`. Use fresh disposable stand state. The single-contract capture command
expects one contract package; bundled multi-contract examples have their own
complete-suite command below.

```bash
SDK_TESTS="$AVM_SDK_HOME/current/scripts/tests"
CONTRACT="$PWD"
python3 "$SDK_TESTS/collect_contract_tests.py" \
  --contract "$CONTRACT" --builder "$AVM_CONTRACT_BUILD" \
  --stand "$AVM_LOCAL_STAND" --profile "$AVM_WASMTIME_PROFILE" \
  --output "$CAPTURE_DIR" -- bash scripts/test.sh
python3 "$SDK_TESTS/measure_contract_resources.py" \
  --corpus "$CAPTURE_DIR" --sdk "$(dirname "$CONTRACT")" \
  --builder "$AVM_CONTRACT_BUILD" --stand "$AVM_LOCAL_STAND" \
  --profile "$AVM_WASMTIME_PROFILE" --output "$MEASUREMENT_DIR"
avm-contract-build build --manifest-path Cargo.toml
bash scripts/test.sh
git add avm-memory.json
```

Install the SDK's Python test dependencies first. Both output directories must
be new external scratch directories. Test commands that optimize modules must
call `wasm-opt` through `PATH` and supply its executable with `--optimizer`
to both capture and selection. The
bundled collector also captures direct guest ABI vectors used by the examples.

Selection replays every captured invocation with its original input, expected
status, output and FUEL budget. It searches stack sizes on the 16-byte grid,
checks the next smaller size fails, and checks one byte less arena fails.
It then retests the page-rounded maximum. It preserves explicit build settings
and macro arena overrides. Build recipes are restored after every probe, even
on failure. Review the retained results and rerun your **whole original test
command on the deployment artifacts**; raw-call replay alone cannot replace
stateful assertions, funding checks or a test's external oracle.
Keep all build-recipe project directories available until selection finishes;
test cleanup must not remove a temporary source variant that still needs rebuilding.

`--for-testing` permits bootstrap resources only for collecting tests: the
existing admission cap, 1-MiB stack and 8-MiB-plus-16-byte arena when unspecified.
These are not measurements. Artifacts contain `NOT-FOR-DEPLOYMENT`, provenance
says `memory-test-only`, and stderr warns against deployment. Such bootstrap
values cannot be saved by `record-memory`. A successful ordinary build removes
its stale marker and records purpose `deployment`.

## Explicit settings

Each explicit field overrides the corresponding measurement:

```toml
[build]
stack_bytes = 24576
arena_bytes = 4097
max_memory_bytes = 262144
```

Stack must be positive and a multiple of 16. Arena is an integer byte capacity.
Maximum memory must be a positive multiple of 65536 within the runtime admission
cap and at least the compiled initial memory. All three explicit values allow
a deployment build without a measurement file. An explicit `arena_bytes` macro
parameter takes precedence over the configured arena; the builder reads the
actual compiled capacity and records it. Older guest SDKs without capacity
metadata must be updated; an unknown arena is never reported as zero.

The builder verifies stack-first layout in the actual module: the downward-growing
linear stack starts at address zero, before static data and the heap. It guards
stack-pointer updates so wrapped underflow traps before the function accesses
data using that pointer. Diagnostic globals and the capacity helper are removed
from the final export surface. The emitted maximum and actual stack/arena are
recorded in `target/avm-build-provenance.txt`; two clean builds must agree.

**Larger production inputs can require more stack, arena and memory than the
tests. Exhausting a limit can stop the call.** Test the largest supported state,
inputs, batches and output serialization, and increase explicit limits when
needed. Changing sources, dependencies, compiler settings or supported inputs
requires repeating qualification. Resource changes alter WASM bytes and CodeHash;
deploy a new code version and recheck its FUEL funding.

## Local memory checks

`avm-local-stand ... --check-memory` reports each fresh invocation's CodeHash,
entrypoint, initial bytes, `declared_max_bytes`, `peak_bytes`, `near_limit` and
`rejected_growths`. Peak includes allocator, entrypoint and cleanup growth. It
measures linear-memory capacity, not live Rust allocations or host RSS. Linear
memory cannot shrink within an instance. A warning appears at **80%** of the
declared maximum. No observed invocation, incomplete capture or rejected growth
fails the check. Quoting and inspection do not execute a guest.

`AVM_MEMORY_REPORT` appends synchronous observations to a locked JSONL file
without changing the original command's outcome. Once stack and arena have
been measured or explicitly set, `avm-contract-build record-memory
--manifest-path Cargo.toml --report "$AVM_MEMORY_REPORT"` can refresh the peak.
Repeat `--code-hash` for every tested variant, including optimized artifacts.
Foreign contracts, absent observations and rejected growth cannot provide a
successful measurement. Core-WASM `memory.grow` still returns `-1` on rejected
growth; the check fails even when guest code handles that sentinel. Use
disposable state because actual VM outcomes and state changes are preserved.

## Bundled examples

All 18 contracts retain their own measured project file. Run
`scripts/tests/measure_example_memory.py --collect-only` with its required tool
and output arguments, then select resources with `measure_contract_resources.py`.
Run the full collector again with `--collect-only --deployment` to check the
original scenario suites on deployment builds. `contract_memory.py` verifies
each emitted memory section and resource provenance. The builder itself always
performs two independent clean compilations and compares their admitted bytes.
