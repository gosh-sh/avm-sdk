# Acki Nacki AVM SDK

Public developer distribution for building Rust AVM contracts and running
them in the local AVM stand.

The repository contains:

- the source of `avm-contract-sdk`;
- deterministic example contracts;
- installation and smoke-test scripts;
- developer documentation;
- manual release gates that build and test `avm-local-stand` and
  `avm-contract-build` from exact source commits before reviewed publication.

Production AVM runtime source remains in the private Acki Nacki repository.
Every release records the exact source commit and SHA-256 hashes of all shipped
files.

## Quick Start

Requirements: Linux x86-64-v3, the binary's declared GNU libc version, Bash,
Python 3.11+, `gh`, `jq`, and Rust/Cargo 1.93.0 with the wasm32 target.
The repository and release downloads are public. See the release manifest for
measured ELF dependencies and [release notes](docs/release-notes.md) for limits.

```bash
./scripts/install.sh
export PATH="${HOME}/.local/bin:${PATH}"
./scripts/smoke.sh
```

## Documentation

- [Getting Started](docs/getting-started.md)
- [FUEL And SHELL](docs/fuel-and-fees.md)
- [Command-Line Reference](docs/cli-reference.md)
- [Contract Examples](examples/README.md)
- [Unreleased Tooling And Event-Read Preview](docs/tooling-preview.md)
- [Releases And Provenance](docs/releases.md)
- [Guest SDK MIT License](LICENSE.md)
- [Compatibility, Platform And License Scopes](docs/release-notes.md)

## Release Process

`avm-dev-<source-commit>-sdk-<sdk-commit>` releases are immutable snapshots of
Acki Nacki `avm-dev` and this distribution repository.
The release archive contains the two binaries, the Wasmtime profile, SDK
source, examples, documentation, checksums, and provenance metadata.
