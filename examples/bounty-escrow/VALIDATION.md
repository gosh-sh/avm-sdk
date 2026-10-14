# Current validation

The SDK 0.2.0 migration and its independently rerun results are recorded in
[SDK-0.2.0.md](SDK-0.2.0.md). The record below is historical evidence for the
pre-PR #14 inputs. Its retired-publisher and old-package-rehearsal descriptions
are not current instructions or evidence for the 0.2.0 candidate.

## Historical validation record

# Integration validation

Integration checked on 12 October 2026 against upstream SDK commit
`72ee42dbf923f081bf29b4a1a1b6ea012b6af54f`, using Rust 1.93.0,
Linux x86-64, and the verified official reference-runtime release
`avm-dev-335a0f09fdbc-sdk-0cbca40fad9a`.

These results are fresh runs of the integrated source, separate from the
[original standalone CI run](https://github.com/h2so4nackl-code/avm-bounty-escrow/actions/runs/38196158944).

## Executed checks

| Check | Result |
| --- | --- |
| Reference runtime archive and extracted file SHA-256 verification | PASS |
| Root SDK formatting / native Clippy (`-D warnings`) | PASS |
| Root SDK tests | PASS: 8 unit + 4 canonical I/O = 12 |
| Four existing examples: formatting and WASM Clippy (`-D warnings`) | PASS |
| New example in the SDK wildcard formatting / WASM Clippy loop | PASS |
| New example formatting / all-target native Clippy (`-D warnings`) | PASS |
| Escrow/receiver native regression suite | PASS: 35; 0 failed |
| Generated action sequences within one native regression | PASS: 100 × 80 actions |
| Official reproducible builder, escrow and receiver | PASS: both WASM/ACAB/descriptor triples |
| Canonical ACAB method/event IDs | PASS |
| Each contract's WASM Clippy (`-D warnings`), receiver without escrow guest feature | PASS |
| WASM required exports and zero host imports | PASS for both contracts |
| Actual local-stand scenario | PASS |
| Final principal accounting | PASS: 1000 = 700 paid + 300 refunded; pending = 0 |
| Empty stand queue and journal replay comparisons | PASS |
| Existing packaged CLI smoke with reference binaries | PASS |
| Current release packaging script and packaged file checksums | PASS using verified reference binaries |
| Full gate run from the packaged example source | PASS: 35 native tests, reproducible builds, IDs, WASM lint/surface, stand/replay |
| Shell / Python / workflow YAML syntax | PASS |
| Original standard fixtures and SDK source/manifests/lockfile | Byte-identical to baseline |

The local-stand journey includes failed payout delivery with principal return,
retry and settlement, failed expiry refund with principal return and retry,
approved payment after expiry, and rejected duplicate/unauthorized/premature/late
operations. Five outbound attempts finish in milestone phases `[3, 3, 4]`.
Rejections are checked against contract status codes in canonical executor output.

The initial wildcard `cargo fmt` command could not resolve targets for a virtual
workspace with explicit default members. Adding `--all` to the example formatting
loop resolved that integration issue; the final wildcard gate passed for all five
example workspaces. Stable rustfmt emits existing nonfatal warnings for the SDK's
nightly-only formatting settings; formatting checks exit successfully. No SDK
format configuration or standard fixture was changed.

## Artifact SHA-256 from the integrated build

| Contract | Artifact | SHA-256 |
| --- | --- | --- |
| escrow | `contract.wasm` | `2d82a7ede969258374d3ab7dbfec44580b7eab568a0d0b9c2912d296c79fc1bc` |
| escrow | `contract.acab` | `116a73ca267c1c7fd4128ae37059516283ccc6de20a5401386cf7be47f828a95` |
| escrow | `contract.descriptor` | `5f82f8befe396d0a21ece803f39ad03ea861a69e67ef4b0dbc8509a56a1803fe` |
| receiver | `contract.wasm` | `15f4d60cb7991963015573b5e020d3020a139368f834946b1b711bd0d9a63a5c` |
| receiver | `contract.acab` | `708fed4aee3a4310a8875da668c72dee0d2aadc14d924aa135b9e3fc1eeb8883` |
| receiver | `contract.descriptor` | `76ab2ed990fab0a31c8d3f43792a86d8f72449001a047193e8f8579a13cdd462` |

These hashes describe the integrated build. They are not asserted to equal the
standalone project's previous build hashes. Reproducibility is checked within
this build context by the official builder.

## Matching local inspection and replay

- `account_state_root`: `4c47af6dc2e339295eb88e1024c9fb71b41456b8b018892ba42beef8e6da9ad1`
- `registry_root`: `7e0d1d2f5e8c0a4c745e73683ab41e102e172b8ee871f35646525ed738559427`
- `accepted_input_count`: `37`
- `cumulative_fee_shells`: `614043`

Execution SHELL fees are separate from synthetic main-value principal. The
account and registry roots, accepted-input count, and cumulative fees above were
compared between the actual stand inspection and its replay. No live funds or
network endpoint were used.

## Packaging rehearsal

The current `scripts/package-release.sh` was executed with the verified reference
`avm-contract-build` and `avm-local-stand` binaries staged in its expected source
build-output layout. The complete packaged file SHA-256 manifest passed. The full
escrow gate was then run from the packaged example source, against those packaged
binaries, and passed. This checks archive inclusion and relative dependency/script
paths. It does not represent a runtime rebuilt from private source.

## Checks requiring the upstream environment

The private `gosh-sh/acki-nacki` source checkout, focused AVM runtime-source
checks/tests, source stand E2E, and a newly compiled runtime release have not been
run in this workspace. Those existing upstream release gates remain required.
The release workflow now additionally runs the escrow gate with the freshly
packaged binaries before publication; compatibility with an unpublished runtime
is not inferred from the reference-runtime results.

Generated logs and the complete local-stand journal are ignored build evidence.
The new hosted workflow uploads them as `bounty-escrow-evidence`. No external
audit, mainnet deployment, upstream acceptance, or reward is established here.
