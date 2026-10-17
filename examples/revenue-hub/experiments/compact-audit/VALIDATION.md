# Local qualification — 16 October 2026 UTC / 17 October Berlin

This is a local preparation result, not a hosted GitHub Actions result.
The experiment is additive over `main` commit
`5218800bd65b90cb6f5830f2becd17782e509e65`. No original contract, root Cargo
workspace or existing CI workflow is modified. PR #1 is still independent.

| Gate | Result |
| --- | --- |
| Rustfmt / Clippy all-targets, warnings denied | PASS |
| Deterministic native suite | 10 passed |
| Official AVM builder, four temporary contracts | PASS |
| Canonical method/event IDs | Unchanged |
| Pinned Binaryen optimization repeated twice | Identical bytes |
| Optimized official guest versus native ABI vectors | 10 passed |
| Complete synthetic financial journey | 10,010 received = 10,010 paid |
| Audit totals / counts | 5,005, 3,003, 2,002 / 2, 1, 1 |
| Restart from copied persisted state | Identical inspect; journey resumed |
| Final queue / replay | Empty / matching roots, fees and input count |
| Canonical sizes (1, 3, 8, 16 members) | 215, 407, 887, 1,655 bytes |
| Matched rent measurements | 15 probes; hour/day differences equal byte-seconds |
| Legacy direct upgrade | Rejected for developer, package-owner and user |

The correct binding owner's native reserved upgrade call also fails with 3201,
without state changes or `CodeUpgradeIntent`. This qualifies a known migration
blocker, not a successful migration. Legacy fixture data remains unchanged.

Pinned runtime: `06e96ef71006cca020575cbf35e6b82cc82a324e`.
Pinned SDK: `0748f4f0767a26da8bc363bcb6b5585539d6970e`.
Guest SDK: 0.1.1. Rust: 1.93.0. Binaryen: 133.

Official optimized compact artifacts from this run:

- `contract.wasm`: 56773 bytes; SHA-256 `0d4e597eb3724416f6accc33d9777f6c272d197a6ca36ccba6513833bad3fa3c`.
- `contract.acab`: 552 bytes; SHA-256 `072cc5a2bbf0554da0aae1faf06a555e2731d852e2cdd71b4a1aa9b613e27b58`.
- `contract.descriptor`: 66 bytes; SHA-256 `48d8e8dcf9ebb422b45e921fec2eda90bba3e79e657ca0fc7a1eff2f373e008d`.

Reproduce with the command in README.md. The workflow uploads generated logs,
full local journals and JSON summaries from `experiments/compact-audit/evidence/`.
Those run-specific outputs are not committed. The preparation archive includes
these local evidence files separately for review.

Limits: no unpublished candidate runtime, mainnet transaction, real funds,
existing-account migration, crash-in-mid-write test, delivery recovery service,
independent security audit or hosted CI approval is claimed.
