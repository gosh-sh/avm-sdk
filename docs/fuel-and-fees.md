# FUEL And SHELL

## The Two Units

AVM has no `vmSHELL`, VM-private currency, or VM-private balance.

All account balances and message transfers use `extra`, with an explicit
`currency_id` for every entry. Currency ID 0 is valid; it has no special role.
Entries remain sorted by ID and unique, and a zero amount is preserved.

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

## Operation prices in SDK 0.5.0

SDK 0.5.0 uses the exact operation schedule in the bundled
[`wasmtime-release-profile.v1`](../profiles/wasmtime-release-profile.v1).
`operator_cost` lists a fixed FUEL price for each admitted WASM instruction;
`variable_operator_costs` adds charges for requested volume. These prices
apply to both paid and local-free execution.

- `block`, `loop`, `else`, `end`, `nop`, `drop`, `return` and `unreachable`
  have zero instruction price.
- Other instructions have explicit prices from 1 to 255 FUEL. Calls,
  indirect calls, loads/stores, division/remainder, floating-point operations
  and conversions can have different prices. Use the bundled schedule for
  the exact opcode and release.
- `memory.copy`, `memory.fill` and `memory.init` add a charge for each
  requested byte. `table.copy` and `table.init` add a charge for each
  requested element.
- `memory.grow` adds a charge for each requested 64-KiB page, including a
  failed growth request. The instruction price is charged separately.
- Unlisted variable charges are zero. The complete fixed and variable
  schedules are recorded together with `fuel_semantics_revision=4` in the
  profile.

Volume charges use the requested length, not the amount of new data or the
number of cache misses. A bulk operation with zero requested length still
has its instruction price. Include the volume charges, allocator code,
function entries, input/output handling and cleanup when budgeting the
complete execution's FUEL.

Existing contract WASM remains admissible under the same Core-WASM V2 rules,
but its FUEL use can change without recompilation. Re-run concrete init,
message and batch inputs with the matching tools/profile/configuration. Check
both the attached reserve and the applicable message or batch cap. A larger
reserve cannot exceed the configured cap. On paid routes a changed
`fuel_used` also changes rounded SHELL charges and unused-reserve refunds;
the payer, local-free conditions and refund rules below remain the same.

## Chargeable FUEL

For chargeable `MessageIsolated` execution, the runtime derives:

```text
fuel_budget = min(
    fuel_fee_shells * fuel_per_shell,
    max_message_execution_fuel,
)
```

The runtime enables Wasmtime FUEL, starts the execution with this budget, and
records the exact `fuel_used`. The [memory payment](#memory-payment) never
reduces this budget. Settlement converts used FUEL and the memory payment
back to ordinary SHELL with one ceiling division:

```text
memory_fee_units = pages_at_end * memory_page_price
required_shells = ceil((fuel_used + memory_fee_units) / fuel_per_shell)
charged_shells = min(fuel_fee_shells, required_shells)
unused_shells = fuel_fee_shells - charged_shells
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

## Memory Payment

A paid call also pays for the linear memory it ends with. The bundled profile's
`memory_page_price` is the price, in FUEL-equivalent fee units, of one 64-KiB
WASM page for one call. `pages_at_end` is the contract's linear-memory size
in pages when the call stops, whether it succeeds, fails or traps. It counts
every page of that size, including grown pages that were never written. A
failed `memory.grow` adds no pages. Memory cannot shrink during a call, so
`pages_at_end` is at least the module's initial memory.

The bundled `memory_page_price` is a provisional development value and can
change before network acceptance. Size reserves with the price in the profile
you run.

The memory payment is not FUEL. It never enters `fuel_used`, the FUEL budget
or the message and batch caps, and it does not depend on the contract's
[memory class](#memory-classes).

Before a paid call starts, the reserve must cover the module's initial memory:

```text
ceil(initial_pages * memory_page_price / fuel_per_shell) <= fuel_fee_shells
```

Otherwise the call fails with status 56 (`VM_INSUFFICIENT_BALANCE`) before
WASM starts, nothing is charged, and the reserve follows the callback or burn
rules above.

If a call that would otherwise succeed needs more SHELL than its reserve, it
fails with status 56, returns no output and commits none of its effects; the
whole reserve is charged. A call that already failed keeps its own status and
is charged at most its reserve. Local-free calls pay neither compute nor memory.

An `AccountBatch` invocation pays its ending pages once for the whole batch
from the pooled reserve. A successful batch credits its remainder under the
Library rules; after a failed batch the remainder remains available to the
fallback.

Size every paid reserve for the complete call:
`fuel_used + pages_at_end * memory_page_price` must not exceed
`fuel_fee_shells * fuel_per_shell`. Memory that a call never touches still
costs its pages, so keep initial memory and growth as small as the contract
tests allow; `--check-memory` reports peak linear-memory capacity.

## Memory Classes

The runtime runs each contract in one of ten memory classes: the first class
whose maximum contains the module's declared maximum linear memory. Each class
has its own pre-created engine and allocation pool. A class changes neither
operation prices, FUEL budgets nor the memory payment: equal code, state and
input produce equal `fuel_used`, status and `pages_at_end` in every class.
The class maxima, `memory_class_maxima_bytes` in the bundled profile, are
provisional development values and can change before network acceptance.

## Code Publication Fee

A deploy that carries code pays a Code Library publication fee in
SHELL from the funding of its deploy address. The fee follows the size of the
WASM module and its declared function count:

```text
units    = ceil(code_len / 40960)
estimate = 159904 * units + 136 * (function_count - min(function_count, 256 * units))
fee      = 1000 * ceil(estimate / 159904)
```

`code_len` is the byte length of the module and `function_count` the number of
entries in its function section, which the deploy declares in its body. Every
started 40 KiB unit pays 1000 SHELL and includes 256 functions, so a contract
with at most 256 functions per started unit pays 1000 SHELL per unit: 2000 SHELL
for a 41441-byte contract, 26000 SHELL for 1 MiB and 52000 SHELL for 2 MiB.
A deploy that publishes a new selection carries its module bytes and pays this
fee for them; a deploy that reuses a published selection pays no publication
fee. Fund the deploy address for this fee, the account
initialization fee and the deploy's FUEL reserve before deploying.

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
  when estimating `fuel_used` and `pages_at_end`.
- Size paid reserves for compute FUEL plus the memory payment of the ending
  pages, not for compute alone.

Any implementation that stores a `vmSHELL` balance, treats
`fuel_fee_shells` as a separate token, charges the local-free path, or debits a
prepaid internal reserve again at the receiver is incorrect.
