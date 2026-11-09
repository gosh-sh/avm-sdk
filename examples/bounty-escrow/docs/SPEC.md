# Contract specification

## Authority and asset model

The configured owner approves work and requests expired refunds. The configured
worker submits work. Either configured party triggers an approved payout. The
owner calls by external messages under its owner key and funds through its own
wallet, because an external message carries no value. The worker calls through
its own wallet, so the escrow sees its wallet address as an internal caller.
Status accepts any caller; SHELL fee donations accept any internal caller.
Business operations reject every other caller.

The escrow keeps the owner key in the first config field, at state offset 0,
and its descriptor declares `owner_key_offset = 0`: the node admits an external
call to the escrow only when it is signed by that key. Init requires the key to
equal the deploy `owner_id`. The receiver fixtures keep the deploy `owner_id` at
state offset 0 the same way and accept bind and reject only from that key. The
tested stand deploys the escrow and both receivers with the developer identity
as `owner_id`, and one wallet per identity: developer (owner), user (worker) and
package-owner (a foreign wallet).

Principal uses the example's explicit `PRINCIPAL_CURRENCY_ID = 0` ECC entry.
This is an application choice for new instances, not a token assignment or a
conversion/identification of the removed unnamed main asset. A single exact funding message must supply the sum
of milestone amounts before any work is accepted. The account balance must cover
all retained principal when dispatching a transfer. The receiver compute reserve
uses separately funded SHELL, never principal. The precheck is not a full fee
estimator: the runtime still validates fixed delivery/callback fees and rent.

## Milestone state transitions

| Phase | Operation | Next phase |
| --- | --- | --- |
| Open | Worker submits nonzero hash before deadline | Submitted |
| Submitted | Worker revises hash before deadline | Submitted with new hash |
| Submitted | Owner approves exact current hash before deadline | Approved |
| Approved | Owner/worker requests payment | Approved, transfer pending |
| Approved and pending | Successful authenticated receipt | Paid |
| Approved and pending | Failed receipt returns exact principal | Approved, idle |
| Open/Submitted after expiry | Owner requests refund | Same phases, refund pending |
| Refund pending | Successful receipt | All remaining Open/Submitted become Refunded |
| Refund pending | Failed receipt returns exact principal | Original phases, idle |

The deadline is exclusive for funding, submission, and approval: those operations
require `block_time < deadline`. Refund requires `block_time >= deadline`.
Approved work may be paid at any later time. Any unresolved transfer blocks all
business mutations; status and fee donations remain available.

## Accounting

For a funded escrow:

`funded = paid + refunded + sum(amount of every milestone not Paid or Refunded)`.

A pending transfer remains a liability until its callback settles. Its amount
must match either one approved milestone or the entire remaining unapproved
balance. Success updates the corresponding paid/refunded phase and counter;
failure must return exact principal and leaves phases/counters unchanged. These
checks are applied on every state decode and every successful state output.

All amounts are unsigned 128-bit integers with checked arithmetic. Wire fields
are little-endian. State uses a fixed length and enforces zero padding, valid
phases, nonzero active delivery hashes, and a canonical receipt ledger.

## Config wire layout (456 bytes)

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 32 | Client owner key (`owner_key_offset = 0`), equal to the deploy `owner_id` |
| 32 | 64 | Client wallet: DApp ID then account ID |
| 96 | 64 | Worker wallet: DApp ID then account ID |
| 160 | 64 | Refund destination: DApp ID then account ID |
| 224 | 64 | Payment destination: DApp ID then account ID |
| 288 | 16 | Total principal |
| 304 | 4 | Deadline (AVM logical block time) |
| 308 | 1 | Milestone count, 1 through 8 |
| 309 | 3 | Zero padding |
| 312 | 128 | Eight milestone amounts; unused entries zero |
| 440 | 16 | Nonzero outbound SHELL compute reserve |

The owner key and both wallets are nonzero and the two wallets differ.

Initialization extracts this config from the canonical ACDP deployment body.
It does not confuse the deployment envelope with raw configuration. The ACDP
parser validates its declared variable-section lengths and zero reserved bytes.
Runtime package/hash/signature validation remains the runtime's responsibility.

## State wire layout (2,864 bytes)

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 456 | Immutable config |
| 456 | 16 | Funded principal (zero or exact total) |
| 472 | 16 | Paid principal |
| 488 | 16 | Refunded principal |
| 504 | 8 | Total dispatch attempts |
| 512 | 1 | Pending kind: idle 0, payment 1, refund 2 |
| 513 | 1 | Pending milestone index; refund uses 255 |
| 514 | 6 | Zero padding |
| 520 | 16 | Pending principal |
| 536 | 8 | Pending attempt number |
| 544 | 8 | Milestone phases (0/1/2/3/4) |
| 552 | 256 | Eight delivery hashes |
| 808 | 8 | Number of accepted callback causal hashes |
| 816 | 2,048 | Up to 64 accepted causal hashes; unused slots zero |

Idle pending fields must all be zero. A pending attempt is the most recent
dispatch. Receipt count equals attempts minus one when pending, and equals
attempts when idle. Accepted receipt hashes are nonzero and distinct.

## Method bodies

| Method | Body | Authority |
| --- | --- | --- |
| `bounty.escrow.fund` | Empty; exact total as the sole ECC 0 entry | Client wallet |
| `bounty.escrow.submit` | Index (1 byte) + delivery hash (32 bytes) | Worker wallet |
| `bounty.escrow.approve` | Index + exact submitted hash | Client owner key |
| `bounty.escrow.pay` | Index (1 byte) | Client owner key or worker wallet |
| `bounty.escrow.refund` | Empty | Client owner key |
| `bounty.escrow.status` | Empty | Any caller |
| `bounty.escrow.topup` | Empty; only transferred SHELL (ECC 2) | Any internal caller |

No extra principal or currencies are accepted by business/status operations.
There is no code-upgrade entrypoint. Method/event IDs in `ids.rs` are extracted
from the official builder's canonical ACAB by `scripts/sync_ids.py`.

## Payment body and receipt (32 bytes)

`BEP1` (4 bytes), kind (1), index (1), zero padding (2), attempt (u64), amount (u128).

The destination must receive an ordinary internal message with a callback
request, from its once-bound escrow. A success receipt echoes the exact body.
Native callback processing checks receiver address, original sender address,
handler method, callback flag/header, fresh nonzero causal hash, and correct
value. Success carries no returned principal; failure has an empty body and
returns the exact pending principal. Success accepts only optional returned SHELL (ECC 2); failure accepts the exact
pending ECC 0 amount plus optional returned SHELL. Other currencies are rejected.

The contract cannot observe the runtime-materialized outgoing message hash at
dispatch time. It therefore relies on AVM-native callback provenance, one pending
transfer, and the stored accepted receipt hashes. It does not claim to authenticate
arbitrarily supplied synthetic callback bytes outside that runtime trust model.

## Error codes

| Code | Meaning |
| --- | --- |
| 2000 | Invalid method/arguments/config |
| 2001 | Unauthorized caller |
| 2002 | Malformed or inconsistent state |
| 2003 | Operation incompatible with phase/funding |
| 2004 | Unexpected transferred value/currency |
| 2005 | Deadline condition failed |
| 2006 | Another transfer is unresolved |
| 2007 | Invalid, stale, or mismatched callback |
| 2008 | 64-attempt bound reached |
| 2009 | Insufficient retained principal or SHELL reserve |
| 2100 | Receiver authorization failed |
| 2101 | Receiver state/body/binding invalid |
| 2102 | Receiver deliberately rejects delivery |

Failures produce no contract state update, outbound intent, or contract event.
The surrounding AVM transaction may still charge execution fees.

## Application limits

The receiver retains funds and receipt evidence for testing; it does not offer
wallet withdrawal. The escrow has no arbitration, partial funding, cancellation
before expiry, asset adapters, gas withdrawal, or administrative recovery. Both
the 64-attempt cap and an indefinitely missing native callback can lock funds;
those are explicit bounds of this SDK fixture. Do not fund it on mainnet without
an application design, independent review, and further runtime validation.
