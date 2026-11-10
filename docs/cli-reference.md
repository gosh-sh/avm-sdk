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
must have a reviewed `Cargo.lock`. Rust/Cargo 1.99.1 is enforced. Successful output is
written to `target/avm` below that directory. The SDK 0.6.0 builder enables
`+wide-arithmetic`; its receipt records the exact guest compiler, Cargo and
target flags. The native host compiler in the release profile is a separate
identity, Rust/Cargo 1.98.0.

`[build].stack_bytes`, `[build].arena_bytes` and `[build].max_memory_bytes`
override the corresponding tested resources in `avm-memory.json`. Deployment
builds require complete measured or explicit resources. Stack uses 16-byte
alignment, arena uses exact bytes, and maximum memory uses 64-KiB pages.
Use `build --for-testing` for marked measurement artifacts and
`record-memory --manifest-path Cargo.toml --report <JSONL>` to save test peaks.
Repeat `--code-hash <hash>` for other tested variants of that contract.
See [contract memory](contract-memory.md) for constraints and local checks.

`avm-local-stand check-contract-tests --wasm <module> --manifest-path <Cargo.toml>
--tests <JSONL> --wasmtime-profile <profile>` replays captured raw guest ABI
vectors for resource selection. It uses the normal admission and execution
paths, checks statuses and output bytes with the original FUEL budgets, and
reports initial and peak memory. This command does not maintain stand state;
rerun the original stateful tests on the selected deployment artifacts.

With `--memory-payment`, every JSONL case also needs a `memory_payment` object
with decimal-text `reserve_shells`, `route` (`paid` or `local-free`) and
`remainder` (`callback`, `burn` or `library`), and its `fuel_budget` must be
the FUEL budget of that reserve. The report then adds one receipt per case
under `memory_payment_preview`: initial and ending pages, `memory_page_price`,
memory fee units, compute FUEL, required and charged SHELL, the remainder and
the resulting status. Receipts change no state.

`avm-local-stand quote-memory-payment --wasm <module> --manifest-path
<Cargo.toml> --wasmtime-profile <profile> --entrypoint
<init|process_message|process_batch> --compute-fuel <FUEL> --pages-at-end
<pages> --reserve-shells <SHELL> --route <paid|local-free> --remainder
<callback|burn|library>` prints the same receipt for an estimated call
without executing it.

## avm-local-stand

Stateful stand commands require:

```text
avm-local-stand <command> --state <path> --wasmtime-profile <path> [options]
```

Commands and command-specific options:

- `init`: `--dapp-id`.
- `fund-user`: `--address`, `--shells`, optional `--extra-currencies`
  (`id:amount` entries separated by commas, in strictly ascending id order,
  without SHELL, currency 2, which `--shells` credits).
- `deploy`: either `--wasm` + `--acab` + `--descriptor`, or
  `--package-id` + `--selected-code-id`; plus `--salt-hex`, `--init-json`,
  optional `--owner-id` (64 hex digits, default the `user` identity's key),
  and the timestamp/fee options.
- `call`: `--address`, `--method`, `--args-json`, `--identity`,
  `--extra-currencies`, and the timestamp/fee options. The identity must hold
  the destination's [owner key](owner-key-and-external-calls.md), and
  `--extra-currencies` must be empty: an external call carries no value.
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

`--check-memory` additionally checks an executing command's synchronous guest
runs, reports peak linear-memory bytes and the maximum, warns at 80% of the declared
maximum and fails on rejected growth or incomplete capture.
See [contract memory](contract-memory.md) for the report's scope.

## Network client

Three `avm-local-stand` commands build, sign and send external messages to a
network node. They keep no local state and print one JSON object:

```text
avm-local-stand network-call --endpoint <url> [--token-file <path>]
  --wasmtime-profile <path> [--network-id <hex>] --key <path>
  --address <dapp_id:account_id> --acab <path> --method <name>
  --args-json <json> --fuel-fee-shells <n> [--nonce <n>]
  [--created-at <s>] [--expire-at <s>]
avm-local-stand network-deploy --endpoint <url> [--token-file <path>]
  --wasmtime-profile <path> [--network-id <hex>] --key <path>
  --dapp-id <hex> --wasm <path> --acab <path> --descriptor <path>
  (--package-owner-key <path> | --committed) --salt-hex <hex>
  --init-json <json> [--owner-id <hex>] --fuel-fee-shells <n>
  [--created-at <s>] [--expire-at <s>] [--quote-address]
avm-local-stand network-status --endpoint <url> [--token-file <path>]
  --message-hash <hex>
```

- `--endpoint` is the node's HTTP API base URL, for example
  `http://10.0.0.1:8600`; the commands use plain HTTP. `--token-file` holds
  the API token that the node may require; it is sent as
  `Authorization: Bearer <token>`.
- `--key` and `--package-owner-key` name an ed25519 key file: the 32-byte
  secret seed as 64 hex digits, or a JSON file
  `{"public": "<hex>", "secret": "<hex>"}`.
- The messages are signed for the network execution configuration of this
  bundle and the `--wasmtime-profile`. `--network-id` replaces its network id
  for a network that uses another one; the id is part of every signature.
- `network-call` encodes the method arguments with the contract's ACAB and
  signs the call with `--key`, which must be the destination's owner key. The
  nonce defaults to the current Unix time in milliseconds: a message is
  accepted only with a nonce above the last one accepted for the destination
  and key, so pass `--nonce` to send several calls in the same millisecond or
  to repeat a call deliberately.
- `network-deploy` signs the deploy with `--key` and nonce 1, the first
  external of that key to the new address. With `--package-owner-key` it
  publishes the artifacts as a new package owned by that key; with
  `--committed` it deploys over the committed code of the same artifacts,
  which must already be published. `--owner-id` defaults to the public key of
  `--key`. `--quote-address` prints the address, package and selection ids
  without sending: fund the address first, because an external message
  carries no value.
- `--expire-at` defaults to 300 seconds after `--created-at`, which defaults to
  the current time.

A command sends the message with `POST /v2/avm/messages`. When the node
answers `WRONG_PRODUCER`, `NOT_BLOCK_PRODUCER` or `THREAD_MISMATCH`, it sends
the same bytes to the first producer the answer names. It then reads
`GET /v2/avm/messages/{message_hash}` every 0.5 seconds while the message is
`queued`, and stops 10 seconds after its `expire_at`: a node removes an
expired message from its queue only when the next message arrives for the
same thread, so the status can stay `queued` after `expire_at`. The result's `outcome` holds
`message_hash`, `status`, `error`, `http_status`, the final `url` and the
number of `posts` and `polls`. `status` is one of:

| `status` | Meaning |
|---|---|
| `consumed` | A block of the destination thread consumed the message. That block can still be discarded before it is final |
| `not_final` | Still queued 10 seconds after `expire_at`: its outcome is not known |
| `failed` | The message left the queue without being consumed; `error.code` is the node's code, for example `MESSAGE_REJECTED` with `error.message` `AccountKeyNotAllowed` or `NonceReplay` |
| `refused` | The node refused the message when it was sent; for example `INVALID_MESSAGE` whose `error.message` ends with `ExternalValueNotAllowed` for an external that carries value |

The command exits with status 0 only for `consumed`, and `network-deploy
--quote-address` with 0 after building the deploy. `network-status` makes one
status read with the same fields. Only the node that queued a message knows
its status, for 10 minutes.

## Additional tooling

The following tooling is available in SDK 0.3.0 and later; these options are
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
