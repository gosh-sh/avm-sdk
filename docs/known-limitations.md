# Known limitations of this release

This page collects what this SDK release does not do, with what you can
do instead. Each item links to the page with the details.

## Reading from a network

- **No network reads of accounts or events.** The SDK sends deploys and calls
  to a network and reads the status of a message you sent. It has no command
  to read an account's state, balance or events from a node. Check state and
  events on the local stand; see
  [what you can read from a network](network.md#what-you-can-read-from-a-network-today).
- **Message status is short-lived.** Only the node that queued a message
  knows its status, for 10 minutes; see
  [network message statuses](statuses-and-errors.md#network-message-statuses).
- **`consumed` is not final.** The block that consumed a message can still be
  discarded before it is final.
- **No AVM data in GraphQL.** The node's GraphQL service does not index AVM
  accounts, messages or events.
- **Plain HTTP only.** The network commands talk to the node's HTTP API over
  plain HTTP; see the [CLI reference](cli-reference.md#network-client).

## Events

- **Events on a network cannot be read with the SDK.** They are recorded in
  blocks, but no command or node API of this release returns them; see
  [events on a network](events.md#events-on-a-network).
- **The stand lists events as raw records.** `avm-local-stand events` prints
  every event of the stand as hex, without a filter by account or a decoder;
  see [read events on the local stand](events.md#read-events-on-the-local-stand).
- **No reads of internal events.** Contracts built with this SDK cannot
  request reads of another account's internal events.
- **Deploy metadata is small.** A deploy's `salt`, `ui_name` and
  `deploy_meta` must fit 235 bytes together, because of the
  [`avm.account.init` event](events.md#the-deploy-event-avmaccountinit).

## Execution

- **No AccountBatch execution.** Code built with
  `execution_mode = "account_batch"` deploys, but every message to it ends with
  status 21. Use `message_isolated`; see [FUEL and SHELL](fuel-and-fees.md#which-routes-are-free).
- **Fixed callback budget.** A callback handler always runs on 1 SHELL of
  FUEL and memory; see [callback budget](fuel-and-fees.md#callback-budget).
- **No BN254 host functions.** Contracts that need BN254 must bring their own
  WASM implementation; see [BN254 host functions](runtime-limits.md#bn254-host-functions).
- **No sender code-version lookup.** A contract can check an internal
  sender's derived address but not which code version it runs; see
  [runtime boundaries](tooling-preview.md#runtime-boundaries).
- **No rent accessor in contracts.** The guest ABI does not expose an
  account's storage debt; use stand inspection during development; see
  [storage rent](runtime-limits.md#storage-rent).

## Local stand compared with a network

- **One-step code deploy.** The stand runs a deploy that carries code in one
  command. It does not model the compile window, the `Verify` wait or
  finality, so a network deploy completes later; see
  [contract deploy](contract-deploy.md#publish-new-code-the-three-phase-deploy).
- **Logical time.** The stand numbers transactions with its own logical-time
  counter, so message hashes on the stand differ from the network's.
- **Fixed limits.** The stand CLI offers no override for its limits; see
  [runtime limits](runtime-limits.md).

## Configuration values

- **Development values.** The stand's limits and rent prices are the
  development configuration of this release, not mainnet values; see
  [runtime limits](runtime-limits.md).
- **Provisional prices and profile values.** The precompile prices and the
  bundled profile's `memory_page_price`, `memory_class_maxima_bytes`,
  `signals_based_traps` and `pooling_linear_memory_keep_resident` are
  provisional and can change; see [memory payment](fuel-and-fees.md#memory-payment)
  and [precompile prices](fuel-and-fees.md#provisional-precompile-compute-prices).
- **No production acceptance.** The examples are educational, and this
  release does not establish production-network or performance acceptance;
  see the [release notes](release-notes.md).
