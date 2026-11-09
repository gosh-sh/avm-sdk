# SDK 0.4.0 revalidation — 2026-11-06

Executed on Linux x86_64 with Rust/Cargo 1.99.1 and the checksum-verified
`avm-dev-d103b6ccc759-sdk-21364083b24a` bundle. The bundle builder and its matching
Core-WASM V2 profile were used together on fresh local stand state.
Source parent: `016689696c6053dba9f0c59e75539c0c51b4a122`; this revision adds documentation,
evidence and (for Vault AMM/Treasury) example-local toolchain/config pins only.
Contract source and SDK crate bytes were not changed. The SDK crate is identical
between the earlier 0.3.0 release and 0.4.0 base.

## Executed checks

- `sh scripts/check.sh`: PASS; identifier self-tests, generated ABI consistency,
  formatting, native and wasm32 Clippy (`-D warnings`), **58 native tests**.
- Both `avm-contract-build build --manifest-path contracts/<contract>/Cargo.toml`:
  PASS, including the builder's repeat-build reproducibility checks.
- Decode both rebuilt ACABs using `avm-contract-build abi --acab <file> --json`:
  **27 method/event IDs** matched the generated Rust constants.
- `python3 scripts/e2e.py --sdk /root/.local/share/avm-sdk/current --output <scratch>`:
  PASS; **32 call probes**, each within 1 SHELL reserve. Replay matched
  account/registry roots, cumulative fees and accepted-input count.
- The compiler emits its upstream `wide-arithmetic` unstable-feature warning;
  it did not fail Clippy or the builder. Full command output and SHA-256 artifact
  hashes are in `evidence/sdk-0.4.0-*.log` and `sdk-0.4.0-validation.json`.

## Rebuilt deploy size

55,195 WASM + 2,103 ACAB + 66 descriptor
+ 64 init + 289 conservative envelope = **57,717 B**, leaving
**7,819 B** below 65,536 B. Init size is the tested configuration;
changing configuration/toolchain/source requires remeasurement.
The runtime guide documents 288 B; with that accounting this total is one byte
smaller. The wire-envelope difference was not independently measured.

## Scope

These are executed local results for fresh deployments, not upstream CI,
maintainer approval or production qualification. Existing native-only edge cases
were not added to the stand journey; see README for their exact scope.
No heap peak, exact per-call FUEL, funded-account upgrade/migration or Mainnet
qualification is claimed. No real funds or network transactions were used.
Earlier README/validation figures are historical and belong to their stated
compiler/SDK revision; use this report for the current 0.4.0 rebuild.
