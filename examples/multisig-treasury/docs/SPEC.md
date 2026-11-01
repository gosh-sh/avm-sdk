# Multisig Treasury – specification notes

## Lifecycle
`Open` (collecting approvals, deadline `created + ttl`) → `Queued` (approvals ≥
threshold, `eta = queue time + delay`, deadline `eta + grace`) → `Executing`
(transfers only, payout in flight) → removed. Cancelled and expired proposals are
removed by `cancel`. Executing proposals can be neither revoked nor cancelled.

## Invariants (checked on every decode and before every state output)
* `1 ≤ threshold ≤ owners ≤ 8`, owner keys unique and nonzero, `delay ≤ 30 d`.
* Balances: unique currency, no zero entries (emptied entries are dropped).
  Distinct currencies in balances, pending payouts and `{SHELL}` total at most 8.
  SHELL and possible principal refunds retain capacity even without a balance row.
  New credits enforce the same reservation limit: at most seven distinct
  non-SHELL currencies may be held or pending. Previously overcommitted states
  are rejected; this example provides no migration for existing accounts.
* Policies: unique currency, never SHELL, `fast ≤ window limit`, `spent ≤ window limit`.
* Proposals: ids strictly increasing and below `next_id`; kind 1..7; approvals only
  from occupied owner slots (except executing proposals); `Queued` ⇒ approvals ≥
  threshold; `Open` ⇒ `eta = 0`; every `Executing` proposal has exactly one pending payout.
* Pending payouts: unique recipients, amount > 0, never SHELL, linked to an executing proposal.
* Serialized table counts cannot exceed their respective maxima; unused table
  slots and padding are zero. These structural checks do not authenticate the
  historical provenance of arbitrary state images.

## Value flow
`deposit` credits `held[currency]`. A transfer debits `held` and the FUEL reserve
(SHELL) before the message leaves; SHELL that comes back in a callback is credited.
Conservation, checked by the fuzz test after every step: `deposited == held +
in_flight + delivered` per currency.

## Callbacks
Accepted only from the pending recipient, with a nonzero cause hash not seen in the
last 8 callbacks, no callback request, method `treasury.callback`. Success: body must
equal the receipt and no principal may come back. Failure: the exact principal must
come back with an empty body. Anything else is `ERR_CALLBACK` and the payout stays
pending, with no timeout or recovery operation. Callbacks skip the solvency check
(their own value arrives with them).

## Reconciliation
`reconcile` runs after owner removal, threshold changes, revokes and failed
payouts: it masks approvals to current owners, reopens queued proposals that lost
approvals and queues open ones that now have enough.
