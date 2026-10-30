# SDK tooling preview

These commands describe the unreleased source candidate. SDK 0.2.0 does not
include these additions. Use a matching builder and local-stand binary for the
commands below; check the source identities recorded in the distribution.

## Constructor data

`MessageInput.message.body()` continues to contain the complete deploy envelope.
In `init`, `input.deploy()?` returns a borrowed view with `init_data()`, `salt()`,
`ui_name()`, `owner_id()` and `package_id()`. An empty name is possible. These
accessors check envelope structure; host admission still validates signatures,
hashes and configured limits. Decode `init_data()` using the ABI init type.

## Generated IDs and dispatch

Add `[bindings] enabled = true` to `avm-contract.toml`. The builder regenerates
`target/avm/bindings.rs` before its two clean compilations and supplies its path
through `AVM_CONTRACT_BINDINGS`. Include it inside a module:

```rust
pub mod generated { include!(env!("AVM_CONTRACT_BINDINGS")); }
```

`generated::methods` contains method-name-derived `u64` constants and
`generated::errors` contains manifest error codes. Preview the source without
compilation with `avm-contract-build bindings --manifest-path Cargo.toml`.
Identifier collisions and reserved contract error codes are rejected.

For typed dispatch, also set `[bindings] dispatch = true` and put an explicit
`caller = "internal"`, `"external"` or `"either"` on every ABI method. Implement
`generated::Handlers` and export `generated::Contract<YourHandlers>` through the
normal SDK macro. Internal/external handler contexts expose the sender address
or external public key. The dispatcher checks caller kind before decoding the
method body and routes failures through your `on_dispatch_error` hook. It does
not invent application status codes. Internal sender authorization remains your
contract's responsibility; an address is not proof of its current code version.

Types use stable `TypeN` names matching ABI type IDs. Struct fields follow field
ID order; enum variants use `VN` names matching tags. Decoders reject malformed
lengths, booleans, optional/enum tags, UTF-8 and trailing bytes. Opaque type ID 0
remains a borrowed byte slice. Recursive layouts, depth over 64 and arrays of
zero-width elements are currently rejected by generation. ACAB and VM admission
still do not validate method body types; this is generated contract code.

## Inspect ABI and account state

```bash
avm-contract-build abi --acab target/avm/contract.acab
avm-contract-build abi --acab target/avm/contract.acab --json
avm-contract-build abi --acab target/avm/contract.acab \
  --method tools.add --body-file call-body.bin --json
avm-local-stand inspect --state demo.state --wasmtime-profile "$PROFILE" \
  --account "$ADDRESS"
```

ABI inspection reports types, method IDs, fixed input sizes where available,
init/state roots, events and errors. `--body-file` is raw canonical body bytes.
Account inspection returns raw data and its ABI projection without executing a
message. Opaque state has a null projection; unavailable/invalid metadata or
invalid typed data is reported in `state_decode_error` alongside raw data.
An absent account is an error.

Inspection also exposes the selected `local_config`. Attempted WASM failures
name the VM status and report used/budgeted FUEL. See [runtime limits, rent and
addresses](runtime-limits.md) for the local values and their scope.

## Reproducible builds and reinstall checks

The pinned builder gives external path dependencies stable Cargo
identities and remaps source paths, including SDK panic locations. Identical
sources and build inputs in different absolute directories produce identical
WASM, code hashes, ACAB and descriptors. The build receipt records this policy
and actual remappings. A generated tree preserves the directory layout from the
common ancestor of the contract workspace and its path-dependency workspaces.
It links original files and projects Cargo workspace ownership into private
manifests. Root packages, lexical parent traversal and helper processes retain
that layout; the resolved locked package/feature graph must remain identical.
Original manifests and locks are not edited. Builds sharing the target namespace
serialize through a file lock; conflicting generated entries fail with repair
instructions. Compiler, Cargo, lockfile and wrapper validation and
the two clean builds remain mandatory. Path normalization does not make
different sources, build-script inputs or compiler profiles identical.
Earlier builders can produce different code/package hashes from these same
sources; re-quote deployment addresses after rebuilding with the candidate.

If a build script reads inputs above that common ancestor, include them explicitly:
`avm-contract-build build --manifest-path path/to/Cargo.toml --source-root path/to/source-tree`.
The root must contain the contract and every path-dependency workspace. Keep the
same relative source-tree layout across builds. Paths outside that tree and
application-generated strings containing absolute paths are not normalized.

Reinstall checks executable permissions as well as every installed file's
contents. A nonexecutable command or bundled shell script causes an actionable
failure before activation; apply the reported `chmod 755` repair and retry.
Successful installation no longer silently accepts missing executable bits.

## Quote, fund and deploy

Add `--quote-address` to the same `deploy` arguments you intend to submit. It
returns the deterministic address without funding, execution or state changes.
Fund that exact address using `fund-user`, then repeat `deploy` without the flag.
The existing `funding_required` response remains supported. Prefunding an absent
address creates a funding placeholder, not an active contract. An already active
address cannot be deployed again.

Constructor failures now report the returned status; contract success requires
0. ABI errors must use codes >=1000. A block time earlier than an account's last
storage-payment time is rejected with both times in the diagnostic.

## Runtime boundaries

The [deploy callback example](../examples/deploy-callback-repro/README.md)
exercises ordinary callbacks, requested successful-deploy callbacks and pre-init
rejection notifications with matching candidate tools. It accepts explicit tool,
profile and output paths and leaves the source checkout unchanged.

These helpers use the existing canonical ABI. They do not add WASM imports,
dynamic libraries, network access, clocks or randomness to contracts. The
runtime continues to reject imported host functions, including BN254 imports.
Contract code executes within the fixed deterministic core-WASM profile.

An internal sender address does not prove the sender's current code or package.
The SDK does not expose an authenticated sender-code lookup. Contract authorization
must follow the application's explicit trust model.
