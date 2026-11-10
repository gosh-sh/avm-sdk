# Contract deploy

A deploy creates an account from a deploy message and binds it to code in the
Code Library. It either carries a new WASM module or names code that is
already committed.

## Deploy over committed code

A deploy that names committed code, with `code_len = 0`, can be an external
message or an internal message from a contract. It carries no module bytes,
keeps the 65536-byte message body limit and pays no publication fee. On the
local stand, deploy over committed code with `deploy --package-id <id>
--selected-code-id <id>`; `inspect` lists the package and its prepared
selections. A deploy that carries module bytes whose code hash is already
committed fails with `VM_CODE_ALREADY_EXISTS`: name the committed code instead.

## Publish new code: the two-phase deploy

Only an external deploy can carry a new module. On the network it completes
in two phases, and the destination address moves through these states:

1. `Uninit`: the block producer of the destination thread admits the deploy,
   with its signatures, times, nonce and funding, and records it without the
   code bytes. The publication fee is fixed now from the declared code length
   and function count.
2. `Verify`: within `code_deploy_window_seconds` (120 seconds of block time)
   that producer validates and compiles the module and publishes its bytes;
   every node then compiles it in the background. A module that fails
   admission validation instead ends the deploy with `VM_DEPLOY_FAILED`:
   nothing commits and the publication fee is burnt.
3. `Finalized` and `Init`: once the block that published the code is final,
   a `Finalized` record is written, and `code_deploy_window_seconds` after it
   the `Init` record runs `init`. With a successful `init` the code, its
   selection, package and ABI metadata and the account commit together.

While the address is `Uninit` or `Verify`, messages to it wait in its queue.
A block carries at most one `Init` record with a successful `init` for a
given code hash. A producer that
misses the compile deadline is penalized by a `CodeDeployPenalty` record of
the block; the deploy's sender takes no part in it.

The deploy is signed without its module bytes: its signatures cover the deploy
with the WASM omitted, under a separate hash domain, and still bind the code
hash, `code_len` and `function_count`.

The local stand runs a deploy that carries code in one command: it does not
model the producer's compile window, the `Verify` wait or finality, so a
network deploy completes later than the same deploy on the stand.

## Deploy on a network

`avm-local-stand network-deploy` signs a deploy and sends it to a network node.
Derive the address with `--quote-address`, fund it by an internal message, for
example from a wallet, and then send the deploy:

```bash
avm-local-stand network-deploy --endpoint "$AVM_NODE" \
  --wasmtime-profile "$AVM_PROFILE" --key deployer.key \
  --package-owner-key package-owner.key --dapp-id "$DAPP_ID" \
  --wasm target/avm/contract.wasm --acab target/avm/contract.acab \
  --descriptor target/avm/contract.descriptor \
  --salt-hex 01 --init-json null --fuel-fee-shells 100000 --quote-address
```

With `--package-owner-key` the deploy publishes the artifacts as a new package
through the two-phase deploy. With `--committed` instead it deploys another
account over the committed code of the same artifacts, at once. The result
names the address, `package_id`, `selected_code_id` and `code_hash`.

The node reports the deploy message's status, not the phases of its address:
`consumed` means a block consumed the deploy, which for new code is the block
of its `Uninit` record. The later `Verify`, `Finalized` and `Init` records do
not change that status. Messages to the address wait in its queue until
`Init`, so the first call to the new contract that ends `consumed` shows that
the deploy completed. A deploy that failed leaves no account, and such a call
is not consumed.

## Fees and caps

| Item | Value |
|---|---|
| Publication fee | 1000 SHELL for every started 40960-byte unit of code, with 256 functions per unit; see [code publication fee](fuel-and-fees.md#code-publication-fee) |
| `max_code_deploy_body_size` | 3145728 bytes, the body of a deploy that carries code |
| `max_code_size` | 3145440 bytes, the module: the body cap minus the 288-byte fixed deploy header |
| `max_code_deploy_input_bytes` | 3276800 bytes, the `init` input of a deploy that carries code |
| `max_body_size` | 65536 bytes, every other message body |
| `code_deploy_window_seconds` | 120 |

Fund the deploy address before deploying for the publication fee, the account
initialization fee and the deploy's FUEL reserve. The configuration with these
values has a 637-byte canonical encoding, so its identity differs from earlier
bundles' configuration.
