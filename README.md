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

SDK 0.6.7 runs Core-WASM V2 contracts on Wasmtime 48.0.8 and builds them with
guest Rust/Cargo 1.99.1. Follow the documentation included with the exact
installed bundle. Native runtime tools have their separate Rust/Cargo 1.98.0
build identity in the profile and provenance. Earlier releases retain their own
profiles and compilers.

## Use a matching bundle

Requirements: Linux x86-64-v3, the binary's declared GNU libc version, Bash,
Python 3.11+, `gh`, `jq`, and Rust/Cargo 1.99.1 with the wasm32 target.
Use the tools, profile and documentation from the same bundle. These
instructions require the matching SDK 0.6.7 Core-WASM V2 bundle; keep older
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

Read the pages in this order when you start; later, use the table to find
the page that answers your question.

| Page | Answers |
|---|---|
| [Concepts](docs/concepts.md) | What are addresses, owner keys, external and internal messages, FUEL and SHELL, packages and code ids? |
| [Getting Started](docs/getting-started.md) | How do I install the SDK, build a contract, deploy and call it on the local stand? |
| [Contract Examples](examples/README.md) | What does a real contract look like? |
| [Owner Key And External Calls](docs/owner-key-and-external-calls.md) | Who can call my contract, and how do I set and rotate its key? |
| [Events](docs/events.md) | How do I emit an event and read events? |
| [FUEL And SHELL](docs/fuel-and-fees.md) | What does a call cost, which routes are free, how big a reserve do I need? |
| [Runtime Limits, Rent And Addresses](docs/runtime-limits.md) | Which size and FUEL limits apply, how does storage rent work, how is an address derived? |
| [Contract Memory Maximum And Local Checks](docs/contract-memory.md) | How do I size stack, arena and memory? |
| [SDK Tooling](docs/tooling-preview.md) | How do I generate bindings, inspect ABI and account state, build reproducibly? |
| [Contract Deploy](docs/contract-deploy.md) | How does a deploy work on a network, and what does it cost? |
| [From The Local Stand To A Network](docs/network.md) | How do I deploy and call on a network, and what can I read back from it? |
| [Message Statuses And Common Errors](docs/statuses-and-errors.md) | What does this status or error mean, and what do I do? |
| [Known Limitations](docs/known-limitations.md) | What does this release not do? |
| [Command-Line Reference](docs/cli-reference.md) | Every command and option |
| [Contract Crypto Helpers](docs/crypto.md) | How do I hash and verify signatures in a contract? |
| [Unsafe Code](docs/unsafe.md) | Where does the contract SDK use unsafe code, and why is it safe? |
| [Releases And Provenance](docs/releases.md) | How do I verify and install a release bundle? |
| [Compatibility, Platform And License Scopes](docs/release-notes.md) | What changed in each release? |
| [Guest SDK MIT License](LICENSE.md) | The license of the contract SDK |

## Release Process

`avm-dev-<source-commit>-sdk-<sdk-commit>` releases are immutable snapshots of
an Acki Nacki source commit and this distribution repository. The release
notes name the runtime source that the tools are built from. The source
commit in the tag can be a later commit that holds the same runtime source.
The release archive contains the two binaries, the Wasmtime profile, SDK
source, examples, documentation, checksums, and provenance metadata.

Contract cryptography is documented in [crypto helpers](docs/crypto.md), with
a [signature verifier example](examples/signature-verifier/README.md).
