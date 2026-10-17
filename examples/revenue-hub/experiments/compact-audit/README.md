# Compact audit ledger experiment

This opt-in experiment removes retained zero padding from the Revenue Hub audit
ledger. It does **not** replace `contracts/audit`, change the normal application,
or provide an on-chain migration. It is intended for review as an alternative
for **new installations**. The normal legacy audit separately includes the ABI/mode correction from standalone PR #1.

For three configured beneficiaries the application state is 332 bytes rather
than 65,540. The published SDK 0.2.0 local stand measures a 407-byte canonical
account with one SHELL balance entry, rather than 65,615 bytes. The reduction is
`(65,615 - 407) / 65,615 = 99.3797%` of that canonical storage base.

## State and behavior

All integers are little-endian. State consists of:

| Offset | Bytes | Meaning |
| --- | ---: | --- |
| 0 | 4 | ABI bytes payload length, total state length minus 4 |
| 4 | 4 | Format/version marker `AUC2` |
| 8 | 32 | Binding authority public key |
| 40 | 1 | Fixed configured beneficiary count, 1–16 |
| 41 | 3 | Zero reserved bytes |
| 44 | 64 × n | Full authorized receiver addresses |
| 44 + 64 × n | 32 × n | `last_nonce:u64`, `total:u128`, `count:u64` |

Total state is `44 + 96*n`. Configured beneficiaries retain their slots even
before binding or while inactive. No index is deleted or reassigned. The full
source address, one-time owner-only binding, unique sources, strictly increasing
nonce, checked totals/counts, per-message status and event semantics are retained.
The schema identifier is SHA-256 of `avm.revenue.audit.compact.v2`.

Batch success now supports both `FullReplace` and `DataPatchList`, respecting the
mode supplied by the host. The small account normally needs FullReplace; account
balance inventory can affect the whole-account threshold, so data length alone
must not select the mode. Patch results still coalesce one 32-byte record per
changed member. Method and event IDs are unchanged, but the persistent format,
schema, offsets and compiled code are incompatible with the old layout.

`compact_legacy` and `expand_legacy` are **offline conversion helpers**, not
migration entrypoints. They check canonical reserved bytes and support lossless
comparison in tests. They do not grant migration authority.

## Reproduce

Requirements: Rust/Cargo 1.93.0 with rustfmt, Clippy and the wasm32 target;
Python 3.11+; Node; the pinned SDK 0.2.0 Linux bundle; Binaryen 133. The existing
root installation scripts verify the downloaded release checksums.

```bash
bash scripts/install_runtime.sh
bash scripts/install_binaryen.sh
bash experiments/compact-audit/scripts/verify.sh \
  "$PWD/.runtime/avm-sdk-linux-x86_64" \
  "$PWD/.runtime/binaryen-version_133/bin/wasm-opt"
```

The experiment is an independent Cargo workspace using the repository's in-tree guest
SDK and local core and the legacy contract as a test oracle. Runtime verification
copies the project to a temporary directory, substitutes the compact audit only
there, and builds all contracts with the official builder. Vault/receiver sources
are copied unchanged. Temporary manifests/locks and a legacy qualification
fixture do not modify the repository's normal contracts or Cargo workspace.

The official builder checks reproducible compiler outputs. Existing Binaryen
optimization is required: the unoptimized vault exceeds the deploy-envelope
limit. Canonical method/event IDs are checked after building. Runtime state and
funding are synthetic, file-backed and temporary. No network transactions occur.
Generated evidence is ignored by Git and uploaded by the separate CI workflow.

## Qualification

- Ten deterministic native tests include 512 differential batches and 17,408
  message inputs over all counts 1–16 and both output modes, plus binding/auth,
  overflow, max/oversized batches, corrupt states, restart bytes, and rejection
  of the legacy reserved upgrade call.
- Ten WASM/native ABI vectors run against the officially built, optimized guest.
- The full local financial journey reconciles 10,010 synthetic units, including
  rejection/refund/retry, duplicate audit reporting and governance. Final audit
  totals are 5,005 / 3,003 / 2,002; counts are 2 / 1 / 1; the queue drains.
- The journey resumes from a byte-identical copied state in a fresh Stand object.
  Every command launches a new process. Replay matches account/registry roots,
  cumulative fees and accepted-input count. This is not crash-in-mid-write testing.
- Five account sizes are measured at three elapsed intervals each. Matched
  snapshot comparisons isolate time-dependent storage cost from other charges.
- Three direct legacy upgrades are rejected, including `package-owner`; legacy
  data remains identical and replay still passes.

See [VALIDATION.md](VALIDATION.md) for the local preparation result. Hosted CI is
not claimed until this branch is published and that workflow actually passes.

## Rent: local measurements only

The [maintainer's clarification](https://github.com/gosh-sh/avm-sdk/issues/21#issuecomment-6204319064)
explains canonical bytes, policy overhead, elapsed time and prior debt. Mainnet
pricing and a grace period are not confirmed. This experiment executes the
**published** runtime `06e96ef71006cca020575cbf35e6b82cc82a324e`, not the unpublished
candidate discussed in that comment.

With one SHELL balance entry and zero debt, the measured canonical sizes and
incremental fees for an additional 86,400 seconds are:

| Ledger | Canonical bytes | Additional local SHELL / day |
| --- | ---: | ---: |
| Legacy | 65,615 | 5,669,136,000 |
| Compact, 1 beneficiary | 215 | 18,576,000 |
| Compact, 3 beneficiaries | 407 | 35,164,800 |
| Compact, 8 beneficiaries | 887 | 76,636,800 |
| Compact, 16 beneficiaries | 1,655 | 142,992,000 |

Each value equals `bytes * 86,400`. Calls at 1, 3,601 and 86,401 seconds use copies
of one funded snapshot. Subtracting the 1-second total charge isolates the extra
hour/day. For three beneficiaries, total charges are 10,408 and 35,175,208 SHELL
at 1 and 86,401 seconds; their difference is 35,164,800. This difference is not the
full transaction fee. No additional per-byte-time overhead appears in these
specific probes; no general zero-overhead policy is inferred. Frozen/unpaid
candidate behavior and mainnet economics are not qualified.

## Existing-account migration is blocked

The current legacy contract does not emit `CodeUpgradeIntent`. With the correct
binding owner and the reserved upgrade method, the native contract returns
`E_AUDIT_AUTH` (3201). The local `upgrade` command rejects all three identities
with `retained account did not switch to the requested selected_code_id`.
Installing migration logic in the new code cannot retroactively authorize the
old code. Do not claim this experiment can upgrade an existing account in place.

A future migration would need an explicitly supported, authorized route; no such
route is demonstrated here. A new audit address also cannot simply replace the
one embedded in existing receivers: coordinating new receiver/vault bindings,
in-flight receipts, accounting and nonce continuity is separate work. Never reset
nonces, discard prior debt, or edit runtime state to force admission. Rent is
collected before WASM, so compaction must not be assumed to erase accumulated debt.

Existing asynchronous-delivery limits remain: a failed audit report can leave
this ledger behind a completed payment. An older report arriving after a newer
nonce is rejected, just as in the original. Restart tests do not add a delivery
recovery service, exactly-once guarantees, a production approval or an independent
security audit.

For the in-tree SDK adaptation and current validation, see [FOLLOWUP-VALIDATION.md](../../FOLLOWUP-VALIDATION.md). The original VALIDATION.md describes standalone preparation only.
