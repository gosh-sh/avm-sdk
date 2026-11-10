# Owner wallet

An ordinary external call is admitted only when it is signed by the owner key
that the destination contract keeps in its own data, and an external message
carries no value. A contract that other people call is therefore reached by
internal messages, for example from each caller's own wallet. This
MessageIsolated example is that wallet.

The descriptor declares `owner_key_offset = 0`, and the account data is the
32-byte owner key alone. `init` writes the deploy's `owner_id` there with
`avm_contract_sdk::set_owner_key`; `avm-local-stand deploy --owner-id <key>`
selects it. A zero `owner_id` leaves a wallet without a key, which rejects
every external call.

| Method | Caller | Effect |
|---|---|---|
| `wallet.send` | external, owner key | one internal message with value; state unchanged |
| `wallet.set_owner_key` | external, owner key | writes a new 32-byte key; it authorizes external calls from the next block, and a zero key makes the wallet keyless |
| `wallet.receive` | any caller: an internal message, or an external under the owner key | accepts the message value; state unchanged |
| `wallet.callback` | callback of a send | keeps the returned value and emits the `wallet.callback` event |

`wallet.send` takes, after the length prefix of its ABI `bytes` argument: the
destination DApp id and account id (32 bytes each), the destination method id
(`u64` LE), the internal message `fuel_fee_shells` (`u128` LE), a callback flag
(`0` or `1`), the number of extra currencies (`u8`), that many entries of
currency id (`u32` LE) and amount (`u128` LE) in strictly ascending id order,
and the message body. The external call's own `fuel_fee_shells` is paid from
the wallet's SHELL; the internal message's FUEL reserve and value are debited
from the wallet balance. With the callback flag set, the destination's status
code and any returned currencies arrive in the `wallet.callback` event (fields
`extra` and `status_code`), and a failed delivery returns the value.

A contract that accepts calls through wallets stores the wallet addresses of
its participants and compares them with `Caller::Internal(address)`; the
value arrives as the message's extra currencies.

Run `bash examples/wallet/stand-e2e.sh` with the matching `avm-contract-build`
and `avm-local-stand` on `PATH`, or select them with `AVM_CONTRACT_BUILD`,
`AVM_LOCAL_STAND` and `AVM_WASMTIME_PROFILE`. It deploys a user wallet funded
with `fund-user --shells ... --extra-currencies 0:500` and a developer wallet
over the committed code, then checks that a foreign key and an external with
value are rejected for free, a send with its callback, a failed delivery that
returns `0:100` with status `1704`, malformed arguments (`1702`), a send from
an internal caller (`1701`), a key rotation, a keyless wallet and the replay.
`AVM_WALLET_WORK` optionally retains the journey.
