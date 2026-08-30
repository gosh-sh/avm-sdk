# FUEL And SHELL

## The Two Units

AVM has no `vmSHELL`, VM-private currency, or VM-private balance.

AVM uses two different units:

- **FUEL** is the deterministic Wasmtime compute meter. It limits how much WASM
  code an execution may run.
- **SHELL** is ordinary transferable value stored as extra currency
  `currency_id = 2` in AVM accounts and messages.

FUEL is not a token and is never stored in an account. On a chargeable route,
the runtime converts a reserved amount of ordinary SHELL into a deterministic
FUEL budget using the active public `fuel_per_shell` configuration value.

## Message Fields

An AVM message can carry ordinary SHELL in two independent ways:

- `extra[currency_id = 2]` is transferred value. The destination receives it.
- `fuel_fee_shells` is a compute reserve. The destination does not receive it
  as transferred value.

`fuel_fee_shells` is a fixed message-header field so the runtime can distinguish
transferred SHELL from SHELL reserved for execution. It is not another currency.

## Which Routes Are Free

Every execution is metered by Wasmtime FUEL, including a free execution. Free
means that no SHELL is charged for that FUEL; it does not mean unlimited CPU.

An internal `MessageIsolated` execution is local-free only when all of these are
true for the active routing view:

1. source and destination have the same `dapp_id`;
2. source and destination are in the same active routing partition;
3. the destination executes in `MessageIsolated` mode;
4. the message is not a callback.

The runtime gives this execution the configured deterministic free FUEL cap. If
the contract exhausts the cap, execution fails with an out-of-FUEL error, but no
SHELL is burned for compute.

The following routes are chargeable:

- external messages;
- cross-DApp internal messages;
- same-DApp messages whose source and destination are separated by a split;
- callbacks;
- `AccountBatch` execution, under its Library-defined pooled economics.

Local-free applies only to delivery and receiver compute for that message. It
does not waive storage rent, code publication fees, account initialization
fees, name fees, or other protocol operations that have their own prices.

## Reserves And DApp Splits

A contract may set a non-zero `fuel_fee_shells` reserve on a same-DApp outbound
intent even while source and destination are currently local.

The runtime resolves the actual route before committing the canonical message:

- if the route is local-free, the committed message contains
  `fuel_fee_shells = 0` and the proposed reserve is not debited;
- if a split makes the route chargeable, the committed message contains the
  proposed reserve and the paid-route rules apply;
- if a chargeable route has no sufficient reserve, it fails before receiver
  WASM starts.

This allows one contract intent to remain valid across a future split without
charging the common local path. Applications that require delivery after a
possible split should propose a sufficient reserve instead of relying on the
current topology remaining local.

## Chargeable FUEL

For chargeable `MessageIsolated` execution, the runtime derives:

```text
fuel_budget = min(
    fuel_fee_shells * fuel_per_shell,
    max_message_execution_fuel,
)
```

The runtime enables Wasmtime FUEL, starts the execution with this budget, and
records the exact `fuel_used`. Settlement converts used FUEL back to ordinary
SHELL with ceiling division:

```text
charged_shells = ceil(fuel_used / fuel_per_shell)
unused_shells = fuel_fee_shells - min(fuel_fee_shells, charged_shells)
```

The sender prepays the reserve when the outbound message is materialized. The
receiver does not debit the same reserve again.

If a callback was requested, unused reserve is returned to the original sender
through the callback as ordinary SHELL value. Without a callback, unused
reserve is burned. AVM does not create an automatic refund message solely to
return unused reserve.

Fixed delivery and callback fees are separate from `fuel_fee_shells`. A
chargeable message may therefore debit transferred SHELL, the FUEL reserve, and
fixed fees independently.

## Funding Is A Separate Concern

The FUEL reserve is message-scoped. It is not a DApp credit account, spending
ceiling, sponsorship facility, or non-transferable balance.

Base AVM does not provide a `DappConfig`-style native balance, an `is_unlimit`
flag, or a DApp-bound currency. Ordinary SHELL held by an AVM account remains
ordinary transferable value. A local-free message can execute without paying
SHELL for receiver compute, but that does not create a persistent DApp funding
pool and does not make the account's SHELL non-transferable.

Applications that require sponsored execution or funds that cannot leave a
DApp need an explicit protocol or application design for that property. They
must not represent it as `vmSHELL` or infer it from `fuel_fee_shells`.

## Contract Guidance

- Treat FUEL as a compute limit, not as money.
- Treat `fuel_fee_shells` as ordinary SHELL reserved for a potentially
  chargeable execution.
- Use `extra[currency_id = 2]` only for SHELL that the destination should
  receive as value.
- Give same-DApp outbounds a reserve when they must continue working after a
  DApp split. The local-free path will not charge it.
- Request a callback when unused reserve must return to the sender.
- Simulate against the same AVM configuration, code, input, and account state
  when estimating `fuel_used`.

Any implementation that stores a `vmSHELL` balance, treats
`fuel_fee_shells` as a separate token, charges the local-free path, or debits a
prepaid internal reserve again at the receiver is incorrect.
