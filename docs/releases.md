# Releases And Provenance

Release tags use
`avm-dev-<12-character-source-commit>-sdk-<12-character-sdk-commit>`. A release
archive is accepted only after both exact commits pass:

- focused formatting, check, and clippy gates;
- SDK and contract-builder tests;
- the release deploy/upgrade lifecycle E2E;
- the release standard-contract stand E2E;
- a black-box CLI smoke test against the packaged files.

Each archive contains `BUILD-MANIFEST.json` with the full Acki Nacki source
commit, SDK repository commit, Rust toolchain, target triple, and Wasmtime
profile hash. `SHA256SUMS` covers the distributable files.

Release binaries are built from private Acki Nacki sources. The repository's
SDK source, examples, scripts, and documentation can be reviewed independently;
the binaries are verified through their manifest and hashes.
