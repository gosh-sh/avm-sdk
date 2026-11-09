# Fixed-supply token

This MessageIsolated example credits 1,000,000 internal token units to the
holder wallet whose 64-byte address (DApp ID then account ID) is the init data.
Holders are wallet addresses: a holder spends through its own keyed wallet
([`wallet`](../wallet/README.md)), which sends `token.transfer` as an internal
message, and the token spends only the balance of the calling wallet. The token
has no owner key, so the node rejects every external call to it with
`AccountKeyNotAllowed`. `token.transfer` takes a nonzero 64-byte recipient
wallet address followed by a nonzero little-endian `u64` amount. The ledger has
eight holder slots, including zero-balance holders, and no allowances or
subsequent mint operation. It does not represent an AVM ECC balance.

The contract emits `token.transferred` for initialization and each successful
transfer. Malformed arguments, insufficient balances and holder exhaustion
return the declared contract error without state or event effects. The state
decoder checks that the sum of balances equals the recorded supply.

Run `bash examples/fungible-token/stand-e2e.sh` from an SDK source checkout
with the matching `avm-contract-build` and `avm-local-stand` on `PATH`. The
script builds the real token and wallet WASM, deploys one funded wallet per
stand identity and the token, and drives every transfer through the sender's
wallet with a callback request: the exact token status comes from the
`wallet.callback` event and the balances from the token account. It checks the
initial supply, transfer, rejected overdraft, return transfer, self-transfer,
zero amount, a transfer from a wallet without tokens, all eight holders and the
rejected ninth holder. It then checks that the node rejects an external to
another identity's wallet, an external to the token and an external carrying
value, and finally verifies events and journal replay. `AVM_CONTRACT_BUILD`,
`AVM_LOCAL_STAND` and `AVM_WASMTIME_PROFILE` select an explicit matching tool
set. Set `TMPDIR`, `TMP` and `TEMP` to an external scratch directory;
`AVM_TOKEN_WORK` optionally retains the journey there for inspection.

Adapted from Alan's [SDK example contribution](https://github.com/gosh-sh/avm-sdk/pull/12),
commit `ffeb7c037d742cc8bc7b4232645c9cd4d322ac1a`. Its financial contract
implementation is retained.
