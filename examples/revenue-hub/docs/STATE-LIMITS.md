# Audit state compatibility — 16 October 2026

The normal Revenue Hub audit path passes on the **published SDK 0.2.0 local
stand**, with guest SDK 0.1.1 unchanged. Init uses FullReplace; its first status
or binding update already uses DataPatchList. Two defensive error paths needed
a small correction. This is local compatibility evidence, not mainnet admission
or execution of the unpublished candidate.

## Sources and exact versions

- [Maintainer clarification, issue #20](https://github.com/gosh-sh/avm-sdk/issues/20#issuecomment-6204318621):
  candidate SDK `353157393d832aa611a616b9eb68928fa942617a`, matched runtime
  `b3488b45ebbfcd1668e549b5c630a61c89b55a4d`. The author explicitly says these
  commits are local, not the published SDK or the draft PR's delivered code.
- [Publication status](https://github.com/gosh-sh/avm-sdk/pull/17#issuecomment-6204320896):
  the candidate archive is retained locally, not a public release asset.
- Tested release: [SDK 0.2.0 bundle](https://github.com/gosh-sh/avm-sdk/releases/tag/avm-dev-06e96ef71006-sdk-0748f4f0767a),
  SDK `0748f4f0767a26da8bc363bcb6b5585539d6970e`, runtime
  `06e96ef71006cca020575cbf35e6b82cc82a324e`. Archive SHA-256
  `8728497cf1e476d3d770a09b19929f7c196f2637654642727432b4c285c85143`.
- The [official fixture account encoder](https://github.com/gosh-sh/avm-sdk/blob/0748f4f0767a26da8bc363bcb6b5585539d6970e/scripts/tests/fixture_oracle.py)
  describes the tested account layout. The official guest's
  [output validation](https://github.com/gosh-sh/avm-sdk/blob/0748f4f0767a26da8bc363bcb6b5585539d6970e/crates/avm-contract-sdk/src/output.rs)
  validates the requested mode in the exported ABI. Context-free
  `encode_batch_output()` alone does **not** perform that input-dependent check.

## Account sizing and transition

`STATE_BYTES` is **65,540**, comprising a 4-byte application/ABI bytes-length
prefix plus 65,536 retained payload bytes. These are all data-state bytes.
The account has a separate canonical header and balance vector:

`1 flag + 32 selected-code ID + 16 main balance + 2 currency count + 4 data length = 55 bytes`.

Each extra currency adds `4 ID + 16 amount = 20 bytes`. With the single SHELL
currency entry used in this stand, the audit account therefore measures
`55 + 20 + 65,540 = 65,615 bytes`. This is checked against the stand's own
`inspect.accounts[].canonical_byte_length`, not just inferred from data length.
Other balance inventories can increase it; even zero extra currencies would
leave this audit above the 65,536-byte threshold.

Init is a special FullReplace operation. The **previous** entire image selects
ordinary-call mode; a large resulting init state does not make patch init legal.
The audit's first post-init call already has a large previous image. The contract
must obey `required_state_output_mode` rather than choose from `state.len()`.
Its isolated binding/status/receipt paths already did so; they are unchanged.

The disposable non-financial fixture executes these exact release cases:

| Previous canonical account | Data bytes (one SHELL entry) | Actual requested/output mode |
| ---: | ---: | --- |
| 65,535 | 65,460 | FullReplace |
| 65,536 | 65,461 | FullReplace |
| 65,537 | 65,462 | DataPatchList |

Forcing the opposite mode returns the released diagnostic `VM status 43` in all
three cases. A subsequent correct call succeeds. This is guest-output/mode
rejection, not evidence of an oversized output (the candidate identifies 43 as
`VM_ABI_OUTPUT_MISSING`, and 45 as oversized output).

A second test starts at exactly 65,536 bytes and attaches a new extra currency.
That call still uses FullReplace; its resulting account grows to 65,556 bytes.
The **following** call switches to DataPatchList. The data length is unchanged,
which directly tests inclusion of balances and use of the previous image.
All these accepted calls replay with identical roots, fees and input count.

## Release results versus candidate rules

| Rule | Published 0.2.0 evidence here | Candidate clarification |
| --- | --- | --- |
| Init requires FullReplace | Actual audit deployment and exported guest tests | Explicitly required |
| Previous whole-account threshold | Actual below/equal/above tests and balance transition | 65,536 bytes, including header/balances |
| Successful output must match required mode | Actual wrong-mode release calls; exported audit ABI vectors | Follow required mode; missing output is distinct from overflow |
| Resulting data at most 1 MiB | Audit data is 65,540; the release maximum is **not qualified by this suite** | 1,048,576 bytes in both modes |
| Full input/output envelopes at most 1 MiB | Actual application frames are below 262,144; release maximum not qualified | Separate 1,048,576-byte input and output caps; overhead reduces usable data |
| Patch bytes/ranges | Max supported application batch tested: 16 ranges, 512 replacement bytes; complete output below 262,144 | 262,144-byte and 512-range patch limits |

No maximum above is inferred from a successful small-output encoding. The exact
candidate host and package are unavailable here, so **candidate execution,
upper-limit admission and source/API compatibility are unverified**. Its newer
Rust output types may require a separate migration. This change keeps the
published guest SDK, contract state schema, ABI IDs and deployment descriptors.
It does not raise any limit. Storage pricing from issue #21 is outside this check.

## Confirmed defects and minimal correction

At baseline `5218800`, patch-required init returned successful FullReplace.
The exported guest would reject that mismatch rather than return an application
failure. Init now returns `E_AUDIT_ARGS` with no effects for that incompatible mode.

The batch failure closure always returned an empty DataPatchList, even when
rejecting a FullReplace-required batch. The SDK validates batch mode even if
all per-message statuses fail, so that output disappeared at the ABI boundary.
The closure now returns unchanged state in the requested mode, with the same
failure statuses and no events or outbound intents. Successful batch execution
still requires DataPatchList. An impossible large-account FullReplace request
is not thereby made admissible to the host; the correction makes the guest's
rejection serializable. An empty rejected batch has no per-message status slot;
it returns only unchanged state and no effects.

Three of the six new native tests fail against the original source; all six
pass after the correction. [Before-fix output](../evidence/audit-modes-before-fix.txt)
is retained. No vault/receiver accounting, beneficiary, authorization, replay,
overflow, governance or reserve logic was modified.

## Regressions and reproduction

- Six new native tests cover patch-init rejection; first binding/status after
  init; isolated binding/status/receipt in each requested mode; incompatible
  batch rejection including empty batches; malformed/oversized batch atomicity
  in both modes; and 256 messages across all 16 members.
- The 16-member test verifies sorted disjoint patches, 16 coalesced records,
  512 changed bytes, event input indices, exact totals/counts, frame/output bounds
  and complete rejection of a replayed batch without additional credit.
- Ten actual audit WASM entrypoint vectors cover init plus valid, empty,
  oversized and malformed-state batches in both modes. Canonical output must
  equal native output byte for byte; guest input ownership is also checked.
  This uses Node's WebAssembly engine and tests the guest ABI, not host admission.
- The actual application journey asserts the audit's canonical size and first
  post-init patch mode, exercises settlement/retry/duplicate receipts/governance,
  reconciles 10,010 synthetic units to beneficiaries and audit records, empties
  the queue, then replays roots, input count and fees.

Run the normal full gate (Linux x86-64-v3, glibc >=2.39, pinned Rust 1.93.0,
Python >=3.11 and Node >=18; Node 24.19.0 was used here):

```bash
bash scripts/check.sh
```

After building the application, rerun just the state/guest qualification with:

```bash
bash scripts/check_state_limits.sh
```

The fixture is built twice by the official builder and is never used by the
application. Its lock contains only the fixture, local core and vendored SDK.
All stand state is temporary. No network deployment, real funds or key import is
used. Native total: **80 application + 13 SDK = 93 passing tests**.
The 10 guest vectors and black-box stand journeys are additional checks, not
included in that native test count.

Machine-readable evidence:
[state boundaries](../evidence/state-limits-sdk020.json),
[guest modes](../evidence/audit-guest-modes.json),
[application/replay summary](../evidence/avm-e2e-summary.json), and
[complete validation](../evidence/VALIDATION.json).
