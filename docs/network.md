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
- ed25519 key files: one that signs the deploy, the owner key that will sign
  calls and, to publish new code, a package-owner key. A key file holds the
  32-byte secret seed as 64 hex digits, or JSON
  `{"public": "<hex>", "secret": "<hex>"}`. Keep these files private;
- a way to send SHELL to the new address with an internal message, for
  example from a wallet contract, because an external message carries no
  value; on shellnet the [SHELL minter](#get-test-shell) sends it;
- optionally, the HTTP base URL of a Block Manager, to attribute your
  messages to it; see [Block Manager attribution](#block-manager-attribution).

The commands sign for the network execution configuration of your bundle and
its Wasmtime profile. Pass `--network-id <hex>` if the network uses another
network id.

## Step 1: test everything on the local stand

Deploy, call and upgrade the contract on the stand first, with the same
`init` data and the same owner key layout, and check its events there; see
[Events](events.md#read-events-on-the-local-stand) and
`avm-local-stand inspect --account` in the [tooling guide](tooling-preview.md#inspect-abi-and-account-state).
On shellnet you can then read the account and its events back from the Block
Manager; see [Shellnet](#shellnet).

## Step 2: quote the address

```bash
avm-local-stand network-deploy --endpoint "$AVM_NODE" \
  --wasmtime-profile "$AVM_PROFILE" --key deployer.key \
  --package-owner-key package-owner.key \
  --wasm target/avm/contract.wasm --acab target/avm/contract.acab \
  --descriptor target/avm/contract.descriptor \
  --salt-hex 01 --init-json null --fuel-fee-shells 100000 --quote-address
```

`--quote-address` sends nothing. It prints the `address`, `package_id`,
`selected_code_id` and `code_hash` of the deploy. Pass `--owner-id <hex>` when
the owner key differs from the deploy key; it defaults to the public key of
`--key`.

## Step 3: fund the address

Send SHELL to the quoted address with an internal message; on shellnet,
[get test SHELL](#get-test-shell) from the minter. Fund the whole
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

Code is published once per network, whoever publishes it: the package and
code ids come only from the `wasm`, `acab` and `descriptor`, so the same build
gives the same ids for every developer. A deploy that carries code already
published is refused with `VM_CODE_ALREADY_EXISTS` and charges nothing, while
its message is still `consumed`; deploy over the published code with
`--committed`.

With `--bm-endpoint`, the command confirms the deploy: it reads the account
from that Block Manager (`GET /v2/account`) before the send and every 2
seconds after the message is `consumed`, and adds a `confirmation` field to
its output:

| `confirmation.status` | Meaning |
| --- | --- |
| `active` | The deploy changed the account and the account is active. |
| `refused` | The account stayed as it was for 60 seconds: the network refused the deploy. Its `error` names the likely cause: the code is already published (`VM_CODE_ALREADY_EXISTS`), deploy with `--committed`; for `--committed`, the selection is not published. |
| `not_active` | The account is not active 600 seconds after the deploy. |

The command exits with status 0 only when the message is `consumed` and, with
`--bm-endpoint`, the confirmation is `active`; otherwise with status 2.
Without `--bm-endpoint` it does not confirm: for new code, `consumed` then
means the first phase only, and you read the account until it is `active`;
see [Read accounts and events](#read-accounts-and-events).

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

## Block Manager attribution

A Block Manager (BM) serves the network to its users. When your external
message carries a BM attribution token, the network counts each of your
messages that executes successfully and pays its fee toward that BM's reward.
The token is the BM's signed certificate, the same for all its users for its
lifetime; it does not change what your message does or costs.

Pass the BM's HTTP base URL to `network-deploy` or `network-call` with
`--bm-endpoint`:

```bash
avm-local-stand network-call --endpoint "$AVM_NODE" --bm-endpoint "$AVM_BM" \
  --wasmtime-profile "$AVM_PROFILE" --key owner.key \
  --address "$CONTRACT_ADDRESS" --acab target/avm/contract.acab \
  --method causal.a.version --args-json null --fuel-fee-shells 100000
```

The command reads the BM's current token with `GET /v2/avm/bm_token` and
sends it with the message. It fails without sending when the BM gives no
token. On a network that accepts unattributed messages, as shellnet does, a
message whose token the network does not accept, for example the token of a
BM the network does not license, still executes, without attribution. A
message without `--bm-endpoint` is not attributed.

## Shellnet

Shellnet is the public test network. Its AVM values:

| Value | Shellnet |
|---|---|
| Node HTTP API (`--endpoint`), sends and message status | `http://94.156.178.12:8602` |
| Block Manager (`--bm-endpoint`) | `http://shellnet.ackinacki.org:80` |
| Account and event reads | `https://shellnet.ackinacki.org`: `GET /v2/account` and GraphQL `POST /graphql` |
| Network id (`--network-id`) | The bundle's default, 32 bytes of `0x22`: omit the option |
| API token (`--token-file`) | Not needed: sends, status and reads take no token |

The network commands speak plain HTTP and need the port in the URL, so give
`--endpoint` and `--bm-endpoint` exactly as above. The Block Manager serves
reads, not sends: `/v2/avm/messages` is on the node only.

Shellnet has two system minters, each in its own DApp
(`dapp_id = account_id`):

| Currency | ECC id | Account id | Who may call |
|---|---|---|---|
| NACKL | 1 | `0c645e8bb3606e494feaabc3c5b185afd572c0b60936030477fcc84cf5d2f812` | Only its owner key |
| SHELL | 2 | `51f87c509122d9596d67f96e05f84bfdeb506ffa7cd70426a2d9109be3409896` | Anyone, with any key |

Only these accounts mint on shellnet; no other network designates them.

### Get test SHELL

The SHELL minter's one method, `shellnet.mint`, mints SHELL and sends it in
one internal message to the `destination` you name, also to an address that
has no contract yet: the SHELL then waits there for your deploy. Sign the call
with any key file; the minter pays for the call itself. The minter's ABI is
[`examples/wallet/shellnet-shell-minter.acab`](../examples/wallet/shellnet-shell-minter.acab).

```bash
MINTER=51f87c509122d9596d67f96e05f84bfdeb506ffa7cd70426a2d9109be3409896
avm-local-stand network-call --endpoint http://94.156.178.12:8602 \
  --bm-endpoint http://shellnet.ackinacki.org:80 \
  --wasmtime-profile "$AVM_PROFILE" --key any.key \
  --address "$MINTER:$MINTER" \
  --acab examples/wallet/shellnet-shell-minter.acab --method shellnet.mint \
  --args-json '{"destination": "<dapp_id>:<account_id>", "amount": 10000000,
                "fuel_fee_shells": 1, "method_id": 6078841212400898889}' \
  --fuel-fee-shells 100000 > mint.json
```

- `amount` is the SHELL the destination receives; give it as a JSON number.
- `fuel_fee_shells` is the FUEL fee of the minter's message: at least 1 for a
  destination in another DApp. Sent to an address without a contract it buys
  nothing and burns, so the address receives exactly `amount`.
- `method_id` is the method the message calls, never 0. A destination without
  a contract does not run it; for a deployed contract name a method that
  accepts value, here `wallet.receive` of the wallet example.

A request the minter cannot serve, for example `method_id = 0` or an invalid
address, is still `consumed` but sends nothing. Check the balance as below.

### Read accounts and events

`GET /v2/account?account_id=<hex>&dapp_id=<hex>` on
`https://shellnet.ackinacki.org` returns the account's last finalized state in
`boc`, base64. For an AVM account the decoded bytes start with `02000000`,
followed by fixed-width little-endian fields; an option is the byte `0`, or
the byte `1` and its value; a list is its `u64` length and its items:

| Field | Encoding |
|---|---|
| account hash | 32 bytes |
| account id | 32 bytes |
| storage used bytes | `u32` |
| storage last paid | `u32`, block time |
| storage due payment | option of `u128` |
| last transaction logical time | `u64` |
| balances | list of `u32` ECC id and `u128` amount |
| init code hash | option of 32 bytes |
| state | `u32`: `0` no code, `1` active, `2` frozen and then its 32-byte account hash |
| code hash | option of 32 bytes |
| data hash | option of 32 bytes |
| data | option of a list of bytes |

An address without an AVM account answers `404`, or with a TVM account of the
same id, whose `boc` does not start with `02000000`. The wallet example's
[`shellnet_read.py`](../examples/wallet/shellnet_read.py) decodes the
account and prints JSON:

```bash
python3 examples/wallet/shellnet_read.py account https://shellnet.ackinacki.org "<dapp_id>:<account_id>"
```

The events of finalized blocks are in the GraphQL query
`blockchain { avm_events }` on `https://shellnet.ackinacki.org/graphql`. It
filters by `src_dapp_id`, `src_account`, `event_class`, `event_name`, `kind`
(`EXTERNAL` or `INTERNAL`) and an inclusive `time_from`/`time_to` range in
block seconds, combined by AND, and pages with `first`/`after` and
`last`/`before` in ascending `chain_order`, 50 events per page by default.
Each event has `chain_order`, `block_id`, `tx_hash`, `lt`, `time`, `seq`,
`kind`, `event_class`, `event_name`, `src_dapp_id`, `src_account`, `topics`
and `fields`, a list of `name`, `type` and `value`:

```bash
curl -s https://shellnet.ackinacki.org/graphql -H 'Content-Type: application/json' \
  -d '{"query": "{ blockchain { avm_events(src_account: \"<account_id>\", first: 10) { pageInfo { hasNextPage endCursor } edges { node { time event_name fields { name type value } } } } } }"}'
```

A deploy shows as the event `avm.account.init` of the new account, with its
`account`, `package`, `selected_code`, `owner`, `salt` and `init_data_hash`.
`python3 examples/wallet/shellnet_read.py events <reader> <account_id> <event_name>`
prints the events of one account and name.

### Quickstart: mint, deploy, call, read

[`examples/wallet/shellnet-quickstart.sh`](../examples/wallet/shellnet-quickstart.sh)
runs the whole path with the wallet example and new random keys in its work
directory:

1. quotes the wallet address (`network-deploy --quote-address`);
2. mints 10,000,000 SHELL to it and waits until `/v2/account` shows them;
3. deploys the wallet over its code, which is already published on shellnet
   (`--committed`), and waits until the account is `active` with the quoted
   code hash;
4. sends 100 SHELL from the wallet to itself with a callback (`wallet.send`);
5. reads the `wallet.callback` event of the wallet from GraphQL and checks
   that the delivery's status code is 0.

```bash
export AVM_SHELLNET_WORK="$HOME/avm-shellnet-run-1"
bash examples/wallet/shellnet-quickstart.sh
```

It needs the bundle's `avm-contract-build`, `avm-local-stand` and profile, as
the [wallet example](../examples/wallet/README.md) does, plus `jq` and
Python 3.11+. `AVM_SHELLNET_WORK` keeps the keys and every command's JSON
output; a new directory gives new keys and a new wallet. After you change the
wallet, its code is new: set `AVM_SHELLNET_PUBLISH=1` to publish it, in three
phases that take about two minutes.

## What you can read from a network today

| What | Available in this release |
|---|---|
| Send a deploy or a call | Yes: `network-deploy`, `network-call` (`POST /v2/avm/messages`) |
| Status of a message you sent | Yes: `network-status` (`GET /v2/avm/messages/{message_hash}`), from the node that queued it, for at most 10 minutes |
| Account state, balance or code | On shellnet: the Block Manager's `GET /v2/account`, see [Read accounts and events](#read-accounts-and-events) |
| Events | On shellnet: the Block Manager's GraphQL `blockchain { avm_events }` |

Use the message status this way:

- Ask the node that queued the message, the one in the outcome's `url`.
  Another node answers that it does not know the message, and the node that
  queued it forgets it after 10 minutes.
- `consumed` means a block executed the message. That block can still be
  discarded before it is final. Its effects, the account state and the
  events, are final once the Block Manager shows them.
- `consumed` does not tell whether the contract succeeded. Read the account
  or the events, or request a callback, to see the result.
- To confirm that a deploy completed, give `network-deploy` the
  `--bm-endpoint`, or read the account until it is `active`.
