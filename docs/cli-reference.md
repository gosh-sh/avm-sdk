# CLI Reference

`avm-local-stand` emits JSON on stdout and diagnostics on stderr.
`avm-contract-build build` emits exactly three text lines: artifact name and BLAKE3
digest. The ACAB digest uses the AVM_ABI domain; WASM and descriptor use raw
BLAKE3. `--version` reports the builder version. Compiler/lock provenance is
written separately to `target/avm-build-provenance.txt`.

## avm-contract-build

```text
avm-contract-build build --manifest-path <Cargo.toml>
```

The contract directory must contain `avm-contract.toml` and its Cargo workspace
must have a reviewed `Cargo.lock`. Rust/Cargo 1.93.0 is enforced. Successful output is
written to `target/avm` below that directory.

## avm-local-stand

Every invocation requires:

```text
avm-local-stand <command> --state <path> --wasmtime-profile <path> [options]
```

Commands and command-specific options:

- `init`: `--dapp-id`.
- `fund-user`: `--address`, `--shells`.
- `deploy`: either `--wasm` + `--acab` + `--descriptor`, or
  `--package-id` + `--selected-code-id`; plus `--salt-hex`, `--init-json`,
  optional `--owner-id`, and the timestamp/fee options.
- `call`: `--address`, `--method`, `--args-json`, `--identity`,
  `--extra-currencies`, and the timestamp/fee options.
- `publish-version`: `--wasm`, `--acab`, `--descriptor`, optional
  `--package-id`, `--salt-hex`, `--init-json`, optional `--owner-id`, and the
  timestamp/fee options.
- `set-default`: optional `--selected-code-id`.
- `deprecate`: optional `--selected-code-id`, optional `--deprecated`.
- `upgrade`: `--address`, `--selected-code-id`,
  `--migration-payload-hex`, `--identity`, and the timestamp/fee options.
- `delete-account`: optional `--address`.
- `compact-library`: optional `--package-id`.
- `inspect`, `events`, `replay`: no command-specific options.

Timestamp/fee options are `--block-time`, `--created-at`, `--expire-at`, and
`--fuel-fee-shells`. Identities are `developer`, `package-owner`, or `user`.
Addresses are 64 lowercase hex characters for the DApp id, `:`, then 64
lowercase hex characters for the account id.

Unknown commands, aliases, duplicate options, incomplete artifact triples, and
options not owned by the selected command are rejected.

## Source candidate additions

The unreleased source candidate adds the following tooling; these options are
not available in SDK 0.2.0 binaries:

- `avm-contract-build bindings --manifest-path Cargo.toml [--dispatch]` prints
  generated Rust method IDs, errors and optional typed dispatch.
- `avm-contract-build abi --acab contract.acab [--json]` prints canonical ABI
  metadata. Add `--method name --body-file body.bin` to decode a method body.
- `avm-contract-build build --source-root <directory>` declares shared source
  inputs above the inferred common workspace root for reproducible builds.
- `avm-local-stand deploy --quote-address` returns the address derived from the
  same deploy arguments without submitting. Fund it, then repeat without the flag.
- Artifact-based `deploy` without `--package-id` creates an independent package,
  even when the stand already contains packages. An explicit `--package-id`
  selects publication of a version in that existing package.
- `avm-local-stand inspect --account <address>` returns raw and ABI-decoded state
  without executing a call. Plain `inspect` also reports `local_config`.

See the [tooling guide](tooling-preview.md) and
[runtime limits](runtime-limits.md) for behavior and development-profile values.
