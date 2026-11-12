# Runtime limits, rent and deployment identity

These are the **SDK 0.6.1 local stand** settings. Its FUEL caps are the network
execution defaults. All of them are development configuration, not mainnet
promises. The runtime configuration leaves
launch values for these limits and rent prices unspecified (`TBD`). Obtain the
active network configuration before estimating production costs or capacity.

The bundled Wasmtime profile's engine settings `signals_based_traps` and
`pooling_linear_memory_keep_resident` are provisional development values, as
are its [memory page price](fuel-and-fees.md#memory-payment) and
[memory classes](fuel-and-fees.md#memory-classes). They can change before
network acceptance; a bundle with other values has a different profile identity.

## Inspect the configuration you are using

```bash
avm-local-stand inspect --state demo.state --wasmtime-profile "$PROFILE"
```

`local_config` reports the selected stand's limits and rent settings. This is a
read-only projection of its execution configuration. It does not change the
configuration or attest to a remote network. The account list in the general inspection additionally
reports `storage_last_paid`, `storage_due_payment`, and
`storage_unpaid_tx_count` from committed account metadata.

## FUEL and failure diagnostics

The stand uses `max_message_execution_fuel = 1031871955`,
`fuel_per_shell = 1000000`, and `max_bucket_execution_fuel = 2063743910`.
On a chargeable isolated call the budget is the smaller of the message reserve
converted to FUEL and the message cap. Increasing `fuel_fee_shells` beyond the
cap cannot buy a larger execution. Local-free messages are also metered and
capped. The bucket cap bounds one `AccountBatch` invocation; each
`MessageIsolated` call has its own message allowance. See [FUEL and SHELL](fuel-and-fees.md).

SDK 0.5.1 raises these caps from 10000000 and 100000000 FUEL; operation
prices, volume charges and the memory payment are unchanged from SDK 0.5.0.
A reserve above 10 SHELL now buys up to the message cap. Recheck the
complete execution against the [bundled price schedule](fuel-and-fees.md#operation-prices-in-sdk-050).

CLI failures after an attempted WASM execution report the VM status name,
`fuel_used`, `fuel_budget`, and `max_message_execution_fuel`. Status **31** is
`VM_WASM_OUT_OF_FUEL`; status **30** is `VM_WASM_TRAP`. Status **56** is
`VM_INSUFFICIENT_BALANCE`: a paid reserve does not cover the initial memory
before execution, or the compute FUEL plus the
[memory payment](fuel-and-fees.md#memory-payment) after it. The meter includes the
runtime's metered allocation, guest entrypoint and cleanup work, not just your
method body. Validation failures before execution have no attempted FUEL to
report. An external call that reached WASM execution was admitted: when it
fails, it is committed with its charge and its spent nonce, and none of its
effects are kept (`state_committed` and the receipt in the CLI output).

Status **33** is `VM_WASM_STACK_LIMIT`: the call exceeded the WASM stack limit,
for example through recursion that is too deep. Bound recursion depth in the
contract instead of relying on this limit.

The contract's linker-selected linear-memory stack is a separate bound. Its
stack-first layout and generated pointer guards turn overflow into a deterministic
WASM trap (status **30**) before data or heap storage can be overwritten.
Choose its size from complete contract tests or `build.stack_bytes`;
see [contract memory](contract-memory.md).

The stand CLI does not offer an override for these limits. Build contracts with
the pinned compiler and account for the configured FUEL budget.

The module's declared maximum comes from its committed `avm-memory.json`
unless `build.max_memory_bytes` explicitly overrides it in the contract
manifest. This is bounded by the runtime's global memory cap.
Use `--check-memory` on local executing commands to inspect peak linear-memory
capacity, the declared maximum and rejected growth; see [contract memory](contract-memory.md).

## State, output and deploy body are different limits

The current stand selects `FullReplace` when the **previous canonical account
image**, including its header and balances, is at most **65536 bytes**. Larger
active accounts require `DataPatchList`. Use the input's
`required_state_output_mode`; the threshold is not a 64-KiB total-state ceiling.
Init requires `FullReplace`.

`max_account_data_bytes = 1048576` bounds the resulting state data for both
output modes. A patch cannot evade that limit. Patch lists also have
`max_account_patch_bytes = 262144` and `max_account_patch_ranges = 512`, with
canonical sorted, nonoverlapping ranges and valid bounds. The complete input
and message-output envelopes each have a **1048576-byte** cap, so envelope
overhead can make the usable data size smaller than the state-data cap. Storage
chunking does not enlarge contract-visible limits.

Status **43**, `VM_ABI_OUTPUT_MISSING`, means the guest returned no ABI output.
It is not a size-limit code. For example, SDK output construction can fail if the
wrong state mode is returned. Check the required mode and output-construction
result. Status **45** means oversized WASM output; status **50** denotes the
state-size boundary; status **51** denotes the message-body boundary. Error
precedence can expose an earlier failure before a later size check.

`max_body_size = 65536` bounds every message body except that of a deploy that
carries code. A code deploy, an external deploy with a non-zero `code_len`, may
have a body of up to `max_code_deploy_body_size = 3145728` bytes: its fixed
288-byte envelope, metadata, init data and the embedded WASM module, which
`max_code_size = 3145440`, the WASM admission cap, bounds. The `init` input of
a code deploy contains the complete deploy message and has its own cap,
`max_code_deploy_input_bytes = 3276800`; every other input keeps the
1048576-byte input cap above. Only an external deploy can carry code, and its
code bytes are paid by the size-based
[publication fee](fuel-and-fees.md#code-publication-fee). On the network such a
deploy completes in [two phases](contract-deploy.md#publish-new-code-the-two-phase-deploy).
Reusing an already published selection avoids embedding its code again; such a
deploy keeps the 65536-byte body limit and the init-data limits. Code admission
also rejects a `br_table` instruction with more than 65520 targets.

## Storage rent

The local stand uses `storage_price_per_byte_second = 1` SHELL and
`storage_unpaid_tx_freeze_threshold = 3`. The rent formula is:

```text
new rent = (canonical account bytes + policy-charged chunk overhead)
           * storage_price_per_byte_second
           * (block_time - storage_last_paid)
```

Prior unpaid debt is also due. Rent is collected before WASM/FUEL. Unpaid
attempts accumulate debt; the third consecutive unpaid attempt under this
profile freezes the account before normal user WASM. Full payment clears the
debt and unpaid count; unfreezing requires full due payment and normal
transaction costs. There is no AVM rule granting a TVM-style 30-day grace period.
An untouched account has no transaction storage phase; cold-state sweeping
would require a separately defined maintenance procedure.

Execution failure does not make already charged protocol fees free. Transferred
ECC value and unused prepaid FUEL have separate refund rules; see
[FUEL and SHELL](fuel-and-fees.md). The local CLI's ordinary `call` command rolls
back an error, so it does not demonstrate committed failure settlement. Do not
use that local behavior to infer network fee refunds.

Rent metadata belongs to the committed account index, not contract `data`.
The current guest ABI does not expose a debt/last-paid accessor. Use read-only
stand inspection for development monitoring; do not interpret account balance
alone as rent debt. No mainnet rent price or grace period is asserted here.

## Deterministic addresses

For an ordinary/UI account, with exact byte strings and little-endian lengths:

```text
init_data_hash = blake3(canonical_init_data)
account_id = blake3(
    b"AVM_ACCOUNT_ID" || u16_le(len(ui_name)) || ui_name
    || package_id[32] || owner_id[32]
    || u16_le(len(salt)) || salt || init_data_hash[32])
address = dapp_id[32] || account_id[32]
```

`dapp_id` is a routing namespace and is absent from the account-ID preimage.
`owner_id` is an opaque 32-byte identity; authorization is a separate check.
The package identity derives from its first code:
`package_id = blake3(b"AVM_CODE_PACKAGE" || blake3(first_wasm_bytes))`.
A later selected code version does not change the package identity or address.
DNS NameContract reservations have a separate system rule; this formula must
not be used for them.

Use `deploy --quote-address` with the exact intended deploy arguments, fund that
address, then repeat without the flag. Quoting neither deploys nor funds it.
Changing constructor bytes changes the ordinary address, even with the same
salt and code. Mutually embedding two yet-unknown addresses in their respective
init data creates a circular dependency; there is no init-independent CREATE2
mode. Applications can instead use a shared deployment identifier/factory or
an authenticated post-deployment linking step. Authorization and race handling
for that step belong to the application.

## BN254 host functions

[Issue #18](https://github.com/gosh-sh/avm-sdk/issues/18) requests BN254 addition,
multiplication and pairing imports. The current core-WASM runtime admits no
such host imports. Contracts that use BN254 must supply their own WASM
implementation within the configured execution budget. No BN254 precompile or
increased mainnet FUEL cap is included in this release.

Use the configuration hash, profile and source identity of the runtime you are
running when comparing measurements. An SDK local stand is not a production
network configuration.
