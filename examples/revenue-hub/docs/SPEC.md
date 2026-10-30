# Revenue Hub specification

## Money model and invariant

Principal is the explicit application ECC `PRINCIPAL_CURRENCY_ID = 0`;
operational SHELL is ECC 2. This is a new-instance example choice, not a live
token denomination or a mapping/conversion of the former unnamed main asset.
A deposit requires positive principal and may also donate positive SHELL; no
other ECC is accepted. Membership is fixed on initialization: each of 1–16
members has a unique key, a unique recipient address, and positive basis-point
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

## Claim lifecycle

1. The member signs the index of their own claim. Amount and destination are
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

Three unique public keys are fixed at deployment. Supported actions are:

| Action | Value | Effect |
| --- | --- | --- |
| 1 — Pause | Zero | Reject new deposits and claims |
| 2 — Resume | Zero | Allow new deposits and claims |
| 3 — Set operating reserve | Positive u128 | Set outgoing receiver FUEL reserve in SHELL |

Proposal body: `nonce:u64 + action:u8 + value:u128`, all little-endian (25 bytes).
The nonce must be the next monotonic proposal nonce. A proposer contributes one
vote. Another signer must approve the identical full body. A key can vote once;
votes are represented as a three-bit set.

`ready = created_block_time + configured_delay`.

`expires = ready + configured_vote_window`.

Execution is allowed for any external signer when at least two votes exist and
`ready <= block_time < expires`. An executed proposal is cleared and cannot be
replayed. A live proposal cannot be replaced; an expired proposal may be replaced
with the next nonce and a new vote set. Overflow of timestamps/nonces is rejected.

Governance never changes member keys, weights, or destinations, and offers no
principal withdrawal or arbitrary call. Native receipts still settle during a
pause. Governance has an explicit availability power: pausing or choosing an
unfunded reserve can prevent new claims.

## Vault wire layout

All fields are little-endian; addresses are DApp ID then account ID.

### Configuration — 1,792 bytes

| Offset | Bytes | Field |
| --- | --- | --- |
| 0 | 96 | Three signer public keys |
| 96 | 4 | Governance delay |
| 100 | 4 | Vote window |
| 104 | 16 | Initial operating reserve (current reserve in persisted state) |
| 120 | 1 | Member count |
| 121 | 7 | Zero padding |
| 128 | 1,664 | Sixteen 104-byte member slots |

Each member slot contains key (32), destination (64), weight (2), padding (6).
Unused slots are zero. Init extracts this configuration from the admitted ACDP
deployment envelope; it requires a signer from the configured governance set.

### Persisted state — 3,560 bytes

| Offset | Bytes | Field |
| --- | --- | --- |
| 0 | 1,792 | Configuration; reserve slot is governance-controlled |
| 1,792 | 16 | Received principal |
| 1,808 | 16 | Total paid principal |
| 1,824 | 8 | Claim nonce |
| 1,832 | 8 | Proposal nonce |
| 1,840 | 1 | Paused flag |
| 1,841 | 7 | Zero padding |
| 1,848 | 48 | Active proposal or all-zero idle proposal |
| 1,896 | 1,664 | Sixteen 104-byte member accounting records |

Proposal fields: nonce (8), action (1), votes (1), padding (6), value (16), created
(4), ready (4), expires (4), padding (4).

Member accounting fields: claimable (16), paid (16), pending (16), pending nonce
(8), attempt LT boundary (8), latest callback creation LT (8), latest causal hash
(32). An idle member has zero pending amount/nonce/boundary. Accounting, nonce
bounds, padding, proposal validity, and active/unused slots are checked on decode
and every successful state output.

## Receiver fixture

The 224-byte receiver state holds its init authority, audit address, once-bound
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
payload. Owner is at offset 4; member count at 36; source bindings begin at 68.
Sixteen optional source-address slots are followed by reserved zero data.
Records begin at offset 2,052; each is 32 bytes:

`last_nonce:u64 + received_total:u128 + receipt_count:u64`.

Only the initialization authority can bind each member index once to a distinct
receiver in the same DApp. Reports must have the correct index/source, regular
internal method, zero transferred principal, no extras, and an increasing nonce.

`process_batch` supports up to 256 canonical messages. Valid reports update a
staged record set; invalid reports receive individual failure statuses and have
no effect. Multiple valid reports to one member coalesce into one final patch.
Patches are sorted by member index, and events retain the original input index.

External setup/status and internal isolated fallback support the state-update
mode requested by the VM. The fallback and batch test compare resulting ledgers.

Audit is an asynchronous receipt log, not a condition for financial settlement.
If a queued report later fails, the log can lag a paid claim; no recovery service
is implemented. The E2E test explicitly reconciles its successful journey.

## Output measurements and build admission

`batch_probe` compares actual canonical batch patch output against a hypothetical
full-replace serialization with the same event/status sections. That hypothetical
representation is a byte-size comparison, not an admitted alternate output mode
for this DataPatchList batch execution. No CPU, FUEL, or throughput comparison is
claimed.

The official builder checks two Rust compilations for reproducibility. Deployment
then uses a pinned Binaryen optimization, run twice independently and compared
byte-for-byte. Original compiler and optimized artifacts are retained with SHA-256
hashes. Actual optimized artifacts are deployed and executed in the local stand.

## Error ranges

| Codes | Scope |
| --- | --- |
| 3000–3009 | Arguments, authorization, state, pause, value, pending/empty claims, callbacks, overflow, balance |
| 3010–3014 | Proposal validity, duplicate vote, timelock, expiry, missing quorum |
| 3100–3102 | Receiver authority, invalid receipt, deliberate rejection |
| 3200–3203 | Audit arguments, source authority, receipt replay, arithmetic overflow |

ABI metadata names every code. Failed isolated operations return no contract
state update or effects; surrounding runtime execution fees may still apply.
