# Multi-Currency Vault — SPEC

A `message_isolated` AVM contract (copied from `contract-a-v1` for its valid
relative SDK path) that acts as a multi-currency custody ledger. Any
same-DApp account can deposit AVM "main" value and/or extra currencies
(SHELL is not one of them — deposits carrying SHELL are rejected whole).
Each depositor can withdraw up to their own tracked balance, always back to
their own account, never to a chosen third party. SHELL reaches the vault
only through a separate, external-only, permissionless top-up method — no
contract can ever relay SHELL to another account, confirmed empirically
(see `docs/PLATFORM_FINDINGS.md`). An owner can pause deposits only; owner
compromise can never move or seize any depositor's funds.

This is a standalone document: every section below is complete in itself,
with no "see an earlier round" pointers. `docs/THREAT_MODEL.md` (also
standalone) analyzes this specification for exploitability.
`docs/PLATFORM_FINDINGS.md` records the empirical platform spike this
specification is grounded in — read it first; most non-obvious decisions
below exist because of a specific observed result, cited inline, not
because of guesswork. `docs/MAINTAINER_QUESTIONS.md` consolidates every
open question this spike raised against the SDK itself, in priority
order, for anyone who wants the full list without hunting through every
section below. This specification is kept current with the
implementation, not frozen at Phase 1 — `src/` implements exactly this
document (Phase 2, approved through several Gate 2a review rounds; `../
REVIEW.md` records every judgment call and correction along the way), and
this document is corrected in place whenever implementation or review
work finds it wrong, rather than left to drift (Gate 2a round 3 item 5).

## 1. Design decisions and why

**No upgrade path in v1.** `avm-contract-sdk`'s own output validation
(`crates/avm-contract-sdk/src/output.rs::validate_message_context`)
requires a non-failure response to `method_id == CODE_UPGRADE_METHOD_ID` to
carry a `CodeUpgradeIntent`, or the SDK rejects the encoded output before it
ever reaches the runtime. Since this contract has no upgrade path, that
exact method id is explicitly detected and rejected (error 1518) rather than
left to fall through to generic dispatch, where it would otherwise risk
hitting that SDK-level rejection in a confusing way.

**`FullReplace` state, with a conservative byte cap that bounds new rows
only.** The real `message_isolated` + `FullReplace` state-size ceiling is
not documented anywhere in this repo's `docs/` — only community-measured in
open issue #20 (roughly 65,594 bytes accepted, roughly 66,140 bytes
rejected with `VM status 43`). `MAX_LEDGER_ENTRIES = 220` (reduced from
300, Gate 2b round 2 TASK A — see below) is chosen to stay comfortably
under that one data point with a wide margin; §2.4 below derives the true
worst-case byte count and justifies the exact number in writing,
including the rent implications of a large state (`docs/
PLATFORM_FINDINGS.md` P1, open issue #21).

**`MAX_LEDGER_ENTRIES` and `MAX_PENDING_WITHDRAWALS` went through two
rounds of change, for a reason unrelated to raw state-size rejection
above — now resolved.** A real-stand experiment (`docs/
PLATFORM_FINDINGS.md` P18/P19) found that a `WITHDRAW`'s own
Settle/Restore callback can silently fail to resolve once combined state
complexity crosses an empirically-found threshold well short of anything
in open issue #20. Gate 2b round 2 (TASK A) reduced both caps (300→220,
32→12) as an empirical margin of caution while the root mechanism was
still unconfirmed. **Gate 2b round 3 (TASK F/G) found the actual root
cause by a real-stand A/B test on never-committed spike copies**: it was
`state.rs` `parse()`'s own per-depositor cap check, an O(n²) pairwise
comparison over every pending record — removing exactly that loop, and
nothing else, resolved all 8 previously-stuck configurations tested
(`docs/PLATFORM_FINDINGS.md` P20). That check is now removed (see its own
removal comment in `state.rs`), and `MAX_PENDING_WITHDRAWALS` is restored
to its original architectural value, **32** — directly verified safe at
both 220 and 300 ledger rows, for both Settle and Restore, with no
failing point found anywhere in the range tested (`docs/
PLATFORM_FINDINGS.md` P21). `MAX_LEDGER_ENTRIES` stays at 220 rather than
being restored toward 300: ledger row count was never shown to be the
actual driver, and a smaller worst-case state size helps the separate
`MIN_OPERATIONAL_SHELL` margin finding below. Every number in this
document derived from the oldest caps (`332`, `28,439` bytes, `31,159`
bytes, etc.) or from the round-2 caps (`232`, `19,859` bytes, `1,200,000`,
etc.) is now historical — it describes what was true before these
changes, not the contract's current worst case (`24,359` bytes,
Phase-4-corrected from an intermediate `21,639`-byte figure — see §2.4
below for the full derivation).

**Fuel-costing headroom of the new, leaner `parse()` at the true
shipping worst case was measured directly on the real stand, and
confirms 220/32 need not change.** The true worst case is **252
ledger rows + 32 simultaneously pending withdrawals**, not 220+32: a
restore bypasses `MAX_LEDGER_ENTRIES` entirely and unconditionally
recreates the row it restores (§1/§2.4 below), so draining 220 rows to
32 pending, refilling the ledger back to 220 with unrelated deposits,
then restoring (not settling) those 32 — recreating them on top of the
already-full 220 — reaches 252 rows; 32 subsequent partial withdrawals
on those restored rows then refill all 32 pending slots without
zeroing any row. Reachability is proven by the host test
`handlers.rs::true_worst_case_state_is_252_rows_plus_32_pending`.

**TASK M5 (Gate 3d) is the decisive measurement: it closes the question
Gate 3c's TASK 3/M4 left open — whether the real, asynchronous Settle/
Restore callback at the true 252-row + 32-pending worst case resolves
cleanly, and what headroom it actually has before the cap/caps would
need to change.** Every earlier figure in this section (the `vault.topup`
probe's own K = 5,850/5.7× headroom, and TASK M4's confirmation that it
does not represent the Settle callback's real constraint) is superseded
by this measurement, not merely supplemented — see `REVIEW.md` for the
full history of how this number evolved and was each time found not to
transfer to the real callback.

**Method.** A fresh spike family (`spike/vault-m5/`, `spike/actor-m5/`,
Hard Rule 5, deleted after use): the SAME build-time-gated dummy-work-K
loop as every prior round (`std::hint::black_box`-guarded, now
corrected to 64-byte arrays — see the correction note below), plus
`vault.seed_full`/`vault.seed_pending` (spike-only, `AVM_SPIKE_SEED`-
gated byte-splice methods, reused from TASK M2's own technique) to
construct large fixtures without the real currency-registry limits
real `DEPOSIT` calls hit at scale, plus a reinstated, spike-only copy
of the REMOVED O(n²) per-depositor check (`AVM_SPIKE_OLD_CHECK`-gated).
A relay actor drives one genuine `DEPOSIT` then one genuine `WITHDRAW`
against each fixture, with a build-time flag to reject the vault's own
payout (forcing Restore) or accept it (forcing Settle); resolution is
read exclusively from a separately-probed, byte-decoded `vault.topup`
read-after the fact (`pending_count` back down = resolved), never from
receipts or the relaying actor's own synchronous reply.

**A real measurement bug, found and fixed before any number below is
trustworthy.** A first version of the dummy-work loop used 32-byte
arrays, not the established 64-byte calibration (two 32-byte depositor-
key halves compared together) — caught by directly re-testing TASK
M4's own 1-row+1-pending/K=610 case on this round's build: it resolved
cleanly, where TASK M4 reported a trap. Fixing the array size to 64
bytes did **not** reproduce TASK M4's boundary either (K=610 still
resolved, this time up to past K=16,000) — ruling out the array-size
bug as the full explanation. Two further differential tests (a
stripped vault with no `seed_full`/`seed_pending` dispatch at all, and
an actor matching TASK M4's own 101-byte withdraw-body shape) also
still resolved at K=610. **TASK M4's own K=590/610 boundary does not
reproduce under this round's repeated, controlled re-testing** — this
round's own bisected 1-row+1-pending boundary is K≈16,150 (16,140
resolves, 16,160 traps), roughly 26× higher. This is logged as a
correction to TASK M4's own REVIEW entry, not silently overwritten; the
likely explanation is a measurement error in that round's own
bisection (TASK M4's spike is already deleted and cannot be
re-inspected to confirm how), in the same spirit as TASK M2's own
correction of TASK M's uncontrolled sweep.

**(a) Settle-leg K-boundary by state size (bisected, fee = 2,000,000
throughout):**

| State | K resolves | K traps |
|---|---|---|
| 1 row + 1 pending | 16,140 | 16,160 |
| 100 rows + 32 pending | 12,625 | 13,000 |
| 190 rows + 32 pending | 10,025 | 10,350 |
| 220 rows + 32 pending | 9,300 | 9,650 |
| **252 rows + 32 pending (true worst case)** | **8,350** | **8,440** |

Headroom shrinks monotonically with ledger row count (confirming TASK
J's own original finding that row count, not pending count, dominates
this cost), from ≈16,150 at 1 row down to ≈8,400 at the true 252-row
worst case — roughly 100 K-units lost per additional row, not a
cliff.

**(b) Restore-leg K-boundary at 252+32: 8,200 resolves / 8,350 traps.**
Restore costs measurably more than Settle at the identical state (it
inserts the restored amount back into the ledger; Settle does not
touch the ledger at all) — a real, reproducible, if modest (≈2%),
difference in the same direction `handle_callback`'s own code shape
predicts.

**(c) K-cost of the REMOVED check, measured directly rather than
theoretically calibrated.** Re-enabling the removed O(n²) per-depositor
check (same shape as production's removed code: for each of 32
pending records, a full linear scan counting same-depositor matches,
distributed 8 depositor groups × 4 — its own real architectural shape,
not an arbitrary count) at the true 252+32 worst case **traps even at
K = 0** — the check's own real cost, alone, with zero dummy padding,
already exceeds the entire measured ~8,400-unit Settle headroom at
that size. To get a finite number, the same check was measured at a
1-row ledger instead (where the no-check baseline has ~16,150 units of
headroom to spend): with the check enabled, the boundary drops to
K≈2,960 (2,800 resolves, 3,125 traps). **K-cost of the removed check
≈ 16,150 − 2,960 ≈ 13,190** — far above the old, theoretical
calibration (1,024 "comparisons," derived from the check's own
pairwise-key-compare shape, used throughout every earlier round's own
headroom arithmetic in this section). The real check, built from
idiomatic Rust iterator chains (`pending.iter().filter(...).count()`),
costs roughly 13× that theoretical estimate in the same units — the
old calibration was never validated against the real check's own
compiled cost until this round.

**(d) Measured facts, stated plainly instead of a pass/fail rule.**
`K-boundary(252+32) ÷ K-cost(removed check, ≈13,190)` comes out to
≈0.64× for Settle and ≈0.63× for Restore — below any margin multiple
that would have been reassuring. **Gate 3e's own instruction retired
the "≥2× before K=0" decision rule entirely, not because the
measurement was wrong, but because the number it was calibrated
against (TASK M4's own K=590/610 figure) was itself wrong, and no
combination of the two caps can satisfy ≥2× against the real,
corrected numbers: even the near-empty 1-row+1-pending case only has
~16,140 units of spare K, short of the ≈26,380 units twice the real
removed-check cost (≈13,190) would require.** Measured per-row cost is
≈28 K/row; per-pending cost is ≈24 K/pending — neither cap, pushed to
zero, buys enough margin against a ≈13,190-unit fixed cost for the
ratio to reach 2× on this contract's own real numbers. The rule is
dropped as unsatisfiable, not relaxed.

**(e) K = 0 sanity: the current (check-removed) logic resolves cleanly
at 252+32 for both Settle and Restore with zero dummy padding** —
this was always the real question the ≥2× rule stood in for, and it
has a direct, measured yes: confirmed on a spike at K=0 (Gate 3d) and,
after Gate 3e TASK F2, on the real production fast path itself, via a
dedicated host test exercising the true 252-row worst case for both
the add-to-existing-row and insert-a-new-row Restore sub-cases
(`tests::fast_path::restore_at_252_rows_insert_add_and_overflow`,
differentially checked byte-for-byte against the pre-fast-path logic)
— no dummy-K spike needed for this fact going forward; it is now a
real, always-running test. Whether the earlier real-call marathon
(REVIEW.md entry 42, 287 rows + 32 pending) itself ran with K=0 on its
own spike remains **not confirmed** — inferred likely from its stated
purpose, not stated explicitly, and its producing spike is already
deleted.

**Gate 3e TASK F1/F2: the callback fast path, now in production,
measured to roughly 1.6–1.7× the prior callback cost at the true
worst case.** Rather than shrink either cap (which the ≥2× rule's own
retirement showed would not reach a comfortable margin anyway) or
accept the tighter number, a spike (`spike/vault-f1/`, Hard Rule 5,
deleted after use) tested dispatching a callback BEFORE any full
`VaultState::parse`, straight from the raw state bytes: full header
validation, exact length via checked arithmetic, and the ENTIRE
pending section validated exactly as `parse()` does — but the
*ledger* section's own content (shape, nonzero amounts, sort order) is
no longer decoded or re-validated on this path, only its declared
byte length. Settle copies the ledger bytes through verbatim; Restore
locates the one row that changes by binary search over the raw
85-byte records, using a comparator that decodes each candidate's key
fields in the same order `LedgerKey`'s own derived `Ord` uses (`currency_id`
compared as a decoded `u32`, never as raw little-endian bytes — the
two do not agree for e.g. id 256 vs id 1).

**Gate check (same session, same build family, bisected): fast-path
spare K at the true 252+32 worst case is ≈13,000–14,000 for Settle and
≈12,250–13,000 for Restore, against this round's own re-measured
baseline of ≈7,600–7,625 (Settle) and ≈7,525–7,600 (Restore) — ≈1.7×
and ≈1.6× respectively, both clearing the round's own ≥1.5× gate for
both legs.** This baseline re-measurement itself did not reproduce
Gate 3d's own 8,350/8,440 (Settle) and 8,200/8,350 (Restore) figures
exactly — it came in roughly 9% lower, in the same session, same
build family, same method — a smaller, second instance of the same
category of session-to-session drift this section has now seen twice
(TASK M4's own much larger error being the first). The *fee-
independence* and *relative-ordering* (Restore costs more than
Settle) findings held up under both measurements; only the absolute
K-value drifted. TASK M4's own number is distrusted specifically
because repeated, controlled re-testing (array-size fix, a stripped
vault, a body-size match) all still disagreed with it by roughly 26×,
not a measurement-noise-sized gap; this round's own ≈9% drift is
treated as ordinary noise and reported as a range, not chased further.

**The fast path is now the production dispatch for every callback**
(`lib.rs`, `handlers::handle_callback_fast`, `state::VaultState::
fast_callback`) — the pre-fast-path logic (`handlers::handle_callback`)
is kept `#[cfg(test)]`-only, as the differential tests' own ground
truth, not left in as dead production code. **What this changes for
this contract's own guarantees:** a callback no longer requires the
stored ledger to be fully valid — only its header and pending section.
A corrupted ledger row never blocks a Settle or Restore; the very next
`DEPOSIT` or `WITHDRAW`, which still calls the full `parse()`
unconditionally, catches it there and returns `ERR_INVALID_STATE`
(`tests::fast_path::corrupted_ledger_content_settle_succeeds_then_
next_op_rejects` demonstrates both halves of this directly). This is
judged acceptable because nothing in this contract's own writer logic
can ever produce a ledger row `parse()` itself would reject — the gap
only matters against a state corrupted by some mechanism outside this
contract's own control, and in that scenario, delaying the rejection
by at most one more call (the next `DEPOSIT`/`WITHDRAW`) rather than
catching it on the callback itself is a materially smaller risk than
the alternative this round closed off (the callback's own real cost
having too little margin at the true worst case). `DEPOSIT` and
`WITHDRAW` are both unchanged and continue to call the full `parse()`
on every invocation, exactly as before.

Full investigation — including TASK M4's own correction, the array-
size bug, the full bisection data for every state size, and the F1
gate measurement in full — is in `REVIEW.md`, not duplicated here.
- The deploy-envelope size limit was bisected twice (TASK J4,
  `spike/sizebisect/`, then corrected at Gate 3b TASK O,
  `spike/sizebisect-real/`). **TASK J4's wasm-only framing was wrong**:
  it used a minimal 80-byte spike ACAB and concluded the limit was "a
  function of wasm size specifically, not the combined envelope" —
  re-bisecting with the real contract's own 3,182-byte ACAB (confirmed
  byte-identical to the production build) shifted the wasm boundary by
  exactly the ACAB size difference (3,102 bytes), proving **the limit
  is on the combined `wasm + acab + descriptor` envelope**: both
  bisections land on the identical combined total, **65,237 bytes
  succeeds, 65,241 bytes is rejected** with `BodyTooLarge` (a 4-byte
  window), unaffected by this or any later round's own wasm size. The
  current production envelope and its margin against that limit are
  tracked in exactly one place now — `README.md`'s own "Build
  artifacts" section — not repeated here; that margin has moved
  several times across rounds (shrinking when the wasm grows, e.g.
  this round's callback fast path, growing when it shrinks) purely
  because the wasm size changed, never because the 65,237-byte limit
  itself did. See `docs/MAINTAINER_QUESTIONS.md` #11 for the full
  bisection table (undocumented anywhere in this repo's own `docs/`).
- **Honesty note (TASK J5):** this synchronous local stand cannot model
  real elapsed time between a `WITHDRAW` and its Settle/Restore callback,
  so any interaction between callback delay and rent accrual (does rent
  accrue against the vault while a withdrawal sits pending, and could a
  long real-world delay push `MIN_OPERATIONAL_SHELL`'s margin into a case
  this repo never constructed?) remains genuinely untested here, not just
  unreported — already noted in round 3 (`docs/PLATFORM_FINDINGS.md` P21;
  `THREAT_MODEL.md` §6).

**Withdrawals always pay out to the caller's own account — there is no
caller-chosen destination argument anywhere in this contract.** A depositor
calling `WITHDRAW` always receives their payout at the exact
`Caller::Internal(source)` address that sent the withdraw request; nothing
in the body names a destination. This is structural, not a policy
preference layered on top: the destination of every withdrawal, by
construction, is an account that has already proven it exists and can send
messages, because it is the same account that just called `withdraw`. This
single decision eliminates an entire class of problems a chosen-destination
design would otherwise have to solve: a withdrawal can never target an
address that was never deployed (`docs/PLATFORM_FINDINGS.md` addendum C
confirmed a payout to a genuinely undeployed same-DApp address produces no
debit, no callback, and no error — silently unresolvable forever; that
scenario cannot arise here because the destination is never a free-form
address). It also means an attacker can never be named as the destination
of a withdrawal that isn't their own, which closes an entire category of
forged-settlement attempts by construction rather than by a runtime check
(`docs/THREAT_MODEL.md` §5.10 discusses this precisely). What remains open
even with this design is narrower, and comes in two distinct stuck
routes, both tracked as `docs/THREAT_MODEL.md` §6 "L": the
same-DApp-but-split case (`docs/PLATFORM_FINDINGS.md` Q3, still unresolved
— whether a chargeable route that fails before receiver WASM starts still
delivers a callback), and the SHELL-starved dropped callback case (a
withdraw-shaped chain that is accepted and begins executing, then has its
own return callback never delivered purely because its total SHELL cost
exceeded what the initiating account held — directly observed, not
hypothetical, `docs/PLATFORM_FINDINGS.md` round-5 probe P5 and round-7
addendum). Separately — not a stuck-pending-withdrawal case at all, but a
different failure mode entirely — a depositor whose own account logic
doesn't accept a plain, methodless value transfer (discussed below) can
have a withdrawal fail at delivery, though that case always resolves via
the ordinary failure callback and ledger restore, never getting stuck.
All three are documented explicitly rather than hidden.

**Both `DEPOSIT` and `WITHDRAW` require the caller's own account to share
the vault's `dapp_id`.** `Caller::Internal(source).dapp_id` must equal the
vault's own `dapp_id` (read directly from `MessageInput.destination.dapp_id`
— `MessageInput.destination` is a plain, unconditional `pub` struct field,
`crates/avm-contract-sdk/src/input.rs` line 309; `docs/PLATFORM_FINDINGS.md`
probe P2 confirmed this is always the contract's own address by
construction, since `decode_message_input` requires the message's own
declared destination to equal this same field or reject with
`DestinationMismatch` — no stand probe beyond reading that source was
needed, since this is a plain struct field, not something that could
behave differently at runtime) or the call fails (error 1520,
`cross_dapp_caller_forbidden`). This is checked at both entry points
independently, not only at deposit time, because a depositor's balance can
also arrive via being credited directly by `DEPOSIT` itself — there is no
other path to acquire a ledger balance in this contract (transfers between
ledger accounts do not exist in v1 — see below), so checking the caller's
own `dapp_id` at withdraw time as well is simple defense in depth rather
than a load-bearing second control. `docs/PLATFORM_FINDINGS.md` Q3 found
this local stand rejects genuinely cross-DApp outbound intents outright
with no way to test whether a route that fails before receiver WASM starts
still calls back — this contract avoids depending on an unverified answer
to that question for the common case by simply not allowing cross-DApp
depositors at all.

**There is no `TRANSFER` method in this contract.** An earlier design
included one (letting a depositor move ledger balance to any other
same-DApp address), gated only by a zero-address check on the recipient.
The problem was never the recipient address being malformed — it was that
*any* valid same-DApp address could be named, including one that has never
deposited anything and has no relationship to the sender at all. A
depositor could transfer dust to up to `MAX_LEDGER_ENTRIES` distinct
never-before-seen addresses, consuming ledger capacity this contract has no
mechanism to reclaim: Invariant I9 (below) only removes a row when its
*own owner* drains it to zero — nothing forces an unwilling or unaware
recipient to do that, and forcing it would mean this contract seizing a
depositor's funds, which Invariant I5 forbids categorically. The original
task description itself frames an internal transfer as optional, "only if
it stays simple" — once it became the only entry point that could assign a
depositor a ledger row without their own action, it stopped being simple,
and this contract drops it entirely rather than trying to patch around the
gap (for instance, "the recipient must already hold a row" does not prevent
dust being left on a row its owner never asked to receive more of). Method
id `3` — what would have been `TRANSFER_METHOD_ID` — is permanently
retired, not reassigned to anything else, so no future revision of this
document or its tests can be confused about what that id once meant.

**SHELL (the AVM protocol currency exposed at `extra[currency_id = 2]`)
reaches this vault only through a separate, external-only, permissionless
`TOPUP_METHOD_ID` — `DEPOSIT` rejects it outright.** `docs/
PLATFORM_FINDINGS.md` P1 found, and reconciled against an earlier
contradictory data point in round 5 (the earlier result could not be
reproduced under controlled retesting and is retracted), that **no guest
contract can ever send `extra[currency_id=2]` through its own
`OutboundIntent`** — every attempt materializes as `EconomicsError`,
regardless of the amount, the destination's health, or whether that
destination would have succeeded or failed. Since `DEPOSIT_METHOD_ID`
requires `Caller::Internal`, and every internal message is, by definition,
constructed by some other contract's own `OutboundIntent`, **no deposit
could ever have carried SHELL in the first place** — there is no code path
by which an internal message reaching this vault could contain SHELL as
`extra`. `DEPOSIT` therefore explicitly rejects any `extra` entry with
`currency_id == 2` (error 1519, `shell_not_custodied`) as defense in depth,
even though the restriction above suggests this can never actually trigger
via the internal-only deposit path — kept because the exact boundary of the
`EconomicsError` restriction (every amount? every shape of accompanying
fields?) was not exhaustively tested, and a cheap, always-correct check
costs nothing. Round 6 directly tested whether this restriction is unique
to `currency_id == 2` or applies more broadly: a spike sender round-tripped
an `OutboundIntent`-forwarded `extra` entry for currency ids `0`, `1`, `3`,
and `7` to a healthy destination, each requesting a callback; all four
completed with a genuine `status_code == 0` success callback (confirmed by
decoding the callback diagnostic, not merely the top-level call status —
`docs/PLATFORM_FINDINGS.md`, round-6 addendum). None hit `EconomicsError`.
**Currency id `2` (SHELL) is the only currency this restriction has ever
been observed to apply to**; `DEPOSIT`'s rejection rule is therefore
correctly scoped to `currency_id == 2` only, and this round's negative
result is recorded as direct confirmation of that scope, not merely an
assumption carried over from P1. The only way SHELL can reach *any*
account on this platform,
including this vault, is a genuinely external, signed message (`docs/
PLATFORM_FINDINGS.md` P1: an external call carrying `extra[2]` succeeded in
every case tested, including against a fully SHELL-drained destination).
`TOPUP_METHOD_ID` is therefore `External`, permissionless (no authorization
to check — anyone funding the vault's operations is welcome to), and
requires `value_main() == 0`, at least one `extra` entry, and every `extra`
entry present to have `currency_id == 2` (error 1526,
`topup_requires_shell_only`, otherwise — this rejects both a call mixing a
real deposit into a top-up and a pointless empty-extra call that would
still cost the vault SHELL to process for nothing in return). A top-up
credits nothing to any ledger row; the SHELL simply becomes part of the
vault's own real account balance, with no depositor ever owed any of it
back (SHELL is excluded from Invariant I1 entirely, unconditionally).
`TOPUP_METHOD_ID` is never gated by the deposits-paused flag — pausing
deposits must never also cut off the vault's only replenishment path for
the one resource every external call, including the pause call itself,
consumes.

**A failure callback restores the ledger unconditionally — there is no
check on the callback's own carried value.** An earlier design required the
callback's own carried `value_main`/`extra` to cover the pending amount
before crediting, reasoning that crediting on a false signal could
double-credit real value. `docs/PLATFORM_FINDINGS.md` probe A8 directly
disproves the premise this was protecting against: a failure callback's
carried value is **not a real, protocol-credited amount at all** — in two
otherwise-identical experiments differing only in whether the receiving
account's callback-handling code reported success or failure for that
callback, the receiving account's actual `balance_main` was identical in
both cases, and never reflected the carried value either way. The
underlying transfer was never actually debited from the sender to begin
with for a failing destination (confirmed separately via probes A1 and A3:
a successful send debits the sender by exactly the sent amount; a failing
one never touches the sender's balance, not even transiently) — so there is
nothing real for a callback to "return." The one quantity
`docs/fuel-and-fees.md` documents as genuinely prepaid-and-refunded is the
FUEL reserve specifically (`fuel_fee_shells`), a different field this vault
always sets to `0` on payouts, so it is not in play here either. A
verification gate checking a value that is never real would have been
protecting against a risk that does not exist, while carrying its own real
downside: `docs/PLATFORM_FINDINGS.md` probe A8 also shows that if this
contract's own callback-handling code were to deliberately fail (as a
verification gate's negative branch would), that failure rolls back the
handling execution's own effects just like any other failure — meaning a
callback handler that fails after confirming correlation throws away the
one chance it has to resolve that withdrawal, since there is no second
callback coming. This contract's callback handler therefore never
deliberately fails once the one real authorization check (below) passes —
it always either settles or restores, unconditionally.

**A pending withdrawal can only be settled or restored by a callback whose
own caller equals that withdrawal's depositor.** The SDK's own decode layer
already enforces that a callback's `cause_address` equals the destination
of the message being delivered (i.e., that the callback genuinely claims to
be about *us*) — this contract additionally requires
`Caller::Internal(source)` on the callback message itself to equal the
specific withdrawal's own depositor (who, per the design above, is also
that withdrawal's only possible destination). `docs/PLATFORM_FINDINGS.md`
confirmed live that a genuine callback's caller is exactly the account that
actually executed and reported the outcome — not a spoofable system
address — and that the SDK gives guest code no way to construct a
callback-flagged message except as a real runtime-generated reply:
`OutboundIntent` (`crates/avm-contract-sdk/src/output.rs`) has no `flags`
field at all, and `ExternalMessage`'s flag space has no callback bit
either. This was pushed further than source-reading alone: a fully
SDK-free, hand-rolled raw contract (no dependency on `avm-contract-sdk`
whatsoever) was built and deployed specifically to attempt smuggling a
settable flag past the host's own output decoder — the host rejected even
one extra trailing byte appended after an otherwise byte-for-byte-correct
output (`"TrailingBytes"`), confirming the host's guest-output decoder is
exactly as strict as the documented wire format implies, with no gap
anywhere a flag could be smuggled through, raw bytes or not.

**The callback-handling branch (settle or restore) is gated strictly by the
wire-level callback flag, never by `method_id` matching alone.** Dispatch
checks `input.message.is_callback()` before any method-id-based branch. A
plain, non-callback-flagged internal message whose `method_id` happens to
numerically collide with some real `withdrawal_callback_method_id(id)`
falls through to ordinary dispatch, matches none of the named methods, and
fails as `unrecognized_method` (error 1002) — it can never reach the
settlement logic, regardless of its body content.

**A withdrawal's own outbound payout is pinned to a fixed shape:
`method_id = 0`, `body = []`.** See §3 for why `method_id = 0` is safe to
use here, with the exact source citation.

**A malicious withdrawal destination cannot forward the payout onward and
also report failure.** `docs/PLATFORM_FINDINGS.md` addendum B confirmed
this directly, not merely by reading source: a `MessageOutput` constructed
to combine `status_code != 0` with a non-empty `outbound_intents` (keep the
money moving *and* claim failure) is rejected by the runtime's own output
validation before it is ever admitted, surfaced as `"VM status 43"` —
matching `crates/avm-contract-sdk/src/output.rs`'s `EffectsOnFailure` check.
A depositor's own account — which could itself be a misbehaving or buggy
contract — therefore cannot both keep money it receives and tell this
vault the delivery failed to get a second credit; whichever single outcome
it reports is what genuinely happened.

**Restores bypass `MAX_LEDGER_ENTRIES` — only genuinely new deposit rows
are capped.** A withdrawal that emptied (and, per Invariant I9, removed)
its own ledger row must always be able to restore that row on failure, even
if the ledger has reached capacity from *other* depositors' activity in the
meantime. The alternative — a restore that can be rejected by the cap —
would mean a withdrawal's own funds become unrestorable for a reason that
has nothing to do with that withdrawal, which is strictly worse than the
state-exhaustion griefing the cap exists to limit in the first place. See
§2.4 for the resulting true worst-case state size.

**A ledger row an attacker funds and never withdraws can hold its slot
indefinitely.** With `TRANSFER` removed, nobody can be handed a row against
their will — the only remaining ledger-exhaustion vector is entirely
self-funded: an attacker funds many distinct addresses, has each deposit an
economically negligible amount of a distinct currency, and simply never
withdraws it. Nothing in this contract creates any incentive or mechanism
to reclaim that row — doing so would mean seizing a depositor's funds,
which Invariant I5 forbids unconditionally. `MAX_LEDGER_ENTRIES` can
therefore be held at capacity for as long as the attacker chooses, not
merely "temporarily," blocking every new depositor/currency pair the entire
time they choose to leave it that way. This is stated here as a documented,
permanent limitation of a conservative, non-upgradeable v1 cap, not
something expected to self-resolve.

**`admin_seq`: an explicit, contract-owned sequence number for every
owner/pending-owner call, independent of whatever the platform does or
doesn't do about external-message replay.** Whether the platform
deduplicates an exact replayed external message is unverified either way
(`docs/PLATFORM_FINDINGS.md` Q5 — left genuinely unresolved, not merely
undocumented). Rather than rely on that unknown, or on the weaker argument
that every admin method just happens to be idempotent under replay, every
`PROPOSE_OWNER`/`ACCEPT_OWNER`/`SET_PAUSE` call must supply the header's
current `admin_seq` value as an explicit body argument. The contract
rejects a call whose supplied value doesn't match exactly (error 1524,
`stale_admin_sequence`) and increments `admin_seq` by exactly one on every
successful admin call, advanced only via `checked_add` (on the practically
unreachable `u32::MAX` overflow case, the call fails with error 1527,
`admin_sequence_exhausted`, before any mutation, rather than wrapping). A
replayed, byte-identical old admin call now fails regardless of platform
behavior — this is a guarantee the contract makes for itself. Every
successful admin call also emits an event carrying the *new* `admin_seq`
value, and the current value is separately and always readable via
`raw_state` on the response to *any* subsequent call to the vault, even an
unrelated one — `docs/PLATFORM_FINDINGS.md` P3 confirmed `raw_state` is
present on every `avm-local-stand call` response, reflecting that account's
state immediately after the call, at a fixed, known header offset. This
corrects an earlier, repeated claim in this project's own history that
"there is currently no documented way to read raw account state via the CLI
(issue #5)" — that claim is true only for the `inspect` command
specifically; `call` itself exposes state as a side effect of any valid
invocation, including a deliberately harmless one, to anyone who can
construct one.

**The callback-settlement branch is independent of pause state.** Neither
settling nor restoring a pending withdrawal ever checks the `flags` header
byte. A withdrawal in flight when the owner pauses deposits resolves
exactly as it would unpaused — pausing deposits must never also strand an
already-in-flight withdrawal's resolution.

**The owner-key blast radius is exactly: pause deposits, and/or rotate the
owner.** That is the entire capability `owner_pubkey` grants, whether held
by a legitimate deployer or by an attacker who has somehow compromised it
(including, in the worst case, by never bothering to replace the demo
fallback key — see below). An owner, legitimate or not, cannot move,
redirect, or seize any depositor's ledger balance under any circumstance
(Invariant I5); every withdrawal remains fully usable by its rightful
depositor regardless of who currently controls `owner_pubkey`. Owner
compromise is a griefing/control risk to the deposits-paused flag and to
who the owner is — it is never a funds-theft risk.

**The owner key is gated by a plain environment variable, following the
pattern already established by `contract-a-v1`.** `AVM_EXAMPLE_VAULT_OWNER_PUBKEY`,
read via `option_env!` at compile time: if set (to 64 lowercase hex digits),
it becomes the initial `owner_pubkey`; if unset, a fixed, publicly-known
demo key is used, identical in spirit to `contract-a-v1`'s own
`FIXTURE_PACKAGE_OWNER_PUBKEY` fallback. An alternative design using a
Cargo feature (on by default, disabled via `--no-default-features` for a
"production" build) was considered and rejected: `docs/PLATFORM_FINDINGS.md`
confirmed directly against the real tool that `avm-contract-build build`
accepts **no arguments beyond `--manifest-path`** — not
`--no-default-features`, not `--features`, nothing — so a Cargo-feature
toggle cannot be expressed through the one tool this repo's conventions use
to build an example at all. A plain environment variable works because it
reaches the underlying `cargo`/`rustc` invocation without needing any CLI
flag, unlike a Cargo feature. **The demo key's private-key secrecy is
explicitly unverified, not confirmed safe.** A repository-wide search found
no private key material anywhere in this public checkout paired with the
fixed fallback constant — this proves only that this public repo doesn't
contain it, not that nobody else does. A real deployment must set the
environment variable to a real, freshly-generated key and must never rely
on the fallback; the example's README (a Phase 2 deliverable) must say this
loudly and explicitly, not as a footnote.

**Real-stand admin testing needs `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` set to
one of the stand's own identities, not left at the demo fallback (Gate 2a
round 2 item 8).** `docs/getting-started.md` establishes the exact same
pattern for `contract-a-v1`'s own upgrade-authority key: `avm-local-stand
init` generates fresh `developer`/`package_owner`/`user` identities and
writes their **public** keys to `init.json`; `contract-a-v1` is then
rebuilt with `AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY="$(jq -er
.identities.package_owner init.json)"` specifically so the stand can
produce a genuine signature matching that key. This vault's own
`PROPOSE_OWNER`/`ACCEPT_OWNER`/`SET_PAUSE` are exactly analogous: they are
`External` methods gated on a pubkey match against `owner_pubkey`
(§4), and `avm-local-stand call --identity <name>` can only sign as one of
the stand's own generated identities. A vault built with the fixed,
publicly-known `FIXTURE_VAULT_OWNER_PUBKEY` fallback (§1 above) can
**never** have any of these three methods genuinely authorized on a real
stand — every such call would be rejected at `1514`/`1515`
(`unauthorized_owner`/`unauthorized_pending_owner`), not because the logic
is wrong, but because **no stand identity holds it**: `init.json`'s
`developer`/`package_owner`/`user` keys are freshly, randomly generated
each time `avm-local-stand init` runs, so none of them is ever going to
coincidentally equal the fixed `FIXTURE_VAULT_OWNER_PUBKEY` constant, and
the stand only ever signs as one of its own generated identities — it has
no mechanism to sign as an arbitrary externally-supplied key. This says
nothing about whether that fallback key's secrecy holds *elsewhere*
(outside this stand, outside this repo), which remains exactly as
unverified as stated above (Gate 2a round 3 item 5 — a correction to an
earlier draft of this paragraph, which overstated this as "nobody holds
that fallback key's private half" without the stand-identity
qualification, contradicting the more careful hedge already established
above). Exercising these three methods for real therefore requires
rebuilding this contract with `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` set to one
of `init.json`'s own identity pubkeys first — the same two-step
"initialize the stand, then bind the build to one of its identities"
sequence `contract-a-v1` already documents, not a new mechanism invented
for this vault.

**A single message can carry at most 8 distinct extra currencies — a real
platform limit, not a FUEL/size degradation, and not specific to any one
method or caller type.** `docs/PLATFORM_FINDINGS.md` P14 found this
directly: a real `TOPUP` call carrying 9 distinct non-SHELL currencies on
a completely fresh account (eliminating any cumulative-per-account
confound) was rejected immediately with `TooManyExtraCurrencies`, before
guest execution ever started; 8 was accepted. Nowhere in this checkout's
own docs (`docs/fuel-and-fees.md`, `docs/cli-reference.md`,
`docs/getting-started.md`) states this limit. Confirmed directly only for
`External` messages (`TOPUP` needed no relay contract to test); inferred,
not independently confirmed, for `Internal` messages (`DEPOSIT`), since
the rejecting check names the shared canonical-message envelope itself,
not anything caller-type-specific. Practical consequence: a depositor
wanting to establish more than 8 distinct currency rows for themselves
needs more than one `DEPOSIT` message — `ceil(count / 8)` at minimum —
never one. This does not change `MAX_LEDGER_ENTRIES = 220`'s own meaning
(a lifetime cap across every depositor combined, never assumed reachable
in a single message), but it is worth stating as an explicit operational
property rather than leaving it to be rediscovered.

**SHELL exhaustion is a platform-wide account property, more severe than an
earlier draft of this analysis assumed, and this vault's `WITHDRAW` method
defends against its own worst consequence.** `docs/PLATFORM_FINDINGS.md`
probe P1 found that every external call to any account costs that
account's own SHELL to process, regardless of authorization outcome —
even a call that gets rejected immediately for the wrong owner key still
costs the destination SHELL, because the platform charges for message
delivery before the contract's own authorization logic ever runs. P1 also
found, correcting an earlier hopeful and explicitly-unverified assumption,
that an **internal** message to a SHELL-exhausted destination also fails —
observed as a failure callback carrying an undocumented `status_code = 56`
— so SHELL exhaustion is not merely "the external admin surface goes
quiet," it can make an account unreachable by any message type at all.
Probe P5 went one step further and found the specific consequence that
matters most for this contract: **a withdrawal's own return callback can
fail to be delivered purely because the vault itself was too close to
SHELL-empty when it initiated that withdrawal** — in a direct experiment, a
forward-and-callback-request chain to a failing destination consumed a
sender's *entire* remaining balance and the sender's own callback-handling
code never ran at all (confirmed by the complete absence of the diagnostic
state write that code always performs when it runs), while an otherwise
identical chain to a succeeding destination, from the same starting
balance, completed with SHELL to spare. Since a withdrawal's debit and
pending record are committed *before* the outbound payout is even sent
(checks-effects-interactions), this means a withdrawal can be fully
accepted — balance debited, pending record created — and then never
resolve, through no fault of the depositor at all, purely because the
vault itself could not afford to see its own callback through. This is
treated as a property of AVM accounts generally, not a defect specific to
this contract, and it is addressed directly — and this is where this
document now explicitly names and labels the two field groups every
later section (§4 in particular) cites back to by letter:

- **Field group (a) — the account's own real balance.**
  `MessageInput.balance_main` (`crates/avm-contract-sdk/src/input.rs`
  line 312) and `MessageInput.extra` (same file, line 313). Populated by
  the host from the account's actual current holdings — this account's
  cumulative balance as of the start of this call — never from anything
  the inbound message itself carries.
- **Field group (b) — the inbound message's own one-time carried value.**
  `CanonicalMessage::value_main()`/`::extra()` (same file, lines 261–266
  and 275–280 — for an `Internal` message these proxy
  `InternalMessage.value_main`/`.extra` at lines 182/191; for an
  `External` message they proxy `ExternalMessage.value_main`/`.extra` at
  lines 214/221). This is what *this specific message* is carrying, not
  the account's running total.

`WITHDRAW_METHOD_ID` checks the vault's own current SHELL holding — group
**(a)**, filtered for `currency_id == 2` — against a fixed constant,
`MIN_OPERATIONAL_SHELL`, before doing anything else, and refuses to even
start a withdrawal if the vault's own SHELL is below it (error 1528,
`vault_underfunded`). This is in deliberate contrast to what `DEPOSIT` and
`TOPUP` must read instead — group **(b)** (§4) — conflating the two would
be a real bug, not a style choice: reading the account's cumulative
balance where the message's one-time carried amount belongs would credit
or validate against the wrong number entirely.

`MIN_OPERATIONAL_SHELL` is a heuristic safety margin, not a formal
guarantee. **The round-6 derivation below is now historical** — it used
fixtures shaped like this contract's own withdraw flow but not the real,
implemented vault, and produced a placeholder value (`1,600,000`) that
Gate 2b round 3 (TASK H) superseded with a real-vault-measured figure
(`1,300,000`), itself later superseded by Phase 4's correction
(`1,500,000`, final value) a few paragraphs below. It is kept here
because its measurement methodology and the qualitative findings it
produced (the identical success/failure cost, the ~5× unexplained
variance) remain relevant background for the final derivation. Four dedicated spike accounts (`spike/mini-vault`
and `spike/mini-vault-big`, two of each, one routed to an always-succeeding
destination and one to an always-failing one) measured the full withdraw
chain's SHELL cost (`charged_shells` on the withdrawing account's own `call`
receipt — the same methodology as earlier rounds' P5) at two state sizes:

| State size | Destination outcome | Measured chain cost (SHELL) |
|---|---|---|
| 91 bytes (small) | success | 2,367 |
| 91 bytes (small) | failure | 2,367 |
| 31,234 bytes (this contract's worst case, from §2.4) | success | 157,172 |
| 31,234 bytes (this contract's worst case, from §2.4) | failure | 157,172 |

Two things stand out, both recorded in `docs/PLATFORM_FINDINGS.md`'s
round-6 addendum in full: first, success and failure cost identically
within each state-size tier here — unlike the asymmetric 2,212/3,189 figures
round 5's probe-receiver-based P5 measured — consistent with the dominant
cost driver being the *withdrawing account's own* `FullReplace` state
rewrite (which happens on every withdrawal regardless of destination
outcome), not the destination's behavior. Second, a materially larger,
one-time cost was observed for the *same* nominal conditions on a separate
occasion during this round's probing (`781,851`, for a plain report call on
a large-state account, not a withdraw) — a roughly 5× swing versus the
`157,172` figure above that this spike could not fully attribute to a
single cause (candidates include small, uncontrolled differences in how
much rent had silently accrued since each account's last touch — see the
rent-rate reconfirmation below). Round 6 set `MIN_OPERATIONAL_SHELL` to
**`1,600,000`** on this basis — roughly **10×** the largest clean
withdraw-chain measurement above (`157,172`) — chosen to absorb both the
ordinary safety margin earlier rounds applied *and* the roughly-5×
variance this round directly observed between nominally similar calls,
rather than a margin sized only for the single best-case number. **This
`1,600,000` figure is historical, superseded by the `1,300,000`
derivation below, itself later superseded by Phase 4's `1,500,000`
correction.** Even a placeholder this size was only ever a
heuristic, not a proof: accrued storage rent (`docs/PLATFORM_FINDINGS.md`,
open issue #21, reconfirmed directly in round 6 — see §2.4) can in
principle make the true required amount arbitrarily larger than any
fixed threshold if the vault has gone a long time without being touched
while holding a large state — see §2.4's rent discussion for why this is
a named, not a solved, residual risk, and why `MAX_LEDGER_ENTRIES`'s
value is chosen with this interaction in mind.

**`MIN_OPERATIONAL_SHELL` final value: `1,500,000`, set at Phase 4
(fresh-context adversarial review, finding 1) — no longer provisional.**
This figure has been through five derivations: the round-7 placeholder
(`1,600,000`, from fixtures that were not this contract's own
implementation); a Phase-3 derivation from the real, unmodified vault
measured at three state sizes including the *original* 300/32-cap worst
case (`1,870,000`, superseded); Gate 2b round 2's re-derivation after
that worst case was found to silently break a `WITHDRAW` callback's own
resolution (P18) and both caps were reduced as an unconfirmed-root-cause
margin of caution (`1,200,000`); **Gate 2b round 3's re-derivation**
after the actual root cause was found (`state.rs` `parse()`'s own O(n²)
per-depositor check — confirmed by real-stand A/B test, `docs/
PLATFORM_FINDINGS.md` P20) **and fixed**, which let
`MAX_PENDING_WITHDRAWALS` be restored to 32 (verified safe at this value,
P21), against a worst-case byte count of `21,639` (up from `19,859` but
still below the original `31,159`) — `1,300,000`, itself now superseded;
and **Phase 4's correction**, below, which found that `21,639` was
*itself* not the true worst case.

**The `WITHDRAW` call's own cost — rent isolated via P15's same-block-
time technique — was `1` shell at every one of all three sizes,
identically for the call that goes on to Settle and the one that goes on
to Restore** (the two outcomes diverge only later, at the callback, which
`WITHDRAW` itself has no way to predict or depend on). This confirms
round 6's and TASK 0's own finding that this contract's compute cost is
negligible and does not scale with state size in any practically
load-bearing way — the real, size-dependent risk `MIN_OPERATIONAL_SHELL`
must cover is **accrued storage rent**, not compute.

**Phase 4 correction: `21,639` bytes (220 rows + 32 pending) was never
the true worst case — `24,359` bytes (252 rows + 32 pending) is.** A
fresh-context adversarial reviewer found, and `src/handlers.rs`'s
`true_worst_case_state_is_252_rows_plus_32_pending` host test confirms
by actually constructing it, that the "252 rows requires every pending
withdrawal to have already resolved" reasoning §2.4 previously relied on
(to justify treating 220 rows + 32 pending as the binding worst case
rather than 252 rows + 0 pending) is incomplete: once the ledger has been
driven to 252 rows via 32 restores bypassing `MAX_LEDGER_ENTRIES`
(leaving 0 pending), a *partial* withdrawal — one that debits a row to
something other than exactly zero — creates a new pending record
**without removing its row**, because Invariant I9 only ever removes a
row a debit brings to exactly zero (`state.rs::debit_ledger`), and
`handle_withdraw` never checks `MAX_LEDGER_ENTRIES` at all (only the two
pending caps, since withdraw never adds a row). All 32 global pending
slots can therefore be refilled on top of 252 still-intact rows,
reaching `91 + 252×85 + 32×89 = 24,359` bytes — `2,720` bytes (`32 × 85`)
larger than the previously-documented worst case. This is now the
binding figure for the derivation below; `21,639`/`1,300,000` are
historical, from before this correction.

**Derivation.** `docs/PLATFORM_FINDINGS.md` P15 confirmed the local rent
rate as exactly `1.000` SHELL/byte/second at this contract's own real
worst-case sizes. Using the worst-case byte count this document now
cites (`24,359` bytes, corrected above at Phase 4; historically `21,639`
bytes at Gate 2b round 3, `19,859` bytes under round 2's
unconfirmed-cause caps, and `31,159` bytes under the original 300/32
caps before any reduction) and the same explicit, stated policy choice
of a **60-second** margin window (how long the vault must survive on its
own residual SHELL after a withdrawal begins, before whoever operates it
is expected to notice and top it up — a deliberately round, conservative
number, not derived from any measurement, since no monitoring-cadence
requirement exists anywhere in this specification):
`24,359 × 1.000 × 60 = 1,461,540`, rounded up to **`1,500,000`**. The
`1`-shell compute cost measured above is folded in as rounding headroom,
not added separately — it is six orders of magnitude smaller than the
rent term and does not move the result. **Gate 2b round 3 TASK G3 found
this 60-second framing optimistic against real-world overhead**, tested
against the smaller, then-current worst case: a fresh vault funded with
exactly that value, or even 3× it, brought to that round's worst-case
shape, did not survive real setup cost plus 60 seconds of elapsed time
before a withdrawal attempt on the real stand
(`docs/PLATFORM_FINDINGS.md` P21) — see the "what this number does not
cover" paragraph below for the precise numbers. That finding was never
re-tested against the larger, corrected worst case here, so if anything
it understates how optimistic the 60-second framing is at the true
worst case.

**Scaling decision: fixed, not scaled to current state size — made
explicitly, not left implicit.** The alternative (computing a
threshold proportional to the vault's *current* serialized size at the
moment each `WITHDRAW` begins) was considered and rejected: it would
require embedding this document's own rent-rate assumption
(`1.000` SHELL/byte/second, a local-stand measurement explicitly
flagged as unconfirmed on any real network — §2.4) into the contract's
own logic, creating a correctness dependency on a number that could
drift without this contract ever being redeployed. A single fixed
constant, sized for the worst case the contract's own caps (
`MAX_LEDGER_ENTRIES`, `MAX_PENDING_WITHDRAWALS`) can ever produce, is
simpler, carries no such drift risk, and is strictly more conservative
at every smaller state size (a near-empty vault clearing `1,500,000`
has far more real margin than this derivation's own 60-second window
implies, since it accrues rent far slower).

**What this number does not cover — and what Gate 2b round 3 found it
does not even fully cover at its own stated worst case.** Two distinct
things, kept separate:

1. **The original P18/P19 callback-resolution gap is now root-caused and
   fixed** (`docs/PLATFORM_FINDINGS.md` P20, confirmed by real-stand A/B
   test: removing `state.rs` `parse()`'s own O(n²) per-depositor check,
   and nothing else, resolved every previously-stuck configuration
   tested). `MIN_OPERATIONAL_SHELL` never had any bearing on that gap —
   it only gates whether a withdrawal may *begin*, never whether its
   later callback can finish — and that remains true; it is simply no
   longer an open gap this constant needs a disclaimer for.
2. **A separate, newly-measured gap: this constant's own 60-second
   margin framing is optimistic against real-world overhead.** Gate 2b
   round 3 TASK G3 ran this test slightly ahead of TASK H's own
   recomputation above — against the then-current value, `1,200,000`
   (not yet `1,300,000`) — funding a fresh vault with exactly that
   amount, bringing it to the (then-current, 220/12) worst-case shape,
   and the very first withdrawal attempt — at only +1 second of elapsed
   time, once ordinary deploy/seed/deposit setup cost is accounted for —
   already failed, rejected before any state change (consistent with
   this same Invariant I17 check, though the exact downstream status
   code is not independently visible — a known tooling limitation).
   Funding **3×** that tested value (`3,600,000`) survived to +1s and
   +10s but failed the same way by +60s. The specific numbers tested
   predate this round's own cap/constant changes, but the *finding* —
   that this constant's 60-second framing is optimistic once any real
   setup overhead is counted — does not depend on the exact figure and
   is not re-tested against `1,300,000`, nor against the current,
   Phase-4-corrected `1,500,000`, specifically (left as a residual
   gap, not silently assumed fixed by either recomputation). **
   `MIN_OPERATIONAL_SHELL` is a withdrawal-admission gate, not an
   operating balance** — this was already stated as a design intent;
   this round's measurement makes it concrete with real numbers rather
   than a general caveat, and a real deployment's keeper must size the
   vault's actual holdings against the burn-rate formula below, watching
   the vault far more frequently than once every 60 seconds, not against
   this constant.

**A sharper, newly-observed version of this same risk: a severely
rent-delinquent large-state account can have a call admitted and charged
while its own contract code never runs at all — and, in that exact state,
a value credit the call was supposed to deliver can be silently dropped.**
Round 6 observed this directly, not as a theoretical extrapolation: a
large-state spike account left untouched for 10,000 seconds of block-time
had accrued enough rent debt that a subsequent call paying down that debt
consumed its entire request-time FUEL allowance doing so — the call
returned `"status": "ok"` with `charged_shells` close to the account's
whole prior balance, but `"events": []` and `"raw_executor_output": ""`
(the same "admitted and charged, but guest code never actually runs" state
`docs/PLATFORM_FINDINGS.md` P1 first identified in round 4). That same call
had also carried a `value_main` credit; a follow-up call confirmed the
credit never landed — withdrawing it immediately afterward failed with
`EconomicsError` for insufficient balance, and only succeeded once the
value was re-sent in a call where the contract's own code demonstrably ran
(non-empty `raw_executor_output`). **Rent affordability is therefore
resolved before the contract's own logic ever runs, and in the worst case
can consume the entire call without the contract running at all** — this
is consistent with, and sharpens, round 4's P1 finding that SHELL
exhaustion can make an account unreachable: here, the account was not
literally out of SHELL, but *so far behind on rent* that a single call's
FUEL budget could not cover both the debt and genuine execution. This is
the direct answer to round 6's question of whether `input.extra` is read
before or after rent collection: whenever the contract's own code *does*
run, `input.extra` already reflects the post-rent-collection balance (rent
affordability is gated upstream, at materialization, before the guest ever
starts) — but the contract may not get to run at all, and in that case
`WITHDRAW_METHOD_ID`'s own `MIN_OPERATIONAL_SHELL` check never has a chance
to fire either. `MAX_LEDGER_ENTRIES` and `MIN_OPERATIONAL_SHELL` reduce how
often this can happen; neither eliminates it.

**`created_block_time` does not appear anywhere in this contract's state.**
An earlier design kept a per-withdrawal creation timestamp to drive a
time-based reclaim mechanism; that mechanism was removed once it became
clear a balance-based heuristic could not distinguish "this payout
genuinely never happened" from "this payout happened and only the callback
was lost" (the vault's balance for a currency is shared across every
depositor's entries, so a bare balance check proves nothing about any one
pending withdrawal specifically). With the reclaim mechanism gone, nothing
in this contract consumes a creation timestamp, so the field is not stored
at all, rather than kept as dead data.

**`value_main` and extra currencies are different ledger "slots", never
conflated.** `docs/fuel-and-fees.md` states SHELL is specifically
`extra[currency_id = 2]`, and the wire format
(`crates/avm-contract-sdk/src/input.rs`) keeps `value_main` and `extra` as
entirely separate fields on every message and on the account's own current
balance. This was confirmed directly, not merely inferred: `value_main` is
a real, separately balance-constrained account currency (forwarding more of
it than an account currently holds fails outright with `EconomicsError`),
and `fund-user --shells` on the local stand funds `extra[2]` specifically,
never `value_main`. This contract never assumes the two are
interchangeable or that one implies something about the other.

**Withdrawal settlement correlation uses the outbound payout's
`handler_method_id` as the value the eventual callback is delivered with as
its own `method_id`.** This is the only hook the SDK exposes for routing a
callback back to specific contract logic, and it was verified live, not
merely assumed from the SDK's naming: a forward-and-callback-request chain
correctly delivered a callback whose own `method_id` matched exactly what
was specified as `handler_method_id`, letting the sender recover which
specific pending withdrawal the callback belongs to purely from that value.

**Depositors whose own account is itself a contract must be able to accept
a plain `method_id = 0` call carrying value.** The payout message's fixed
shape (`method_id = 0`, `body = []` — see §3) is an ordinary value transfer
with no declared method and no arguments. A depositor that is a smart
contract (not a simple externally-owned-style account) whose own dispatch
logic rejects any method id it doesn't explicitly recognize will have its
*own* withdrawal fail at the point of delivery. This resolves safely — the
failure callback restores this vault's ledger unconditionally, as described
above — but the depositor's funds remain in the vault, their withdrawal
simply having failed, until they either fix their own contract's dispatch
logic or otherwise arrange to accept the transfer, and they must
re-request the withdrawal afterward. This is a constraint on what a
depositor's own account must tolerate, not a bug in this vault, and must be
documented plainly for a reader of this example (a Phase 2 README
requirement, restated in full in §10).

**No message-hash-based deduplication of inbound deposits or withdrawal
requests is implemented.** No documentation found anywhere states whether
AVM internal messages are guaranteed to be delivered exactly once; this is
named as a trust assumption (`docs/THREAT_MODEL.md` §3), not silently
assumed safe. Building a defense against possible duplicate delivery would
require remembering every processed message's hash forever, which directly
conflicts with this contract's bounded-state requirement — and none of the
existing example contracts in this repository implement such a defense
either.

**Atomicity of a failing transfer — "the destination's balance is
unaffected and the sender's balance is unaffected" — was verified as an
*outcome*, on *one route*, not as a *mechanism* proven to hold everywhere.**
Every claim in this document that a failure "fully rolls back" rests on
observing account balances before and after a call on the local, same-DApp,
synchronous delivery route — never on reading the host's own accounting
logic, which is closed-source (this repository's own `README.md`:
"Production AVM runtime source remains in the private Acki Nacki
repository"). This is named here as an explicit, separate trust assumption
precisely because it is not a comfortable one to lean on silently: whether
a cross-DApp or post-split chargeable route (still unresolved, `docs/
PLATFORM_FINDINGS.md` Q3) produces the same outcome, or genuinely retains
value at risk on a route this spike had no way to reach, is unverified
either way. §9's adversarial simulator strategy includes a "retain" branch
specifically to demonstrate, concretely, what breaks in this contract's
accounting if this assumption is ever wrong somewhere this spike couldn't
test — and to state plainly that this contract has no code path that would
catch it if so, rather than claiming a defense that doesn't exist.

## 2. State layout (`FullReplace`, little-endian, hand-parsed like every other example in this repository)

### 2.1 Header — fixed, 91 bytes

| Offset | Len | Field | Notes |
|---|---|---|---|
| 0 | 4 | `magic` | ASCII `"MCVT"`; a state blob failing this check is rejected as malformed/foreign |
| 4 | 1 | `version` | `1`; reserved for a future incompatible layout |
| 5 | 32 | `owner_pubkey` | the current owner's external pubkey |
| 37 | 32 | `pending_owner_pubkey` | all-zero means no rotation is currently proposed |
| 69 | 1 | `flags` | bit 0 = deposits paused; bits 1–7 are unused and must be `0` |
| 70 | 4 | `reserved` | must be all-zero; a non-zero value here means the state is malformed |
| 74 | 4 | `admin_seq` | `u32`, starts at `0`, advanced only via `checked_add`, must be supplied exactly by every owner/pending-owner call before that call has any other effect |
| 78 | 4 | `next_withdrawal_id` | `u32` monotonic counter via `checked_add`, never reused, never wraps |
| 82 | 4 | `ledger_count` | the number of ledger entries that immediately follow this header |
| 86 | 4 | `pending_count` | the number of pending-withdrawal records that follow the ledger entries |
| 90 | 1 | `build_flags` | bit 0 = this build is using the demo owner fallback key (set once at `init()` from whether `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` was present at compile time); bits 1–7 unused, must be `0` |

### 2.2 Ledger entries — `ledger_count` records of exactly 85 bytes each, immediately following the header

| Offset within record | Len | Field |
|---|---|---|
| 0 | 32 | depositor `dapp_id` |
| 32 | 32 | depositor `account_id` |
| 64 | 1 | `currency_tag`: `0` means Main (`value_main`), `1` means Extra |
| 65 | 4 | `currency_id` (`u32`); must be exactly `0` when `currency_tag` is Main; must **never** be `2` under any `currency_tag` — SHELL is never represented as a ledger row at all |
| 69 | 16 | `amount` (`u128`); **always strictly greater than `0`** |

Entries are kept strictly sorted in ascending order by the tuple
`(dapp_id, account_id, currency_tag, currency_id)`, and this ordering is
re-validated defensively on every read of the state blob, not merely
assumed from how the contract itself writes it (mirroring the same
defensive posture the SDK's own decoder uses for sorted wire data, e.g. its
`Unsorted` check on extra-currency lists). A balance that reaches exactly
`0` via a withdrawal debit — the only way any ledger entry's amount can
ever decrease in this contract, since there is no `TRANSFER` method
(§1) — causes that row to be removed from the list entirely, not
retained as a zero-value row. This removal is load-bearing for keeping
the state-size
cap meaningful: without it, a deposit-then-withdraw-to-zero cycle repeated
many times would permanently consume ledger capacity for nothing.

### 2.3 Pending withdrawals — `pending_count` records of exactly 89 bytes each, immediately following the ledger entries

| Offset within record | Len | Field |
|---|---|---|
| 0 | 4 | `withdrawal_id` (`u32`) |
| 4 | 32 | depositor `dapp_id` — this is also the payout destination; they are the same account by construction, so no separate destination field exists anywhere in this record |
| 36 | 32 | depositor `account_id` |
| 68 | 1 | `currency_tag` |
| 69 | 4 | `currency_id` |
| 73 | 16 | `amount` (`u128`) — the amount already debited from the ledger and held pending resolution |

Record length is `4 + 32 + 32 + 1 + 4 + 16 = 89` bytes — 4 bytes longer than a
ledger entry (§2.2's 85 bytes) because of the leading `withdrawal_id` field.
An earlier round of this document mislabeled this record as 85 bytes
(copy-pasted from §2.2's ledger-entry size) while the field table itself was
already correct; round 6 fixes the label and recomputes every number in
§2.4 that depends on it.

There is no `created_block_time` or any other timestamp field in this
record — nothing in this contract consumes one, since the time-based
reclaim mechanism that would have needed it does not exist (§1). Every
record present in this list is, by construction, still outstanding: a
withdrawal that reaches a terminal outcome — settled via a verified-success
callback, or restored via a failure callback that passes the depositor
match check — is removed from this list in the exact same state update
that resolves it. A `withdrawal_id` is monotonic and never reused, so a
stale reference to an already-resolved id and a reference to an id that
never existed are indistinguishable, and are handled identically: both
fail with `withdrawal_not_found` (error 1510) and mutate nothing. A
withdrawal whose payout produces no callback at all — via either of the
two stuck routes this contract cannot resolve (the same-DApp-but-split
case, and the SHELL-starved dropped callback case, both `docs/
THREAT_MODEL.md` §"L") — simply never leaves this list; there is no
recovery mechanism for either case in this contract, by deliberate choice
(§1 explains why a balance-based heuristic recovery mechanism was
rejected). Because `MAX_PENDING_WITHDRAWALS` is shared across every
depositor, as few as `MAX_PENDING_WITHDRAWALS` (32) stuck records
accumulated via either route permanently disable `WITHDRAW` for every
depositor in the vault, not only the ones whose own withdrawals got
stuck — see `docs/THREAT_MODEL.md` §5.4/§6 for the full consequence,
including the collusion-cost analysis added there at Gate 2b round 3.

Sorted ascending by `withdrawal_id`.

### 2.4 Conservative caps, the true worst-case state size, and why these specific numbers

```text
MAX_LEDGER_ENTRIES          = 220    (bounds genuinely new deposit rows only — see below; there is no TRANSFER method in this contract, §1)
MAX_PENDING_WITHDRAWALS      = 32    (global, shared across every depositor)
MAX_PENDING_PER_DEPOSITOR    = 4
MIN_OPERATIONAL_SHELL        = 1500000 (checked against the vault's own real SHELL holding before any withdraw may even start — see §1)
```

**These caps carry a two-round history.** `docs/PLATFORM_FINDINGS.md`
P18/P19 found a real `WITHDRAW`'s own Settle/Restore callback silently
failing to resolve once combined state complexity crossed a threshold
well short of anything discussed below. Gate 2b round 2 (TASK A) reduced
both caps (300→220, 32→12) as an empirical margin of caution while root
cause was still unconfirmed. **Gate 2b round 3 (TASK F/G) found the real
root cause**: a real-stand A/B test on never-committed spike copies —
one with `state.rs` `parse()`'s own O(n²) per-depositor validation loop
intact, one with exactly that loop removed and nothing else changed —
showed the loop's own fuel cost, not ledger rows, not rent, was the
cause (`docs/PLATFORM_FINDINGS.md` P20; all 8 previously-stuck
configurations tested resolved once the loop was removed). That loop is
now removed from `state.rs` (see its own removal comment there), and
`MAX_PENDING_WITHDRAWALS` is restored to **32** — this contract's
original architectural ceiling — directly verified safe at both 220 and
300 ledger rows, for both Settle and Restore outcomes, with no failing
point found anywhere in the range tested (P21). `MAX_LEDGER_ENTRIES`
stays at 220: ledger row count was never shown to be the actual driver,
and keeping the worst-case state smaller helps the separate
`MIN_OPERATIONAL_SHELL` margin finding (§1, P21's TASK G3). See
`REVIEW.md` entries 26 and 31–32 for the full experiment logs across
both rounds, including the "receipt count alone does not prove a
callback resolved" and "a seeded ledger row with no real backing fails
at materialization, not at the callback" methodology corrections this
investigation forced on itself along the way.

A deposit that would need to create a genuinely **new** ledger row beyond
220 existing rows is rejected (error 1508, `ledger_capacity_exceeded`). A
withdrawal request that would need a new pending row beyond the global cap
of 32, or beyond this specific depositor's own cap of 4 currently
outstanding, is rejected (error 1509, `pending_capacity_exceeded`). Neither
check ever applies to a withdrawal's own *restore* on failure (§1) — a
restore recreates a row that this same withdrawal's own debit already
removed, so the true worst-case row count is `MAX_LEDGER_ENTRIES +
MAX_PENDING_WITHDRAWALS = 252`, not 220: every one of the 32 possible
pending withdrawals could, in the worst case, need to restore a row that
doesn't currently exist, while 220 *other*, entirely unrelated rows are
already present from other depositors' activity.

**Phase 4 correction: the 252-row ceiling and the 32-pending ceiling are
simultaneously reachable, not mutually exclusive.** This section
previously reasoned that reaching 252 rows requires all 32 pending
withdrawals to have *already resolved* (via restore), and so treated
"220 rows + 32 pending" and "252 rows + 0 pending" as the only two
candidate extremes. That missed a further step: once 252 rows exist with
0 pending, a *partial* withdrawal — debiting a row to something other
than exactly zero — opens a new pending record **without removing its
row**, since Invariant I9 only removes a row a debit brings to exactly
zero, and `handle_withdraw` never checks `MAX_LEDGER_ENTRIES` (only the
two pending caps — it never adds a row). So all 32 pending slots can be
refilled on top of all 252 rows: **252 rows and 32 pending,
simultaneously, is reachable**, and is the true worst case — confirmed
by `src/handlers.rs`'s `true_worst_case_state_is_252_rows_plus_32_pending`
host test, which actually constructs it via this exact drain → refill →
restore → partial-withdraw sequence.

Worst-case state size: `91 (header) + 252 × 85 (ledger entries) + 32 × 89
(pending records) = 91 + 21,420 + 2,848 = 24,359` bytes (corrected at
Phase 4; the previous `21,639`-byte figure — 220 rows + 32 pending —
undercounted by `32 × 85 = 2,720` bytes, since it did not account for the
252-row ceiling and the 32-pending ceiling being reachable together; the
`19,859`-byte figure described round 2's unconfirmed-cause caps, and the
original `31,159`-byte figure described the pre-reduction 300/32 caps —
all now historical). This stays comfortably under the one
community-reported data point for the undocumented state-size ceiling
(open issue #20: roughly 65,594 bytes accepted, roughly 66,140 bytes
rejected), with margin of a bit under a factor of three.

**Restating the worst case in rent terms.** The byte-size-vs-rent argument
below predates both rounds' cap changes and was never the reason for
either; it is kept, updated to the current worst-case figure, because
its reasoning (why a larger cap wouldn't have been a rent problem)
remains correct regardless of which specific cap value is in force.
Open issue #21 measured storage rent on
this local stand at roughly **1 SHELL per byte per second**, collected from
an account the next time it is touched, with mainnet's policy stated to
differ (rent is charged only after 30 days of inactivity) but with no exact
mainnet formula documented anywhere. Round 6 reconfirmed this rate directly
and at scale, not merely by citing the issue: a dedicated large-state spike
account (`spike/mini-vault-big`, a 31,234-byte account — close to this
contract's own *original* 31,159-byte worst case plus a small fixed
account-record overhead) left untouched for 10,005 seconds of block-time
accrued exactly `storage_due_payment = 310,342,012`, i.e. `310,342,012 /
(31,234 × 10,005) ≈ 0.993` SHELL/byte/second — matching issue #21's
figure to within 1% (`docs/PLATFORM_FINDINGS.md`, round-6 addendum,
"rent-rate reconfirmation"). Applying that measured rate to this
contract's current, Phase-4-corrected worst-case size (252 rows, 32
pending): `24,359 bytes × 86,400 seconds/day ≈ 2,104,617,600 SHELL per
day` of accrued rent if the vault's state were at its absolute worst
case and went completely untouched for a full day (historically,
`21,639 bytes` gave `1,869,609,600` — both figures smaller than the
original 300/32-cap figure (`2,692,137,600`)) only
because `MAX_LEDGER_ENTRIES` stayed at 220 rather than being restored
toward 300, and still the same order of magnitude as issue #21's own
worked example (a 6.3 KB registry owing roughly 543 million SHELL after
one day; this vault's state is roughly 3.9 times larger at its worst
case, and the rent figure above is proportionally larger). This number is not a
reason to shrink `MAX_LEDGER_ENTRIES` further, for three reasons, stated
here rather than left implicit: first, actual rent cost scales with the
account's *actual* current state size, not with the cap it is permitted to
reach — a lightly-used vault pays proportionally little rent regardless of
what the cap is set to, so lowering the cap does nothing for the common
case; second, the one scenario where the cap size does matter for rent — a
ledger deliberately filled to capacity with dust by an attacker (§1, §5.5
of the threat model) — is already named as an accepted, permanent
limitation independent of the exact cap chosen, and lowering the cap would
only make that same griefing scenario cheaper to trigger (fewer dust rows
needed to fill a smaller cap) in exchange for a proportionally smaller
worst-case rent bill, which is not an obviously favorable trade; third, and
most importantly, the local stand's per-second accrual rate is explicitly
not known to reflect mainnet's actual pricing or long-arrears behavior at
all (issue #21 is itself an open request for the maintainers to document
this) — choosing a smaller cap by optimizing against a number that may not
apply to the deployment this contract would actually run on risks solving
the wrong problem. This open rent question is recorded below as the
**second** maintainer question — round 6 promotes a different, more basic
question about `TOPUP_METHOD_ID` itself to the top of that list (see §10),
because until it is answered, it is not established that this contract's
entire SHELL-replenishment story works outside this local stand at all.

**Open questions for the SDK maintainers, recorded here rather than
resolved** (see §10 for the first, newly-added-in-round-6 question about
`TOPUP_METHOD_ID`'s real-network viability, which is more fundamental than
this one): what is the exact mainnet rent formula (distinct from the local
stand's measured roughly-1-SHELL-per-byte-per-second, reconfirmed directly
in round 6 — see above), and what happens to an account whose accrued rent
arrears grows very large relative to its balance over a long period — is
the debt simply held indefinitely until the account is next touched and
can cover it, no matter how large it grows, or is there some other
resolution (freezing, deletion, a cap on arrears) that isn't described in
`docs/fuel-and-fees.md` or anywhere else in this repository? Round 6 adds
a second, sharper half to this same question, motivated by a direct
observation (§1): can a single call's FUEL allowance be entirely consumed
paying down accrued rent, to the point that the contract's own code never
runs and a value credit that same call was supposed to deliver is silently
dropped (observed directly this round) — and if so, is there any
documented way for a caller to know in advance whether a given call will
have enough FUEL to both pay down existing debt and actually execute, or
is the only way to find out to send it and check `raw_executor_output`/
`events` for emptiness afterward? **Round 7 adds a third question in this
same area:** `docs/fuel-and-fees.md` states mainnet rent is charged only
after 30 days of inactivity, but does not say what counts as "activity"
for resetting that clock — does *any* touch reset it (including, for
instance, a call that fails outright at materialization before the
account is genuinely charged anything, as round 7's P9b observed for an
undersized `TOPUP` attempt — `docs/PLATFORM_FINDINGS.md`'s round-7
addendum), or only a touch that results in a genuine charge or genuine
guest execution? This matters directly for this contract's own design:
if a cheap, harmless, frequently-repeatable touch (e.g., a trivial
external call that is expected to fail) is enough to reset the clock,
that would be a meaningfully different and easier operational story for
a keeper to rely on than needing a touch that is expensive or that must
succeed. **A fourth question, added at the start of implementation:** on
this local stand, a keeper can read an account's accrued-but-uncollected
rent debt directly via `avm-local-stand inspect`'s per-account
`storage_due_payment`/`storage_last_paid` fields (used throughout this
spike's rent probes) — but `inspect` is explicitly a local debugging
command, and nothing in this repository's docs says whether a real
network exposes an equivalent read path. Without one, a keeper on a real
network would have no way to know how close to delinquent the vault
actually is except by estimating elapsed time since its own last touch
and multiplying by an assumed byte-rate — itself unverified for mainnet
(the question directly above). How, concretely, is a keeper meant to
read an account's accrued rent debt on a real network?

`MIN_OPERATIONAL_SHELL`'s current value and the evidence behind it are
given in full in §1 above (final value `1,500,000`, Phase 4, correcting
Gate 2b round 3 TASK H's `1,300,000` — itself derived from the real,
implemented vault's own withdraw-chain measurements, superseding round
6's `1,600,000` placeholder, derived from vault-shaped but not
real-vault withdraw-chain measurements, not the generic forward/callback
probe used in earlier rounds) — not repeated here to avoid the two
sections drifting
out of sync the way the superseded `20,000` figure and round-5 probe
numbers did in this paragraph before round 6. The residual-risk framing is
unchanged: this is named as a residual risk in `docs/THREAT_MODEL.md`, not
papered over by the existence of the check — the check defends against the
common, easily-reached case (a vault that is simply low on SHELL), not
against an unbounded worst case this spike has no way to measure in
advance.

## 3. Method IDs

```text
NORMAL_METHOD_TAG    = 0x4D43_5654   ("MCVT")
CALLBACK_MAGIC_TAG   = 0x5744_4C43   ("WDLC")

DEPOSIT_METHOD_ID                  = NORMAL_METHOD_TAG<<32 | 1
WITHDRAW_METHOD_ID                 = NORMAL_METHOD_TAG<<32 | 2
# id 3 is permanently retired — it was TRANSFER_METHOD_ID in an earlier
# design; transfer does not exist in this contract (see §1). Left unused
# rather than reassigned, so no future revision of this document or its
# tests can be confused about what that id once meant.
# id 4 is permanently retired — it was RECLAIM_STALE_WITHDRAWAL_METHOD_ID in
# an earlier design; no time-based reclaim mechanism exists in this
# contract (see §1 and §2.3).
# id 8 is permanently retired — it was VERSION_METHOD_ID in an earlier
# design, a permissionless external version probe. It was removed once it
# became clear that, per docs/PLATFORM_FINDINGS.md's SHELL-cost finding,
# every external call costs the vault SHELL regardless of authorization,
# so a permissionless, non-essential probe is pure unmitigated exposure
# with no offsetting benefit.

# Chosen per pending withdrawal as the outbound payout's
# CallbackIntent.handler_method_id — this value is what the eventual
# callback message is delivered with as its own method_id, letting this
# contract recover which specific withdrawal a callback belongs to:
withdrawal_callback_method_id(id: u32) = CALLBACK_MAGIC_TAG<<32 | id
```

**The `NORMAL_METHOD_TAG<<32|N` formula applies to `DEPOSIT_METHOD_ID` and
`WITHDRAW_METHOD_ID` exactly as given above — these are both `Internal`-
caller-only (§4), so the numeric `method_id` on the wire is always just a
literal `u64` some other contract's own `OutboundIntent` chose directly;
there is no name-based lookup involved anywhere in that path, and the
formula is the real, final, implementation-level constant.** The same is
true of `withdrawal_callback_method_id(id)`, since this contract itself
chooses that value when constructing its own `CallbackIntent`.

**Erratum, discovered at the start of implementation: the formula does
*not* apply to the four externally-invoked methods
(`TOPUP_METHOD_ID`/`PROPOSE_OWNER_METHOD_ID`/`ACCEPT_OWNER_METHOD_ID`/
`SET_PAUSE_METHOD_ID`), and this section now gives their real values
instead of a formula.** An `avm-local-stand call --method <name>` call
resolves the declared ABI method name to a numeric `method_id` via the
host's own internal hash — not via this formula — before it is ever
embedded on the wire; `--method` has no raw-numeric form (confirmed
empirically: `--method "0x...": "ACAB method ... is absent"`). A
throwaway spike probe (`spike/methodid-probe/`, never committed) declared
each of the four method names below in its own `avm-contract.toml` and
recorded the real wire-level `method_id` the host actually produced for
each — confirmed deterministic across repeated calls, and confirmed to
depend only on the method name string, not on the calling contract's own
ACAB/descriptor bytes (the same name re-tested under a completely
different ACAB digest hashed identically). These are therefore the real,
final, implementation-level constants for these four methods — the
`NORMAL_METHOD_TAG<<32|{5,6,7,9}` values an earlier round of this
document assigned them were never reachable via the only way an external
caller can actually invoke a named method, and are retired here rather
than kept as a dead, misleading formula:

```text
TOPUP_METHOD_ID            = 0xc0aa_c68a_6cd1_7e18   (ABI name "vault.topup")
PROPOSE_OWNER_METHOD_ID    = 0xfc41_83aa_b166_1acf   (ABI name "vault.propose_owner")
ACCEPT_OWNER_METHOD_ID     = 0x8fc0_3d2a_5e71_b6df   (ABI name "vault.accept_owner")
SET_PAUSE_METHOD_ID        = 0x33c0_aeb4_3f53_60d5   (ABI name "vault.set_pause")
```

This changes nothing about any invariant, security property, or this
contract's observable behavior — it is purely a "what literal number does
the wire actually carry for this already-approved method" correction, not
a design change. It is logged here, and in `REVIEW.md`, as a judgment
call made under Hard Rule 6 rather than silently resolved: these four
values must be re-verified (the same way they were discovered) if this
contract's declared ABI method names ever change, since the hash is a
function of the name string, not of anything else about the contract.

`CODE_UPGRADE_METHOD_ID` (`0xc2c6_c51b_c610_249d`, defined in
`crates/avm-contract-sdk/src/lib.rs`) is explicitly checked for and
rejected (error 1518, `upgrade_not_supported`) before any other dispatch,
because `avm-contract-sdk`'s own output validation
(`crates/avm-contract-sdk/src/output.rs::validate_message_context`) would
otherwise reject a non-failure response to that exact method id for not
carrying a `CodeUpgradeIntent` — and this contract never carries one, since
it has no upgrade path.

**The withdrawal payout's outbound message is pinned to a fixed shape:
`method_id = 0`, `body = []` (empty).** Verifying that `method_id = 0` is
actually safe to use here required checking the exact source, not merely
asserting it. `crates/avm-contract-sdk/src/input.rs`'s
`decode_internal_message` function contains this check:

```rust
if let Some(callback) = callback {
    if read_u64_at(header, 8)? == 0 && callback.status_code == 0
        || callback.status_code != 0 && !body.is_empty()
    {
        return Err(DecodeError::InvalidValue);
    }
}
```

`read_u64_at(header, 8)` is the message's own `method_id` field. This
rejects `method_id == 0` **only when the message itself is a callback**
(`callback` is `Some(...)`) **and** that callback reports success
(`status_code == 0`). The withdrawal payout message this contract sends is
not a callback at all — it is an ordinary forward call that *requests* one
— so its own `callback` field is always `None`, and this check never
applies to it. The `withdrawal_callback_method_id(id)` values used for the
*reply* side are always tagged with `CALLBACK_MAGIC_TAG` in their upper 32
bits and therefore can never equal `0` either, so this specific restriction
cannot bite on either side of how this contract uses method ids. This was
confirmed by reading the exact source quoted above, not by running a
positive test on the stand — there was no need to, since the payout message
path never has `callback.is_some()` in the first place, so there is no
behavior to observe beyond what the source already shows conclusively.

## 4. Entry points

**Erratum cross-reference (see §3 for the full writeup):** the
`*_METHOD_ID` names in this table's first column are symbolic; the real
wire-level numeric value for `TOPUP_METHOD_ID`/`PROPOSE_OWNER_METHOD_ID`/
`ACCEPT_OWNER_METHOD_ID`/`SET_PAUSE_METHOD_ID` is the host-computed hash
of each one's declared ABI name, not the `NORMAL_METHOD_TAG<<32|N`
formula — `DEPOSIT_METHOD_ID`/`WITHDRAW_METHOD_ID` are unaffected
(`Internal`-only, §3).

**Body layout, empirically confirmed byte-for-byte, no length prefix.**
The body-length column below for `PROPOSE_OWNER`/`ACCEPT_OWNER`/
`SET_PAUSE` (36/4/5 bytes respectively) was verified directly against the
real stand, not merely assumed: a throwaway spike probe
(`spike/bodyprobe/`, never committed) declared the exact same `avm-
contract.toml` ABI shape this contract uses (`kind = "fixed_bytes"` with
the matching `len`) and recorded the guest-observed `input.message.
body()` for each method, called via the CLI with a flat hex payload of
the expected length. Result: the body arrived **exactly** as the flat
bytes supplied — `36`/`4`/`5` bytes, respectively, byte-for-byte
identical to what was sent, with **no 4-byte length prefix**. This
differs from the earlier-documented `kind = "bytes"` (variable-length)
ABI behavior (`docs/PLATFORM_FINDINGS.md` bonus findings: a `bytes`-typed
argument *is* auto-wrapped with a 4-byte LE length prefix ahead of the
raw bytes) — `fixed_bytes` carries no such prefix, consistent with its
length already being fixed and known from the ABI schema itself, so
nothing needs to be prefixed. This confirms the exact-length body checks
in this contract's implementation (`body.len() != 36/4/5`) are already
correctly matched to the real wire shape, with no SPEC change or
length-prefix stripping required.

| Method | Caller | Body | Effect |
|---|---|---|---|
| (callback, `flags & CALLBACK` set on the inbound message) | `Internal`, must be this contract's own address, and the caller must equal the specific pending record's depositor | — (the callback's own header, not a body this contract parses) | Settle or restore the referenced pending withdrawal; see §5. Gated strictly on the wire-level callback flag, never on `method_id` matching alone (Invariant I4c); additionally rejects (error 1513) if the callback's own `method_id` does not carry this contract's `CALLBACK_MAGIC_TAG` in its upper 32 bits (Gate 2a round 2 item 5), rather than blindly treating its lower 32 bits as a withdrawal id. Never checks pause state. |
| `DEPOSIT_METHOD_ID` | `Internal`; caller's `dapp_id` must equal the vault's own (error 1520 otherwise) | any | Credits `input.message.value_main()` (if greater than `0`) to the caller's `Main` ledger entry, and each non-SHELL entry of `input.message.extra()` (if its amount is greater than `0`) to the caller's corresponding `Extra(currency_id)` entry — **the message's own one-time carried amount (field group "(b)", defined in §1 above), never `input.balance_main`/`input.extra` (the account's cumulative current balance, field group "(a)", also defined in §1)** — fixed in round 7: an earlier round mislabeled this citation as "§3" (Method IDs), where no such group distinction is ever drawn; reading (a) here would credit the caller's *entire running balance* on every deposit instead of just what that one message carried, silently double-counting every prior deposit. Rejects the entire message (error 1519) if any `extra` entry has `currency_id == 2`. A message carrying nothing eligible (zero `value_main`, no non-SHELL `extra`) succeeds as a no-op. Rejected while deposits are paused (error 1506). |
| `TOPUP_METHOD_ID` | `External`; permissionless, no authorization check | must be empty (error 1501, `invalid_body`, otherwise — Gate 2a round 2 item 5: `TOPUP` has no declared ABI input type at all, so a non-empty body is always malformed) | Requires `value_main() == 0`, at least one `extra` entry present, and every `extra` entry to have `currency_id == 2` (error 1526, `topup_requires_shell_only`, otherwise). Credits nothing to any ledger row — the SHELL simply becomes part of the vault's own real account balance. Never gated by the deposits-paused flag. |
| any other method id (not a callback, not one of the methods listed in this table) | — | — | Fails cleanly with error 1002 (`unrecognized_method`), no effect whatsoever, regardless of caller type or body content. |
| `WITHDRAW_METHOD_ID` | `Internal`; caller's `dapp_id` must equal the vault's own (error 1520 otherwise); checked **first**, before even the body-shape checks below (Gate 2a round 2 item 4 — see §5) | `currency_tag(1) + currency_id(4) + amount(16)` = 21 bytes total; no destination field — the payout always goes to the caller's own address; `value_main() == 0` and an empty `extra()` are required on this inbound call (error 1501 otherwise, checked immediately after the caller-type check above, before every other check in this row — see §5) — a `WITHDRAW` call cannot itself carry SHELL or any other currency into the vault | See §5 for the full state machine. Once the caller-type and body-shape checks above pass, requires the vault's own current real SHELL holding — `input.extra` (group (a), the account's own balance, **not** `input.message.extra()`), filtered for `currency_id == 2` — to be at least `MIN_OPERATIONAL_SHELL` (error 1528, `vault_underfunded`, otherwise, with no other effect), before anything else that could mutate state. |
| `PROPOSE_OWNER_METHOD_ID` | `External`; pubkey must equal the stored `owner_pubkey` (error 1514 otherwise) | `expected_admin_seq(4) + new_owner_pubkey(32)` | Requires `expected_admin_seq == admin_seq` exactly (error 1524 otherwise). Sets `pending_owner_pubkey` to the supplied value (an all-zero value clears any currently-proposed rotation). Increments `admin_seq` by one via `checked_add` (error 1527 on the practically-unreachable overflow case). Emits an event carrying the new `admin_seq` value — if encoding that event ever fails (provably unreachable for this fixed, single-field event; error 1530, `event_encoding_failed`, otherwise), no state change applies either (Gate 2a round 2 item 5). |
| `ACCEPT_OWNER_METHOD_ID` | `External`; pubkey must equal the stored `pending_owner_pubkey`, which must itself be non-zero (error 1515 otherwise) | `expected_admin_seq(4)` | Same `admin_seq` check and increment pattern as above. Sets `owner_pubkey := pending_owner_pubkey`, then clears `pending_owner_pubkey` to all-zero. Emits an event carrying the new `admin_seq` value, with the same explicit 1530 failure path as `PROPOSE_OWNER`. |
| `SET_PAUSE_METHOD_ID` | `External`; pubkey must equal the stored `owner_pubkey` (error 1514 otherwise) | `expected_admin_seq(4) + flags(1)` | Same `admin_seq` check and increment pattern. Sets header `flags` bit 0 directly to the supplied value (absolute assignment, not a toggle); rejects the call (error 1516) if any other bit in the supplied byte is set. Emits an event carrying the new `admin_seq` value, with the same explicit 1530 failure path as `PROPOSE_OWNER`. |

`WITHDRAW`, `PROPOSE_OWNER`, `ACCEPT_OWNER`, and `SET_PAUSE` all require
`value_main() == 0` and an empty `extra` list on the inbound call —
a non-zero value or any carried extra currency on any of these is a
validation error (error 1501, `invalid_body`), never silently merged with
a deposit. `TOPUP_METHOD_ID` has the opposite requirement, stated in its
own row above. There is no `TRANSFER` method and no `VERSION` method in
this contract — both existed in earlier designs and were removed (§1); no
entry exists for either.

**There is no such thing as a genuine "no-change success" at the wire
level — checked directly against the SDK's own source, not assumed (Gate
2a round 3 item 5).** `crates/avm-contract-sdk/src/output.rs`'s
`validate_message_shape` requires every `status_code == 0` output to
carry a real `state_update`:

```rust
if output.status_code == 0 {
    let update = output.state_update.as_ref().ok_or(EncodeError::MissingStateUpdate)?;
    ...
}
```

there is no path to a successful `MessageOutput` with `state_update:
None` — `EncodeError::MissingStateUpdate` rejects it before the encoded
output ever reaches the runtime. This means `TOPUP_METHOD_ID` (which
never touches this contract's own ledger/header content in any way) and
`DEPOSIT_METHOD_ID`'s own no-op path (nothing eligible carried) both
still construct and return a full `StateUpdate::FullReplace` of the
account's state bytes — byte-for-byte identical to what was already
there — purely because the SDK mandates it on every success, not because
this contract chooses to "rewrite" anything. `handlers.rs`'s
`success_with_state` already does exactly this unconditionally for every
success path, so no code change was needed here; this is a
documentation correction of what that already-correct code was doing,
not a behavior change.

## 5. Withdrawal state machine, and the DEPOSIT check order

```text
request (any caller, WITHDRAW_METHOD_ID) — the full, final check order,
every step before any state mutation. Grouped in two phases: phase A
validates the request's own shape and legitimacy (body, content,
*who* may even send this), entirely independent of the vault's own
resource/funding state; phase B checks resource/state conditions.
This grouping is deliberate, not incidental: it guarantees a
malformed-or-wrong-caller-type request gets the *same* rejection code
regardless of how (under)funded the vault happens to be at that
moment — the property `SPEC.md` §8's dispatch-pinning tests rely on.

  Phase A — request shape and legitimacy:
  -> require Caller::Internal(source) — i.e. this is not an External
     caller (checked FIRST, Gate 2a round 2 item 4 — before even the body
     shape/content checks below, let alone any resource/state condition;
     "is this message even allowed to invoke this method at all" does not
     depend on anything decoded from the body, so there is no reason to
     parse an untrusted body before confirming the caller is even the
     right kind. This document's own history has now relocated this check
     twice: an earlier draft checked Invariant I17 first, making an
     External caller's rejection code depend on the vault's funding; the
     next draft checked body-shape first, making it depend on the body
     the caller happened to send; this ordering removes both
     dependencies);
     otherwise: failure (1500, invalid_caller), no state change
  -> require body.len() == 21 (currency_tag(1) + currency_id(4) + amount(16));
     otherwise: failure (1501, invalid_body), no state change
  -> require input.message.value_main() == 0 and input.message.extra()
     is empty (a WITHDRAW call cannot itself carry SHELL or any other
     currency into the vault);
     otherwise: failure (1501, invalid_body), no state change
  -> parse currency_tag, currency_id, amount from the body
  -> require currency_tag is Main(0) or Extra(1), and if Main, that
     currency_id == 0;
     otherwise: failure (1503, invalid_currency_selector), no state change

  Phase B — resource/state conditions, in this order:
  -> require the vault's own current real SHELL holding (input.extra,
     group "(a)", currency_id == 2) to be >= MIN_OPERATIONAL_SHELL
     (Invariant I17);
     otherwise: failure (1528, vault_underfunded), no state change
  -> require source.dapp_id == the vault's own dapp_id;
     otherwise: failure (1520, cross_dapp_caller_forbidden), no state change
  -> require amount > 0; otherwise: failure (1504, invalid_amount)
  -> require the caller's own ledger entry for the requested currency to
     exist and hold at least `amount`;
     otherwise: failure (1505, insufficient_balance), no state change
  -> require this request would not exceed MAX_PENDING_WITHDRAWALS
     globally, nor MAX_PENDING_PER_DEPOSITOR for this specific depositor;
     otherwise: failure (1509, pending_capacity_exceeded), no state change
  -> DEBIT the caller's ledger entry by `amount` (checks-effects-interactions:
     this effect is committed before the outbound interaction is even sent);
     if this debit brings the entry to exactly 0, remove the row entirely (I9)
  -> allocate a fresh withdrawal_id via checked_add on next_withdrawal_id;
     on overflow: failure (1522, withdrawal_id_exhausted), no state change
     at all (this check happens before the debit above is finalized into
     the returned state update)
  -> insert a new pending record keyed by (withdrawal_id, source, currency, amount)
  -> emit exactly one OutboundIntent{
         destination = source,
         value_main-or-extra = amount (in whichever of value_main/extra
             matches the requested currency_tag/currency_id),
         fuel_fee_shells = 0,
         method_id = 0,
         body = []
     }, together with a CallbackIntent targeting that same outbound intent,
     handler_method_id = withdrawal_callback_method_id(withdrawal_id)
  -> success; the call that initiated this returns successfully regardless
     of what happens next — resolution of the pending record happens later,
     as described below

callback arrives (flags & CALLBACK set; method_id = withdrawal_callback_method_id(id) for some id)
  -> require method_id's upper 32 bits == CALLBACK_MAGIC_TAG (Gate 2a
     round 2 item 5) — a second, independent check on the method_id's own
     shape, not a replacement for the wire-level callback-flag dispatch
     gate above (Invariant I4c);
     otherwise: failure (1513, invalid_callback), no state change
  -> look up the pending record for `id`;
     if none exists (never existed, or already resolved and removed):
         failure (1510, withdrawal_not_found), no state change
  -> require Caller::Internal(source) on this callback message to equal
     the pending record's own depositor address exactly;
     otherwise: failure (1513, invalid_callback), no state change
  -> read status_code ONLY from the callback's own header
     (CallbackHeader.status_code) — never inferred from the callback
     message's body, its carried value_main/extra, or anything else:
       if status_code == 0:
           remove the pending record entirely (Settled — the funds were
           already genuinely delivered to the depositor; there is nothing
           further to do or to credit)
       if status_code != 0:
           CREDIT the depositor's ledger entry for this withdrawal's
           currency by exactly `amount` (recreating the row if it was
           removed by I9 when this withdrawal was first created — this
           restore is never subject to MAX_LEDGER_ENTRIES, see §2.4),
           unconditionally — there is no check on the callback's own
           carried value, because that value is not a real credit at all
           (see §1's full explanation); then remove the pending record
           (Restored). **The credit itself can fail — a `u128` overflow,
           practically unreachable since Invariant I2's deposit-time
           precheck is specifically designed to make it impossible
           (§7) — in which case this returns failure (1529,
           ledger_amount_overflow, the same code a DEPOSIT-time ledger
           overflow uses), not 1513 (Phase 4 adversarial review, finding
           3: this contract previously, incorrectly, returned 1513 here,
           the same code the caller-mismatch check above uses, despite
           1513 being documented elsewhere in this file — §6 — as
           exclusively a caller/depositor mismatch).** With that overflow
           case now correctly coded as 1529, the caller-mismatch check
           above genuinely is the only way this branch returns 1513.
  -> this entire branch never checks the deposits-paused flag, or any
     other pause state, under any circumstance

[there is no reclaim or recovery mechanism for a withdrawal whose payout
 produces no callback at all — see §2.3 and the "L" liveness section of
 docs/THREAT_MODEL.md for the two distinct routes that can cause this
 (same-DApp-but-split; SHELL-starved dropped callback) and why no
 mechanism exists to recover from either one in this contract, and why
 a small number of them accumulating (as few as 32, the global pending
 cap) disables WITHDRAW for every depositor, not only the stuck ones]
```

**The `DEPOSIT` check order.** No partial credit is ever observable on any
failure path (Invariant I6 — see below for exactly why), regardless of how
far this handler's own local mutations got before it decided to fail:

```text
request (any caller, DEPOSIT_METHOD_ID)
  -> require Caller::Internal(source);
     otherwise: failure (1500, invalid_caller), no state change
  -> require source.dapp_id == the vault's own dapp_id;
     otherwise: failure (1520, cross_dapp_caller_forbidden), no state change
  -> require deposits are not currently paused (header flags bit 0);
     otherwise: failure (1506, deposits_paused), no state change —
     checked here, before the SHELL-in-extra content check below,
     because "we are not accepting deposits right now" is a blanket
     gate that should apply uniformly, before validating the specific
     content shape of a call that would be rejected anyway once
     unpaused (a deliberate ordering choice, not derived from any
     platform behavior — see REVIEW.md)
  -> require input.message.extra() contains no entry with
     currency_id == 2 (SHELL); checked atomically against the whole
     message — if present anywhere, the entire message is rejected,
     none of its other, otherwise-valid currencies are partially
     credited;
     otherwise: failure (1519, shell_not_custodied), no state change
  -> compute every (depositor, currency) credit this message implies:
     input.message.value_main() (field group "(b)") if > 0, credited
     to the caller's Main entry; each entry of input.message.extra()
     with amount > 0, credited to the caller's matching
     Extra(currency_id) entry
  -> if this set of credits is empty (nothing eligible carried):
     succeed, no change to any ledger content — but still carrying a
     `StateUpdate::FullReplace` of the unchanged bytes, not literally no
     state_update at all; see the note on this below §4's entry-points
     table (Gate 2a round 3 item 5)
  -> count how many of these credits would create a genuinely NEW
     ledger row (key not already present); ONLY IF this count is > 0
     (Gate 2a round 2 item 1 — an all-existing-row deposit must succeed
     even if current_ledger_row_count already exceeds MAX_LEDGER_ENTRIES,
     reachable only via a restore bypassing that cap, since it adds no
     row at all), require current_ledger_row_count + new_row_count <=
     MAX_LEDGER_ENTRIES — checked atomically, before applying ANY credit
     in this message, so a cap breach caused by one credit rejects the
     whole message, never just that one credit;
     otherwise: failure (1508, ledger_capacity_exceeded), no state change
  -> for each credit, in one pass, mutating the real ledger as it goes
     (Gate 2a round 3 item 1 — see below for why this is still atomic):
       -> require existing_ledger_balance_for_key + pending_amount_for_key
          + this_credit's_amount fits in u128, via checked_add end to end
          (§7 I2 — a pending withdrawal on this same key has already been
          debited out of the ledger, but could still be restored back
          onto it later, so it must be counted here too, not just the
          ledger's own current, already-reduced balance);
          otherwise: failure (1529, ledger_amount_overflow), no state
          change (see below)
       -> apply this credit via checked_add (cannot itself fail: the
          check above already proved existing_ledger_balance_for_key +
          this_credit's_amount fits, which is all this step's own
          internal check computes — the pending-aware check above is
          strictly tighter)
     This is a single pass, not precheck-everything-then-apply-everything:
     every credit in one DEPOSIT message targets a pairwise-distinct key
     (`avm-contract-sdk` itself decodes `extra()` with strictly ascending,
     duplicate-free `currency_id`s, and `value_main` is a structurally
     separate Main key), so there is nothing to gain from checking every
     credit before applying any of them.
  -> success
```

**Atomicity of every failure path above — including the one inside the
per-credit loop, which really can be reached after this message's *own*
local ledger mutations for earlier, non-overflowing credits have already
happened — is NOT a property of check ordering.** It is Invariant I6
(§7): a failure's `MessageOutput` always carries `state_update: None`,
independently enforced by the SDK's own output validation
(`EffectsOnFailure`) rejecting any encoded output pairing a non-zero
`status_code` with a state change before it ever reaches the runtime.
Whatever this handler's own local copy of the ledger looks like at the
moment it decides to return a failure is irrelevant: that value is simply
never referenced by the `MessageOutput::failure` it returns, and is
dropped along with the rest of its stack frame. (An earlier draft of this
document — and of `src/handlers.rs`'s own comments — described this
atomicity as resulting from validating every credit before applying any
of them; that framing was corrected in Gate 2a review round 3, item 1,
and is now stated the way it actually works.)

## 6. Error codes (`status_code`, all values `>= 1000` per the SDK's reserved range of `1..1000`)

| Code | Name | Meaning |
|---|---|---|
| 1002 | `unrecognized_method` | method id matches none of this contract's defined methods, and the message is not a callback; no effect |
| 1500 | `invalid_caller` | wrong caller kind (`Internal` vs `External`) for the dispatched method |
| 1501 | `invalid_body` | malformed, wrong-length, or improperly-shaped body for the dispatched method (includes a non-zero `value_main`/non-empty `extra` on a method that requires neither) |
| 1502 | `invalid_state` | the stored state blob failed its own defensive validation on read (bad magic, non-zero reserved bytes, unsorted entries, or similar) |
| 1503 | `invalid_currency_selector` | `currency_tag` was not `0` or `1`, or `currency_id` was non-zero while `currency_tag` was Main |
| 1504 | `invalid_amount` | a zero amount was supplied to `WITHDRAW` |
| 1505 | `insufficient_balance` | a withdraw amount exceeds the caller's tracked ledger balance for that currency — this is also exactly what withdrawing `currency_id == 2` (SHELL) hits, since no ledger entry for SHELL can ever exist (§8, test 6) |
| 1506 | `deposits_paused` | a `DEPOSIT` call arrived while the deposits-paused flag is set |
| 1508 | `ledger_capacity_exceeded` | a genuinely new deposit row would exceed `MAX_LEDGER_ENTRIES`; never returned for a withdrawal restore |
| 1509 | `pending_capacity_exceeded` | a withdrawal request would exceed the global or the per-depositor pending-withdrawal cap |
| 1510 | `withdrawal_not_found` | no outstanding pending record exists for the referenced withdrawal id, whether because it never existed or because it already resolved |
| 1513 | `invalid_callback` | a callback's own caller does not match the referenced pending record's depositor — the only condition this code is used for (Phase 4 adversarial review, finding 3 fixed a prior mix-up where a restore's own `u128` overflow, below, also returned this code) |
| 1514 | `unauthorized_owner` | caller's pubkey does not match the stored `owner_pubkey` for an owner-only method |
| 1515 | `unauthorized_pending_owner` | caller's pubkey does not match the stored `pending_owner_pubkey`, or no rotation is currently proposed |
| 1516 | `invalid_pause_flags` | a `SET_PAUSE` call set a reserved bit beyond bit 0 |
| 1517 | `invalid_destination` | a transfer recipient was the zero address — **retained in this table for historical reference only; no code path in this contract can ever trigger it**, since `TRANSFER` does not exist and no other method accepts a caller-supplied destination at all |
| 1518 | `upgrade_not_supported` | `method_id == CODE_UPGRADE_METHOD_ID` |
| 1519 | `shell_not_custodied` | a `DEPOSIT` call's `extra` includes an entry with `currency_id == 2` |
| 1520 | `cross_dapp_caller_forbidden` | the caller's own `dapp_id` does not match the vault's own `dapp_id`, on either `DEPOSIT` or `WITHDRAW` |
| 1522 | `withdrawal_id_exhausted` | `next_withdrawal_id` would overflow `u32` on `checked_add` |
| 1524 | `stale_admin_sequence` | an owner/pending-owner call's supplied `expected_admin_seq` does not exactly match the header's current `admin_seq` |
| 1526 | `topup_requires_shell_only` | a `TOPUP_METHOD_ID` call carried a non-zero `value_main`, carried an `extra` entry with `currency_id != 2`, or carried no `extra` entries at all |
| 1527 | `admin_sequence_exhausted` | `admin_seq` would overflow `u32` on `checked_add` |
| 1528 | `vault_underfunded` | the vault's own current SHELL holding is below `MIN_OPERATIONAL_SHELL` at the start of a `WITHDRAW` call |
| 1529 | `ledger_amount_overflow` | a `DEPOSIT` credit would overflow `u128` on `checked_add` once the precheck also accounts for the same key's outstanding pending amount (§7 I2) — requires a currency balance summing to near `u128::MAX` across ledger + pending together, extreme but not merely hypothetical the way a restore's own overflow is (the precheck exists specifically to make that restore-time case unreachable); rejects the entire `DEPOSIT` message atomically, no partial credit. **Also used for the restore-time overflow itself** (§5, `handle_callback`'s `status_code != 0` branch) — the same practically-unreachable condition this precheck exists to prevent, reusing this code rather than 1513 (Phase 4 adversarial review, finding 3). |
| 1530 | `event_encoding_failed` | `encode_event_fields` returned an error while building a `PROPOSE_OWNER`/`ACCEPT_OWNER`/`SET_PAUSE` event — provably unreachable for this contract's own fixed, single-field event (Gate 2a round 2 item 5; `REVIEW.md`), kept as an explicit, dedicated failure path rather than silently emitting a default/empty event as part of an apparently-successful admin action; no state change |

**Retired codes, listed for historical reference only, triggerable by no
code path in this contract:** 1507 `transfers_paused` (there is no
`TRANSFER` method and therefore nothing for a "transfers paused" flag to
gate — the header's `flags` bit 1 is simply unused and must be zero); 1511
and 1512 (an earlier design's "already finalized" and "not yet stale"
errors for a time-based reclaim mechanism that does not exist in this
contract); 1521 `insolvent_reclaim` (an earlier design's solvency check on
that same removed reclaim mechanism).

## 7. Invariants

- **I1.** For every currency key `K` other than SHELL: the vault's actual
  account balance for `K` — `input.balance_main` for `K = Main`, or the
  matching entry of `input.extra` for `K = Extra(currency_id)`
  (`crates/avm-contract-sdk/src/input.rs` lines 312–313 — the account's own
  current holdings, never the inbound message's carried amount) — is
  always `>=` the sum of every ledger entry for `K` plus the sum of every
  pending-withdrawal record for `K`. This is exactly what a Phase 3 test
  checking I1 must read to verify it: the account-balance fields, not
  anything derived from a message body. SHELL (`currency_id == 2`) is
  excluded from this invariant entirely and unconditionally — it is never
  ledgered under any circumstance, so there is nothing to compare it
  against.
- **I2.** No balance, anywhere in this contract's own bookkeeping, ever goes
  negative or silently overflows; every arithmetic operation on a balance,
  a counter, or a sequence number uses `checked_*` operations, never
  wrapping or saturating silently into an incorrect value. For `DEPOSIT`
  specifically (Gate 2a round 2 item 2), this means the overflow precheck
  must account for more than the ledger's own current balance for a given
  key: a pending withdrawal on that same key has already been *debited*
  out of the ledger (so it does not appear in `ledger_balance`), but it
  can still be *restored* back onto that same key later, on a callback
  with `status_code != 0` (§5). The true worst-case total a key could ever
  need to hold again is therefore `existing_ledger_balance +
  pending_amount_for_key + this credit's own amount`, computed via
  `checked_add` end to end; checking only `existing_ledger_balance +
  amount` (ignoring outstanding pending) would let a DEPOSIT succeed now
  and only overflow later, at restore time, which is exactly the gap this
  invariant forbids.
- **I3.** A given withdrawal is settled or restored **at most once**. This
  is enforced by removing its pending record from state the moment it
  resolves (settled or restored) rather than retaining a terminal marker;
  a monotonic, never-reused `withdrawal_id` is what makes "no record
  found" always mean "not currently outstanding," whether that's because
  the id never existed or because it already resolved — there is no third
  possibility to distinguish.
- **I4.** Only the depositor who holds a given ledger entry — verified as
  `Caller::Internal(source)` on the inbound message matching the entry's
  own stored address exactly — can ever debit that entry, via `WITHDRAW`.
- **I4b.** (Re-added at the start of implementation — this was already
  cited in prose throughout this document and in `docs/THREAT_MODEL.md`
  §5.2, but had never actually been given its own bullet here.) A pending
  withdrawal can only be settled or restored by a callback whose own
  `Caller::Internal(source)` equals that withdrawal's own recorded
  depositor exactly (§5); a callback from any other caller — even one
  genuinely addressed to this contract, with a valid `cause_address` —
  fails as `invalid_callback` (1513) with no effect, regardless of which
  `withdrawal_id` it references.
- **I4c.** (Re-added alongside I4b — cited by name in §8 test 1 but never
  previously defined here.) The callback-settlement branch is reached
  only via the wire-level callback flag (`input.message.is_callback()`),
  never via `method_id` pattern-matching alone: a plain, non-callback
  message whose `method_id` happens to numerically collide with some
  `withdrawal_callback_method_id(id)` falls through to ordinary dispatch
  and fails as `unrecognized_method` (1002), never reaching settlement
  logic, regardless of its body content.
- **I4d.** (Re-added alongside I4b/I4c — this was always true by
  construction and discussed at length in §1 and §5.10, but, like I4b/I4c,
  had never been captured as its own invariant bullet.) A withdrawal's
  payout destination is always exactly the account that requested it
  (`Caller::Internal(source)` on the original `WITHDRAW` call); there is
  no field anywhere in this contract's body formats through which a
  caller can name a different destination, for any method, under any
  circumstance.
- **I5.** Pausing deposits never traps any depositor's funds under any
  circumstance (withdrawals are never gated by the pause flag, and neither
  is the callback-settlement branch that resolves them — see I14 below).
  The owner role has no entry point, direct or indirect, that can move,
  redirect, or seize any depositor's ledger balance; the owner-key blast
  radius is exactly "pause deposits, and/or rotate the owner," stated in
  full in §1.
- **I6.** Every failure (`status_code != 0`) leaves the account's `state`
  byte-for-byte unchanged from immediately before that call, and emits no
  outbound intents, no callback intents, and no event intents — this is
  enforced both by this contract's own logic and, independently, by the
  SDK's own `EffectsOnFailure` output validation, which would reject an
  encoded output combining a non-zero status code with any such effect
  before it ever reached the runtime.
- **I7.** A replayed or duplicated callback for a given withdrawal cannot
  double-credit or double-restore that withdrawal's ledger entry — this
  follows directly from I3: once a withdrawal's pending record is removed,
  any further reference to that same withdrawal id, genuine replay or
  otherwise, finds nothing and fails safely as `withdrawal_not_found`.
  Duplicate delivery of the *original* deposit or withdraw *request*
  message (as opposed to its callback) remains an assumed platform
  guarantee (`docs/THREAT_MODEL.md` §3), not something this contract
  independently re-implements protection against.
- **I8.** `ledger_count` never exceeds `MAX_LEDGER_ENTRIES +
  MAX_PENDING_WITHDRAWALS` (252, accounting for every possible pending
  withdrawal's restore recreating a row beyond the 220-row cap that bounds
  genuinely new rows — §2.4). `pending_count` never exceeds
  `MAX_PENDING_WITHDRAWALS` (32) globally, and no single depositor's count
  of currently-outstanding pending withdrawals ever exceeds
  `MAX_PENDING_PER_DEPOSITOR` (4). Any operation that would breach one of
  these bounds is rejected atomically, before any mutation of state begins
  — never partially applied.
- **I9.** No ledger entry with `amount == 0` is ever stored; a debit that
  brings an entry to exactly zero removes that entry from the state blob
  in the same update, rather than leaving a zero-value row behind.
- **I10.** No ledger entry ever has `currency_tag == Extra` with
  `currency_id == 2` — SHELL is never represented as a ledger row under
  any circumstance. This does not mean a deposit carrying SHELL is
  partially accepted with the SHELL portion silently dropped — `DEPOSIT`
  rejects such a message in its entirety (error 1519); SHELL can only ever
  enter the vault's real balance via `TOPUP_METHOD_ID`, which by
  definition never touches the ledger at all.
- **I11.** A ledger credit issued in response to a failure callback always
  restores exactly the `amount` recorded in that withdrawal's own pending
  record — never a value read from the callback itself, and never gated by
  any check on what the callback claims to carry. This cannot over-credit,
  because the amount restored is always exactly what Invariant I9 removed
  when that same withdrawal was first created, never a separately-supplied
  or externally-influenced number.
- **I12.** `next_withdrawal_id` is advanced only via `checked_add`; on the
  overflow case, a withdraw request fails (error 1522) before any state
  mutation begins, and the counter never wraps or reuses a previously-used
  value.
- **I13.** Every owner or pending-owner call's supplied
  `expected_admin_seq` must equal the header's current `admin_seq` exactly,
  checked before that call has any other effect whatsoever; `admin_seq`
  itself increments by exactly `1`, via `checked_add`, on every successful
  admin call, and never changes otherwise. A replayed, byte-identical old
  admin call therefore always fails once `admin_seq` has moved past the
  value it carries, regardless of whether the underlying platform message
  itself was ever deduplicated.
- **I14.** The callback-settlement branch — settling on success, restoring
  on failure — never consults the header's `flags` byte under any
  circumstance. A withdrawal that is pending at the moment the owner pauses
  deposits resolves exactly as it would have resolved had deposits never
  been paused at all.
- **I15.** `TOPUP_METHOD_ID` never creates, modifies, or removes any ledger
  row, regardless of the amount of SHELL it carries; it is the only entry
  point in this contract that is never gated by the deposits-paused flag
  (`deposits_paused` as an error condition applies to `DEPOSIT` only).
- **I16.** `admin_seq` is advanced only via `checked_add`; on the
  practically-unreachable `u32::MAX` overflow case, an admin call fails
  (error 1527) before any mutation — this mirrors Invariant I12's treatment
  of `next_withdrawal_id` exactly, for the same reason.
- **I17.** `WITHDRAW_METHOD_ID` never debits any ledger entry, allocates any
  withdrawal id, or sends any outbound intent unless the vault's own
  current real SHELL holding (read from `input.extra`, the account's own
  current balance, filtered for `currency_id == 2` — not anything
  ledgered, since SHELL is never ledgered) is at least
  `MIN_OPERATIONAL_SHELL` at the moment the call begins; otherwise the call
  fails immediately (error 1528) with no effect of any kind. This check is
  a heuristic defense against a withdrawal being accepted and then never
  resolving purely because the vault itself could not afford to see its
  own requested callback through (`docs/PLATFORM_FINDINGS.md` probe P5) —
  it is not a formal guarantee that every accepted withdrawal will
  complete, since accrued storage rent can in principle exceed any fixed
  threshold (§2.4's rent discussion).

## 8. Phase 3 tests explicitly required by this specification

1. **Callback-space method id collision.** Send a plain internal message
   — `flags & CALLBACK` **not** set — whose `method_id` exactly equals
   `withdrawal_callback_method_id(id)` for a real, currently-pending `id`.
   Assert it fails as `unrecognized_method` (1002), the pending record for
   `id` is left completely untouched, and no credit, debit, or removal
   occurs anywhere. This is the direct regression test for Invariant I4c's
   guarantee that method-id collision alone, without the wire-level
   callback flag, can never reach settlement logic.
2. **Malicious destination: forward then fail.** Using the sanctioned
   failure-flow-hook pattern already established by this repository's own
   `contract-b` example (an environment-variable-gated hook, inert in
   normal builds), build a depositor fixture whose own account, upon
   receiving a vault payout, attempts to construct a `MessageOutput` that
   both emits its own outbound intent *and* reports failure in the same
   response. Assert the SDK's own `EffectsOnFailure` output validation
   rejects this before it can ever reach this vault as a callback at all
   (observed on the real stand as `"VM status 43"` — `docs/
   PLATFORM_FINDINGS.md` addendum B) — i.e., assert this vault never has to
   defend against this shape itself, because the platform already
   forecloses it, and assert the test would fail loudly, not silently
   pass, if some future platform change ever stopped rejecting it.
   **NOT DONE as a dedicated Phase 3 regression test (Gate 2b round 2
   TASK C):** this test's own assertion requires exercising the SDK's
   output-shape validation itself — a thing that happens at the real
   platform/VM boundary, not inside this vault's own handler functions —
   so it is not reachable from a host-level unit test the way tests 2's
   siblings are; it needs a real `avm-local-stand` run against a new
   depositor fixture built with the sanctioned `AVM_E2E_FAILURE_FLOW`-
   gated hook pattern, which was not built this round. Residual risk is
   low, not zero: addendum B's own observation — cited directly above —
   already exercised the *identical* SDK validation path (`"call
   rejected canonical operation: local call returned VM status 43"`) via
   a differently-constructed probe, and explicitly confirmed the
   rejection is "a property of the destination's own returned output
   shape, independent of who called it," i.e. not specific to the probe
   contract that happened to trigger it. So the underlying platform
   guarantee this test exists to confirm has genuinely been observed on
   the real stand — just not through this specific depositor-fixture
   shape, and not as a test that re-runs automatically in this crate's
   own `cargo test` suite to catch a future platform regression. That
   gap (no regression coverage, not "unconfirmed behavior") is the
   residual risk left open here.
3. **Restore bypasses the ledger cap, tested at the exact boundary (corrected
   in round 6 — an earlier draft of this test miscounted the row total after
   the victim's withdrawal; cap values updated to 220/219/221 at Gate 2b
   round 3 TASK H, from the original 300/299/301 figures — both are
   structurally the same test, just against the current vs. the original
   `MAX_LEDGER_ENTRIES` value).** Fill the ledger to exactly
   `MAX_LEDGER_ENTRIES` (**220**) entries via ordinary deposits from distinct
   depositors, including the eventual victim as one of those 220. Have the
   victim withdraw their *entire* balance of some currency, debiting their
   row to exactly zero, which Invariant I9 removes — the ledger is now at
   **219** rows (220 minus the victim's now-removed row), *not* 220. With the
   ledger at 219, have a brand-new depositor make their first deposit,
   bringing the ledger back up to exactly 220 rows — confirm this deposit
   succeeds normally (220 is, at this point, the new-row count, not a
   restore, and the cap has not yet been exceeded). With the ledger now at
   220 and the victim's withdrawal still pending, deliver a failure callback
   for it and assert the restore succeeds, recreating the victim's row as a
   **221st** row — i.e., assert `ledger_capacity_exceeded` is never returned
   for a restore, specifically at the exact moment the ledger is already
   full from *other*, unrelated depositors' activity in the interim, not
   merely "at the cap" in the abstract. (Implemented in
   `src/handlers.rs::restore_bypasses_ledger_cap_at_exact_boundary`, which
   already uses these current 220/219/221 values.)
4. **32 dust withdrawals to undeployed addresses is no longer possible.**
   A direct regression test for an earlier design's sharpest griefing
   finding: attempt to construct the scenario of filling
   `MAX_PENDING_WITHDRAWALS` by withdrawing to many distinct,
   never-deployed addresses, permanently occupying every pending slot with
   no possible resolution. Confirm this attack cannot even be *expressed*
   in this contract anymore — `WITHDRAW`'s body has no destination
   argument at all (§4); every withdrawal necessarily targets the caller's
   own, already-existing account, which has, by definition, already proven
   it exists by sending the withdraw request itself.
5. **A depositor contract that rejects `method_id = 0`.** Using the same
   sanctioned failure-flow-hook pattern as test 2, build a depositor
   fixture whose own `process_message` dispatch fails on any method id it
   doesn't explicitly recognize — ordinary, expected behavior for a
   contract that only understands its own ABI and nothing else. Have it
   deposit into the vault, then withdraw. Assert: the payout fails at the
   point of delivery to this depositor's own account; the resulting
   failure callback correctly and unconditionally restores the depositor's
   ledger balance (Invariant I11); and the depositor retains full access
   to their funds and can successfully re-request the withdrawal once
   their own contract accepts the transfer (or via some other means of
   accepting a plain value transfer), with no funds lost at any point.
6. **Withdraw of `currency_id == 2` fails.** Request a withdrawal with
   `currency_tag = Extra`, `currency_id = 2`, any positive amount. Assert
   it fails with `insufficient_balance` (1505) — not any
   SHELL-specific error code — because there is no code path anywhere in
   this contract by which a ledger entry for SHELL could ever come to
   exist, so this must fail the exact same way withdrawing any other
   currency the caller never deposited would fail.
7. **Deposit containing SHELL is rejected whole — a host-level unit test
   with a hand-crafted `MessageInput`, not a stand end-to-end scenario.**
   A `DEPOSIT` call carrying a non-zero `value_main` and/or one or more
   other, legitimate currencies, *and* an `extra` entry with
   `currency_id == 2`, fails in its entirety (`shell_not_custodied`,
   1519) — assert that none of the other, otherwise-valid currencies
   carried in the same message are partially credited; the rejection is
   atomic, all-or-nothing. **This must be constructed directly against
   this contract's own `process_message` with a synthetic `MessageInput`
   carrying that `extra` shape, not driven through `avm-local-stand`**:
   `../SPEC.md` §1 and `docs/PLATFORM_FINDINGS.md` (P1, reconfirmed in
   round 6's currency round-trip probe) establish that no guest contract
   can ever construct an `OutboundIntent` carrying `currency_id == 2` in
   the first place, so the real stand has no way to make an internal
   message carrying SHELL actually exist to deliver to this `DEPOSIT`
   handler — this test exists to verify the handler's own defensive
   check behaves correctly on an input the real platform is believed
   never to produce, not to reproduce a scenario the stand can enact
   end-to-end.
8. **Topup rejects mixed or empty content.** A `TOPUP_METHOD_ID` call
   carrying any non-zero `value_main` fails (1526). A call carrying any
   `extra` entry whose `currency_id != 2` fails (1526), even if a valid
   `currency_id == 2` entry is also present in the same call. A call
   carrying no `extra` entries at all fails (1526). A call carrying one or
   more `extra` entries, every one of which has `currency_id == 2`, and no
   `value_main`, succeeds with no ledger change of any kind. A topup call
   succeeds even while deposits are currently paused (Invariant I15).
9. **Callback path ignores pause.** With deposits currently paused,
   deliver both a success callback and a failure callback, for two
   separate pending withdrawals belonging to two different depositors.
   Assert both resolve exactly as they would have resolved had deposits
   never been paused — the success case settles normally, and the failure
   case restores the ledger normally (Invariant I14).
10. **Admin replay tests.**
    - Pause deposits (consuming `admin_seq = N`, advancing it to `N+1`),
      then unpause (consuming `N+1`, advancing to `N+2`). Replay the
      *original* pause call — still carrying the stale value `N` — and
      assert it fails with `stale_admin_sequence` (1524), leaving the
      currently-unpaused state intact rather than re-pausing deposits.
    - Owner rotation A→B→A: A proposes B (`seq=N`, advancing to `N+1`), B
      accepts (`seq=N+1`, advancing to `N+2`), B proposes A back
      (`seq=N+2`, advancing to `N+3`), A accepts (`seq=N+3`, advancing to
      `N+4`). Replay A's very first propose-B call from the start of this
      sequence — still carrying the long-stale value `N` — and assert it
      fails with `stale_admin_sequence` rather than re-triggering any
      proposal, regardless of how much time or how many other admin calls
      have elapsed since.
11. **Reentrancy: a depositor whose own payout handler immediately sends a
    new withdraw and a new deposit back to the vault, in the same
    execution that's handling its own incoming payout.** Using the same
    sanctioned failure-flow-hook pattern as tests 2 and 5, build a
    depositor fixture whose `method_id = 0` handler, upon receiving its
    payout, immediately constructs two new outbound intents back to the
    vault — one a new withdraw request, one a new deposit — within that
    same execution, before that execution's own success is even reported
    back to the vault as a callback (mirroring `docs/
    PLATFORM_FINDINGS.md` probe P4's `ReentrantDepositor` fixture exactly).
    Assert: the reentrant withdraw is evaluated against the
    already-correctly-debited ledger balance from the original withdrawal
    — it fails with `insufficient_balance` if it would exceed what
    genuinely remains, or succeeds correctly against the reduced balance
    otherwise, but is never evaluated against a stale, pre-debit balance;
    the reentrant deposit succeeds normally; and the original withdrawal's
    own pending record is still present — not yet settled — for the
    entire duration of the depositor's own execution, resolving only after
    that execution, including its reentrant sends, fully completes. This
    matches probe P4's directly observed ordering exactly.
    **NOT DONE as a dedicated Phase 3 regression test (Gate 2b round 3
    TASK I), recorded explicitly rather than silently dropped — same
    treatment as test 2 above.** This requires the same sanctioned
    `AVM_E2E_FAILURE_FLOW`-style real-stand depositor fixture test 2
    needed, now additionally constructing two outbound intents (not one)
    from within its own `method_id = 0` handler — genuinely new spike
    engineering this round's time went to TASK F/G's real-stand A/B
    investigation instead. Residual risk: lower than an untested unknown,
    but not zero — probe P4 (`docs/PLATFORM_FINDINGS.md`) already
    directly observed the *exact* ordering this test would assert (a
    reentrant send's effects landing against the already-debited balance,
    the original pending record surviving until the full reentrant chain
    completes) for a single reentrant withdraw, on the real stand, with a
    real fixture — so the specific ordering claim is not unconfirmed, only
    not re-verified as a dedicated, auto-run Phase 3 regression test tied
    to this exact two-intent shape.
12. **A depositor whose own payout handler re-withdraws on every single
    payout it receives, attempting to recurse without bound.** Build a
    depositor fixture whose `method_id = 0` handler, upon receiving any
    payout at all, unconditionally issues a brand new withdraw request for
    its entire remaining balance in that same execution — modeling either
    a bug or a deliberate attempt to force unbounded recursive execution.
    Assert the chain **always** terminates, and that it terminates at one
    of exactly two points, never running away unboundedly: either
    `pending_capacity_exceeded` (1509) once this depositor's own
    outstanding pending-withdrawal count reaches `MAX_PENDING_PER_DEPOSITOR`
    (4) — each new reentrant withdraw attempt is correctly counted against
    this cap while the previous one is still genuinely outstanding, per
    Invariant I8 and the exact same ordering `docs/PLATFORM_FINDINGS.md`
    probe P4 already demonstrated for a single reentrant call — or
    `insufficient_balance` (1505) once the depositor's own ledger balance
    is exhausted first, whichever bound is reached first. This test was
    not separately run on the platform spike (it follows analytically
    from Invariant I8 combined with probe P4's confirmed ordering, rather
    than requiring a fifth dedicated probe contract), but is required here
    as a Phase 3 implementation test because "a cap exists on paper" and
    "the real implementation correctly enforces it under recursive
    pressure" are not the same claim, and only the second one is
    acceptable to report as verified. **Closed, Gate 2b round 2 TASK C:**
    two host-level tests, each driving real sequential `handle_withdraw`
    calls against one depositor — `handlers::tests::
    pending_withdrawals_cap_enforced_via_real_sequential_calls` (the
    `pending_capacity_exceeded` branch) and `handlers::tests::
    pending_withdrawal_recursion_terminates_via_insufficient_balance_before_cap`
    (the `insufficient_balance` branch, new this round) — together cover
    both termination points this test requires. Per this test's own text,
    closing it analytically (real handler calls demonstrating both bounds)
    rather than via a new real-stand recursive-depositor fixture is the
    explicitly sanctioned approach here, not a lesser substitute.
13. **A withdrawal is rejected outright when the vault itself is
    underfunded — a coarse stand (E2E) test, not pinned to the exact
    boundary.** Drain a test vault's own SHELL below
    `MIN_OPERATIONAL_SHELL` (via repeated external calls on the real
    `avm-local-stand`, mirroring `docs/PLATFORM_FINDINGS.md` probe P1's
    draining procedure). Attempt a `WITHDRAW` call with a depositor who
    genuinely holds sufficient ledger balance to cover it. Assert the call
    fails immediately with `vault_underfunded` (1528), with absolutely no
    effect — no debit, no pending record created, no outbound intent
    emitted — confirming the check in Invariant I17 happens before any of
    those effects, not alongside or after them. This test only needs to
    land *somewhere* below the threshold, reached through genuine stand
    mechanics (real external calls genuinely draining real SHELL) — it is
    not trying to hit the exact boundary value, which is what test 14
    exists for, separately, at the host level.
14. **`MIN_OPERATIONAL_SHELL` boundary, exact — a host-level unit test
    with a crafted `input.extra`, not a stand test, because the real
    stand has no way to land a vault's SHELL on one exact value on
    demand; asserts against the final, measured value (`1,500,000`,
    §1, Phase-4-corrected from `1,300,000`), no longer provisional.**
    Construct a synthetic `MessageInput`
    directly against this contract's own `process_message`, with
    `input.extra` (field group "(a)", §1) set to carry exactly
    `MIN_OPERATIONAL_SHELL - 1` at `currency_id == 2`, and attempt a
    `WITHDRAW` with a depositor who holds sufficient ledger balance;
    assert it fails with `vault_underfunded` (1528), no effect.
    Separately, construct one with `input.extra` carrying exactly
    `MIN_OPERATIONAL_SHELL` (not one more) at `currency_id == 2` and
    repeat the same withdrawal; assert Invariant I17's check does **not**
    reject it — the call must proceed past that check at the boundary
    value itself, not just somewhere comfortably above it. **Also
    construct a third case: `input.extra` with no entry for
    `currency_id == 2` at all (an absent SHELL entry, which must be
    treated as exactly `0`, not skipped or treated as "unknown"**) —
    assert this is indistinguishable from an explicit `0` and fails with
    `vault_underfunded` identically, confirming the implementation does
    not accidentally special-case "entry absent" as "check inapplicable."
    (Whether the withdrawal's own chain then succeeds or fails further
    downstream, in the `MIN_OPERATIONAL_SHELL`-exactly case, is not what
    this test is checking; it is checking only that the I17 gate's
    comparison is `>=`, not `>`, and that absence means zero, matching
    `../SPEC.md` §5's stated threshold exactly.) This test asserts
    against the `MIN_OPERATIONAL_SHELL` constant itself (final value
    `1,500,000`, §1, Phase-4-corrected from Gate 2b round 3's
    `1,300,000`, itself re-derived after `MAX_PENDING_WITHDRAWALS` was
    restored to 32), not a hardcoded
    literal — it tracks whatever that constant is set to automatically.
15. **`TOPUP_METHOD_ID` succeeds unconditionally with respect to the
    ledger's own contents.** Deposit several different depositors' several
    different currencies into the vault first (so the ledger is non-empty
    and holds a mix of `Main` and multiple `Extra(currency_id)` rows, none
    of them SHELL — Invariant I10). Then send a `TOPUP_METHOD_ID` call
    carrying only `extra = [(currency_id: 2, amount: N)]` for some `N > 0`.
    Assert it succeeds, and assert every existing ledger row's currency and
    amount is byte-for-byte unchanged afterward — `TOPUP` must never read,
    iterate, or touch the ledger in any way (Invariant I15), regardless of
    what the ledger currently holds for other depositors and currencies.
16. **`WITHDRAW` carrying SHELL in the inbound message is rejected, not
    silently ignored — also a host-level unit test with a hand-crafted
    `MessageInput`, for the same reason as test 7.** Send a
    `WITHDRAW_METHOD_ID` call whose `extra` includes an entry with
    `currency_id == 2` (any positive amount), alongside an otherwise
    well-formed withdraw body for some other currency the caller genuinely
    holds. Assert the call fails with `invalid_body` (1501) — the existing
    `value_main() == 0` / empty-`extra()` requirement on `WITHDRAW` (§4)
    already covers this case structurally, since a non-empty `extra` of
    any kind, SHELL included, violates it — and assert no debit, no
    pending record, and no outbound intent occurs. **As with test 7, the
    real stand has no way to produce this input end-to-end either**: the
    only caller type `WITHDRAW` accepts is `Internal`, and any internal
    message's `extra` is, by construction, whatever some other contract's
    own `OutboundIntent` put there — which §1/P1 established can never
    carry `currency_id == 2`. This test is a defensive-check regression
    test against a synthetic input, same as test 7, not a reproduction of
    a reachable real-world call. This is the direct regression test
    confirming
    a `WITHDRAW` call can never be used as a backdoor path to hand SHELL to
    the vault outside of `TOPUP_METHOD_ID`.
17. **One permanently-dropped callback, plus a separate, normal withdrawal
    in flight at the same time, asserting the two are never confused with
    each other.** (Restored in round 7 — this test was in an earlier
    round's list, dropped during a later round's consolidation, and
    `../SPEC.md` §9's simulator-strategy writeup wrongly pointed to test
    11 as if it already covered this; it does not, since test 11 is about
    reentrancy ordering within a single withdrawal, not correlation
    between two independent ones.) Using the fake-environment hook the
    Phase 3 simulator already needs for its own "dropped callbacks"
    branch (§9), set up one withdrawal whose payout is sent but never
    receives any callback at all — permanently pending, by design, the
    same way an undeployed-destination or a same-DApp-but-split delivery
    would leave it (`docs/THREAT_MODEL.md` §6 "L"). While that withdrawal
    remains permanently pending, have a *different* depositor request and
    complete an ordinary, normal withdrawal end-to-end. Assert: the normal
    withdrawal settles correctly, using its own `withdrawal_id` and its
    own `handler_method_id` correlation, completely unaffected by the
    other withdrawal's permanently-stuck state; the stuck withdrawal's own
    pending record remains in state, unresolved, for the entire test,
    never accidentally consumed, cleared, or confused with the normal
    withdrawal's own callback when it arrives. This is the direct
    regression test for Invariant I3's "at most once" guarantee holding
    correctly even in the presence of an unrelated, permanently-unresolved
    pending record sitting in the same `pending_count` list.

## 9. Phase 3 test strategy: a model-based simulator with an adversarial fake environment

Beyond ordinary property/fuzz testing of random, valid operation sequences
checked against Invariants I1 through I17 after every single step, Phase 3
implements a swappable fake "AVM environment" specifically to exercise
behaviors this specification depends on that are properties of *this one
local stand*, empirically observed, not proven platform specifications
guaranteed to hold everywhere:

- **Refund vs. retain on destination failure, both branches.** This
  spike's own probes only ever observed "never debited in the first
  place" for a failing destination on the one route actually reachable
  (local, same-DApp, synchronous). The simulator's fake environment must
  be able to produce *both* outcomes so this contract's logic is exercised
  against the branch it does not expect, not only the one branch this
  spike happened to observe.
- **The "retain" branch must demonstrate the Invariant I1 break
  concretely, and the test must label it explicitly undefended, not
  "handled."** This specification's atomicity assumption (§1) was verified
  as an outcome, on one route, not as a mechanism proven to hold
  everywhere. Under the simulator's retain branch (value stays credited to
  a destination that still reports failure), the test must assert that
  `sum(ledger) + sum(pending)` for the affected currency comes to *exceed*
  the account's real balance — Invariant I1, genuinely broken — and the
  test's own documentation must state plainly that this contract has no
  code path that catches this: the carried-value verification gate that
  would once have caught exactly this case was deliberately removed from
  this contract's design, specifically because the real platform, on the
  one route this spike could reach, does not exhibit this behavior (`docs/
  PLATFORM_FINDINGS.md` probe A8). This is a demonstration of what is *not*
  defended against, included on purpose so the gap is visible and
  documented rather than silently assumed away.
- **Dropped callbacks** — a withdrawal's payout is requested, and no
  callback of any kind ever arrives, modeling either of the two stuck
  routes this specification cannot resolve (the same-DApp-but-split case,
  and the SHELL-starved dropped callback case, both `docs/
  THREAT_MODEL.md` §"L"). The simulator must confirm the affected
  withdrawal correctly remains pending forever, with no incorrect
  auto-resolution of any kind, and — this is the part a single-withdrawal
  test cannot show — that this permanently-pending state does not corrupt
  or block correlation for any *other*, unrelated withdrawal's own
  callback handling, and must also confirm the global-cap consequence
  directly: accumulating 32 such stuck records (via either route, in any
  mix) correctly causes every *subsequent* withdrawal attempt — even from
  a depositor with no stuck withdrawal of their own — to fail with
  `pending_capacity_exceeded` (1509), not something subtler or silently
  wrong (**corrected in round 7 — this previously, wrongly,
  pointed to test 11, which is about reentrancy ordering within a single
  withdrawal and does not cover this at all; test 17 is the actual
  fixed-scenario regression test for exactly this one-stuck-plus-one-
  normal case** — the simulator verifies the same property under broader,
  randomized conditions with multiple concurrent withdrawals, some stuck
  and some resolving normally).
- **Duplicated callbacks** — the same callback, or a second callback
  referencing the same `withdrawal_id`, delivered twice. The simulator
  must confirm the second one is absorbed as `withdrawal_not_found` (1510)
  once the first has already resolved and removed the pending record —
  never a double-credit or double-restore.
- **Duplicated original request messages** — a deposit or a withdraw
  request delivered twice by the simulated environment. The simulator
  must confirm the resulting outcome matches the trust assumption named in
  `docs/THREAT_MODEL.md` §3 exactly (duplicate delivery produces
  double-crediting or double-debiting, as an explicitly documented
  consequence of relying on the platform's own exactly-once-delivery
  guarantee) — this is recorded as a known, named consequence of an
  explicit trust assumption, not discovered as a silent surprise during
  testing.
- **A failing callback handler.** The simulator must confirm this
  contract's own callback-handling logic never deliberately returns
  failure once the depositor-match check (Invariant I4b) passes — and,
  separately, must demonstrate concretely what would be lost if it ever
  did: a withdrawal whose resolution is thrown away with no second chance,
  exactly as `docs/PLATFORM_FINDINGS.md` probe A8 found for a callback
  handler that deliberately fails. This keeps the "never deliberately fail
  the handler" design rule enforced by a test, not merely documented in
  prose.

## 10. Phase 2 deliverables this specification requires

- **A README section stating loudly, not as a footnote, that the default
  build uses a publicly-known fixed owner key whose private-key secrecy is
  explicitly unverified**, and that any real deployment must set
  `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` to a freshly-generated real key before
  being used for anything beyond local testing and demonstration.
- **A README section documenting exactly who funds the vault's operational
  SHELL, and how — and stating plainly, in round 6, that this whole story
  is verified only on the local stand, not on a real network.** The vault
  begins with whatever SHELL its deployer funds it with at genesis (on the
  local stand, via `fund-user`, before or immediately after deployment; on
  a real deployment, via whatever the equivalent funding step is before
  the vault is first relied upon — unverified, since `fund-user` is
  explicitly a local-stand-only testing helper, not documented as having a
  mainnet equivalent). Ongoing replenishment, for as long as the vault
  operates, is designed to come only from anyone sending a plain,
  **externally signed** message invoking `TOPUP_METHOD_ID` and carrying
  ordinary SHELL as `extra[currency_id=2]` — this is the *only* way SHELL
  can ever reach the vault after genesis on the local stand
  (`docs/PLATFORM_FINDINGS.md` confirmed no contract can ever relay SHELL
  to another account internally, under any circumstance), and the only
  design this contract could implement given that constraint. **Whether
  this mechanism works at all on a real network is explicitly
  UNVERIFIED, not merely untested — there is a real, unresolved question
  about where the SHELL an external `TOPUP` call carries is supposed to
  come from.** Every `TOPUP` call this spike exercised was issued through
  `avm-local-stand call --extra-currencies "2:N"`, which is a **stand-only
  minting convenience** — the exact same flag this spike already used to
  mint ordinary (non-SHELL) currencies into test accounts out of thin air
  for other probes (round 4's currency `7`, round 5's various amounts,
  round 6's currencies `0`/`1`/`3`/`7`). Nothing in this spike ever
  confirmed that this flag models a real network's external-message
  economics rather than a test-harness shortcut. And there is a concrete,
  structural reason to doubt it does: `ExternalMessage`
  (`crates/avm-contract-sdk/src/input.rs` lines 211–225) carries no
  `AccountAddress` source at all, only a bare `pubkey` — confirmed
  previously (`docs/PLATFORM_FINDINGS.md` A9) — so there is no on-chain
  account for a real network to debit the carried SHELL from when a real
  signer sends a real external message. On the local stand this is a
  non-issue because `--extra-currencies` simply mints the amount
  requested; a real network, by contrast, must get that SHELL from
  *somewhere* real, and no document in this repository describes what
  that somewhere is. Wallets, tooling, or any automation intending to keep
  the vault funded must always send `TOPUP` as a direct external call,
  never routed through an intermediate contract, *if* this mechanism
  works on a real network at all — which is exactly what is unverified.
  **Consequence, stated explicitly rather than left implicit:** if a real
  network turns out to have no working way to attach real SHELL value to
  an external message (because, structurally, there is no source account
  to debit it from), then `TOPUP_METHOD_ID` would be unusable there, and
  this vault's design would have no way to replenish its own SHELL once
  genesis funding runs out. Given Invariant I17's `MIN_OPERATIONAL_SHELL`
  pre-check, the resulting failure mode is a **permanent withdrawal
  outage, not a loss of funds**: every ledger balance remains exactly
  intact and correctly tracked, but no depositor could successfully call
  `WITHDRAW` ever again once the vault's own SHELL drops below the
  threshold, because every attempt would be rejected before any debit
  (error 1528) — deposits recorded in the ledger stay stuck, visibly and
  permanently, not lost or stolen. This is now the **first and most
  important open maintainer question** this specification raises (ahead
  of the rent-formula question in §2.4, which is moot if this one has no
  answer): on a real Acki Nacki network, how does an external message
  actually carry real SHELL value given that `ExternalMessage` has no
  source account to debit it from — is there a separate funding/escrow
  step not modeled anywhere in this repository's docs, or is
  `--extra-currencies` on `avm-local-stand call` purely a test-harness
  convenience with no real-network analog at all?
- **A README note that a depositor whose own account is a smart contract
  must be able to accept a plain, method-less value transfer** (the
  withdrawal payout's fixed shape, `method_id = 0`, empty body) **or their
  own withdrawals will fail at the point of delivery** — safely, with
  their ledger balance unconditionally restored, but requiring them to fix
  their own contract or otherwise arrange to accept the transfer before
  successfully re-requesting the withdrawal.
- **A README section giving the vault's operating-burn formula, in plain
  terms, as a guide for whoever is responsible for keeping it funded.**
  On the local stand, storage rent accrues at roughly **1 SHELL per byte
  of the vault's current state, per second** since it was last touched
  (open issue #21, reconfirmed directly at scale in round 6 — §2.4); this
  is the single number anyone funding the vault needs to reason about its
  burn rate with. State the formula directly: *daily burn (SHELL) ≈
  current state size (bytes) × 86,400* — and that this is a **local-stand
  measurement, not a confirmed mainnet rate** (§2.4's open maintainer
  question). A vault sitting at its worst-case 24,359-byte state
  (252 ledger rows + 32 pending, Phase-4-corrected — see §2.4) would
  burn on the order of 2.1 billion SHELL per day at this rate if never
  touched; a near-empty vault burns proportionally almost nothing. This
  number exists so a reader can sanity-check `MIN_OPERATIONAL_SHELL`
  against their own vault's actual state size and expected touch
  frequency, not treat it as an abstract constant.
- **A README statement, in the same section, that `MIN_OPERATIONAL_SHELL`
  is a *chain-cost margin*, not a funding level — and must never be
  presented to a reader as "how much SHELL to keep in the vault," now
  with real numbers showing exactly how thin that margin is in
  practice.** `MIN_OPERATIONAL_SHELL` (final value `1,500,000`, §1 —
  Phase-4-corrected for the true 252-row/32-pending worst case) exists
  solely to answer one narrow question at the moment a withdrawal
  starts: "can the vault plausibly afford to see this one withdrawal's
  own payout chain through?" Measured against the burn formula directly
  above, at this contract's own worst-case 24,359-byte state and the
  local rate, `1,500,000` SHELL represents roughly **61.6 seconds** of
  rent at that size (the explicit, stated margin-window policy is 60
  seconds; rounding the derived `1,461,540` up to a clean number adds a
  little over 1.6 seconds of extra margin) — not a day, not an hour, about
  one minute. **Gate 2b
  round 3 TASK G3 measured that this 60-second framing is itself
  optimistic**: a vault funded with exactly the (then-current)
  `MIN_OPERATIONAL_SHELL` value failed its very first withdrawal attempt
  once ordinary deploy/seed/deposit setup cost was accounted for, before
  even 1 additional second had elapsed; funding **3×** that value
  survived to roughly 10 seconds but failed by 60. A vault actually
  relying on `MIN_OPERATIONAL_SHELL` as its idea of "being funded" would
  be rent-delinquent far sooner than a minute after being touched at
  worst-case size — likely immediately. The README must state this gap
  explicitly, with these real numbers, so no reader mistakes the
  withdrawal-gate constant for an operating balance target: genuinely
  keeping the vault funded is the keeper's job (a separate README
  requirement below), sized against the burn formula and the vault's
  actual touch frequency, watched far more often than once every 60
  seconds, not against `MIN_OPERATIONAL_SHELL`. **The README must also
  carry `docs/PLATFORM_FINDINGS.md` P18/P19/P20's finding, now resolved
  rather than merely flagged:** at real-stand-measured combined state
  sizes, a withdrawal's Settle/Restore callback was directly observed to
  silently fail to resolve on the real stand, permanently stranding the
  pending record. Gate 2b round 3 root-caused this by a real-stand A/B
  test to `state.rs` `parse()`'s own O(n²) per-depositor validation loop
  (P20) and removed it; `MAX_PENDING_WITHDRAWALS` was restored to 32 and
  directly verified safe at both 220 and 300 ledger rows, for both
  outcomes, with no failing point found in the range tested (P21). This
  is no longer an open risk the README needs to warn a reader away from
  trusting a well-funded vault about — it is a closed, root-caused, and
  fixed issue — but the historical record (what was ruled out before the
  real cause was found: vault SHELL balance, requesting more
  `--fuel-fee-shells` on the triggering call) stays in `docs/
  PLATFORM_FINDINGS.md` for anyone auditing the investigation's own
  rigor.
- **A short README operator note for a vault running near its caps**
  (Gate 2b round 2 TASK E, figures updated at Gate 2b round 3): an
  operator can read the current ledger-row count and pending-withdrawal
  count directly off the vault's own state at any time
  (`inspect`/`replay` on the local stand; the equivalent real-network
  read on whatever a deployment uses instead). With the root cause now
  fixed, approaching `MAX_LEDGER_ENTRIES` (220) or
  `MAX_PENDING_WITHDRAWALS` (32) is no longer a silent-stranding risk —
  Gate 2b round 3 directly verified both caps resolve cleanly right up
  to their own limits — but hitting either cap still means new
  `DEPOSIT`/`WITHDRAW` requests are correctly rejected outright (errors
  1508/1509) until usage drops back down, which is itself worth an
  operator's attention well before it happens: encouraging depositors to
  withdraw fully (removing their row via Invariant I9) rather than letting many
  small balances accumulate, and not treating "the cap hasn't rejected
  anything yet" as proof the vault is at a safe size.
- **A README caution on `TOPUP_METHOD_ID`'s margin: confirm genuine
  execution, not just a successful-looking call.** Round 6 and round 7
  both directly observed a call that reports `"status": "ok"` while the
  contract's own code never actually ran — SHELL was charged (partially
  paying down accrued rent) but no real execution happened, distinguishable
  only by checking whether `raw_executor_output`/`events` came back
  non-empty. Anyone sending a `TOPUP` to a vault suspected of being
  significantly behind on rent must check for this directly rather than
  trusting the top-level status: **send enough SHELL to comfortably clear
  the vault's entire accrued debt, not merely a token amount, and verify
  the response shows genuine execution** — round 7's P9b directly confirmed
  the alternative failure mode for an *external* topup specifically (an
  amount smaller than the debt is rejected outright with `EconomicsError`
  before anything is credited, atomically and harmlessly — see
  `docs/PLATFORM_FINDINGS.md`'s round-7 addendum), so an undersized topup
  at least cannot make things worse, but it also accomplishes nothing: the
  vault remains exactly as delinquent as before, and the SHELL that was
  supposed to rescue it was never spent trying.
- **A README section on the depositor-side and keeper-side consequences
  of vault SHELL delinquency, directly informed by round 7's P9.** A
  depositor's own `DEPOSIT` message, sent while the vault is severely
  rent-delinquent, can come back with an internal failure callback
  carrying the undocumented `status_code = 56` (confirmed directly in
  round 7, the same code round 4's P1 found for a SHELL-exhausted
  destination) — depositors and any tooling built on top of this contract
  must treat `56` as an ordinary, recoverable delivery failure (their
  value was never taken — round 7 confirmed this directly by reading the
  sender's own balance before and after, not merely inferring it — and
  they can simply retry the deposit later), not as evidence of a bug or a
  loss. Separately, this vault has **no built-in mechanism to notice or
  react to its own SHELL level** beyond rejecting withdrawals once it
  drops below `MIN_OPERATIONAL_SHELL` (Invariant I17) — keeping the vault
  genuinely usable requires an off-contract **keeper**: something that
  periodically checks the vault's own SHELL balance (there is no
  in-contract alert of any kind) and sends a `TOPUP` large enough to clear
  any accrued rent debt *before* the vault reaches a state where even a
  rescue attempt risks the "admitted, partially charged, zero-FUEL"
  outcome above. This is an operational requirement for running this
  example for real, not an optional nicety, and the README must say so
  plainly rather than leaving a reader to discover it the way this spike
  did.
- **A README section stating the global stuck-slot warning explicitly,
  not buried in the threat model alone.** This vault has no reclaim
  mechanism for a withdrawal whose payout produces no callback at all, via
  either of two routes: a same-DApp-but-split delivery, or a SHELL-starved
  dropped callback (`docs/THREAT_MODEL.md` §6 "L"). Because
  `MAX_PENDING_WITHDRAWALS` is a single global cap shared across every
  depositor, **as few as `MAX_PENDING_WITHDRAWALS` (32) stuck pending
  records — accumulated over time, from any mix of depositors, via
  either route — permanently disable `WITHDRAW` for every depositor in
  the vault, not only the ones whose own withdrawals got stuck.** A
  reader relying on this example must understand this is a real,
  standing operational risk for this non-upgradeable v1 design, not a
  theoretical edge case, and that there is no in-contract recovery once
  it happens. `docs/THREAT_MODEL.md` §5.4/§6 now also quantifies what it
  costs a colluding set of accounts to deliberately fill this cap (Gate
  2b round 3 TASK H).
