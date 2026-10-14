# Releases and provenance

Every release uses an immutable tag `avm-dev-<source12>-sdk-<sdk12>`.
BUILD-MANIFEST.json contains both complete 40-character commits. A previously
published tag or asset must never be overwritten.

Write the public release body and release notes using the
[contract-developer release-note recipe](../AGENTS.md#release-notes-for-contract-developers).

The public guest and examples come from the exact SDK commit. Host tools come
from the qualified node commit, its committed Cargo.lock and the separately
approved engine profile. Guest/profile parity is a mandatory gate. A local
research head is not evidence of adoption into avm-dev.

## Manual qualification

All Rust builds and WASM execution run on the qualification pod under the shared
work lock followed by the Rust build lock, with CPUs 0-1, jobs=2, test threads=1,
bounded memory/tasks and isolated task-owned scratch. Storage's admitted work
has priority. Export TMPDIR, TMP and TEMP to /media/futurizt/AVM/tmp in every
launcher. Set CARGO_TARGET_DIR outside both clean source trees and use the
current node scripts/avm-env.sh wrapper.

1. Read the preceding CI failure and verify live source/SDK heads, profiles,
   releases and open release work. Inspect workflow triggers before any push.
2. Commit the complete changes. Run scripts/manual_gates.py with --source,
   --sdk and a new --evidence directory. It preserves the original release
   checks and adds the audit's independent owner assertions. Missing/zero
   matches, stale logs and dirty inputs fail. No Actions dispatch is involved.
3. Run scripts/build-release.py build with the same source/SDK, --gates pointing
   to the passing gates.json and a new --output directory. It captures Cargo's
   executable records and hashes the actual binaries. It records compiler,
   native linker, flags, resolved features, ELF dependencies and license texts.
4. Run scripts/package-release.sh with source, SDK, verified build directory and
   a new output directory. It verifies the exact receipt and creates a sorted,
   normalized archive and its SHA-256 file. Repackage into another directory and
   require byte identity. Independently verify the manifest, complete inner
   inventory, outer checksum and actual source/build receipt.
5. Run scripts/tests/packaged_lifecycle.py on an isolated writable verified
   bundle with --bundle and --evidence. It requires Python blake3 1.0.11 as an
   independent test dependency. Supply --wrong-rustc and --incompatible-state for
   the candidate. Run the historical bundle separately with its own profile/state;
   reject historical populated state under the candidate. Do not count empty
   smoke, unrun CPU matrices or source-only E2E as packaged qualification.
   If application examples are present, also run
   `bash "$bundle/scripts/check-application-examples.sh" "$bundle" "$optimizer"`
   on that same isolated writable verified candidate; `$optimizer` is a verified
   Binaryen 133 `wasm-opt` when required by an example. Retain its complete logs
   and generated evidence with the candidate's exact source/build receipts.
   This runs every included application's native suite, all guest builds,
   artifact checks and actual stand journeys/replay. Failure blocks handoff.
   The source native suites and all guest lint checks also run in
   `manual_gates.py`; dedicated fork workflows use pinned public binaries and
   cannot substitute for private runtime-source or candidate-package qualification.
6. Hand exact heads, logs/hashes, findings ledger and adoption prerequisites to
   the coordinator for independent review. A review handoff is not publication.
   Resolve findings, rerun affected exact-input gates, and ensure the selected
   profile and node implementation are adopted before claiming compatibility.
7. After reviewed green final heads, push the explicitly requested PR using a
   skip marker where needed to prevent Actions. Respect branch rules. Publish
   through the manual GitHub release path, with release notes and qualification
   results, only after the permitted merge. Verify downloaded public assets
   afresh and test the verified installer in an isolated home/bin directory.

The SDK-specific Actions publisher has been retired in favor of these manual
gates. No node/storage Woodpecker pipeline is changed. The scripts never push,
merge, tag, publish or automatically upgrade a user's contracts.

## Package contents and trust

The v2 package contains host commands, guest source and examples, profile,
documentation, MIT guest license, exact node owner license, dependency notice
texts, BUILD-MANIFEST.json, provenance/build.json, provenance/runtime.Cargo.lock,
Cargo artifact evidence and SHA256SUMS. See release-notes.md for license scopes,
platform requirements and compatibility/reset guidance.

The installer checks both downloaded checksum files and all identity bindings
before extraction/activation, rejecting unsafe tar members. The verifier is
from the trusted checkout or previously verified installation, never unverified
downloaded code. SHA-256 does not add an independent signature to the GitHub/HTTPS
trust boundary. Old v1 packages are not silently accepted by the v2 installer.

Retain exact-head gate logs, failed-attempt evidence and build/package receipts
outside Git until coordinator acceptance and public-download verification.
Remove obsolete task-owned duplicates after the rollback/retention requirement
is cleared. Source-owned tests and release notes remain in Git.
