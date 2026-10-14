# Bounty Escrow regression example

A Rust milestone escrow and a bound accepting/rejecting receiver fixture. The
native tests exercise the same entrypoints compiled into AVM WASM. The local-stand
journey checks failed payout delivery and exact principal return, payout retry,
failed expiry refund and retry, approved work paid after expiry, and journal replay.

This is an SDK example with synthetic local principal, not a production wallet.
There is no mainnet deployment, independent audit, upstream acceptance, or confirmed
reward. See [the contract specification](docs/SPEC.md) for the wire layout,
authority model, accounting invariants, and runtime trust assumptions.

## Integration layout

The SDK's four standard contract fixtures are unchanged. This example has its own
workspace and lockfile, like the existing examples. Its default member is the
escrow contract so the existing `examples/*/Cargo.toml` WASM lint loop checks one
guest export set. The full gate explicitly checks both contracts and all native
tests. The receiver disables the escrow's `guest` feature to avoid duplicate WASM
entrypoint exports.

Both contracts and the regression suite depend directly on the repository's
`crates/avm-contract-sdk`; there is no vendored SDK or external Rust dependency.
The root SDK workspace and lockfile remain unchanged. Release packaging already
archives `examples/`, so it carries the contracts, tests, scripts, and this guide.

Source originates from
[`h2so4nackl-code/avm-bounty-escrow@054300d719445a0f3c1229f63e033cc59a8c0d80`](https://github.com/h2so4nackl-code/avm-bounty-escrow/tree/054300d719445a0f3c1229f63e033cc59a8c0d80).
Contract behavior and native regression cases are preserved; Rust source is
formatted with the SDK conventions. Dependency paths, workspace configuration,
lockfile, and check commands are adapted for this tree. Its MIT copyright notice
is retained in [LICENSE](LICENSE).

## Run native checks

Use the SDK's pinned Rust 1.93.0 toolchain with Clippy, rustfmt, and
`wasm32-unknown-unknown` installed. Python 3 is needed for artifact/stand checks.
From the repository root:

```bash
cargo +1.93.0 test --locked --offline
cargo +1.93.0 test --manifest-path examples/bounty-escrow/Cargo.toml --workspace --locked --offline
```

The first command checks the SDK itself; the second runs 35 escrow/receiver
regressions. One regression executes 100 deterministic sequences of 80 actions
and checks principal conservation. The native suite also covers old approval
revisions, malformed frames/state, caller authority, callback replay, pending
transfer isolation, deadline boundaries, fee/principal isolation, and attempt limits.

## Full build and local-stand gate

Linux x86-64-v3 and glibc >= 2.39 are required for the published stand
(Windows: use WSL2); use Python >= 3.11 for the verified installer. From the
repository root, install the immutable reference runtime and run the example:

```bash
bash examples/bounty-escrow/scripts/install_runtime.sh
bash examples/bounty-escrow/scripts/check.sh
```

The installer pins the official SDK 0.2.0 / guest SDK 0.1.1 release
[`avm-dev-06e96ef71006-sdk-0748f4f0767a`](https://github.com/gosh-sh/avm-sdk/releases/tag/avm-dev-06e96ef71006-sdk-0748f4f0767a)
and archive SHA-256 `8728497cf1e476d3d770a09b19929f7c196f2637654642727432b4c285c85143`.
The trusted checkout's v2 installer verifies provenance and the complete inventory
before bounded extraction and activation. The contracts compile against this
checkout's SDK crate; the shared application runner checks guest/profile parity.
See [the SDK 0.2.0 adaptation](SDK-0.2.0.md) for exact inputs and limitations.

To check another verified SDK distribution, including a freshly packaged release:

```bash
bash examples/bounty-escrow/scripts/check.sh /absolute/path/to/avm-sdk-linux-x86_64
```

The gate checks formatting, locked offline native Clippy/tests, builds both
contracts with the official `avm-contract-build`, validates canonical method/event
IDs against the generated ACAB, lints each guest separately, runs the actual
`avm-local-stand` journey, and inspects WASM exports/absence of host imports.
The official builder performs two clean builds and rejects non-reproducible
WASM, ACAB, and descriptor output. Both contracts produce their triples in
`contracts/<name>/target/avm/`.

Evidence is regenerated under `examples/bounty-escrow/evidence/` (ignored by Git):
native/lint/build logs, runtime manifest and hashes, artifact hashes, and the E2E
JSON journal. The final escrow assertion is `1000 = 700 paid + 300 refunded`, with
no pending transfer and an empty queue. Replay must match account and registry
roots, accepted input count, and cumulative SHELL fees. These are synthetic
main-value amounts; execution SHELL is checked separately. Contract rejections
are checked using canonical `ACMO` status codes, not just the CLI's outer status.

## CI and release gates

`.github/workflows/bounty-escrow.yml` runs on a hosted Linux runner with the pinned
reference release, without the private runtime-source checkout. It checks SDK
source and all standard examples before this example and uploads generated
evidence, including on failure.

The retired SDK publisher is not restored. `scripts/manual_gates.py` checks
all native workspace tests and both guests. After receipt-bound candidate packaging
and the standard packaged lifecycle, run `scripts/check-application-examples.sh`
from the isolated writable verified bundle as described in
[manual qualification](../../docs/releases.md#manual-qualification).
A public binary run does not replace private runtime-source or candidate gates.

Original published CI evidence:
[escrow source run 38196158944](https://github.com/h2so4nackl-code/avm-bounty-escrow/actions/runs/38196158944).
That run validates the standalone source; integration results are separate and
recorded in [VALIDATION.md](VALIDATION.md).

## Limits

- At most eight milestones and 64 outbound attempts, including failed attempts.
  Exhausting the attempt bound can leave unresolved principal locked.
- A native callback that never arrives leaves the transfer pending. There is no
  timeout unlock that could allow a second payment before a late first settlement.
- Callback authenticity relies on AVM-native provenance. Ordinary callers must
  not be able to inject VM-owned callbacks.
- The receiver is a bound fixture that retains received principal, not a wallet.
- No arbitration, token adapters, unused-fee withdrawal, administrative recovery,
  or automatic expiry execution is provided. Owner approval is not proof of work quality.
