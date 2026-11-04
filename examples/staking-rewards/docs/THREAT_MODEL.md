# Threat model

Companion to [SPEC.md](SPEC.md), which this file does not duplicate.
Scope: the `staking-rewards` example contract and `core` crate, this
checkout only. Nothing here is a security audit or a completeness claim.

## Trust boundaries

- **External callers** (`Caller::External(pubkey)`): authenticated by
  signature (verified by the runtime, not this contract) for `STAKE`,
  `UNSTAKE`, `CLAIM`. `FUND_REWARDS` is deliberately permissionless — no
  caller-identity check at all, by design.
- **Internal callers** (`Caller::Internal(source)`): only reachable via
  the callback path (`is_callback()`), and only trusted insofar as
  `header.cause_address == input.destination` — `source`'s own identity
  is not used for row correlation (`pending_id`, carried in
  `message.method_id`, is, since a deployed destination's `account_id`
  is hash-derived and can never equal a caller's raw pubkey). Checked
  directly against `output.rs`: no guest-facing type exposes a field to
  set the callback wire flag, a `cause_address`, a `cause_msg_hash`, or a
  callback `status_code` on an outbound message — those are entirely
  runtime-synthesized from real tracked causality. Confirmed on the real
  stand: an unrelated contract sending a bare internal message with a
  guessed `pending_id` as its own `method_id` leaves the target's state
  unchanged — the message is dispatched as an ordinary internal message,
  which the target's own `Caller::External` check rejects, never
  reaching the callback handler at all.
- **The runtime**: trusted to deliver exactly one callback per outbound
  (not independently re-verified here — no receipt/hash dedup list the
  way `bounty-escrow`'s `state.receipts` does, since a stale
  `pending_id` will not match any row once the original payout resolves,
  which is itself sufficient replay protection for this contract's
  one-pending-per-row design — see the
  `callback_rejects_replay_after_already_settled*` tests).

## Accepted limitations (stated, not hidden)

1. **Cap-fill DoS.** An attacker can fill all `MAX_ROWS` slots with
   `min_stake`-sized accounts, blocking new stakers with `TOO_MANY_ROWS`
   until an existing row vacates. Mitigation: a meaningful `min_stake`
   raises the attacker's capital cost; a deployer can seed-stake to
   reserve headroom. Not prevented outright.
2. **`FullReplace` fuel cost grows with state size**, not with the
   callback's own O(scan) lookup cost. Bounded by the chosen `MAX_ROWS`,
   not eliminated.
3. **Residual replay window**: a stale, not-yet-expired signed message
   whose `seq` was never consumed (row deleted and recreated before
   resubmission) is replayable within its own original `MAX_TTL` window.
   Bounded, not zero.
4. **No defensive pre-check that this contract's own held balance covers
   an outbound payout before issuing it** (unlike `bounty-escrow`'s
   `dispatch()`, which checks `input.extra` against `state.funded - paid
   - refunded` before dispatching). `core`'s own invariants guarantee
   `total_staked`/`reward_pool_held` track genuinely committed amounts,
   and a payout that still somehow fails is handled by the existing
   `payout_failure` restore path — but this is reliance on invariant
   correctness plus the runtime's own atomic settlement, not an
   independent belt-and-suspenders check.
5. **A payout destination must be a deployed contract.** A caller who
   supplies a bare, undeployed address gets a payout that the runtime
   itself will fail to deliver — this contract cannot distinguish that
   case from any other delivery failure, and the resulting
   `payout_failure` restore is the caller's own remedy (retry with a
   real receiver), not a distinct error code.
6. **An expensive destination can exhaust the forward leg's own fuel
   reserve.** If the destination's own guest code costs more fuel than
   `fuel_reserve` buys (see SPEC's "Fee model" for the measured floor),
   the forward delivery fails exactly like an undeployed or rejecting
   destination: the whole transfer rolls back atomically, the caller's
   principal and already-accrued reward are restored via
   `payout_failure`, and nothing is lost. `fuel_reserve` is fixed at
   `init` and no call can resize it (limitation 9), so the caller's only
   remedy is to retry with a different, cheaper destination, not a
   distinct error code and not a bigger declared fee. If a destination
   is persistently too expensive for the deployed `fuel_reserve`, every
   payout attempt to it will restore the same way, indefinitely.
7. **Callback delivery itself is not independently authenticated beyond
   the wire format's own guarantees** (`cause_address == destination`,
   `Caller::Internal`) — see "Trust boundaries" above for why forging
   one is not possible through any guest-facing API, confirmed both by
   inspection and on the real stand. This contract trusts the runtime's
   own causal provenance exactly as `bounty-escrow` does — it does not
   re-derive or re-check a message hash against a stored commitment the
   way a HMAC/signature scheme would.
8. **Storage rent is an operator responsibility this contract cannot
   self-remediate** (`docs/runtime-limits.md:70-98`). Rent accrues on
   this account's own state bytes, charged before WASM/FUEL; the third
   consecutive unpaid attempt freezes the account, blocking every
   `STAKE`/`UNSTAKE`/`CLAIM`/`FUND_REWARDS` call until the due rent is
   paid in full. Funds are locked, not lost or corrupted, but this
   contract has no internal rent-payment path. Two top-up paths are
   confirmed, both only on the local stand (see SPEC's "Fee model"): a
   local-stand development command, and a bare SHELL transfer from
   another contract. Neither has been tested against a real network;
   the local stand's own dev-configured rent price/threshold are not a
   mainnet promise either.
9. **`fuel_reserve`'s own sizing is a deployer choice, not a platform
   guarantee for every destination.** The enforced `1..=MAX_AMOUNT`
   range's lower bound is a measured platform floor (a declared `0` is
   rejected with `RouteRequiresFee` before receiver WASM starts) — it is
   not evidence that `1` suffices against every possible payout
   destination's own execution cost (see limitation 6). `fuel_reserve`
   is set once, at `init`, never resizable per-call and never the
   caller's own declared value, specifically so a caller cannot control
   this cost against the pool's own balance. **Whether any unspent part
   of it returns to the pool is not established — assume it does not,
   and that `fuel_reserve` is fully consumed on every call** (see SPEC's
   "Fee model" "Not established" list: the callback's own incoming
   `extra` reads `0` directly, a balance-bisection attempt was
   inconclusive, and the cycle-survival pattern once read as evidence
   for a refund is not proof). This surplus, if it exists at all, would
   not be withdrawable by anyone regardless; there is no admin/owner
   method.
10. **`S`/`R` may never equal SHELL (`currency_id = 2`).** SHELL is the
    platform's own fee/rent currency; a pool denominated in it would mix
    stake/reward accounting with the currency this contract's own
    outbound payouts spend as `fuel_fee_shells`, and with whatever the
    account's own rent debits. Enforced at `init` and re-checked on
    every `decode`, not merely a convention.
11. **Every call — including one this contract itself rejects — is
    still charged to the pool's own SHELL, not the caller's, up to
    essentially the entire balance in a single call, and this contract
    has no way to prevent or mitigate it.** External messages are a
    chargeable route (`fuel-and-fees.md:50-52`); no account ledger
    exists for a bare signing pubkey (`fuel-and-fees.md:5-8`), so the
    external call's own declared `ext.fuel_fee_shells` draws down the
    destination pool's balance — on the local stand, the attacker's own
    pubkey pays nothing for this, since it has no ledger to debit at
    all. A contract-level rejection does not avoid the charge: measured
    at roughly the declared fee per call (`~2`/`~101`/exactly `10,000`
    SHELL at declared `1`/`100`/`10,000` — see SPEC's "Fee model"). A
    single call declaring a fee close to the pool's entire balance
    takes nearly all of it in one shot: funding a pool with exactly
    `10,000` SHELL and sending one rejected call declaring fee `9,995`
    left only `3` SHELL; declaring `9,000` left `998`. Repeated smaller
    rejected calls work too — 14 consecutive `STAKE` calls at a declared
    fee of `100` each (`WRONG_CURRENCY`, state provably untouched)
    drained a 5,000-SHELL pool to unpaid rent by the 14th call and
    outright insufficient balance by the 15th. A pool that only ever
    receives guaranteed-rejected calls has no way to replenish itself,
    so this is an operational drain an operator must monitor and top up
    against — not a fund-loss risk (no staker's `S`/`R` moves), but a
    real one for the pool's own continued ability to pay rent and
    process future calls. Checked against the unmodified `bounty-escrow`
    example from the official bundle, same method, same stand: it is
    charged the identical rate (`~2`/`~101`/`10,000` SHELL/call at the
    same declared fees) — this is a platform-level exposure, not
    specific to this contract.

## Explicitly out of scope

Admin/pause/upgrade (none exist), multi-currency pools beyond exactly
`{S, R}`, cross-contract reentrancy beyond what `message_isolated`
execution mode already rules out, and fee-on-transfer or rebasing
currency semantics for `S`/`R` (assumed ordinary fixed-balance
currencies).
