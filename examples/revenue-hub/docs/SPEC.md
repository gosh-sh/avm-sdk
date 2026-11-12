# Revenue Hub specification

## Money model and invariant

Principal is the explicit application ECC `PRINCIPAL_CURRENCY_ID = 0`;
operational SHELL is ECC 2. This is a new-instance example choice, not a live
token denomination or a mapping/conversion of the former unnamed main asset.
A deposit requires positive principal and may also donate positive SHELL; no
other ECC is accepted. Membership is fixed on initialization: each of 1–16
members has a unique wallet address, a unique recipient address, and positive basis-point
weight. Weights sum to 10,000. All component addresses are in the same DApp.

For cumulative deposited principal `R`, member `i` is entitled to
`E_i(R) = floor(R × weight_i / 10,000)`. Each deposit credits only
`E_i(R_new) - E_i(R_old)`. Therefore final entitlement depends on cumulative
revenue rather than deposit fragmentation. Multiplication uses quotient/remainder
decomposition to avoid overflowing `u128` for `R = u128::MAX`.

At every successful state boundary:

`member_paid_i + member_claimable_i + member_pending_i = E_i(R)`.

`R = total_paid + total_claimable + total_pending + rounding_dust`.

`total_paid = sum(member_paid_i)`.

`0 <= rounding_dust < member_count`.

Held principal is `R - total_paid - total_pending`. Pending principal has left
the vault in an outbound intent and remains an unsettled liability. A new claim
requires the account to cover held principal and the proposed operating reserve.
The runtime still enforces fixed fees and rent; this is not a fee-price oracle.

## Wallets and authority

Members and governance signers act through their own keyed wallet contracts:
the participant signs an external to its wallet and the wallet sends the
internal call, with any value, to the vault. An external call is admitted only
under the owner key the destination keeps in its data, and no external carries
value. The vault keeps no owner key: every external to it is rejected, and it
identifies a member or a signer only by the internal caller address
(`Caller::Internal`). Deposits, top-ups, status and proposal execution accept
any caller. A contract reaches the callback handler only by a native callback.

## Claim lifecycle

1. The member sends the index of their own claim from their wallet. Amount and destination are
   derived exclusively from stored entitlement and immutable configuration.
2. The vault atomically moves that member's claimable balance to pending, assigns
   the next global claim nonce, and emits a transfer with a native callback request.
3. Only that member is blocked while pending. Deposits and other members' claims
   can continue. A later deposit credits new claimable amounts separately.
4. A successful native callback must echo the exact member/nonce/amount, carry
   zero returned principal, and match the configured receiver. It settles paid.
5. Failure callbacks have an empty body and must return the exact pending
   principal. They restore claimable without changing paid.

Per-member callback checks require native callback flags/header, expected source
and handler, correct original sender address, nonzero causal hash, a hash distinct
from the latest accepted receipt, and creation LT greater than both the active
attempt's boundary and the last accepted callback LT. State remains bounded and
retries do not consume an append-only receipt history.

This relies on AVM admitting only authentic VM-owned callbacks and rejecting
canonical queue replay. The contract cannot observe the outgoing materialized
message hash at dispatch time and does not authenticate arbitrarily forged native
callback frames. Creation-LT behavior is tested in the pinned same-DApp local
stand; other routing/runtime combinations are outside the validated scope.

There is no timeout that clears an unresolved claim. A never-arriving callback
leaves that member pending; governance cannot reissue or sweep the liability.

## Governance

Three unique governance wallet addresses are fixed at deployment. Supported actions are:

| Action | Value | Effect |
| --- | --- | --- |
| 1 — Pause | Zero | Reject new deposits and claims |
| 2 — Resume | Zero | Allow new deposits and claims |
| 3 — Set operating reserve | Positive u128 | Set outgoing receiver FUEL reserve in SHELL |

Proposal body: `nonce:u64 + action:u8 + value:u128`, all little-endian (25 bytes).
The nonce must be the next monotonic proposal nonce. A proposer contributes one
vote. Another signer must approve the identical full body. A signer can vote once;
votes are represented as a three-bit set.

The stored origin is the observed time of the first execution, used only as
the reference for elapsed seconds, never as a deadline. Proposal times are `u64` elapsed-second offsets from
that first execution. Configuration contains only durations.

`created = block_time.saturating_sub(origin_time)`.

`ready = created + configured_delay`.

`expires = ready + configured_vote_window`.

Execution is allowed for any caller when at least two votes exist and
`ready <= elapsed < expires`. An executed proposal is cleared and cannot be
replayed. A live proposal cannot be replaced; an expired proposal may be replaced
with the next nonce and a new vote set. A valid vote or execution after expiry
returns success with `hub.expired`, retains the expired proposal as evidence,
and adds no vote or action. A zero governance delay is valid and allows execution
as soon as quorum exists. The vote window must be positive: a zero-length window
admits no additional vote or execution. Wide offsets avoid rejection merely
because a proposal is created near the execution clock's `u32` limit; nonce
overflow still fails closed.

Governance never changes signer or member wallets, weights, or destinations, and offers no
principal withdrawal or arbitrary call. Native receipts still settle during a
pause. Governance has an explicit availability power: pausing or choosing an
unfunded reserve can prevent new claims.

## Vault wire layout

All fields are little-endian; addresses are DApp ID then account ID.

### Configuration - 2,400 bytes

| Offset | Bytes | Field |
| --- | --- | --- |
| 0 | 192 | Three signer wallet addresses |
| 192 | 4 | Governance delay |
| 196 | 4 | Vote window |
| 200 | 16 | Initial operating reserve (current reserve in persisted state) |
| 216 | 1 | Member count |
| 217 | 7 | Zero padding |
| 224 | 2,176 | Sixteen 136-byte member slots |

Each member slot contains wallet address (64), destination (64), weight (2),
padding (6). Unused slots are zero. Init extracts this configuration from the
admitted ACDP deployment envelope. Deploy is permissionless: the configuration,
governance and member wallets included, is part of `init_data` and so of the
account address. A redeploy with the same package, `owner_id`, salt and
configuration is the same address; a different configuration is a different
vault. No method changes the configuration after init except the operating
reserve slot, which governance controls.

### Persisted state - 4,192 bytes

| Offset | Bytes | Field |
| --- | --- | --- |
| 0 | 2,400 | Configuration; reserve slot is governance-controlled |
| 2,400 | 16 | Received principal |
| 2,416 | 16 | Total paid principal |
| 2,432 | 8 | Claim nonce |
| 2,440 | 8 | Proposal nonce |
| 2,448 | 1 | Paused flag |
| 2,449 | 7 | Zero padding |
| 2,456 | 64 | Active proposal or all-zero idle proposal |
| 2,520 | 1,664 | Sixteen 104-byte member accounting records |
| 4,184 | 4 | First-execution block-time observation |
| 4,188 | 4 | Zero padding |

Proposal fields: nonce (8), action (1), votes (1), padding (6), value (16), created
(8), ready (8), expires (8), padding (8).

Member accounting fields: claimable (16), paid (16), pending (16), pending nonce
(8), attempt LT boundary (8), latest callback creation LT (8), latest causal hash
(32). An idle member has zero pending amount/nonce/boundary. Accounting, nonce
bounds, padding, proposal validity, and active/unused slots are checked on decode
and every successful state output.

## Receiver fixture

The 224-byte receiver state holds its owner key (the deploy `owner_id`, at
`owner_key_offset` 0; binding, fault controls and status are that owner's
externals), audit address, once-bound
vault address, member index, last accepted claim nonce, received total, receipt
count, audit reserve, and rejection/duplicate fault controls.

Receipt body is `index:u8 + claim_nonce:u64 + amount:u128` (25 bytes). A receiver
accepts only a regular internal transfer from its bound vault with a callback
request and exact value. Successful receipt returns that body as acknowledgment
and queues a principal-free report to the audit ledger.

The owner can deliberately reject transfer or emit the audit report twice. These
are test-fixture controls; they do not withdraw money, rebind the source, or alter
the received principal. The receiver has no general wallet withdrawal method.

## Audit ledger

The state is 65,540 bytes: a canonical 4-byte length prefix followed by a 65,536-byte
payload. The owner key, the deploy `owner_id`, is at offset 4, the
`owner_key_offset`; binding and status are that owner's externals; member count at 36; source bindings begin at 68.
Sixteen optional source-address slots are followed by reserved zero data.
Records begin at offset 2,052; each is 32 bytes:

`last_nonce:u64 + received_total:u128 + receipt_count:u64`.

Only the owner can bind each member index once to a distinct
receiver in the same DApp. Reports must have the correct index/source, regular
internal method, zero transferred principal, no extras, and an increasing nonce.

The audit is a message-isolated contract: each report runs in its own call. A
valid report patches only its member's 32-byte record and emits one event; an
invalid report fails with its own status and has no effect. AccountBatch
execution is not supported by the node yet; a message to such code ends with
status 21.

External setup/status and internal reports support the state-update mode
requested by the VM.

Audit is an asynchronous receipt log, not a condition for financial settlement.
If a queued report later fails, the log can lag a paid claim; no recovery service
is implemented. The E2E test explicitly reconciles its successful journey.

## Output measurements and build admission

The official builder checks two Rust compilations for reproducibility. Deployment
then uses a pinned Binaryen optimization, run twice independently and compared
byte-for-byte. Original compiler and optimized artifacts are retained with SHA-256
hashes. Actual optimized artifacts are deployed and executed in the local stand.

## Error ranges

| Codes | Scope |
| --- | --- |
| 3000–3009 | Arguments, authorization, state, pause, value, pending/empty claims, callbacks, overflow, balance |
| 3010–3014 | Proposal validity, duplicate vote, timelock, reserved expiry code, missing quorum |
| 3100–3102 | Receiver authority, invalid receipt, deliberate rejection |
| 3200–3203 | Audit arguments, source authority, receipt replay, arithmetic overflow |

ABI metadata names every code. Failed isolated operations return no contract
state update or effects; surrounding runtime execution fees may still apply.
