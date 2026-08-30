# Acki Nacki AVM SDK

Private developer distribution for building Rust AVM contracts and running
them in the local AVM stand.

The repository contains:

- the source of `avm-contract-sdk`;
- production-shaped example contracts;
- installation and smoke-test scripts;
- developer documentation;
- CI that builds and tests `avm-local-stand` and `avm-contract-build` from an
  exact `gosh-sh/acki-nacki` `avm-dev` commit and publishes them as a release.

Production AVM runtime source remains in the private Acki Nacki repository.
Every release records the exact source commit and SHA-256 hashes of all shipped
files.

## Quick Start

Requirements: Linux x86-64, `gh`, `jq`, and Rustup. Authenticate `gh` with an
account that can read this private repository.

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
- [Releases And Provenance](docs/releases.md)
- [MIT License](LICENSE.md)

## Release Process

`avm-dev-<source-commit>-sdk-<sdk-commit>` releases are immutable snapshots of
Acki Nacki `avm-dev` and this distribution repository.
The release archive contains the two binaries, the Wasmtime profile, SDK
source, examples, documentation, checksums, and provenance metadata.
