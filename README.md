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

SDK 0.6.0 runs Core-WASM V2 contracts on Wasmtime 48.0.8 and builds them with
guest Rust/Cargo 1.99.1. Follow the documentation included with the exact
installed bundle. Native runtime tools have their separate Rust/Cargo 1.98.0
build identity in the profile and provenance. Earlier releases retain their own
profiles and compilers.

## Use a matching bundle

Requirements: Linux x86-64-v3, the binary's declared GNU libc version, Bash,
Python 3.11+, `gh`, `jq`, and Rust/Cargo 1.99.1 with the wasm32 target.
Use the tools, profile and documentation from the same bundle. These
instructions require the matching SDK 0.6.0 Core-WASM V2 bundle; keep older
release tools and retained state together.

Set `RELEASE_TAG` to the matching immutable published tag before downloading:

```bash
./scripts/install.sh "${RELEASE_TAG:?set the matching immutable release tag}"
export PATH="${HOME}/.local/bin:${PATH}"
./scripts/smoke.sh
```

A prepared local bundle can be verified and installed with
[`release.py install`](docs/releases.md#install-a-prepared-local-bundle).
See its manifest for measured ELF dependencies and the
[release notes](docs/release-notes.md) for limits.

## Documentation

- [Getting Started](docs/getting-started.md)
- [Owner Key And External Calls](docs/owner-key-and-external-calls.md)
- [Contract Deploy](docs/contract-deploy.md)
- [FUEL And SHELL](docs/fuel-and-fees.md)
- [Runtime Limits, Rent And Addresses](docs/runtime-limits.md)
- [Contract Memory Maximum And Local Checks](docs/contract-memory.md)
- [SDK Tooling](docs/tooling-preview.md)
- [Command-Line Reference](docs/cli-reference.md)
- [Contract Examples](examples/README.md)
- [Releases And Provenance](docs/releases.md)
- [Guest SDK MIT License](LICENSE.md)
- [Compatibility, Platform And License Scopes](docs/release-notes.md)

## Release Process

`avm-dev-<source-commit>-sdk-<sdk-commit>` releases are immutable snapshots of
Acki Nacki `avm-dev` and this distribution repository.
The release archive contains the two binaries, the Wasmtime profile, SDK
source, examples, documentation, checksums, and provenance metadata.
