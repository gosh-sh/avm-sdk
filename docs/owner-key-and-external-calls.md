# Owner key and external calls

An ordinary external call is a call by the owner of the destination contract.
The network admits it only when it is signed by the owner key that the
destination account keeps in its own data. A key that the message names for
itself does not authorize it, and an external message carries no value.

## Declare the key slot

The contract declares where its data holds the key, in the `[descriptor]`
table of `avm-contract.toml`:

```toml
[descriptor]
execution_mode = "message_isolated"
state_schema_id = "..."
owner_key_offset = 0
```

`avm-contract-build` writes the offset as an optional trailing `u32` of the
code descriptor, so a contract with a key slot has a 70-byte
`contract.descriptor` and one without has 66 bytes. The owner key is the 32
bytes `data[owner_key_offset .. owner_key_offset + 32]` of the account.

The account has no owner key when its descriptor declares no offset, when the
slot does not fit in its data, or when the 32 bytes are all zero. Such an
account rejects every external call with `AccountKeyNotAllowed`; internal
messages still reach it. A contract that only other contracts call, such as a
receiver, can therefore omit the slot.

## Write, read and rotate the key

Deploy installs no key: the contract writes it. The SDK convention is that
`init` writes the deploy's `owner_id`, which `avm-local-stand deploy
--owner-id <hex>` selects and which defaults to the stand's `user` identity:

```rust
use avm_contract_sdk::{set_owner_key, owner_key, OWNER_KEY_BYTES};

const OWNER_KEY_OFFSET: u32 = 0;

fn init(input: MessageInput<'_>) -> MessageOutput {
    let mut data = vec![0; OWNER_KEY_BYTES + 8];
    if let Ok(deploy) = input.deploy() {
        // The key slot is inside the data built above.
        let _ = set_owner_key(&mut data, OWNER_KEY_OFFSET, deploy.owner_id());
    }
    MessageOutput::success(StateUpdate::FullReplace(data))
}
```

`owner_key(data, offset)` returns the key, or `None` under the same rules as
the network: a slot outside the data or an all-zero key. `set_owner_key`
writes a key into a slot inside the data and returns `EncodeError::InvalidIndex`
for a slot that does not fit, leaving the data unchanged. A contract rotates
its key with its own method, guarded as it decides, by writing the slot; the
new key authorizes external calls from the next block, because admission reads
the key from the account state before the block. Writing a zero key leaves the
account without a key. Every other state update must keep the slot, so write
the key into each full replacement of the data.

## What admission checks

Before any charge and before WASM, admission compares the message's signing
key with the destination's owner key. A mismatch is dropped with
`AccountKeyNotAllowed`: no debit, no rent or nonce change and no execution, so
a call under a foreign key costs the destination nothing. An admitted call is
paid by the destination account, which must hold the SHELL for its
`fuel_fee_shells`.

An external message with any `extra` currency is rejected with
`ExternalValueNotAllowed`. Fund an account with an internal message or, on
the local stand, with `fund-user`:

```bash
avm-local-stand fund-user --state "$AVM_STATE" --wasmtime-profile "$AVM_PROFILE" \
  --address "$ADDRESS" --shells 100000 --extra-currencies 0:500
```

Replay protection is a nonce per destination account and signing key: a
message is accepted only with a nonce above the last one accepted for that
pair. The local stand assigns nonces and signs with its `developer`,
`package-owner` and `user` identities (`call --identity`). A BM token
attributes paid external execution to a Block Manager; it does not authorize
the call. This SDK release does not include network tooling that builds,
signs or submits external messages.

## Calls from other people

Because only the owner key calls a contract externally, a contract that other
people use is reached through internal messages, for example from each
caller's own wallet. The [owner wallet example](../examples/wallet/README.md)
turns one signed external call into one internal message with value and an
optional callback. The contract compares `Caller::Internal(address)` with the
wallet addresses it stores and receives value as the message's extra
currencies.
