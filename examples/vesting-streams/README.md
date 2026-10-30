# AVM Vesting Streams

A MessageIsolated Rust example for up to eight fixed-beneficiary grants, with
cliffs, linear or stepped accrual, optional cancellation and independent
payout/refund callback lanes. It uses the in-tree guest SDK and retains the
upstream application's MIT license.

The example explicitly chooses ECC id `0` for synthetic test principal. This
does not assign an asset to ECC0, reserve that id in AVM, convert an older
currency format, or provide a live token adapter. Execution funds are SHELL
(ECC id `2`), accounted for separately.

The owner funds the exact aggregate principal once before the earliest start.
Beneficiaries sign claims to immutable payout destinations. Cancellation
freezes earned rights and permits only the unearned part to reach the fixed
refund destination. Each lane allows one pending payment; only an authentic
callback settles or restores its entitlement. Missing callbacks keep that lane
pending. Native callbacks may carry unused FUEL refunds as SHELL; those refunds
cannot replace or enlarge the principal entitlement.

The local journey funds grants of 1,000, 600 and 300 test units, exercises
unauthorized and premature claims, rejects and retries a payout and a refund,
then checks receiver totals `[[400,600],[400,200],[300,0]]`. It requires final
claimed=1,100, refunded=800, held=0, pending=0, an empty queue and exact journal
replay of roots, accepted inputs and fees. This is a local correctness fixture,
not a network, production performance or security audit result.

Use Rust 1.93.0 with rustfmt, Clippy and `wasm32-unknown-unknown`, Python 3.11+
and a matching installed SDK/tool profile. From this example directory:

```bash
export TMPDIR=/path/to/external/scratch
export TMP="$TMPDIR" TEMP="$TMPDIR"
export AVM_EVIDENCE_DIR="$TMPDIR/vesting-check"
bash scripts/install_binaryen.sh "$TMPDIR/vesting-tools"
bash scripts/check.sh /path/to/installed/sdk \
  "$TMPDIR/vesting-tools/binaryen-version_133/bin/wasm-opt"
```

The check runs the native regression suite, 1,200 independent Python bigint
oracle cases, 14,400 generated state transitions, both real WASM builds and
Clippy checks, canonical identifier and no-import export checks, then the
complete stand journey and replay. The official builder checks two clean
compilations. The checksum-pinned Binaryen 133 optimizer runs twice and its
outputs must match. Generated evidence goes into the external directory
selected above.

Recipients must be distinct accounts in the same DApp. Receivers are bound
fixtures that retain payments and provide no withdrawal API. Callback checks
use native VM authenticity and replay protection; they do not promise safety
against arbitrary forged VM headers. No upgrade or state migration is supplied.

See the [application protocol](docs/SPEC.md), [source provenance](docs/PROVENANCE.md)
and [SDK integration notes](docs/SDK-INTEGRATION.md).
