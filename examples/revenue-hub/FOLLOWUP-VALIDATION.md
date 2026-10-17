# Revenue Hub follow-up — 17 October 2026

This follow-up updates the existing integration PR #10; it does not create a
second competing Revenue Hub integration. It imports standalone changes merged
at `8ecfd240afea03d175978ef2570e632f6376f9db` (PR #1 and #2), adapting SDK paths
and Rust formatting to this repository. Base integration head:
`c4ca22aac0d880318014989cf8c656999aeade6b`.

## Behavior

- Legacy audit: reject patch-mode init; preserve the requested output mode and
  unchanged data on batch rejection. State layout, schema, ABI IDs, normal
  successful patches, beneficiary rules and accounting remain unchanged.
- Compact audit: independent optional workspace for new installations. Normal
  application builds retain legacy audit. FullReplace and DataPatchList both
  work for compact batch output; a distinct AUC2 state/schema is required.
- The complete example check now runs six new legacy regressions, actual legacy
  WASM vectors, canonical account threshold probes and the compact experiment.
  CI uploads both ordinary and compact generated evidence.
- The compact local harness copies the in-tree guest SDK unchanged into a
  disposable fixture, retaining SDK workspace package metadata and rebasing only
  temporary Cargo dependency paths. Source SDK, profile, normal vault/receiver,
  root workspace/lockfile and release implementation remain unchanged.

## Local evidence

Tested with Rust 1.93.0, published distribution 0.2.0 / guest 0.1.1, runtime
`06e96ef71006cca020575cbf35e6b82cc82a324e`, packaged SDK
`0748f4f0767a26da8bc363bcb6b5585539d6970e`, and Binaryen 133. The archive was
verified with the trusted checkout's installer and checksum pinned by the
existing install script. Guest source and engine profile match the public bundle.

| Gate | Result |
| --- | --- |
| SDK native tests | 13 passed (9 + 4) |
| Revenue Hub native cases | 80 passed (37 + 13 + 6 + 15 + 9) |
| Compact native suite | 10 passed; includes 512 differential batches / 17,408 inputs |
| Distinct native test total | 13 + 80 + 10 = 103 |
| Format / native Clippy / normal guests' WASM Clippy | Passed |
| Official reproducible builds and deterministic optimization | Passed for normal and compact fixture builds |
| Canonical IDs / normal artifacts' exports and zero host imports | Passed |
| Actual native/WASM output agreement | 10 legacy + 10 compact vectors passed |
| Legacy canonical boundary and balance transition probes | Passed at 65,535 / 65,536 / 65,537 bytes |
| Normal journey and replay | 10,010 received = 5,005 + 3,003 + 2,002 paid; no pending/claimable/dust |
| Compact journey, copied-state restart and replay | Same totals; audit counts 2 / 1 / 1; empty queue |
| Compact canonical size with three members | 407 bytes; 332 application data bytes |
| Rent probes | 15 matched local probes across five sizes |
| Direct legacy upgrade | Rejected for all three fixture identities; legacy data unchanged |
| SDK smoke | Passed |
| Python / Node / shell syntax; git diff whitespace | Passed |

The normal gate and compact gate were executed locally; a temporary-harness
manifest issue was corrected and the complete compact gate then passed. Run-specific
logs and journals are generated under `evidence/` and
`experiments/compact-audit/evidence/`; they are ignored and uploaded by CI.
Historical `SDK-0.2.0.md` and `VALIDATION.md` describe earlier qualification, not
this follow-up. Hosted results must be read from the workflow for the new commit.

## Compatibility and qualification boundary

**No direct migration of existing accounts is supported.** Legacy code lacks an
authorized upgrade handler and existing receivers embed the original audit
address. Offline conversions are comparison tools, not on-chain authority.
Coordinated replacement, receipt/nonce continuity and old rent debt need a
separate supported design. No production-state editing or nonce reset is allowed.

Local rent is not confirmed mainnet pricing. Failed asynchronous audit reports
can leave the ledger behind payments, and stale reports are rejected after newer
nonces. Restart tests do not establish crash-in-mid-write recovery or delivery
repair. Absolute SDK diagnostic paths can still change WASM bytes across source
locations; each official builder's same-location reproducibility checks pass.

No unpublished candidate, private node/runtime qualification, receipt-bound
candidate package, mainnet deployment, real transaction, independent security
audit, upstream acceptance or reward is claimed. Existing release gates remain.

## Reproduce

From the repository root:

```sh
bash examples/revenue-hub/scripts/install_runtime.sh
bash examples/revenue-hub/scripts/install_binaryen.sh
cargo +1.93.0 test --locked --offline
bash scripts/check-application-examples.sh \
  "$PWD/examples/revenue-hub/.runtime/avm-sdk-linux-x86_64" \
  "$PWD/examples/revenue-hub/.runtime/binaryen-version_133/bin/wasm-opt" revenue-hub
```
