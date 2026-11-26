# Message statuses and common errors

This page collects the results you meet most often and what to do about each.
The local stand prints JSON on stdout. An error result has
`"status": "error"` and an `error` text; a deploy to an unfunded address
has `"status": "precondition"`. The network commands print an
`outcome` object; see the [CLI reference](cli-reference.md#network-client)
for its fields.

## Network message statuses

`network-call` and `network-deploy` wait for the outcome of the message and
report one of these `status` values:

| `status` | Meaning | What to do |
|---|---|---|
| `consumed` | A block of the destination thread consumed the message. That block can still be discarded before it is final | Nothing. For a deploy of new code, `consumed` covers the first phase only; see [contract deploy](contract-deploy.md#deploy-on-a-network) |
| `not_final` | Still queued 10 seconds after `expire_at`: its outcome is not known | Ask again later with `network-status` at the node that queued it. A node removes an expired message from its queue only when the next message arrives for the same thread |
| `failed` | The message left the queue without being consumed; `error.code` is the node's code, for example `MESSAGE_REJECTED` with `error.message` `AccountKeyNotAllowed` or `NonceReplay` | Find `error.message` in the tables below |
| `refused` | The node refused the message when it was sent; for example `INVALID_MESSAGE` whose `error.message` ends with `ExternalValueNotAllowed` for an external that carries value | Find `error.message` in the tables below |

The commands exit with status 0 only for `consumed`. They follow the node's
`WRONG_PRODUCER`, `NOT_BLOCK_PRODUCER` and `THREAD_MISMATCH` answers to the
producer of the destination thread by themselves.

`network-status` reads the status once. It reports `queued`, `consumed` or
`failed`. Only the node that queued the message knows it, for 10 minutes;
any other node, or the same node later, answers with `http_status` 404 and
`status` `failed`. See
[what you can read from a network](network.md#what-you-can-read-from-a-network-today).

## Refused external messages

These errors refuse an external call or deploy before it runs. A refused
message is not charged and its nonce stays unused. The same reasons appear
on the network and on the local stand.

| Error | Cause | What to do |
|---|---|---|
| `AccountKeyNotAllowed` | The message is not signed by the destination's [owner key](owner-key-and-external-calls.md), or the account has no owner key | Sign with the owner key (`--key` on a network, `--identity` on the stand). Reach a contract without an owner key with internal messages |
| `NonceReplay` | The nonce is not above the last nonce accepted for this destination and key | For `network-call`, use a higher `--nonce`, or omit it so that the command uses the current time in milliseconds. `network-deploy` always signs nonce 1, so repeating a deploy from the same key to the same address is always a replay |
| `ExternalValueNotAllowed` | The external message carries currency | Send no value with an external. Fund accounts with an internal message, or with `fund-user` on the stand |
| `cannot pay the external: RouteRequiresFee` | `--fuel-fee-shells` is zero | Give the message a non-zero FUEL reserve |
| `cannot pay the external: InsufficientBalance` | The destination cannot pay the message. For a deploy, the funding at its address does not cover `fuel_fee_shells`, the account initialization fee and, for a new selection, the publication fee | Fund the address for the whole charge; see [FUEL and SHELL](fuel-and-fees.md) |

On the local stand these reasons appear inside the error text, for example:

```json
{
  "error": "call rejected canonical operation: local external ingress rejection: AccountKeyNotAllowed",
  "status": "error"
}
```

## Local stand deploy results

| Result | Cause | What to do |
|---|---|---|
| `"error": "funding_required"`, exit status 2 | The deploy address holds no funding; the result's `address` is the exact address | Fund that address with `fund-user`, then repeat the same `deploy` |
| `code deploy destination account exists` | The address is already an active account | Change the `--salt-hex` or the `init` data to get a new address, or call the existing account |

## Execution statuses

These VM statuses come from a message that a block processed for its
destination account. A failed
execution keeps none of its effects. An external call that reached execution
is still committed with its charge and its spent nonce. On the local stand,
`call` exits with status 2 and prints `"error": "execution_failed"` and the
`status_code`:

```json
{"status":"error","error":"execution_failed","status_code":43,"state_committed":true,"receipt":{"charged_shells":"2631", ...}}
```

(Fields of the real output selected with `jq`.)

| Status | Name | Cause | What to do |
|---|---|---|---|
| 12 | `VM_ACCOUNT_FROZEN` | The destination account is frozen and the internal message does not unfreeze it. It fails before WASM, pays the fixed rejection fee of 3,000,000 FUEL and changes nothing in the account | Pay the account's due storage rent with a message that unfreezes it; see [storage rent](runtime-limits.md#storage-rent) |
| 21 | `VM_CODE_DISABLED` | The code was built with `execution_mode = "account_batch"`, which the node does not run | Build the contract with `execution_mode = "message_isolated"` |
| 30 | `VM_WASM_TRAP` | The WASM trapped, including a linear-memory stack overflow | Fix the panic or trap; size the stack from complete tests; see [contract memory](contract-memory.md) |
| 31 | `VM_WASM_OUT_OF_FUEL` | The FUEL budget ran out | Raise `fuel_fee_shells` up to the message cap or do less work per call; see [runtime limits](runtime-limits.md#fuel-and-failure-diagnostics) |
| 33 | `VM_WASM_STACK_LIMIT` | The WASM stack limit was exceeded, for example by deep recursion | Bound the recursion depth |
| 42 | `VM_ABI_INPUT_TOO_LARGE` | The contract input exceeds its cap: 1048576 bytes for a message input, without the account data it carries, or 6291456 bytes for an `AccountBatch` input, account state included | Send a smaller message body; see [runtime limits](runtime-limits.md#state-output-and-deploy-body-are-different-limits) |
| 43 | `VM_ABI_OUTPUT_MISSING` | The contract returned no output, for example because building it failed: a wrong state output mode, an event field list with unsorted or repeated names, outbound event reads that break the [declaration rules](events.md#read-internal-events-in-a-contract), or an output that does not fit the contract's arena | Check the required state output mode and the output construction result; see [contract memory](contract-memory.md) |
| 44 | `VM_ABI_OUTPUT_MALFORMED` | The output is invalid, for example an event with an invalid or reserved name, or a deploy whose `avm.account.init` event exceeds 500 bytes. A node whose runtime does not support declared internal event reads also returns it for an output that declares them | See [event limits](events.md#event-limits) and [read internal events in a contract](events.md#read-internal-events-in-a-contract) |
| 45 | `VM_ABI_OUTPUT_TOO_LARGE` | The output is too large, for example an event above its limits, or an outbound message body or callback result above `max_body_size` | See [event limits](events.md#event-limits) and [runtime limits](runtime-limits.md#state-output-and-deploy-body-are-different-limits) |
| 50 | `VM_STATE_TOO_LARGE` | The account state exceeds its size limit | See [runtime limits](runtime-limits.md#state-output-and-deploy-body-are-different-limits) |
| 54 | `VM_EVENT_LIMIT_EXCEEDED` | The internal events of the call would exceed the account's limits | Use fewer `event_class` values with `EventKind::Internal`, or smaller internal events; see [event limits](events.md#event-limits) |
| 56 | `VM_INSUFFICIENT_BALANCE` | The reserve does not cover the initial memory before execution, or compute FUEL plus the memory payment after it | Raise `fuel_fee_shells`; see [memory payment](fuel-and-fees.md#memory-payment) |
| 59 | `VM_MINT_NOT_ALLOWED` | The output carries a mint intent that the network does not allow: from `init`, or for an account, code or currency that the network configuration does not designate | Only the shellnet SHELL minter mints; fund local stand accounts with `fund-user`; see [known limitations](known-limitations.md#execution) |

Statuses of 1000 and above are your contract's own errors, declared in
`[[abi.errors]]` of its `avm-contract.toml`; `avm-contract-build abi` lists
them. The local stand's `call` reports them as it reports a VM status: it
exits with status 2 and prints `"error": "execution_failed"` with the
status in `status_code`. For contract A returning status 1000:

```json
{"status":"error","command":"call","error":"execution_failed","status_code":1000,"state_committed":true,"receipt":{"charged_shells":"2631", ...},"detail":"..."}
```

Read the status with `jq`:

```bash
jq -r '.status_code' call.json
```

The call is committed and charged like any failed call. A caller contract
receives the status in its callback.

## Code deploy errors

| Error | Cause | What to do |
|---|---|---|
| `VM_CODE_ALREADY_EXISTS` | The deploy carries module bytes whose code is already committed | Deploy over the committed code instead; see [contract deploy](contract-deploy.md#deploy-over-committed-code) |
| `VM_DEPLOY_FAILED` | The module failed admission validation; nothing commits and the publication fee is burnt | Build the module with the matching `avm-contract-build` and check it on the local stand first |
