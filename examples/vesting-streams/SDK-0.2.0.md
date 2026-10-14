# SDK 0.2.0 adaptation — 2026-10-14

This branch merges official `main` at `36d1157f22504b39f77587f3ea325d36216f4ec3`,
including merged PR #14. It retains the deletion of
`.github/workflows/release-avm-dev.yml`; no retired publisher is recreated.
`examples/README.md` keeps the current owner-key/build documentation and adds only
this application's entry. The root SDK, profile, standard fixtures, root lockfile,
installer and v2 build/package implementation match that main commit.

## Changes

- Update only this workspace's locked guest dependency from 0.1.0 to 0.1.1.
- Pin public distribution 0.2.0 / guest 0.1.1; verify its independently observed
  archive digest with the trusted checkout's complete v2 verifier before extraction.
- Preserve the dedicated read-only regression workflow and every application case.
- Extend `manual_gates.py` generically for nested workspaces: format all members,
  native all-target Clippy and all tests, and WASM Clippy for every cdylib guest
  discovered from locked offline Cargo metadata. All original private gates remain.
- Add `scripts/check-application-examples.sh` for complete actual application
  gates from source or a writable verified candidate bundle. Guest/profile parity
  is checked before execution; instructions are in `docs/releases.md` step 5.

The old compiler-remapping wrapper is removed because the 0.2.0 builder rejects `RUSTC_WRAPPER`. No application Rust source or native test was changed.

The dedicated workflow replaces its old five-argument packaging rehearsal with relocated tracked-source execution. The v2 packager takes four arguments and requires actual qualified private inputs/build receipts; copying public binaries is not a package qualification.

## Independently rerun results

On Linux with Rust/Cargo 1.93.0 and the verified public 0.2.0 distribution:

- SDK: **13 tests (9 unit + 4 canonical I/O)**, formatting and all-target Clippy pass.
- Upstream Python installer/provenance assertions: **40 tests pass** with the
  independently pinned `blake3==1.0.11` test dependency.
- The actual changed manual example loop passed **27 source gates** on the
  combined checkout, retaining the four standalone fixtures and checking every
  native workspace and all seven application guests. This is the SDK/example
  portion only, not a run of the inaccessible private qualification gates.
- Application: **60 native tests pass**, all-target Clippy and formatting pass.
- All guests pass separate WASM Clippy under `-D warnings`, official two-clean-build
  reproducibility, canonical ID checking, required exports and zero host imports.
- Full actual stand journey and journal replay pass. Synthetic accounting:
  1,900 = 1,100 paid + 800 refunded; zero held/pending; ten attempts.
- Binaryen 133 deterministic optimization remains required for Revenue/Vesting;
  compiler output and optimized deployment output are both retained.

These are local application results, not newly executed upstream private gates or
hosted CI results. The generated logs/journals are ignored by Git and uploaded by
the dedicated workflows when those workflows execute.

| Guest | Deployment WASM bytes | Deployment WASM SHA-256 |
| --- | ---: | --- |
| vesting | 52095 | `acb8419d027b5256361f9834a79d03e2be1fd98dd1af2f3d548e40db7ac364fe` |
| receiver | 35519 | `3d95e21c409ecb0e332c4de48f9f9869cb798740063ada62891931df0d470d40` |

## Current verified public inputs

- Release: `avm-dev-06e96ef71006-sdk-0748f4f0767a`.
- Archive SHA-256: `8728497cf1e476d3d770a09b19929f7c196f2637654642727432b4c285c85143`.
- Runtime source: `06e96ef71006cca020575cbf35e6b82cc82a324e`.
- Packaged SDK: `0748f4f0767a26da8bc363bcb6b5585539d6970e`.
- Guest crate and engine profile are byte-identical between this checkout and
  the verified public distribution; later main commits concern documentation.
- Platform: Linux x86-64-v3, glibc >= 2.39, Python >= 3.11, Rust/Cargo 1.93.0.

## Integration and relocation limit

All six merge orders for the three adapted application trees passed without
conflicts and produced the same combined tree. The combined checkout passed
**13 SDK + 35 escrow + 74 revenue + 60 vesting = 182 distinct native tests**,
standard-example conventions, smoke and all three complete application journeys.
These local results do not imply upstream execution or merge approval.

The 0.2.0 builder's two clean builds pass for the same source location. However,
the public builder embeds the absolute in-tree SDK diagnostic path in WASM.
Independent and combined checkouts therefore produced different WASM hashes for
all seven guests; canonical ACAB/descriptor bytes remained identical, and each
location passed its own actual stand/replay gate. The removed compiler wrapper
cannot be used with the pinned builder, and its CLI offers no remapping option.
Cross-directory WASM identity is **not** claimed. Rebuilds require new code hashes
and per-location artifact evidence; do not reuse historical deployment selections.
Resolving this compiler-input path limitation would require a separately qualified
upstream builder change, not financial-contract edits or compiler-pin bypasses.

## Qualification boundary

| Check | Independent public/fork run | Manual upstream qualification |
| --- | --- | --- |
| Native SDK/application, formatting, all guests' lint | Executed locally; dedicated CI retains them | Runs in `manual_gates.py` |
| Complete reproducible guest builds, artifacts, stand/replay | Executed with verified public 0.2.0 tools | Must repeat on candidate's built/packaged tools |
| Private node/guest source parity, engine graph/tests, node build | Not executed here | Required on qualification pod |
| Receipt-bound v2 build/package and byte-identical repackaging | Not executed here | `build-release.py` / `package-release.sh` after passing exact-input gates |
| Packaged lifecycle and full included application suites | No candidate-package claim | Required from isolated writable verified candidate before handoff |

The private `gosh-sh/acki-nacki` qualification checkout, its dependency cache and
bounded pod are unavailable to this task. Their checks were not bypassed, simulated,
or counted as passing. No new runtime release, release dispatch, publication,
production-state migration, mainnet deployment or financial-logic change occurred.
Historical pre-0.2.0 evidence remains under `VALIDATION.md` with an explicit scope.
