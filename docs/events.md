# Events

An event is a small record that a contract publishes about something it did,
for example "1000 tokens moved from A to B". Events do not change contract
state and other contracts do not receive them as messages. They are a log for
off-chain readers: wallets, indexers and your own tests.

This page shows how to declare and emit an event, which limits apply, how a
contract reads the internal events of other accounts, and how to read events
on the local stand. On shellnet the Block Manager serves the events of
finalized blocks; see
[Read accounts and events](network.md#read-accounts-and-events).

## Declare the event in the ABI

Declare every event your contract emits in `avm-contract.toml`, with the ABI
type of each field. The [fungible token example](../examples/fungible-token/README.md)
declares one event:

```toml
[[abi.types]]
id = 3
kind = "u64"
name = "event_amount"

[[abi.types]]
id = 4
kind = "fixed_bytes"
name = "event_holder"
len = 64

[[abi.events]]
name = "token.transferred"

[[abi.events.fields]]
id = 1
type_id = 3
name = "amount"

[[abi.events.fields]]
id = 2
type_id = 4
name = "from"

[[abi.events.fields]]
id = 3
type_id = 4
name = "to"
```

After a build, ABI inspection lists the event with its `event_id`:

```bash
avm-contract-build abi --acab examples/fungible-token/target/avm/contract.acab --json \
  | jq -c '.events'
```

```json
[{"event_id":"0xdf658b5745e08f51","fields":[{"field_id":1,"name":"amount","type_id":3},{"field_id":2,"name":"from","type_id":4},{"field_id":3,"name":"to","type_id":4}],"name":"token.transferred"}]
```

The `event_id` is derived from the event name: the first 8 bytes of the
BLAKE3 hash of the name, read as a little-endian `u64`. Use it as the event's
`event_class` when you emit it, as the examples do. The runtime does not
compare an emitted event with the ABI declaration or its `event_class` with
its name; keep them equal, because readers find events by them.

## Emit an event

A contract emits events by adding them to the `event_intents` of a successful
`MessageOutput`. Encode the fields with `encode_event_fields`. This is the
transfer path of the fungible token example:

```rust
use avm_contract_sdk::{
    encode_event_fields, EventField, EventIntent, EventKind, EventValue, MessageOutput,
    StateUpdate,
};

const TRANSFERRED_EVENT_CLASS: u64 = 0xdf65_8b57_45e0_8f51; // event_id of "token.transferred"

let Ok(fields_binary) = encode_event_fields(&[
    EventField { name: "amount".into(), value: EventValue::U64(amount) },
    EventField { name: "from".into(), value: EventValue::Data(from.clone()) },
    EventField { name: "to".into(), value: EventValue::Data(to.clone()) },
]) else {
    return MessageOutput::failure(INVALID_EVENT);
};
let mut output = MessageOutput::success(StateUpdate::FullReplace(ledger.encode()));
output.event_intents = vec![EventIntent {
    input_index: 0,
    kind: EventKind::External,
    event_class: TRANSFERRED_EVENT_CLASS,
    event_name: "token.transferred".into(),
    topics: vec![from, to],
    fields_binary,
}];
output
```

- `input_index` is 0 for a message-isolated contract.
- `kind`: use `EventKind::External` for an event that off-chain readers
  consume. `EventKind::Internal` additionally keeps the event in the emitting
  account's internal event ring under its `event_class`, where other
  contracts can [read it](#read-internal-events-in-a-contract). It needs the
  guest crate's feature [`event-rings`](#enable-internal-event-rings).
- `topics` are short byte strings that identify what the event is about, for
  example the two holder addresses above.
- Field values are `EventValue::Null`, `Bool`, `U64`, `I64`, `U128`, `I128`,
  `Text`, `Data` (bytes) or `Address`.
- Give the fields in strictly ascending byte order of their names:
  `encode_event_fields` returns `EncodeError::InvalidEvent` for unsorted or
  repeated names.
- Events are kept only when the call succeeds. `MessageOutput::failure` cannot
  carry events, and a failed call discards every event it emitted.

## Event limits

The local stand and the network execution configuration of this release use
these limits:

| Rule | Limit |
|---|---|
| Event name | Lowercase dot-separated segments, at least two, such as `token.transferred`; each segment starts with `a`-`z` and continues with `a`-`z`, `0`-`9` or `_`; at most 64 bytes |
| Reserved names | Names starting with `avm.` belong to the runtime |
| Topics | At most 8 per event, each at most 64 bytes |
| Fields | At most 32 per event, each name at most 64 bytes |
| Event size (`max_event_value_bytes`) | 500 bytes: the event name, all topics, all field names and all field values together |
| Events per call output | 256 |
| Internal events kept per `event_class` | The last 5; an older one is dropped |
| Internal event classes per account | 64, with at most 262144 bytes of kept internal events |

A call whose event breaks a size rule fails with status 45
(`VM_ABI_OUTPUT_TOO_LARGE`); an event with an invalid or reserved name fails
it with status 44 (`VM_ABI_OUTPUT_MALFORMED`). Unsorted or repeated field
names do not reach the runtime: `encode_event_fields` returns an error, and
an output that carries such a field list is not written, which ends the
call with status 43 (`VM_ABI_OUTPUT_MISSING`). A call whose internal events
would exceed the per-account limits fails with status 54
(`VM_EVENT_LIMIT_EXCEEDED`). The failed call keeps none of its effects. See
[statuses and errors](statuses-and-errors.md).

## Enable internal event rings

Internal event rings are an option of the guest crate: a contract need not
write or read them. A contract that does enables the feature `event-rings` in
its `Cargo.toml`, as the contract B and
[internal event reads](../examples/internal-event-reads/README.md) examples do:

```toml
avm-contract-sdk = { path = "../../crates/avm-contract-sdk", features = ["event-rings"] }
```

The feature compiles `EventKind::Internal`, the `outbound_event_reads` field of
`MessageOutput` and `BatchOutput` with `EventReadEntry`, and the reader
`MessageInput::internal_event_reads` with `decode_internal_event_results` and
`decode_event_fields`. Without it a contract carries no ring code and emits
only external events; its input still holds the reads that its message
declares and the `internal_event_results` field.

## Read internal events in a contract

A contract cannot search events while it runs. Instead, the sender of an
internal message declares exact reads for that message. Each read names one
ring: the source account's DApp id and account id and an `event_class`.
Before the receiver's code runs, the runtime puts the kept events of each
declared ring into the receiver's input. A read may name any account in any
DApp.

With the feature `event-rings`, declare the reads of an outbound message in
`outbound_event_reads`:

```rust
use avm_contract_sdk::{EventReadEntry, InternalEventReadRequest};

output.outbound_intents = vec![message_to_reader];
output.outbound_event_reads = vec![EventReadEntry {
    outbound_index: 0,
    reads: vec![InternalEventReadRequest { source: emitter, event_class: 7 }],
}];
```

- `outbound_index` names an entry of `outbound_intents`. Give the entries in
  strictly ascending `outbound_index` order, at most one per message.
- A message carries 1 to 128 reads. Give them in strictly ascending order of
  the source DApp id, then the source account id, then `event_class` as a
  number, so that no read repeats.
- An output that breaks these rules is not written, which ends the call with
  status 43 (`VM_ABI_OUTPUT_MISSING`). `MessageOutput::failure` cannot carry
  reads.
- Each read adds 72 bytes to the message, plus 4 bytes for a message with
  reads. They count toward the outbound message size limits and are paid by
  the message fee; reads have no other price.
- Callbacks and refunds, which the runtime builds, and external messages
  carry no reads.

`BatchOutput` has the same `outbound_event_reads` field.

The receiver reads the results with `MessageInput::internal_event_reads`:

```rust
use avm_contract_sdk::decode_event_fields;

let Ok(reads) = input.internal_event_reads() else {
    return MessageOutput::failure(WRONG_READS);
};
for (read, events) in &reads {
    // `read` names the declared ring; `events` are its kept events, oldest first.
    if let Some(newest) = events.last() {
        let fields = decode_event_fields(newest.fields_binary);
        // ...
    }
}
```

- The list has one entry per declared read, in the declared order. It is
  empty for a message without reads.
- Each entry holds the ring's kept events, at most the last 5 of its class,
  oldest first, with their `tx_hash`, `lt`, `time`, `seq`, `event_name`,
  `topics` and `fields_binary`. It is empty when the ring has no events.
- `decode_event_fields` returns the fields that `encode_event_fields` wrote.
- Reads see the state at the start of the receiver's block. Internal events
  emitted earlier in the same block become visible in the next block.
- The results enlarge the receiver's input: each event takes 64 bytes plus
  its name, its topics with a 2-byte length each, and its fields. Size the
  contract's arena for the largest input it accepts; see
  [contract memory](contract-memory.md). Results above 262144 bytes fail the
  message before the contract runs with status 42 (`VM_ABI_INPUT_TOO_LARGE`).

The [internal event reads example](../examples/internal-event-reads/README.md)
deploys an emitter, a caller and a reader in three DApps and checks the reads
on the local stand.

The local stand executes declared reads. On a network, a node executes them
only when its runtime supports read declarations: a node without that support
fails a call whose output declares reads with status 44
(`VM_ABI_OUTPUT_MALFORMED`), and the messages it delivers carry no reads.

## The deploy event `avm.account.init`

Every successful deploy emits one `avm.account.init` event. The runtime
writes it, not your contract. Its fields are `account` (the new address),
`init_data_hash`, `owner`, `package`, `salt`, `selected_code` and `ui_name`,
plus any `deploy_meta` fields of the deploy.

This event has the same 500-byte limit. Keep a deploy's `salt`, `ui_name` and
all `deploy_meta` field names and values within 235 bytes together; a larger
deploy fails its `init` with status 44. Put larger deploy data into the
`init_data`.

## Read events on the local stand

`avm-local-stand events` lists every event of the stand, in the order the
stand committed them. It takes no options besides `--state` and
`--wasmtime-profile`. After the [Getting Started](getting-started.md) deploy
of contract A, which emits no events of its own, the list holds only the
deploy event:

```bash
avm-local-stand events --state "$AVM_STATE" --wasmtime-profile "$AVM_PROFILE"
```

```json
{
  "command": "events",
  "count": 1,
  "events": [
    "0000000004000600020000000000000000d2496b...75695f6e616d650007000000000000000100000002000000"
  ],
  "status": "ok"
}
```

(The event is shortened here; the real entry has 1,472 hex digits.)

Each entry is the hex of one canonical binary event record. The event name
and field names appear in it as plain bytes followed by a zero byte, so you
can find events by name with `jq`, as the example scripts do:

```bash
name_hex="$(printf 'avm.account.init' | od -An -tx1 | tr -d ' \n')00"
avm-local-stand events --state "$AVM_STATE" --wasmtime-profile "$AVM_PROFILE" \
  | jq --arg name "$name_hex" '[.events[] | select(contains($name))] | length'
```

```text
1
```

The `call` command's result also has an `events` array: the events of that
call and of the internal messages it caused, in the same hex form. In the
fungible token example, one `wallet.send` call through a holder's wallet
returns two events, the token's `token.transferred` and the wallet's
`wallet.callback`.

To see the complete flow, run the fungible token stand test from an SDK
checkout with the matching tools on `PATH`:

```bash
bash examples/fungible-token/stand-e2e.sh
```

```text
fungible-token stand e2e passed
```

It deploys the token and three wallets, makes transfers through the wallets
and then checks with `events` that the stand holds exactly ten
`token.transferred` events: one for the initial mint and one for each
successful transfer.

## Events on a network

On a network, the events of a message are recorded in the block that
executes it. The `network-call` and `network-deploy` results report the
message status only. On shellnet the Block Manager indexes the AVM events of
finalized blocks and serves them in the GraphQL query
`blockchain { avm_events }`, with filters by source account, event name,
class, kind and time and with paging; see
[Read accounts and events](network.md#read-accounts-and-events). Test event
output on the local stand first.
