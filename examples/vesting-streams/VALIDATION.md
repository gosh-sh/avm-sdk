# Integration validation

This is local reproduction evidence for a bounded SDK fixture, not an independent
security audit, mainnet deployment, maintainer approval, or bounty acceptance.

## Immutable inputs

- Upstream SDK base: `72ee42dbf923f081bf29b4a1a1b6ea012b6af54f`.
- Imported application: [`e258b0e516519202bdd937445e38efddce8c4741`](https://github.com/h2so4nackl-code/avm-vesting-streams/tree/e258b0e516519202bdd937445e38efddce8c4741).
- Compatibility PR #9: `8998a508056807185841956f7584d0daea827f22`.
- Compatibility PR #10: `fec8b211305e08b10106bcde69371d5285917822`.
- Rust 1.93.0 with rustfmt, Clippy and `wasm32-unknown-unknown`.
- Checksum-verified public runtime: `avm-dev-335a0f09fdbc-sdk-0cbca40fad9a`;
  runtime source `335a0f09fdbc98eb7a1056c0f6b84cd4f39d889e`.
- Runtime archive SHA-256: `2171ee865bc9fc2e1a1274dfdd0f4330dc362b1f49d38c0a35c4de41aa61d6f3`.
- Binaryen 133 archive SHA-256: `2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489`.

## Integration checks

The SDK source and canonical I/O tests match the original vendored source
byte-for-byte. All imported Rust matches the original application after applying
SDK formatting conventions; metadata, oracle fixture, specification, application
licence and application lockfile are unchanged. No contract logic or regression
case was changed. SDK dependencies point to the official in-tree crate, including
native tests, with no vendored SDK or external Rust registry dependency.

The root workspace/lockfile, SDK, profiles, packaging/smoke scripts, licence and
four existing fixtures are unchanged. Git trees for the two other application
workspaces and their dedicated workflows are unchanged in the combined checkout.
The three packaged gates each occur once before release publication. The vesting
gate occupies a distinct position after workflow artifact upload, avoiding the
insertion positions in #9/#10; a failed gate prevents GitHub release publication.
The shared nested-workspace `cargo fmt --all` change reconciles automatically.

All six Git merge permutations of #9, #10 and this independent contribution passed
without conflicts and produced an identical combined tree. This PR itself contains
neither of the other two application sources and is based directly on upstream main.

## Independent source and package checks

Passed SDK formatting, all-target Clippy under `-D warnings`, all 12 SDK tests
(8 unit + 4 canonical I/O), and formatting/WASM Clippy for the four existing
fixtures plus the new workspace. The complete vesting gate passed:

- 60 application regressions: **12 + 60 = 72 distinct native tests**.
- 1,200 seeded Python bigint oracle cases; 14,400 generated state transitions;
  sixteen native pending lanes with reverse settlement and failed-payment retry.
- Native all-target Clippy and both individual WASM guests under `-D warnings`.
- Two clean official builds per guest, canonical method/event IDs, pinned Binaryen
  optimization performed twice with matching output, required WASM exports and
  zero host imports. Original compiler WASM is retained beside optimized WASM.
- Actual three-grant execution: failed beneficiary delivery and exact principal
  return/retry, failed owner refund and retry, cancellation preserving earned
  principal, negative outcomes decoded from ACMO status, and irrevocable completion.
- Final principal: **1,900 = 1,100 paid + 800 refunded**, zero held/pending,
  10 attempts, empty queue; receiver totals `[[400,600],[400,200],[300,0]]`.
- Journal replay matches account/registry roots, accepted-input count and fees.
- Existing smoke and the unchanged official packaging script, archive/file checksums,
  and the full gate from extracted source using verified public reference binaries.
  Vesting WASM/ACAB/descriptor hashes match between source and extracted locations.

## Combined checkout

All three independent full gates passed together with SDK/example conventions
and smoke. Distinct native cases: **12 SDK + 35 escrow + 74 revenue + 60 vesting =
181 tests**, with no failures. All seven application guests passed their complete
build/Clippy/export gates; revenue and vesting passed pinned deterministic
optimization. All three actual AVM journeys and journal replays passed.

To reproduce against the exact compatibility heads in a disposable checkout:

```bash
git fetch https://github.com/h2so4nackl-code/avm-sdk.git \
  examples/bounty-escrow-regressions examples/revenue-hub-regressions
git fetch https://github.com/h2so4nackl-code/avm-sdk.git examples/vesting-streams-regressions
vesting_head="$(git rev-parse FETCH_HEAD)"
git switch --detach 72ee42dbf923f081bf29b4a1a1b6ea012b6af54f
git merge --no-edit --no-ff 8998a508056807185841956f7584d0daea827f22
git merge --no-edit --no-ff fec8b211305e08b10106bcde69371d5285917822
git merge --no-edit --no-ff "$vesting_head"
# Set runtime and optimizer to the checksum-verified public tool directories.
bash examples/bounty-escrow/scripts/check.sh "$runtime"
bash examples/revenue-hub/scripts/check.sh "$runtime" "$optimizer"
bash examples/vesting-streams/scripts/check.sh "$runtime" "$optimizer"
```

Repeat the three merge operations in all six permutations and compare
`git rev-parse HEAD^{tree}`. New heads or a changed base require revalidation.

## Artifact hashes

| Guest | Optimized WASM bytes | SHA-256 |
| --- | ---: | --- |
| vesting | 51,087 | `1e21fa6376fed262d425c7a6d3e3d3fc8ca9f057a2e6f368475c23ea54876fe3` |
| receiver | 34,523 | `4b9a3699ce559297d8e9b32f5581d7f28b5291e6a805f83bbf3c4ca7a5ffa07a` |

`check.sh` regenerates `evidence/native-tests.txt`, native/guest Clippy logs, build
logs, `optimization.json`, `artifacts.json`, and the full `avm-e2e.json` journal.
The dedicated workflow uploads source and extracted-package evidence for its exact
commit; those hosted results must be checked separately from these local runs.

## Runtime and application limits

The public runtime embeds an older SDK revision (`0cbca40fad9a8f11863197cbcbaf6208651752b9`).
This checks the current contract source against its canonical ABI; it does not
replace the runtime's embedded SDK. The private runtime source build/tests and
source-equality release gate were not run locally and remain required upstream.
Package rehearsals reuse public binaries and are not private runtime rebuilds.

This is a synthetic-value, same-DApp reference fixture with eight streams and bound
receivers. Receivers retain payments and are not wallets. Missing authentic native
callbacks can leave a lane pending. Callback checks rely on native VM provenance
and replay protection. SHELL operating costs are separate from principal.
