# In-tree SDK integration

Imported from `h2so4nackl-code/avm-vesting-streams` at
`e258b0e516519202bdd937445e38efddce8c4741` into `examples/vesting-streams`.

The nested workspace contains only core, vesting, receiver, and application tests.
Core and tests use `../../../crates/avm-contract-sdk`; both contracts use
`../../../../crates/avm-contract-sdk`. The upstream SDK, root workspace/lockfile,
and four existing fixtures are unchanged. The MIT application licence is retained.
The single-guest WASM lint loop defaults to vesting; the full gate checks both guests.
`cargo fmt --all` covers nested modules and tests with SDK formatting conventions.
The SDK 0.2.0 builder pins the compiler and rejects compiler wrappers; the old
remapping wrapper is removed. No contract behavior or regression case was changed.

The dedicated read-only workflow runs native/guest checks, immutable public tooling,
actual AVM execution and relocated-source checks with verified public 0.2.0 tools.
Packaging still includes `examples/`, but its v2 interface requires qualified
private inputs and real build receipts. The retired publisher is not restored.
`manual_gates.py` runs native tests and both guests; candidate-package execution
runs through `scripts/check-application-examples.sh` after the standard packaged
lifecycle. Public tools do not replace private-source or candidate qualification.

Canonical IDs come from official ACAB output. `scripts/sync_ids.py --check` verifies
them. After intentional metadata changes, rebuild metadata, regenerate IDs, then
rebuild and run the full gate. The actual journey reads receiver owner state;
publisher and account-owner identities must not be assumed interchangeable.

See [VALIDATION.md](../VALIDATION.md) for exact revisions, merge compatibility,
checks, accounting, reproducibility, and limitations. This is a reference fixture,
not an independent security audit, mainnet deployment, or upstream acceptance.
