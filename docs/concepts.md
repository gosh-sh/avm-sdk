# Concepts

A one-page summary of the terms the other guides use. Each entry links to the
page with the details.

## Contract and account

A contract is a Rust crate built to WASM with `avm-contract-build`. Deploying
it creates an account: an address with its own data (the contract state), a
balance and a link to its code. A message to the account runs the contract's
code on that data. See [Getting Started](getting-started.md).

## Address

An address is written `dapp_id:account_id`: 64 lowercase hex digits of the
DApp id, a colon, and 64 lowercase hex digits of the account id.

- The DApp id is a routing namespace. Accounts of one DApp can send most internal
  messages to each other without a SHELL charge while they stay in the same
  routing partition; see [which routes are free](fuel-and-fees.md#which-routes-are-free).
- The account id is derived from the UI name, the package, the owner id, the
  salt and the hash of the `init` data, so it is known before the deploy.
  `deploy --quote-address` on the stand and `network-deploy --quote-address`
  print it.

See [deterministic addresses](runtime-limits.md#deterministic-addresses).

## Owner key

The owner key is a 32-byte ed25519 public key that the contract keeps in its
own data, at the `owner_key_offset` that its descriptor declares. Only this
key can sign external calls to the account. By the SDK convention `init`
writes the deploy's `owner_id` there. See
[owner key and external calls](owner-key-and-external-calls.md).

## External and internal messages

- An **external message** comes from outside the network, signed with a key:
  a deploy, or a call signed by the owner key. It carries no value. The local
  stand's `deploy` and `call` commands and the network commands send external
  messages.
- An **internal message** is sent by one contract to another as an outbound
  intent of its output. It can carry value (currencies) and can request a
  callback with the result. Other people use your contract through internal
  messages, for example from their own wallet contracts; see the
  [wallet example](../examples/wallet/README.md).

## FUEL and SHELL

- **FUEL** measures computation. Every execution has a FUEL budget, also on
  free routes; FUEL is never stored in an account.
- **SHELL** is ordinary value, currency id 2. A message's `fuel_fee_shells` is
  a SHELL reserve that buys its FUEL budget on a paid route; the unused part
  is returned through a callback or burnt.
- Fees for code publication, account initialization and storage are paid in
  SHELL too.

See [FUEL and SHELL](fuel-and-fees.md) and
[runtime limits](runtime-limits.md).

## Package, code and selection ids

- A **package** groups the versions of one contract. Its `package_id` is
  derived from its first code, and does not change with later versions.
- The **code hash** identifies one WASM module.
- A **selected code id** (`selected_code_id`) identifies a version of the
  package that accounts can run: `publish-version` adds one, `upgrade` moves
  an account to it, `set-default` and `deprecate` manage the versions.

The package owner key signs package changes. See
[contract deploy](contract-deploy.md) and the
[command-line reference](cli-reference.md#avm-local-stand).

## Events

An event is a record that a contract publishes about what it did, for
off-chain readers. See [Events](events.md).

## Local stand and network

- The **local stand** (`avm-local-stand` with `--state`) runs contracts on
  your computer in a state file. It assigns nonces and signs with three
  built-in identities: `developer`, `package-owner` and `user`. You can
  inspect any account's state, list events and replay the whole journal. It
  completes a code deploy in one command.
- A **network** runs the same code on its nodes. The `network-*` commands sign
  with your key files and send messages to a node. A deploy of new code takes
  three phases there, and today you can read back only the status of a message
  you sent.

See [from the local stand to a network](network.md).
