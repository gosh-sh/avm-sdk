# Revenue Hub regression example

A Rust AVM reference application with a governed revenue vault, bound payout
receivers, and an AccountBatch receipt ledger. The same contract entrypoints run
in native tests and the actual AVM stand. Source originates from
[`avm-revenue-hub@9ee2ede44af5f76515a5ab42dcddc8a7299c6354`](https://github.com/h2so4nackl-code/avm-revenue-hub/tree/9ee2ede44af5f76515a5ab42dcddc8a7299c6354),
whose [standalone hosted CI passed](https://github.com/h2so4nackl-code/avm-revenue-hub/actions/runs/38321552126).
Integration validation is recorded separately in [VALIDATION.md](VALIDATION.md).

## Behavior and coverage

- Fixed positive shares sum to 10,000 basis points for 1 to 16 beneficiaries.
  Entitlements accrue from cumulative revenue, with checked full-u128 arithmetic.
  Fragmenting deposits cannot change the resulting distribution.
- Each member claims to their fixed recipient. Members can have independent
  pending payments. A successful native callback settles once; failure must
  return the exact principal before retry. Tests cover more than 64 failed attempts.
- Pause, resume, and operating-reserve changes require distinct votes from two
  of three managers on the exact proposal, with a timelock and expiry. Managers
  cannot redirect principal or change beneficiaries or shares.
- Bound receivers report receipts to the batch ledger. Nonce checks prevent
  duplicate semantic credit. AccountBatch execution retains 65,540 bytes of state
  and coalesces changes into narrow 32-byte record patches with per-input status.

There are 74 application regressions, including 4,500 generated action steps,
10,007 one-unit deposits compared with one lump deposit, full-u128 boundary cases,
100 failed payouts followed by retry, partial batch failures, and isolated/batch
equivalence. The SDK's 13 native tests run separately, for 87 tests in this workflow.
The native byte probe covers batches of 1, 8, 64, and 256 messages. Its output-byte
comparison is not a FUEL, CPU, or throughput benchmark.

## In-tree layout

This example has its own workspace and lockfile. Core, all three contracts, and
tests depend directly on the repository's `crates/avm-contract-sdk`. There is no
vendored SDK and no crates.io dependency. Root workspace, lockfile, SDK source,
and four existing contract fixtures are unchanged. Rust source is formatted
with the SDK conventions; contract behavior and regression cases are retained.
The original MIT attribution is kept in [LICENSE](LICENSE).

The default member is the vault for the SDK's existing single-guest WASM lint
loop. The full gate checks every workspace member natively and all three guests
separately. Release packaging already archives `examples/`, including this
workspace, its specification, and scripts. SDK dependency paths also resolve
inside an extracted distribution.

## Reproduce

Use Rust 1.93.0, rustfmt, Clippy, and the wasm32-unknown-unknown target.
From the repository root, run native SDK and application tests:

```bash
cargo +1.93.0 test --locked --offline
cargo +1.93.0 test --manifest-path examples/revenue-hub/Cargo.toml --workspace --locked --offline
```

For the full gate, use Linux x86-64-v3 with glibc >= 2.39 (Windows: WSL2), Python >= 3.11, curl, tar, and
sha256sum. Install the pinned official runtime and Binaryen optimizer:

```bash
bash examples/revenue-hub/scripts/install_runtime.sh
bash examples/revenue-hub/scripts/install_binaryen.sh
bash examples/revenue-hub/scripts/check.sh
```

The installers verify immutable archive hashes; the runtime installer also
uses the trusted v2 verifier for provenance, full inventory, bounded extraction,
and activation before execution. Binaryen's official archive is about 106 MiB
compressed. Only wasm-opt is extracted. Pins are in [PROVENANCE.json](PROVENANCE.json).

To reuse verified tools, including another freshly packaged SDK:

```bash
bash examples/revenue-hub/scripts/check.sh /absolute/path/to/avm-sdk-linux-x86_64 /absolute/path/to/wasm-opt
```

The official builder performs two clean reproducibility builds. Its release
profile is supplied explicitly because it uses isolated manifests. The SDK 0.2.0 builder pins the compiler and rejects `RUSTC_WRAPPER`;
the obsolete path-remapping wrapper is removed. Then pinned
Binaryen 133 optimizes each compiler WASM twice and compares bytes. The original
`contract.compiler.wasm` is retained, and the tested deployment artifact is
`contract.wasm`. Optimization fits the stand's canonical deployment body limit.
Use the full build script; a plain Cargo build does not perform that final step.

Generated logs, byte measurements, original/optimized artifact hashes, runtime
manifest, and the complete stand journal appear under `evidence/` (ignored by Git).
The actual journey includes failed payout/refund/retry, quorum and timelock
rejections, pause/resume, and a duplicate audit report. It asserts final synthetic
principal **10,010 = 5,005 + 3,003 + 2,002**, zero claimable/pending/dust, matching
receiver and audit totals, and an empty queue. Journal replay must reproduce
account/registry roots, accepted-input count, and execution fees. Contract errors
are decoded from canonical ACMO status, not inferred from the CLI's outer status.

## Hosted and release checks

`.github/workflows/revenue-hub.yml` checks SDK, standard examples, smoke, and the
full gate with the pinned public runtime, then uploads generated evidence even
on failure. It uses read-only permissions and no private runtime-source checkout.

The retired publisher is not restored. `scripts/manual_gates.py` runs every
native workspace member/test and lints all three WASM guests. The full actual
stand/artifact gate must also run from an isolated writable verified candidate
bundle through `scripts/check-application-examples.sh` after its standard packaged
lifecycle. See [manual qualification](../../docs/releases.md#manual-qualification)
and [the SDK 0.2.0 adaptation](SDK-0.2.0.md). Public binaries do not replace the
private node build, engine/source qualification, or receipt-bound candidate gate.

## Scope

This is a same-DApp SDK fixture with simulated main value, separate operating
SHELL, and runtime-authentic native callbacks. It is not a live NACKL wallet,
mainnet deployment, mining/licence integration, or independent security audit.
There is no beneficiary/manager rotation, wallet withdrawal, fee withdrawal,
admin sweep, or timeout reissue of a missing callback. A missing callback can
leave that member pending; others remain independent. Managers have availability
power through pause and reserve changes. Audit delivery is asynchronous: a failed
report can leave it behind a paid claim, and there is no recovery service.

The current guest/profile match the verified public 0.2.0 distribution. Validation
establishes this exact combination, not future-runtime or cross-DApp assurance.
See [SPEC.md](docs/SPEC.md) for wire layouts, invariants, and trust assumptions.
