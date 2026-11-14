# Contract Examples

Each contract keeps `avm-memory.json` with the peak measured by its own tests.
Deployment builds use that page-rounded peak; `build.max_memory_bytes` provides
an explicit override. See [contract memory](../docs/contract-memory.md) to
repeat measurements, check warnings and choose enough capacity.

These contracts are deterministic fixtures for learning the SDK and exercising
the local AVM stand. They use real SDK exports, canonical ABI metadata, state
updates, messages, events, and upgrade intents. They are deliberately small and
are not audited application contracts.

For setup and deployment commands, start with [Getting Started](../docs/getting-started.md).
The available stand commands are listed in the
[Command-Line Reference](../docs/cli-reference.md). Return to the
[SDK overview](../README.md) for installation and release information.

## Example Set

### Application examples for SDK 0.3.0

- [bounty-escrow](bounty-escrow/README.md): milestone funding, asynchronous
  payout/refund accounting and failure/replay regressions.
- [revenue-hub](revenue-hub/README.md): governed payouts, a receipt audit ledger and compact
  state experiments with native/WASM vectors and replay.
- [vesting-streams](vesting-streams/README.md): grant accrual, cancellation and
  callback accounting checked against an independent arithmetic oracle.
- [wallet](wallet/README.md): an owner wallet that keeps its owner key in its
  data and turns one signed external call into one internal message with value
- [fungible-token](fungible-token/README.md): a fixed-supply internal token ledger
  with bounded holder slots, transfer errors and event/replay checks.
- [multisig-treasury](multisig-treasury/README.md): an m-of-n treasury with a
  timelock, per-currency spending policies, a freeze switch and acknowledged
  payouts, checked by behaviour, integration and randomized invariant tests and
  by a journey on `avm-local-stand`.
- [factory-child-provenance](factory-child-provenance/README.md): derived-address
  checks and their limits. A derived address does not authenticate current code.

These are developer examples, not audited financial applications. They use
explicit application assets and require matching tools/profile; an application
asset such as currency ID 0 is separate from fee-paying SHELL at currency ID 2.

### `deploy-callback-repro` (unreleased)

An executable parent/child regression for ordinary callbacks, successful
internal-deploy callbacks and pre-init rejection notifications. It checks exact
callback counts, transferred-SHELL refunds, unchanged receiver effects and
replay using matching tools. See its [instructions](deploy-callback-repro/README.md).

### `contract-a-v1`

A message-isolated sender and the first version in the upgrade pair. It:

- initializes a fixed 72-byte state;
- accepts an external forwarding call;
- sends main value and one extra currency in an internal message;
- requests and validates a callback, then records the callback result;
- exposes a version probe;
- authorizes a package-owner upgrade and applies a deterministic state migration.

Use it with `contract-b` to exercise outbound delivery and callbacks, or with
`contract-a-v2` to exercise publication, selection, and account upgrade.
Before building for the public stand, set `AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY` to
the 64 lowercase hex digits at `.identities.package_owner` in its `init` output.
This configures upgrade authorization at compile time; it takes only a public
key. With the variable absent, the fixed source-test owner remains in use and
will not match a new stand's randomly generated owner.

### `contract-a-v2`

The compatible second version of contract A. It keeps the same state schema and
ABI while changing the version marker. It also exercises contract-initiated
deployment by forwarding a canonical deploy body to an internal destination.
The fixture rejects another code upgrade after version 2.

Use the v1 and v2 pair when validating upgrade authorization, migrated state,
selection changes, and behavior before and after an upgrade.

### `contract-b`

A message-isolated receiver for internal traffic. It:

- validates the transferred main value, extra currency, body, and callback request;
- supports initialization from a contract-initiated deploy;
- emits canonical external and internal events;
- returns callback result data and can drain retained value;
- consumes an internal-event read result;
- supports same-DApp and cross-DApp stand journeys.

Its source contains disabled negative-path hooks selected only by the stand's
failure-flow build. Normal builds do not enable them. Use this contract with
contract A when testing routing, callbacks, refunds, events, rollback, and
cross-DApp behavior.

### `contract-batch-patch`

A message-isolated contract with a 65,540-byte state. Initialization uses
`FullReplace`; its message entry point accepts one canonical internal message
and returns a `DataPatchList` that changes only the eight-byte counter.

Use it to validate large retained state and narrow state patches without
replacing the complete state image.

AccountBatch execution is not supported by the node yet; a message to such
code ends with status 21.

Contract A v1 sends this message within its own DApp to message-isolated code,
so the delivery is local-free: the message carries no FUEL fee and contract A's
reserve is not charged; see [FUEL and SHELL](../docs/fuel-and-fees.md).

## Build An Example

Install the SDK commands first, then build any example from the repository or
a writable development copy of a release. Keep installed immutable release
directories unchanged: copy the examples together with `crates/avm-contract-sdk`
so their relative dependency paths still resolve before building:

```bash
export AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY="$(jq -er .identities.package_owner init.json)"
avm-contract-build build \
  --manifest-path examples/contract-a-v1/Cargo.toml
```

The command writes `contract.wasm`, `contract.acab`, and
`contract.descriptor` under that example's `target/avm` directory. It performs
two builds and rejects non-reproducible output.

For direct source checks, pass the arena capacity that `avm-contract-build`
uses, here the measured value from the example's `avm-memory.json`:

```bash
cargo +1.99.1 fmt --manifest-path examples/contract-a-v1/Cargo.toml -- --check
AVM_GUEST_ARENA_BYTES="$(jq -r .arena_bytes examples/contract-a-v1/avm-memory.json)" \
cargo +1.99.1 clippy \
  --manifest-path examples/contract-a-v1/Cargo.toml \
  --target wasm32-unknown-unknown -- -D warnings
```

## Recommended Use

Keep the checked-in examples unchanged when using them as release fixtures.
Copy the closest example to a new directory for application work, then change
the package name, state schema, ABI, error codes, authorization values, and
contract logic together. If the copy is moved outside this repository, update
the `avm-contract-sdk` path dependency to its installed or vendored location.

Use the examples as focused references:

- start with `contract-a-v1` for a message-isolated contract;
- add `contract-b` for internal messaging, callbacks, events, and deployment;
- use the v1/v2 pair for upgrades;
- use `contract-batch-patch` for large retained state and patch output.

The source-test owner, fixed values, salts, payloads, and failure hooks are test data.
Replace them before using copied code in an application.
