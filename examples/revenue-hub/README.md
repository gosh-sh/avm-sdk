# Revenue Hub example

A fixed-share revenue vault, bound payout receivers and an authenticated
AccountBatch receipt ledger. The same Rust entrypoints run in native regressions
and real guest WASM. Governance is 2-of-3 with a timelock; recipients and weights
are fixed at deployment. This is a local synthetic-funds example, not a production
wallet or audited financial service.

## Principal and fees

`PRINCIPAL_CURRENCY_ID = 0` explicitly selects the example principal for new
ECC-only instances. This does not name a live token, reserve ID 0, identify or
convert the old unnamed main asset, or support existing-state migration.
Deposit with the stand option `--extra-currencies 0:10007`; a simultaneous
positive SHELL donation may use `0:10007,2:10000`. Other assets are rejected.
Claims send the stored member's exact principal amount and immutable destination.

SHELL (ECC 2) funds execution separately. The topup method accepts only positive
SHELL. Success callbacks may return unused FUEL as SHELL; failures must return
exact pending ECC 0 principal plus optional SHELL. Actual rent, delivery,
callback and route charges remain runtime-enforced; the reserve precheck does
not estimate all fees.

## Run checks

Use Rust 1.93.0, rustfmt, Clippy, `wasm32-unknown-unknown`, Python 3 and Node.
No external Rust dependency is added. Binaryen 133 is a build tool: its existing
installer verifies a pinned archive checksum. From the SDK root:

```bash
bash examples/revenue-hub/scripts/install_binaryen.sh /absolute/path/to/tools
bash examples/revenue-hub/scripts/check.sh \
  --sdk /absolute/path/to/verified/avm-sdk-linux-x86_64 \
  --optimizer /absolute/path/to/tools/binaryen-version_133/bin/wasm-opt \
  --out /absolute/path/to/revenue-check
```

Explicit tools can replace `--sdk` with `--builder`, `--stand` and `--profile`.
Use writable output outside the source tree. Current guests require the ECC-only
SDK/runtime ABI; the historical SDK 0.2.0 distribution is incompatible.

This one entry point checks native formatting/Clippy and all 82 vault/receiver/
audit regressions, the separate compact audit's 10 differential regressions,
all guests, official two-build reproducibility, two independent pinned optimizer
outputs, canonical IDs, exports/zero host imports, and 20 native/WASM ABI vectors.
It runs actual legacy and compact local-stand journeys, rejected deliveries and
retry, governance negatives, canonical state-mode boundaries, copied-state restart,
rent deltas, legacy upgrade rejection and journal replay.

Both journeys require `10010 = 5005 + 3003 + 2002` paid, zero pending/claimable/
dust, matching audit totals/counts, and an empty queue. Replay must match roots,
accepted input count and cumulative SHELL fees. The audit is asynchronous;
failed audit delivery can leave it behind payments, and stale nonces are rejected.
These are local correctness checks, not production pricing, throughput, network
or release qualification.

## Layout and limits

[The contract specification](docs/SPEC.md) describes amounts, authority, layouts
and errors. The regular audit retains its 65,540-byte data layout. Under the
current ECC-only account format, a SHELL-only audit account has
`39 + 20 + 65540 = 65599` canonical bytes and uses patches after init.
The [optional compact audit](experiments/compact-audit/README.md) is separate
code/schema for new installations only. It does not authorize direct migration
or upgrade of regular audit accounts.

Missing native callbacks leave individual claims pending. There is no timeout
unlock, sweeping of liabilities, arbitrary withdrawal or recovery service.
Pausing or selecting an unfunded operating reserve can prevent new claims.
Callback authenticity relies on AVM-native provenance and canonical queue replay
protection. Same-DApp local fixtures do not prove other routing environments.

## Source and license

Imported from [h2so4nackl-code/avm-revenue-hub at 8ecfd240](https://github.com/h2so4nackl-code/avm-revenue-hub/tree/8ecfd240afea03d175978ef2570e632f6376f9db)
via SDK PR #10 at `232fc3c23f0c45666964e2d4ddabff7073ddb8d3`.
The original 80+10 regression expectations and financial sums are retained;
canonical frames, explicit principal and constructor/tool interfaces are adapted.
The original MIT attribution is in [LICENSE](LICENSE) and [PROVENANCE.json](PROVENANCE.json).
Historical validation for the original ABI remains on that PR and does not
qualify these current guests.
