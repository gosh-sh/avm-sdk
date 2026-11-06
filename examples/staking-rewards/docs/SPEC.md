# Staking Rewards Distributor — Specification

A single-pool staking contract. Stakers deposit a currency `S`, accrue
a share of a separately-funded reward currency `R` proportional to
their stake, and withdraw via `UNSTAKE`/`CLAIM`. Accumulator-per-share
pattern: a single monotonic `acc_reward_per_share`, scaled by
`PRECISION`, lets every row compute its own reward in O(1).

`core` (`core/src/lib.rs`) is a pure, byte-level state machine with no SDK
dependency — it knows nothing about `OutboundIntent`/`CallbackIntent`/
`Caller` or any other platform type. `contracts/staking-rewards/src/lib.rs`
is the only place that imports an SDK type; it decodes the wire message,
calls into `core`, and constructs the platform-level effects (currency
transfers, callbacks). `contracts/test-receiver/` is a test-only fixture,
never used by production.

Local synthetic-funds example only. No production deployment or audit
claim.

## State layout

Header, 100 bytes, all fields little-endian:

| Offset | Bytes | Field | Type |
|---|---|---|---|
| 0 | 16 | `total_staked` | u128 |
| 16 | 16 | `acc_reward_per_share` | u128 |
| 32 | 4 | `stake_currency_id` | u32 |
| 36 | 4 | `reward_currency_id` | u32 |
| 40 | 16 | `reward_pool_held` | u128 |
| 56 | 16 | `min_stake` | u128 |
| 72 | 4 | `row_count` | u32 |
| 76 | 8 | `next_pending_id` | u64 |
| 84 | 16 | `fuel_reserve` | u128 |

Followed by `row_count` rows, 113 bytes each, sorted ascending by
`account` (the sort is load-bearing: every lookup except the callback fast
path uses `binary_search_by`):

| Offset | Bytes | Field | Type |
|---|---|---|---|
| 0 | 32 | `account` | `[u8; 32]` |
| 32 | 16 | `staked` | u128 |
| 48 | 16 | `reward_debt` | u128 |
| 64 | 8 | `seq` | u64 |
| 72 | 1 | `pending` flag (0 = Idle, 1 = PendingOut) | u8 |
| 73 | 8 | `pending_id` (0 if Idle) | u64 |
| 81 | 16 | `amount_s` (0 if Idle) | u128 |
| 97 | 16 | `amount_r` (0 if Idle) | u128 |

A row is deleted once `staked == 0 && pending == Idle` simultaneously
(never while a payout is in flight: the failure-restore path still needs
the row's own `staked`/`reward_debt` to be exactly what they were at
request time). Deleting a row resets its implicit `seq` to 0 on the next
`STAKE`.

## Constants

| Constant | Value | Why |
|---|---|---|
| `MAX_AMOUNT` | `2^60` | Per-row/global ceiling on `S` (see capacity table). |
| `MAX_ACC` | `2^64` | Ceiling on `acc_reward_per_share`; one past `u64::MAX`, which is why `acc` is a `u128` field, not a `u64` one. |
| `PRECISION` | `2^44` | Fixed-point scale for `acc_reward_per_share`. |
| `DEFAULT_MIN_STAKE` | `10^6` | Test-fixture default; `min_stake` is a real per-deployment init parameter, not a fixed constant. |
| `DEFAULT_FUEL_RESERVE` | `1` | Test-fixture default; see "Fee model" for the measured floor this is grounded in. |
| `MAX_ROWS` | `140` | Row-table cap; see "Capacity and fuel". |
| `SHELL_CURRENCY_ID` | `2` | Platform-fixed (`fuel-and-fees.md:16`); `stake_currency_id`/`reward_currency_id` must differ from it (see "Currency rules"). |

`A=60, C=64, P=44` satisfy the SDK's own `AvmAmount = u128` constraints
(`A+C<=127`, `A+P<=127`, from `crates/avm-contract-sdk/src/lib.rs:34,55`)
with 3 and 23 bits of headroom respectively.

## Message bodies

Fields are little-endian, in the order listed. `AccountAddress` is
`dapp_id:[u8;32] | account_id:[u8;32]`, 64 bytes.

| Method | Bytes | Fields | Required attachment |
|---|---|---|---|
| `init_data` (deploy) | 40 | `stake_currency_id:u32 \| reward_currency_id:u32 \| min_stake:u128 \| fuel_reserve:u128` | none |
| `STAKE` | 24 | `expected_seq:u64 \| amount:u128` | exactly `amount` of `S` |
| `UNSTAKE` | 88 | `expected_seq:u64 \| amount:u128 \| destination:AccountAddress` | exactly `fuel_reserve` of SHELL |
| `CLAIM` | 72 | `expected_seq:u64 \| destination:AccountAddress` | exactly `fuel_reserve` of SHELL |
| `FUND_REWARDS` | 16 | `amount:u128` | exactly `amount` of `R` |

`init_data` validation (`core::State::new`, re-checked on `decode`):

- `stake_currency_id != reward_currency_id`.
- `1 <= min_stake <= MAX_AMOUNT`.
- `1 <= fuel_reserve <= MAX_AMOUNT` (the lower bound is a measured
  platform floor; see "Fee model").
- Neither currency id may equal `SHELL_CURRENCY_ID` (see "Currency
  rules").

No attached currency is accepted at deploy time.

## Currency rules

- `STAKE` requires exactly `S` attached, matching the body's own `amount`.
- `FUND_REWARDS` requires exactly `R` attached, matching its own `amount`.
- `UNSTAKE`/`CLAIM` require exactly `fuel_reserve` of SHELL attached and
  nothing else (see "Fee model").
- Any missing/wrong-currency/wrong-amount/extra-currency combination is
  rejected with `WRONG_CURRENCY`.
- `S`/`R` may not equal `SHELL_CURRENCY_ID`. SHELL is the platform's own
  fee/rent currency (`fuel-and-fees.md:16`): a pool denominated in it
  would mix stake/reward accounting with the currency this contract's own
  outbound payouts spend as `fuel_fee_shells`, and with whatever the
  account's own rent debits (`runtime-limits.md:70-98`).
- Currency id `0` is not special-cased or rejected: `fuel-and-fees.md:8`
  states plainly that "Currency ID 0 is valid; it has no special role," so
  `0` is accepted as an ordinary `S`/`R` choice.

## Per-method equations

Let `acc` = current `acc_reward_per_share`.

**`STAKE(expected_seq, amount)`** (synchronous, no payout): requires
`check_seq` and exactly `amount` of `S` attached.
`accrued_now = floor(staked_old * acc / PRECISION)`; `settle = max(0,
accrued_now - reward_debt_old)`. `staked_new = staked_old + amount`,
rejected unless `row_exposure + amount <= MAX_AMOUNT` and
`global_exposure + amount <= MAX_AMOUNT`, where `exposure = staked +
(pending.amount_s if pending else 0)` — checked against committed
*exposure*, not bare `staked`/`total_staked`, so a later failed-payout
restore (which only ever returns exposure to settled `staked`) can never
push either ceiling over `MAX_AMOUNT`. Also rejected unless `staked_new >=
min_stake`. `reward_debt_new = ceil(staked_new * acc /
PRECISION).checked_sub(settle)` — `ceil`, not `floor`, for the new
baseline specifically, so a staker can never retroactively harvest a
fractional sliver of the rate that existed before their top-up; this
carries at most 1 raw unit of slack forward, recoverable at the next
`CLAIM`/`UNSTAKE`.

**`UNSTAKE(expected_seq, amount, destination)` / `CLAIM(expected_seq,
destination)`** (request time, triggers a combined payout): require
`check_seq`, exactly `fuel_reserve` of SHELL attached, and `pending ==
Idle`. `accrued_now = floor(staked_old * acc / PRECISION)`; `amount_r =
max(0, accrued_now - reward_debt_old)` (full auto-claim); `amount_r_final
= min(amount_r, reward_pool_held)` (the principal-independence clamp; see
invariant 14). For `UNSTAKE`: `staked_new = staked_old - amount`, rejected
unless `staked_new == 0 || staked_new >= min_stake`; applied now, not
deferred (`total_staked -= amount` now too). For `CLAIM`: `amount_s = 0`,
`staked` untouched, and the call is rejected if `amount_r_final == 0`
after the clamp (invariant 16; `UNSTAKE` is exempt by invariant 14).
`reward_debt_new = floor(staked_new * acc / PRECISION)` — exact, zero
slack, since it's the same expression a future read uses at the same
`acc`. `reward_pool_held -= amount_r_final`. A fresh `pending_id` is
assigned (see "Callback model"); state records `pending =
PendingOut{pending_id, amount_s, amount_r: amount_r_final}`. The contract
layer emits one combined `OutboundIntent{S: amount_s, R: amount_r_final}`
+ `CallbackIntent`, with `fuel_fee_shells = fuel_reserve`.

**`FUND_REWARDS(amount)`** (permissionless — no caller-identity check at
all, by design; anyone may fund with their own `R`): rejected if
`total_staked < min_stake` (no stakers to distribute to) or `amount == 0
|| amount > MAX_AMOUNT`. `delta = floor(amount * PRECISION /
total_staked)`, rejected outright if `delta == 0` (a funding amount too
small to move the accumulator at all against the current `total_staked`
contributes nothing and must not silently succeed). `candidate = acc +
delta`, rejected if `candidate > MAX_ACC`. `reward_pool_held += amount`;
`acc_reward_per_share = candidate`.

**Payout success:** `pending = Idle`. Nothing else — every other field
was already applied at request time. If `staked == 0`, delete the row.

**Payout failure** (the delta-rebase restore): let `staked_remaining` and
`reward_debt_current` be the row's current (unchanged since request)
fields. `staked_restored = staked_remaining + amount_s`.

```text
window_term = floor(staked_restored * acc / PRECISION)
            - floor(staked_remaining * acc / PRECISION)
reward_debt_restored = window_term + reward_debt_current - amount_r
```

(clamped to `0` if this would go negative — a staker-favoring clamp, by
exactly the clamped amount). Never observed at real constants
(`restore_clamp_count == 0` across a 100,000-sequence/2,500,729-step
fuzz run and 4,500,000 adversarial operations); does trigger at tiny
ones (`MIN_STAKE=2`, `MAX_AMOUNT=10`, not realistic values): an
exhaustive DFS to depth 8 hits it 704 times, max magnitude exactly `1`
raw unit, no invariant violation. See "Evidence" for exact commands.
`staked = staked_restored`; `total_staked += amount_s`;
`reward_pool_held += amount_r`; `pending = Idle`.

This is exact because `window_term` is computed from the same
`staked_remaining` on both sides of the subtraction: it credits exactly
the remaining portion's own already-correct window earning, nothing more.
The *returning* principal earns nothing from the window it was excluded
from — not a penalty, simply a reflection of it genuinely not being part
of `total_staked` during that window. A naive formula
(`reward_debt = floor(staked_restored * acc / PRECISION) - amount_r`)
would instead implicitly grant the *entire* restored stake — including
the portion that was excluded from `total_staked` for the whole pending
window — a share of any `FUND_REWARDS` that happened during that window,
which is wrong.

## Invariants

Solvency:
1. `total_staked + sum(amount_s over all PendingOut rows) ==` cumulative
   `S` credited via `STAKE` minus cumulative `S` paid out via settled
   `UNSTAKE` (exact).
2. `sum(pending_reward(row) for all Idle rows) <= reward_pool_held +
   ROUNDING_TOLERANCE`, where `ROUNDING_TOLERANCE = 1` raw unit of `R`.
   This is an empirical finding (confirmed flat, zero violations, across
   4,500,000 adversarial operations targeting exactly this bound -- see
   "Evidence"), not an analytic proof.

Core:
3. `acc_reward_per_share` is monotonically non-decreasing and never
   exceeds `MAX_ACC`.
4. `row.staked * acc / PRECISION >= row.reward_debt - 1` (the `-1` is the
   same bounded slack as invariant 2).
5. `FUND_REWARDS` is atomic: currency credit, pool increase, and the
   `MAX_ACC`-guarded accumulator update happen together or not at all.
6. A staker can never claim or be credited another staker's
   `reward_debt`.
7. The accumulator is always read using the pre-change `total_staked`
   before a same-call `STAKE`/`UNSTAKE` changes it.
8. `UNSTAKE` auto-claims in the same request, as one combined
   `PendingOut`, never silently dropped.
9. Every computation uses `checked_mul`/`checked_div`/`checked_add`/
   `checked_sub` or an explicit, bounded, documented clamp — never a
   silent wrap.
10. A failed payout restores exactly, via the delta-rebase formula, not
    the naive formula.
11. `STAKE` and a partial `UNSTAKE` must each leave the row's own stake at
    exactly `0` or `>= min_stake` — covers both directions.
12. Per-`FUND_REWARDS`-call dust is at most `2^(A-P) = 65536` raw `R`
    units, worst case (measured exactly at this value, not below it --
    see "Evidence"). Cumulative and unrecoverable, never a solvency risk
    (invariant 2 is the real guarantee).
13. `expected_seq` matches the row's current `seq` exactly;
    `expire_at - block_time <= MAX_TTL = 300s`; `block_time <= expire_at`
    (checked against current time, not signing time).
14. `UNSTAKE` of principal succeeds whenever `pending == Idle`, the
    amount is valid, and the caller has attached exactly `fuel_reserve`
    of SHELL — independent of all reward-side state. The fee-reserve
    attachment is a separate precondition this invariant does not waive;
    principal independence is specifically about the *reward* side.
15. `total_staked <= MAX_AMOUNT` and every row's `staked <= MAX_AMOUNT` at
    all times, including across a failed-payout restore — enforced by
    admitting `STAKE` against total committed *exposure*, not bare
    `total_staked`.
16. `CLAIM` never creates a `PendingOut` with `amount_r == 0` — rejected
    outright if the pool-shortfall clamp would leave nothing to deliver.
    `UNSTAKE` is exempt by invariant 14.
17. `FUND_REWARDS` rejects outright if the computed accumulator increment
    is exactly `0`.

## Capacity and fuel

**Arithmetic bounds.** With `A=60, C=64, P=44`: lifetime capacity per
staked unit is `2^(C-P) ≈ 1.05M×`; per-call dust is `65,536` raw `R`
units worst case; cost to saturate `acc` scales linearly with
`total_staked` at attack time — `~1.05×10^12` raw `R` at cold start,
`~1.05×10^18` at an illustrative `T=10^12` pool (a scale assumption).
Seed-staking at launch removes the cheap cold-start window; never a
risk to existing principal or rewards either way.

**Row-table cap.** Measured on the real local stand with `MAX_ROWS=5000`
in `core` (so the real cap could not interfere), filling state until
each method genuinely fails with `loop_iters=0` (the method's own real
cost alone, no added work); see "Evidence" below for how this build was
run:

| Method | Last OK | First fail |
|---|---|---|
| `STAKE` (new row) | 522 | 523 |
| `STAKE` (top-up) | 522 | 523 |
| `UNSTAKE` | 523 | 524 |
| `CLAIM` | 523 | 524 |
| `FUND_REWARDS` | 523 | 524 |
| payout-success callback | 287 | 288 |
| payout-failure callback | 287 | 288 |

Every failure is genuine `VM_WASM_OUT_OF_FUEL`. The lowest ceiling is
287 (both callback paths); half of that is 143.5, so `MAX_ROWS <= 143`.
`MAX_ROWS = 140` satisfies that with margin, re-verified directly: each
of `STAKE`/`UNSTAKE`/`CLAIM`/`FUND_REWARDS`/payout-success/
payout-failure succeeds at exactly 140 rows, decoded state confirmed.
`avm-local-stand` exposes only 3 fixed external identities per
`dapp_id`, so 140 rows was reached by synthesizing rows directly
through the real `core` API inside one WASM execution, not 140 separate
signed calls — still a real execution of the real, unmodified crate.
`TOO_MANY_ROWS` itself was confirmed separately with `MAX_ROWS=2` and
the 3 real identities: 2 fill the table, the 3rd is cleanly rejected.

**Callback fuel budget is a constant, not derived from any declared
reserve.** Varying the `OutboundIntent`'s own `fuel_fee_shells` (0 to
10^15) and the triggering external call's own declared value, at a
fixed workload, produced byte-identical pass/fail outcomes and
byte-identical bisected margins every time (54,312/54,281 at 1 row,
28,812/28,781 at 140 rows). `fuel_reserve` can only ever fund the
forward delivery leg, never the callback leg.

## Fee model

**Rules.** `UNSTAKE`/`CLAIM` attach exactly `fuel_reserve` of SHELL. The
contract layer's `combined_payout` uses `state.fuel_reserve` — never
the caller's own declared `ext.fuel_fee_shells` — as the resulting
`OutboundIntent`'s own `fuel_fee_shells`, paired with a `CallbackIntent`
that is *always* present (`contracts/staking-rewards/src/lib.rs:173`).
**Guidance: assume `fuel_reserve` is fully consumed on every call; do
not rely on any portion of it coming back.**

**Measured facts**, all on the real local stand (see "Evidence" for
exact commands/out-of-tree status):

| Fact | Measurement |
|---|---|
| Outbound floor | `fuel_fee_shells = 0` on the outbound → `RouteRequiresFee` before receiver WASM starts; `1` succeeds against `test-receiver` at every tested row count. |
| External-call charge vs. declared fee | `~2` SHELL at declared `1`; `~101` at declared `100`; exactly `10,000` at declared `10,000` — rent held at `0` (identical `block_time`), charged whether the call succeeds or is contract-rejected. |
| One-call drain | Funded a pool with `N=10,000`, one rejected call declaring fee `N-5`: ~9,997 SHELL charged, `3` left. Declaring `N-1,000`: ~9,002 charged, `998` left. A single call can take essentially the whole balance. |
| Callback budget | Constant regardless of declared `fuel_fee_shells` — see "Capacity and fuel". |
| Rent | `new_rent = (canonical bytes + overhead) * storage_price_per_byte_second * (block_time - storage_last_paid)` (`runtime-limits.md:70-98`), collected before WASM/FUEL; third consecutive unpaid attempt freezes the account. Local dev values only (price `1`, threshold `3`). |
| Documented refund rule | `fuel-and-fees.md:104-107`: "if a callback was requested, unused reserve is returned to the original sender through the callback... without a callback, unused reserve is burned." This contract always requests a callback, so this rule, if it applies as written, would return unused reserve to the pool — see "Not established" below for why this isn't confirmed. |

**Not established — do not assume:**
- Whether any unspent `fuel_reserve` actually returns to the pool.
  `InternalMessage.extra` (`crates/avm-contract-sdk/src/input.rs:190`,
  reachable via `MessageInput.extra` at `input.rs:303` or
  `CanonicalMessage::extra()` at `input.rs:266`) is the only SDK field
  that could carry this value on a callback; at `fuel_reserve = 1, 10,
  1000`, success and failure, it reads **0** every time. A separate
  out-of-tree attempt to bisect the pool's real SHELL balance
  before/after one `UNSTAKE` produced unexplained, untrusted numbers
  (see "Evidence"). Earlier cycle-survival experiments (pools surviving
  many cycles at `fuel_reserve = 1000` but failing by cycle 5-7 at `=1`)
  are **not** proof: the caller attaches `fuel_reserve` fresh every
  call, so the pass-through alone needs no standing balance, and the
  per-cycle external-call/callback charges were never separately
  counted in those runs — the cycle-5 failure is unexplained, not
  evidence of anything specific.
- Behavior on a real (non-local-stand) network — nothing in this
  section has been tested anywhere but the local stand.

**Top-up**, local-stand only: `avm-local-stand fund-user --address
<addr> --shells <n>` (`docs/cli-reference.md:33`) works against an
already-deployed address — confirmed on the local stand only, not any
real network; no mainnet CLI equivalent is asserted. A bare,
guest-code-bypassing SHELL transfer from another **contract**
(`method_id: 0`, "Callback model" below) is the only other path,
likewise local-stand only — an external, pubkey-signed caller cannot
send that same bare transfer through this contract's own methods,
since every declared method requires an exact `S`/`R`/fee-reserve
attachment, never a bare SHELL-only one (see "Currency rules").

## Callback model

Each pending payout carries a monotonic `pending_id: u64`, echoed back as
the `CallbackIntent`'s own `handler_method_id`. The callback fast path
(`core::State::payout_success_fast`/`payout_failure_fast`) locates the
row with a bounded linear scan over just the pending-flag byte and
8-byte id of each row (not a binary search: `pending_id` isn't sorted),
touching only that row's bytes, never decoding the full row table. It is
differentially tested against the full-decode `payout_success`/
`payout_failure` functions over the golden vectors and random sequences.

**Authenticity.** `process_callback` trusts `header.cause_address ==
input.destination` and `header.status_code`, but not the internal
sender's own identity, and `pending_id` is a predictable counter.
Checked directly against `output.rs`: neither `OutboundIntent` nor
`CallbackIntent` nor `MessageOutput` exposes any field for a guest to
set the `INTERNAL_FLAG_CALLBACK` wire flag, a `cause_address`, or a
callback `status_code` on a message it sends — those exist only in
`input.rs`'s decode path, synthesized by the runtime, never settable by
the sending guest. Confirmed on the real stand: an unrelated contract
sending a bare internal message with a guessed/replayed `pending_id` as
its own `method_id` left the target pool's state completely unchanged —
dispatched as an ordinary internal message, which the pool's own
`Caller::External` check rejects, never reaching `process_callback`.

A destination's own guest code can reject the forward delivery of an
`S`/`R` payout (returning an `Err`), in which case the whole transfer
rolls back atomically and the originating contract's `payout_failure`
path runs — the caller's principal and already-accrued reward are
restored, not lost, and the caller may retry with a different
destination. SHELL transfers are exempt from needing any guest-side
acceptance at all: a bare SHELL transfer succeeds unconditionally at the
protocol level, confirmed even against a destination explicitly toggled
to reject it (its own `account_hash` changes while its own `data` stays
byte-identical, meaning `process_message` was never invoked).

## Payout destinations

Every `UNSTAKE`/`CLAIM` call supplies the caller's own payout
`AccountAddress` (64 bytes) explicitly in the body — no SDK function
derives one from a bare pubkey. A payout destination must be a deployed
contract: a funded-but-undeployed address is rejected as `"account is
absent"` the moment anything pays into it.

For `S`/`R` (never SHELL), the destination's own guest code must also
accept an internal, non-callback message carrying the currency,
regardless of `method_id` (`combined_payout` always sends `method_id: 0,
body: []`, a bare transfer). An ordinary, unmodified contract whose
first dispatch check rejects any internal caller — the common shape,
including this contract's own — does *not* accept such a transfer; it
resolves as a failure restore, not a stuck or silently-dropped payout.
`contracts/test-receiver/` is a minimal, test-only destination that
does accept it; self-payment also resolves as a failure restore.
Evidence limit: only tested against this contract's own two shipped
contracts, not an arbitrary third-party receiver.

## Tests

`core/tests/golden_vectors.rs` replays `core/tests/fixtures/golden_vectors.json`
(generated by `tools/staking_rewards_model.py`, an independent Python
reference model) byte-for-byte. `core/tests/property_tests.rs` covers
random-sequence differential testing against that model, targeted
regression cases, and a property test asserting every `core` transition
leaves state byte-identical on every `Err` path. `tests/tests/contract_tests.rs`
covers the SDK-facing contract layer directly. `scripts/e2e.py` runs a
real local-stand journey covering every rejection class, a forced
success/failure via `contracts/test-receiver/`, a partial `UNSTAKE`, and
a self-payment — every outcome verified by decoding state, never status.

## Evidence

**Out-of-tree** (a real SDK builder and local stand, but either a
modified `core`/contract source or a different shipped example, never
committed here; not reproducible from the files in this directory;
`scripts/check.py`/`scripts/e2e.py` run **none** of these): the fuel
sweep, the 140-row run, the cap-fill build, the callback-forgery check
(see "Callback model"), every "Measured facts"/"Not established" figure
in "Fee model" above, and the `bounty-escrow` cross-check (unmodified
official example, same method, same `~2`/`~101`/`10,000` SHELL/call at
declared `1`/`100`/`10,000` — a platform-level behavior, not specific
to this contract).

**In this repository, reproducible as shown:**
- `cargo +1.99.1 test --workspace --locked --offline` — golden vectors,
  property tests, `tests/tests/contract_tests.rs`.
- `scripts/e2e.py` (via `scripts/check.py`, or directly with
  `--sdk`/`--output`) — the real local-stand journey: every rejection
  class, a forced success/failure via `test-receiver`, a partial
  `UNSTAKE`, a self-payment.
- `python3 tools/staking_rewards_model.py` (no arguments) — the
  100,000-sequence/2,500,729-step real-constant fuzz run
  (`sequences_run`, `total_steps`, `restore_clamp_count`).
- `python3 -c "import sys; sys.path.insert(0, 'tools'); from
  staking_rewards_model import run_adversarial_drift;
  print(run_adversarial_drift(4_500_000, seed=0))"` — the 4,500,000-op
  adversarial-drift run cited under invariant 2.
- `python3 tools/exhaustive_check.py 8` — the depth-8 tiny-constant
  exhaustive DFS cited under the payout-failure clamp: 1,934,112 nodes,
  1,487,539 leaf sequences, 704 clamp triggers, max magnitude 1, no
  counterexample.
