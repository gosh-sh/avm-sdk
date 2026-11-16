# From the local stand to a network

The local stand and a network node run the same contract code under the same
execution rules, so a contract that works on the stand is deployed to a
network with the same build artifacts. This page lists the steps, links to
the details, and says what you can and cannot read back from a network in
this release.

## Before you start

You need:

- the contract's `contract.wasm`, `contract.acab` and `contract.descriptor`
  from `avm-contract-build build`; see [Getting Started](getting-started.md#build-a-contract);
- the node's HTTP API base URL, for example `http://10.0.0.1:8600`, and its
  API token file if the node requires one; pass it with
  `--token-file <path>`;
- the network's DApp id;
- ed25519 key files: one that signs the deploy, the owner key that will sign
  calls and, to publish new code, a package-owner key. A key file holds the
  32-byte secret seed as 64 hex digits, or JSON
  `{"public": "<hex>", "secret": "<hex>"}`. Keep these files private;
- a way to send SHELL to the new address with an internal message, for
  example from a wallet contract, because an external message carries no
  value.

The commands sign for the network execution configuration of your bundle and
its Wasmtime profile. Pass `--network-id <hex>` if the network uses another
network id.

## Step 1: test everything on the local stand

Deploy, call and upgrade the contract on the stand first, with the same
`init` data and the same owner key layout, and check its events there. The
stand is the only place where this release lets you read account state and
events; see [Events](events.md#read-events-on-the-local-stand) and
`avm-local-stand inspect --account` in the [tooling guide](tooling-preview.md#inspect-abi-and-account-state).

## Step 2: quote the address

```bash
avm-local-stand network-deploy --endpoint "$AVM_NODE" \
  --wasmtime-profile "$AVM_PROFILE" --key deployer.key \
  --package-owner-key package-owner.key --dapp-id "$DAPP_ID" \
  --wasm target/avm/contract.wasm --acab target/avm/contract.acab \
  --descriptor target/avm/contract.descriptor \
  --salt-hex 01 --init-json null --fuel-fee-shells 100000 --quote-address
```

`--quote-address` sends nothing. It prints the `address`, `package_id`,
`selected_code_id` and `code_hash` of the deploy. Pass `--owner-id <hex>` when
the owner key differs from the deploy key; it defaults to the public key of
`--key`.

## Step 3: fund the address

Send SHELL to the quoted address with an internal message. Fund the whole
pre-execution charge before deploying: the deploy's `fuel_fee_shells`, the
account initialization fee and, for new code, the
[code publication fee](fuel-and-fees.md#code-publication-fee). An unfunded or
underfunded deploy is refused and not charged.

## Step 4: deploy

Repeat the same command without `--quote-address`. With
`--package-owner-key` the deploy publishes new code and completes in
[three phases](contract-deploy.md#publish-new-code-the-three-phase-deploy):
`init` runs 120 seconds of block time after the block that published the
code is final. With `--committed` it deploys another account over code that is
already published, at once.

The command exits with status 0 when the deploy message is `consumed`. For
new code, `consumed` means the first phase only. The first call to the new
contract that ends `consumed` shows that the deploy completed.

## Step 5: call the contract

```bash
avm-local-stand network-call --endpoint "$AVM_NODE" \
  --wasmtime-profile "$AVM_PROFILE" --key owner.key \
  --address "$CONTRACT_ADDRESS" --acab target/avm/contract.acab \
  --method causal.a.version --args-json null --fuel-fee-shells 100000 > call.json
```

Only the contract's [owner key](owner-key-and-external-calls.md) can call it
externally. Other people reach it with internal messages from their own
wallets.

## Step 6: check the outcome

`network-deploy` and `network-call` print an `outcome` with the
`message_hash` and the message `status`. See
[statuses and errors](statuses-and-errors.md) for each status and what to do.
To read the status again later, ask the node that queued the message. That
is the node in the outcome's `url`, which can differ from your `--endpoint`
after a redirect to the producer; pass the `url` without its trailing
`/v2/avm/messages` as the endpoint:

```bash
STATUS_NODE="$(jq -r '.outcome.url | sub("/v2/avm/messages$"; "")' call.json)"
MESSAGE_HASH="$(jq -r .outcome.message_hash call.json)"
avm-local-stand network-status --endpoint "$STATUS_NODE" --message-hash "$MESSAGE_HASH"
```

All options are in the [CLI reference](cli-reference.md#network-client).

## What you can read from a network today

| What | Available in this release |
|---|---|
| Send a deploy or a call | Yes: `network-deploy`, `network-call` (`POST /v2/avm/messages`) |
| Status of a message you sent | Yes: `network-status` (`GET /v2/avm/messages/{message_hash}`), from the node that queued it, for at most 10 minutes |
| Account state, balance or code | No |
| Events | No |
| AVM data in the node's GraphQL service | No: GraphQL does not index AVM accounts, messages or events |

The message status is the only network read of this release. Use it this way:

- Ask the node that queued the message, the one in the outcome's `url`.
  Another node answers that it does not know the message, and the node that
  queued it forgets it after 10 minutes.
- `consumed` means a block executed the message. That block can still be
  discarded before it is final.
- Check a contract's state changes, events and failures on the local stand,
  where `inspect --account` and `events` show them, before you send the same
  messages to a network.
- To confirm that a new-code deploy completed, send a call to the new
  contract and wait for `consumed`.
