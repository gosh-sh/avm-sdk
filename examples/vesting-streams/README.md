# AVM Vesting Streams

A Rust AVM reference application for scheduled grants, contributor payments, and
token vesting. One funded contract manages up to eight independent streams with
immutable recipients, cliffs, linear or stepped accrual, and optional cancellation.
Designed as an SDK example and regression-suite candidate.

## Behavior

- The owner funds the exact aggregate grant amount once, before the earliest start.
- Each beneficiary signs to claim their newly accrued entitlement. Principal goes
  to the fixed payout address; callers cannot redirect it.
- A cliff gates the entire grant until its timestamp, then catches up from the
  start. Stepped schedules release complete intervals and the remainder at the end.
- Optional owner cancellation freezes earned rights. Beneficiaries keep the earned
  part; the owner can return only the unearned part to the fixed refund address.
- Every stream has separate beneficiary and refund lanes. One pending lane does
  not block other streams or the opposite lane.
- Successful native callbacks settle principal once. Failed delivery must return
  the exact principal before that lane can retry. Receipts bind source, method,
  causal logical time, and, on success, the exact nonce and payment body.
- Arithmetic supports the full `u128` range without multiplying amount by time
  in `u128`. State stays bounded across repeated claims and retries.

## Verified local journey

The actual AVM journey deploys one vesting account and six bound receiver accounts.
It funds three grants, checks unauthorized and premature claims, rejects and retries
a beneficiary payment, cancels two streams, rejects and retries a refund, and finishes
an irrevocable stream. Final receiver totals are:

| Stream | Funded | Beneficiary received | Refund received |
| --- | ---: | ---: | ---: |
| Linear, cancelled | 1,000 | 400 | 600 |
| Stepped, cancelled | 600 | 400 | 200 |
| Irrevocable | 300 | 300 | 0 |
| Total | 1,900 | 1,100 | 800 |

No principal remains held or pending. The queue is empty and journal replay matches
account and registry roots, accepted-input count, and cumulative fees. These are
local main-value units, not a live NACKL payment or bounty award.

## Validation and reproduction

Requirements: Linux x86-64-v3 with glibc >= 2.39 (Windows: WSL2), Bash, Python >= 3.11, curl, and rustup.
All Rust dependencies use this repository's SDK directly; checks and builds run
offline after tool installation. The example has its own workspace and lockfile.

```bash
rustup toolchain install 1.93.0 --profile minimal --component rustfmt --component clippy
rustup target add wasm32-unknown-unknown --toolchain 1.93.0
bash scripts/install_runtime.sh
bash scripts/install_binaryen.sh
bash scripts/check.sh
```

`check.sh` verifies workspace formatting and the seeded independent Python oracle,
native and WASM Clippy, 60 application regressions, official
reproducible builds, canonical identifiers, two matching optimizer outputs, WASM
exports without host imports, and the actual local stand journey. One regression
checks 1,200 bigint oracle cases; another exercises 14,400 generated state transitions.
Sixteen simultaneous pending lanes and reverse settlement are native regressions;
the actual stand journey automatically drains its delivery queue.

The dedicated `vesting-streams.yml` workflow additionally runs all 13 SDK tests,
existing example conventions, smoke, and the complete gate from relocated tracked
source against verified public 0.2.0 tools. This replaces the obsolete packaging
rehearsal: the v2 packager requires exact private-source qualification and real
build receipts, which a public-binary copy cannot supply. It uploads generated
`evidence/` logs, artifacts, optimizer results and the full stand journal.

`manual_gates.py` checks all native tests and both WASM guests. Candidate-package
execution remains mandatory after the standard packaged lifecycle, through
`scripts/check-application-examples.sh` in an isolated writable verified candidate.
See [manual qualification](../../docs/releases.md#manual-qualification) and
[the SDK 0.2.0 adaptation](SDK-0.2.0.md). The old compiler wrapper is removed;
the pinned builder rejects it. [VALIDATION.md](VALIDATION.md) retains historical
integration evidence separately from the new results.

## Scope

Recipients must be distinct accounts in the same DApp. The receiver is a bound
testing fixture that retains payments; it is not a general wallet or withdrawal
adapter. Missing callbacks keep the affected lane pending, because unlocking it
without an authentic outcome could permit double payment. Receipt checks rely on
AVM-native callback authenticity and queue replay protection, not on resistance
to arbitrary forged VM headers. SHELL operating funds are separate from principal.

See [the protocol specification](docs/SPEC.md), [provenance](docs/PROVENANCE.md),
and [SDK integration notes](docs/SDK-INTEGRATION.md). Project code is MIT licensed;
the upstream SDK retains its own licence. No SDK source is copied here.
