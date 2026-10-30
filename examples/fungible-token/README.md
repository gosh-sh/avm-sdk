# Fixed-supply token

This MessageIsolated example credits 1,000,000 internal token units to the
external signing key that initializes the account. `token.transfer` takes a
32-byte recipient key followed by a nonzero little-endian `u64` amount. Only
the external caller's balance can be spent. The ledger has eight holder slots,
including zero-balance holders, and no allowances or subsequent mint operation.
It does not represent an AVM ECC balance and does not support contract holders.

The contract emits `token.transferred` for initialization and each successful
transfer. Malformed arguments, insufficient balances and holder exhaustion
return the declared contract error without state or event effects. The state
decoder checks that the sum of balances equals the recorded supply.

Run `bash examples/fungible-token/stand-e2e.sh` from an SDK source checkout
with the matching `avm-contract-build` and `avm-local-stand` on `PATH`. The
script builds the real WASM, quotes and funds its address, checks the initial
supply, transfer, rejected overdraft, return transfer, self-transfer, zero
amount, all eight holders and rejected ninth holder, then verifies events and
journal replay. `AVM_CONTRACT_BUILD`, `AVM_LOCAL_STAND` and
`AVM_WASMTIME_PROFILE` select an explicit matching tool set. Set `TMPDIR`,
`TMP` and `TEMP` to an external scratch directory; `AVM_TOKEN_WORK` optionally
retains the journey there for inspection.

Adapted from Alan's [SDK example contribution](https://github.com/gosh-sh/avm-sdk/pull/12),
commit `ffeb7c037d742cc8bc7b4232645c9cd4d322ac1a`. Its financial contract
implementation is retained.
