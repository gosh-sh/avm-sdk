# Public SDK audit closure ledger

The release resolves the completed SDK-001 through SDK-010 audit as one package.
This ledger names acceptance assertions; execution results belong to the exact
release's qualification receipt. A listed assertion is not a claim that it ran.

- **SDK-001 — installer integrity.** `scripts/release.py`, `scripts/install.sh`;
  `scripts/tests/test_release_integrity.py` independently constructs valid and
  hostile tar files. Outer/inner hashes, full-commit/tag binding, receipt/binary
  identity, traversal, links, special files, duplicate and unlisted members,
  missing checksums, immutable existing releases and existing user commands
  must all be checked before activation. Failed validation preserves `current`.
  SDK-R167-002 adds strict complete-profile parsing and a fully rehashed
  profile/lock contradiction negative, preserving old releases and commands.
- **SDK-002 — truthful engine profile.** The node profile revision records
  pooling, its resource formulas and explicit reservation/guard/COW settings.
  `release_profile_attests_pooling_and_explicit_memory_settings` supplements the
  fixed identity, mismatch, CPU, fresh-store, allocator, rejection and FUEL
  assertions. Adoption requires separate owner approval and independent review.
  SDK-R167-001 explicitly disables threads/components in the shared constructor;
  actual SDK/node build feature attestations are separate from its effective
  execution settings. Both graphs execute disabled-proposal, exact FUEL/output,
  isolation and cancellation assertions; an actual node build binds its graph.
- **SDK-003 — current distribution and compatibility.** Source/SDK parity,
  compact-map/account vectors, EventLog canonical-data and malformed-input
  cases, isolated-FUEL admission, and the packaged lifecycle bind the selected
  accepted source. `release-notes.md` describes the incompatible state/profile
  boundary separately from guest API compatibility.
- **SDK-004 — stand correctness.** Existing exact block-start event-read,
  failed-batch burn and exhausted-pool fallback assertions plus the complete
  standard-contract E2E are mandatory. The release uses the selected node tree;
  it does not reproduce these owners inside the SDK.
  SDK-R167-004 confirms public internal-event reads are
  `UNSUPPORTED_IN_REVIEWED_ABI` and `NOT_EXECUTED_THROUGH_PUBLIC_PACKAGE`.
  Their packaged coverage is `passed: null`; source visibility and public
  injection-rejection assertions remain required. No guest ABI expansion occurs.
- **SDK-005 — provenance.** Pinned runtime lock, clean exact source exports,
  compiler/linker identities, resolved dependency/features, actual binary hashes
  and deterministic archive construction are recorded by the manual release
  tooling. Package verification independently rehashes its complete contents.
  Native link arguments force the pinned rust-lld through its recorded driver
  shim; independent regressions reject a different or missing linker.
  SDK-R167-007 distinguishes the actual Wasmtime library from its custom-build
  target. Independent mixed-target, missing/ambiguous-library and rehashed
  nonlibrary-package regressions preserve strict engine feature attestation.
- **SDK-006 — packaged behavior.** `scripts/tests/packaged_lifecycle.py` builds
  all four public examples using the shipped builder, verifies emitted hashes,
  deploys and calls real code, exercises events, batch patches, contract errors,
  FUEL exhaustion, authorized/unauthorized upgrade and populated-state replay.
  Contract A binds the public CLI's generated package-owner key at compile time;
  malformed key configuration rejects before an executable artifact is admitted.
  SDK-R167-003 adds the bounded `fixture_oracle.py`: independently specified
  complete account bodies/balances/index metadata, Library records, event and
  ring bytes, and all three roots for current/historical profiles. Frozen raw
  vectors and content/root mutations check that oracle. Both execution and a
  separate public-command replay must match it. Internal-event read requests
  remain the unsupported capability described under SDK-004; packaged events
  cover emission/listing. Invalid profiles must reject before committing real execution. Run the
  historical bundle separately; reject its populated state under the candidate.
  The historical fixed-owner example's upgrade rejection is an explicit limitation,
  while the candidate must pass the authorized upgrade.
- **SDK-007 — guest parity.** Distributed guest files match the selected node
  source. Canonical I/O and the seven audited ownership/copy/output assertions
  remain mandatory. All public examples are rebuilt; old contracts stay pinned.
- **SDK-008 — compiler enforcement.** Builder rejects mismatched rustc/Cargo,
  missing locks and unreviewed compiler wrappers before compilation. Existing
  reproducibility, export and heap assertions remain intact; regression tests
  explicitly pin their fixture dependencies before invoking the builder.
- **SDK-009 — platform and dependencies.** Binary ELF dependencies and required
  GLIBC versions are measured from the actual artifacts. Notes declare the
  Linux x86-64-v3 requirement, exact Rust toolchain, installation/build tools,
  actual CLI output formats and unrun production CPU/performance gates.
- **SDK-010 — license scope.** Guest SDK keeps MIT. Host binaries carry the
  exact node owner license with its original scope; resolved dependencies carry
  their own license expressions, source identities and notice texts. Packaging
  grants no additional rights and must fail if required notices are missing.
  `test_license_inputs.py` binds omitted notices to exact upstream commits and
  checks crate archive/source hashes independently against the runtime lock.
  It also requires unchanged Rust 1.93.0 standard-library/vendor and toolchain
  notices from the pinned sysroot; the installer binds their compiler identity.

Retain every failed gate with its cause. Zero matched tests, unavailable evidence,
changed inputs after a gate, and an empty-state smoke are not release passes.

SDK-R167-005 adds the six existing `avm-wasm-code` integration suites:
`validate_wasm_module`, `prepare_code`, `get_prepared`,
`invalid_forbidden_sections`, `invalid_mvp_function_body` and
`disabled_feature_encodings`. Each must execute a nonzero test count in the
accumulated campaign; source check/clippy is not an execution substitute.

Public [issue #13](https://github.com/gosh-sh/avm-sdk/issues/13), missing callbacks
on contract-initiated deploy, remains outside the original ten-finding closure.
It is documented in the release notes; ordinary-call callback tests do not
qualify deploy callbacks, and no fix or candidate reproduction is claimed.
