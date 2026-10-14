# SDK tooling and event-read preview

This implementation branch is not a binary release. The published
`avm-dev-06e96ef71006-sdk-0748f4f0767a` commands do not include these additions.
Use a matching reviewed builder/runtime candidate for the commands below.
The [published release limitations](release-notes.md#compatibility) still apply.

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

## Experimental guest event reads

`MessageOutput.internal_event_read_intents` and the same field on `BatchOutput`
attach `InternalEventReadIntent { outbound_index, requests }` groups to produced
messages, following the callback-intent pattern. Groups must have strictly
increasing outbound indices and nonempty request lists. Keep the vector empty
for ordinary output. Requests use proposed `ACM2`/`ACB2` formats and require the
matching experimental runtime; published runtimes reject them. `OutboundIntent`
literals remain compatible. Explicit `MessageOutput`/`BatchOutput` literals need
`internal_event_read_intents: Vec::new()`; message constructors supply it.
Canonical ordinary output bytes remain unchanged when every request list is
empty. Lists must be strictly sorted by source DApp, source account, then numeric
event class, without duplicates.

The receiver obtains bounded canonical binary data through
`MessageInput.internal_event_results`. Reads use its block-start snapshot: a
record emitted during that block becomes visible in the next block. External
calls cannot inject requests or results. Protocol adoption and independent
runtime review remain prerequisites for release.

The [`contract-tools` example](../examples/contract-tools/src/lib.rs) demonstrates
constructor decoding, generated methods/errors/dispatch, a typed internal sum,
and guest-originated event emission/read requests. Its
[public-path gate](../scripts/tests/sdk_issue_public_paths.py) uses actual WASM
and CLI commands, with no fabricated state or injected event-read inputs.

## Deploy callbacks and remaining protocol questions

The runtime candidate implements a requested, empty-body success callback after
successful contract-initiated deploy. This differs from a callback request or
result body emitted by `init`, which remains forbidden. Internal deploy precheck
failures now produce status-63 callbacks or method-zero value refunds while
retaining the parent's forwarding transition. This includes already-active
destinations, malformed bodies and rejected deploy identity. The target account
or placeholder is unchanged. Failures after init starts remain unsupported;
their init-fee/refund contract needs reconciliation. The
[`deploy-callback-repro`](../examples/deploy-callback-repro/repro.sh) checks
success delivery and precheck failures separately, including replay. Track
[issue #13](https://github.com/gosh-sh/avm-sdk/issues/13).

There is no guest API proving an internal sender's selected code/hash, nor an
adopted non-withdrawable DApp balance or arbitrary third-party fee payer. These
remain [concrete research proposals](protocol-proposals.md); see [issue #8](https://github.com/gosh-sh/avm-sdk/issues/8)
and [the funding explanation](fuel-and-fees.md#funding-is-a-separate-concern).
