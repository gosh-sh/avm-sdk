# Vesting stream protocol

## Calendar and authority

All schedule timestamps are `u32` AVM block times. Positive start must precede end;
start <= cliff <= end. A zero step means linear accrual; otherwise step must be
positive and no longer than the duration. Before cliff, earned is zero. At and
after end, earned is the entire amount. Between these bounds, elapsed is time
since start, optionally rounded down to a complete step, and:

`earned = floor(total * elapsed / (end - start))`.

The implementation computes quotient and remainder first, avoiding a potentially
overflowing product. Remainder and elapsed are both below `2^32`, so their product
fits `u128`. Observed time never decreases across successful contract messages.

The configured owner must authenticate initialization and exact one-time funding
before the earliest start. Beneficiary public keys authenticate their own claims.
Owner cancellation is available only for revocable, unfinished, uncancelled
streams. It freezes earned at the cancellation time and leaves pending payments
valid. Refund entitlement is total minus the frozen earned amount. There is no
owner sweep, beneficiary replacement, or destination-update method.

## Accounting and asynchronous payments

For each stream:

`total = held + claimed + refunded + pending_claim + pending_refund`.

The beneficiary and refund lanes each hold at most one pending payment. Dispatch
reserves the entire amount currently due and a global increasing `u64` nonce.
Native success settles the appropriate paid counter. Native failure restores the
entitlement only when its returned main value equals the pending amount and the
body is empty. Invalid messages have no state, event, or outbound effects.

Every payout and refund destination is distinct across all streams, including
zero-entitlement refund destinations of irrevocable streams. This lets the vault
identify a failed callback's lane without relying on a success body. All destinations
belong to the vesting account's DApp and differ from the vesting account itself.
Success echoes the 26-byte receipt: index `u8`, lane `u8`, nonce `u64`, amount `u128`.

Callbacks must be native internal messages from the bound lane destination to
the callback handler, with no callback request or extra currencies. Cause address
is the vesting account, cause hash is nonzero and differs from the last receipt,
and created logical time must exceed the dispatch boundary and last receipt time.
Successful receipts must echo the active nonce and amount and carry zero main value.
This relies on authentic VM-generated callbacks; the contract does not store every
historic receipt hash or an outbound hash unavailable at intent creation. Global
nonce exhaustion fails closed. No timeout clears an unresolved payment.

Principal accounting excludes SHELL, currency id 2, used for execution and delivery.
Dispatch requires the configured SHELL reserve. `topup` accepts only positive SHELL
extra currency and zero principal. Fee exhaustion cannot enlarge entitlement.

## Wire format

All integers are little-endian; addresses are 32-byte DApp id plus 32-byte account id.
Padding and inactive rows must remain zero. State version is identified by descriptor
schema id and exact byte length. No migration or upgrade handler is implemented.

Configuration: 1,656 bytes. Owner at 0 (32 bytes), fuel at 32 (16 bytes), count at
48 (one byte), padding through 55. Eight 200-byte rows start at 56:

| Row offset | Length | Meaning |
| --- | ---: | --- |
| 0 | 32 | Beneficiary public key |
| 32 | 64 | Payout address |
| 96 | 64 | Refund address |
| 160 | 16 | Total principal |
| 176 / 180 / 184 / 188 | 4 each | Start / cliff / end / step |
| 192 | 1 | Revocable flag |
| 193 | 7 | Zero padding |

State: 3,272 bytes. Configuration at 0, funded flag at 1,656, zero padding to
1,659, observed time at 1,660 (4 bytes), global nonce at 1,664 (8 bytes).
Eight 200-byte progress rows start at 1,672:

| Row offset | Length | Meaning |
| --- | ---: | --- |
| 0 | 1 | Cancelled flag |
| 1 | 3 | Zero padding |
| 4 | 4 | Cancellation time |
| 8 | 16 | Frozen earned cap |
| 24 / 40 | 16 each | Claimed / refunded |
| 56 / 128 | 72 each | Beneficiary / refund lane |

Each lane stores pending amount (16), active nonce (8), dispatch logical-time
boundary (8), last accepted receipt logical time (8), and last receipt hash (32).
An idle lane has zero pending, active nonce, and dispatch boundary.

Receiver fixture state is 368 bytes: owner (32), bound source (64), reject flag,
lane, index, five padding bytes, eight 32-byte records, and eight reserved bytes.
Each record is last nonce (8), received principal (16), successful receipt count (8).
Only the configured index may receive. Binding is owner-only and once. Receivers
require authenticated internal payment with native callback request, exact
principal, increasing nonce, and matching lane/index; successful payments echo
the receipt. They retain principal and provide no withdrawal operation.

## Methods and events

Vesting methods: `stream.vesting.fund`, `.claim`, `.cancel`, `.refund`, `.callback`,
`.status`, `.topup` (prefix shared). Claim/cancel/refund take one index byte.
Funding/status/topup take no body. Receiver methods: `stream.receiver.bind`
(64-byte address), `.reject` (boolean byte), `.receive` (26-byte receipt), `.status`.
Initializers take canonical ACDP envelopes containing configuration bytes.

Events: `stream.created`, `.funded`, `.cancelled`, `.pending`, `.settled`, `.failed`,
`.status`, `.fees`. Fields are amount, claimed, held, index, kind, nonce, pending,
refunded, time, using aggregate accounting. Index/kind 255 denote a global or
non-lane event. Exact identifiers and declared errors are in `avm-contract.toml`
and the generated canonical `core/src/ids.rs`.
