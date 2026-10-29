# Open Questions for the AVM SDK Maintainers

This file consolidates every open question this engagement's Phase 1 spike
raised against `avm-sdk` itself (not against this contract's own design),
in priority order — highest-impact first. Each entry restates the question,
why it matters for this contract specifically, and where the full
supporting evidence lives (`SPEC.md`, `THREAT_MODEL.md`, or
`PLATFORM_FINDINGS.md`). None of these are resolved here; this file exists
so a reader does not have to hunt through four documents to find the full
list, per Hard Rule 6 (report what was tested, what was not, and residual
risks — these are the "not tested, not resolvable from this repo" items).

## 1. How does an external message carry real SHELL value on a real network, given `ExternalMessage` has no source account to debit?

**Highest priority — every other question below is secondary to this one,
because the vault's entire SHELL-replenishment design depends on the
answer.** `ExternalMessage` (`crates/avm-contract-sdk/src/input.rs` lines
211–225) carries no `AccountAddress` source at all, only a bare `pubkey`.
Every `TOPUP_METHOD_ID` call this spike ever exercised was funded through
`avm-local-stand call --extra-currencies "2:N"`, a flag that is, on the
local stand, a pure test-harness minting convenience — the exact same flag
this spike used to mint ordinary, non-SHELL currencies into test accounts
out of nothing for unrelated probes. Nothing in this spike ever confirmed
this flag models real external-message economics rather than a stand-only
shortcut.

**Why it matters:** if a real network has no actual mechanism for an
external message to carry real SHELL value, `TOPUP_METHOD_ID` — the vault's
*only* replenishment path after genesis, since no contract can ever relay
SHELL to another account internally (confirmed, P1) — would be unusable
there, and the vault would have no way to recover its own SHELL once
genesis funding runs out. The consequence, given Invariant I17, is a
permanent withdrawal outage: ledger balances stay exactly intact, but no
depositor could ever withdraw again (funds stuck, never stolen or lost).

**Evidence:** `SPEC.md` §10 (full writeup, README requirement);
`THREAT_MODEL.md` §5.9, §6.

## 2. What is the exact mainnet storage-rent formula, and what happens to an account whose arrears grow very large relative to its balance over a long period?

`docs/fuel-and-fees.md` states mainnet rent is charged only after 30 days
of inactivity, with no further formula given. Open issue #21 measured the
**local stand's** rate directly: roughly 1 SHELL per byte of an account's
current state, per second, collected lazily on next touch — reconfirmed
at scale in this spike (a 31,234-byte account accrued `storage_due_payment
= 310,342,012` over 10,005 seconds, ≈0.993 SHELL/byte/second). Nothing
confirms this rate, or the lazy-collection model itself, applies to
mainnet. Separately: is accrued debt simply held indefinitely until the
account can cover it, however large it grows, or is there some other
resolution (freezing, deletion, a cap on arrears)?

**Why it matters:** `MIN_OPERATIONAL_SHELL` and `MAX_LEDGER_ENTRIES` are
both sized using the local-stand rate as the only available data point;
if mainnet's real rate or arrears-resolution policy differs, those
constants could be wrong in either direction.

**Evidence:** `SPEC.md` §2.4; `THREAT_MODEL.md` §5.9, §6; open issue #21.

## 3. Can a single call's FUEL allowance be entirely consumed paying down accrued rent, causing the contract's own code to never run and silently dropping a value credit that call was supposed to deliver — and if so, is there any documented way to know in advance?

Observed directly, twice, not inferred: a severely rent-delinquent
large-state account processed a call that returned `"status": "ok"` with
a large `charged_shells`, yet `"events": []` and `"raw_executor_output":
""` — the guest contract's own code never ran. A `value_main` credit that
same call carried never actually landed, confirmed by a follow-up
withdrawal failing with `EconomicsError`. Round 7 confirmed the same
mechanism applies to an *internal*, deposit-shaped message as well
(`status_code = 56`, sender correctly never debited).

**Why it matters:** this means `input.extra`/`input.balance_main` only
ever reflect a *post*-rent-collection balance when the contract's code
runs at all — but the contract may not get to run, and no in-contract
check (including `MIN_OPERATIONAL_SHELL`) can defend against that, since
the check itself is inside the code that doesn't run. Is there a
documented way for a caller to know in advance whether a given call will
have enough FUEL to both pay down existing debt and genuinely execute, or
is sending the call and inspecting `raw_executor_output`/`events`
afterward the only way to find out?

**Evidence:** `SPEC.md` §1, §2.4; `THREAT_MODEL.md` §5.8, §5.9;
`PLATFORM_FINDINGS.md` round-6 addendum (P6) and round-7 addendum (P9a).

## 4. Does *any* touch reset mainnet's stated 30-day rent-inactivity clock, or only a touch that results in a genuine charge or genuine guest execution?

Round 7 observed a touch (an undersized external `TOPUP` attempt) that
failed outright at materialization with **no charge at all** — `inspect`
confirmed the account's rent-tracking fields were byte-for-byte unchanged.
Whether a touch of that shape would count as "activity" for mainnet's
30-day clock is unknown.

**Why it matters:** if only a charged/executed touch resets the clock, a
keeper's "cheap, harmless, frequently-repeatable ping" strategy would not
actually keep the vault's rent clock current — it would need a touch that
is guaranteed to result in a real charge, which is a meaningfully
different (and more expensive) operational requirement.

**Evidence:** `SPEC.md` §2.4; `THREAT_MODEL.md` §6; `PLATFORM_FINDINGS.md`
round-7 addendum (P9b).

## 5. How is a keeper meant to read an account's accrued rent debt on a real network?

Every rent measurement in this spike was read via `avm-local-stand
inspect`'s per-account `storage_due_payment`/`storage_last_paid` fields —
an explicitly local-stand debugging command. Nothing in this repository
says whether a real network exposes any equivalent read path.

**Why it matters:** without one, a keeper on a real network has no way to
know how close to delinquent the vault actually is except by estimating
elapsed time since its own last touch and multiplying by an assumed
byte-rate — itself the subject of open question 2 above. A keeper flying
blind on this number cannot reliably avoid question 3's failure mode.

**Evidence:** `SPEC.md` §2.4; `THREAT_MODEL.md` §6.

## 6. What does internal-delivery status code `56` mean precisely — is it specific to compute-starved/rent-delinquent delivery, or a broader "destination insolvent" signal?

Observed repeatedly (round 4's P1, round 7's P9a) as the status code
returned on a failure callback when an internal message is delivered to a
SHELL-exhausted or severely rent-delinquent destination. The code is not
documented anywhere in this repository's `docs/` or any issue found.

**Why it matters:** this contract (and its README) currently instructs
depositors and tooling to treat `56` as an ordinary, retryable delivery
failure. If `56` sometimes means something else (e.g., a genuinely
different, non-retryable condition), that guidance could be wrong in a
case this spike never constructed.

**Evidence:** `PLATFORM_FINDINGS.md` P1 (round 4 addendum), P9a (round-7
addendum).

## 7. Does a chargeable route that fails before the receiving contract's own WASM execution starts still produce a callback? (the same-DApp-but-split liveness case)

Original spike finding, never resolved: this single-`init`'d local stand
supports only one DApp id, so a genuinely cross-DApp or routing-partition
split could never be constructed to test this directly. If such a route
exists and never calls back, a withdrawal sent along it remains pending
forever, with no mechanism in this contract to recover it.

**Why it matters:** this is "L" route 1 — one of the two ways a pending
withdrawal can become permanently stuck, and, combined with route 2
(question 3 above), is the direct cause of the global-cap outage
(`THREAT_MODEL.md` §5.4: as few as 32 stuck records disable `WITHDRAW` for
every depositor).

**Evidence:** `PLATFORM_FINDINGS.md` Q3 (original baseline); `SPEC.md`
§2.3; `THREAT_MODEL.md` §6 "L".

## 8. Does the platform cap a message's body size independently of the deploy-envelope and FullReplace state-size limits already measured here — and if so, at what value?

Found at Phase 4 (fresh-context adversarial review, finding 4), confirmed
from `avm-contract-sdk`'s own source: `GuestArena<N>` (`runtime.rs`) must
hold the raw inbound message bytes *and* this contract's own output
simultaneously in a single fixed-size, bump-allocated region — the host
allocates and writes the inbound message in *before* this contract's
code even runs, so a large message body counts against the arena
regardless of whether this contract's own logic ever reads it (and
`handle_callback` never does — it reads only the callback header's
`status_code`). This contract's `arena_bytes = 262_144` is sized for its
own worst-case *legitimate* traffic (24,359 bytes, `SPEC.md` §2.4) with
margin, not against an adversarially large message. A depositor
receiving the vault's withdrawal payout controls a
callback-request-bearing message and can set an arbitrarily large
`callback_result_body` in their own response, which becomes the body of
the CALLBACK delivered back to the vault — a body of roughly `208,700`+
bytes would exhaust this contract's arena before `handle_callback` runs
at all. The two size limits this repository has already measured — the
`65,594`-byte `FullReplace` state-size ceiling (open issue #20,
`SPEC.md` §2.4) and the `65,237`/`65,241`-byte combined deploy envelope
limit (question 11 below, `wasm + acab + descriptor` together) — are
both limits on *other* things (a deployed account's own state, and the
deploy-time envelope); neither is documented as also capping an
arbitrary message's body size, and nothing else in this repository's
`docs/` addresses message body size at all.

**TASK N (Gate 3b) tested this directly on the real stand — NOT
reproduced, at any size tried.** A malicious-depositor spike accepted
the vault's real withdrawal payout, attached a runtime-generated
`callback_result_body` to its own success response, and the withdrawal
resolved cleanly (Settled, byte-decoded confirmation) at **every** size
tested: 50,000 / 200,000 / 262,144 / 1,000,000 / 5,000,000 /
10,000,000 bytes — the largest **38× the vault's own declared
262,144-byte `arena_bytes`**. The theoretical mechanism, read directly
from `GuestArena`/`dispatch_message_inner`'s own source, predicted
failure well before that point; it did not occur. **TASK N2, below,
explains why: on the local stand, no body larger than 65,536 bytes
ever actually arrives at `handle_callback`** — it is silently dropped
upstream, before the arena is ever stressed by it — so the theoretical
mechanism was never actually exercised by any of TASK N's test sizes
above that threshold, and even the largest size that DOES arrive
(65,536 bytes) is well under a quarter of the 262,144-byte arena. What
`arena_bytes` constrains for an inbound callback message that genuinely
is large enough to matter — on mainnet, or for a message shape other
than this one — remains open; see "why it matters, revised again"
below for the specific, narrower question this leaves.

**This is now a narrower, different question than originally posed**:
not "is this reachable," but **"what does `arena_bytes` actually
constrain for an inbound callback message, if not what `GuestArena`'s
own source appears to describe?"** Understanding this would clarify
whether this contract's `arena_bytes = 262,144` choice (`src/lib.rs`)
is actually providing the defensive margin its own derivation assumed,
for *any* inbound message shape, not only the oversized-callback case
that prompted this question.

**TASK N2 (Gate 3b) answered a question TASK N never asked, and the
answer changes what "why it matters" means here: the body is not
exhausting the arena, but above exactly 65,536 bytes it is not
arriving at all.** TASK N only checked whether Settle resolution
succeeded; it never checked whether `handle_callback` actually
received the bytes the depositor attached. A spike-only hook
recording the delivered body's real length (`docs/
PLATFORM_FINDINGS.md` P23) found a sharp, 100%-reproducible threshold
at 2^16 bytes: 65,536 bytes arrives unchanged; 65,537 bytes arrives
as exactly 0 bytes — silently, with the call's own status still
`"ok"` in both cases, no truncation, no error. This vault's own
`handle_callback` never reads that body's content, so no
fund-theft/mis-crediting path exists here specifically — but **a
message body silently vanishing above a fixed, undocumented 64 KiB
threshold, with the enclosing call still reporting success, is a
distinct and more concerning platform behavior than the
arena-exhaustion theory it was found while investigating**, since it
would defeat any future vault logic that DID read callback body
content for a value-bearing purpose, with no way to detect the loss.

**Why it matters, revised again:** no fund-stranding risk was found
within the tested range (confirming TASK N), but a silent, undetectable
data-loss ceiling at 65,536 bytes was found where TASK N looked only
at resolution success. Neither this nor the original `arena_bytes`
question has been tested beyond the local stand — **whether either
behavior exists on mainnet, and if so whether the threshold is the
same, is not established by this engagement.**

**Evidence:** `THREAT_MODEL.md` §5.12, §6 (demoted from an active "L"
route to a tested-and-not-reproduced note, plus a new bullet for the
65,536-byte finding); `docs/PLATFORM_FINDINGS.md` P23 (full
sent-vs-delivered table and method);
`crates/avm-contract-sdk/src/runtime.rs` (`GuestArena`,
`dispatch_message_inner`); `crates/avm-contract-sdk/src/output.rs`
(`validate_message_context`'s `callback_result_body` handling, lines
404–424 — the only checks found anywhere in the SDK's own source on
`callback_result_body` are that it may only be non-empty when a
callback was requested, line 422, and only when `status_code == 0`,
lines 356–363; no size limit of any kind is checked there — so if the
65,536-byte ceiling is real, it lives outside this SDK crate); TASK
N's spike (`spike/actor-bigbody/`, throwaway, not committed); TASK
N2's spikes (`spike/vault-m2/`, `spike/actor-m2/`, throwaway, not
committed).

## 9. Does the platform deduplicate an exact replayed external message (nonce-based)?

`ExternalMessage` carries a `nonce: u64` field, strongly suggesting
nonce-based replay protection is intended, but `avm-local-stand call`
exposes no `--nonce` flag through which two byte-identical external
messages could be submitted to observe whether the second is rejected.

**Why it matters, and why it is lower priority than the questions above:**
this contract does not rely on the answer either way — `admin_seq` makes
its own independent, contract-level replay-rejection guarantee for every
owner/pending-owner call (Invariant I13), so this question is interesting
but not load-bearing for this contract's own safety.

**Evidence:** `PLATFORM_FINDINGS.md` Q5; `THREAT_MODEL.md` §3, §5.11, §6.

## 10. (Tooling gap, not a safety question) How can a script recover a deployed selection's `package_id`/`selected_code_id` without diffing `inspect` output before and after?

Neither id is surfaced directly in any `deploy`/`publish-version` JSON
response in any case this spike tested; recovering them required diffing
`inspect`'s `prepared_selection_ids`/`registry_entries` before and after a
deploy, or — the workaround this spike used repeatedly — republishing a
trivially modified source under a new `state_schema_id` to get a fresh,
directly-deployable package instead of reusing an already-published one.

**Why it matters:** this is an E2E-scripting ergonomics gap, not a
security concern, but it directly affects how Phase 3's multi-depositor
stand E2E script must be written (it must plan for this diffing step, or
the trivial-variant workaround, explicitly).

**Evidence:** `PLATFORM_FINDINGS.md` bonus findings (original baseline);
round 6/7 addenda (`mini-vault-r6`, `mini-vault-big-c`, etc., all built as
trivial variants specifically to sidestep this gap).

## 11. What exactly is the `deploy`-time canonical-envelope size limit that produces `BodyTooLarge`, and is it documented anywhere for mainnet?

Round-4 TASK J4 bisected this directly on the local stand using a minimal
throwaway spike contract whose only content is a compile-time-sized
padding byte array copied into its own returned state (so the optimizer
cannot strip it) — see `spike/sizebisect/` (never committed, Hard Rule 5).
Deploying candidate wasm files of increasing size against a fresh account
each time, by binary search:

- **65,091-byte wasm: deploys successfully** (`"status": "ok"`).
- **65,095-byte wasm: rejected** with `"error": "deploy rejected canonical
  operation: zero-signature canonical envelope rejected: BodyTooLarge"`,
  before any funding precondition is even evaluated (the same rejection
  appears on the very first, otherwise-side-effect-free deploy attempt).

The true boundary lies somewhere in the untested 65,092–65,094-byte gap
(the padding construction's byte-to-byte growth is not perfectly 1:1 near
WASM section length-prefix boundaries, so those three exact sizes were not
directly reachable); 65,091/65,095 is as tight a bracket as this method
produced. That first bisection used a **minimal spike ACAB (80 bytes)**,
far smaller than the real contract's own ACAB — leading round 4 to
conclude the limit was "a function of wasm size specifically, not the
combined envelope," since the tiny, constant ACAB/descriptor never moved
across any trial.

**Corrected at Gate 3b (TASK O): that conclusion was wrong.** Re-bisecting
with a byte-for-byte copy of the real production `constants.rs`/
`handlers.rs`/`state.rs`/`avm-contract.toml` (so `contract.acab` is the
real **3,182 bytes** and `contract.descriptor` the real **66 bytes**,
confirmed identical to the production build's own hashes) found a
**different** wasm boundary — **61,989-byte wasm deploys; 61,993-byte
wasm is rejected** — exactly `3,102` bytes lower than the first
bisection's `65,091`/`65,095`, which is exactly the size difference
between the real ACAB (3,182 bytes) and the spike's minimal one (80
bytes). Computing the **combined envelope** (`wasm + acab +
descriptor`) for both bisections:

| Bisection | wasm | acab | descriptor | combined, succeeds | combined, fails |
|---|---|---|---|---|---|
| Round 4 (minimal ACAB) | 65,091 / 65,095 | 80 | 66 | `65,237` | `65,241` |
| Gate 3b (real ACAB) | 61,989 / 61,993 | 3,182 | 66 | `65,237` | `65,241` |

**Both bisections land on the exact same combined-envelope boundary.**
The limit is on `wasm + acab + descriptor` together, not on wasm size
alone — the round-4 conclusion only looked wasm-specific because its
ACAB was too small and constant across every trial to reveal the
tradeoff. The true boundary is **65,237 bytes combined: succeeds;
65,241 bytes combined: rejected** (a 4-byte window, the same
construction-granularity limit as before).

The current production vault's real envelope and its margin against
this measured limit are tracked in exactly one place now —
`README.md`'s own "Build artifacts" section — not repeated here. That
margin has moved several times across rounds (originally an
(incorrectly) wasm-only-compared ≈15.5%, then ≈8.7%, then ≈9.6%, now
≈5.3% after this round's callback fast path grew the wasm) purely
because the wasm size changed, never because the 65,237-byte limit
itself did. Nothing in this repository's `docs/` documents this
limit's existence, exact value, or whether it is a local-stand-only
artifact or also applies to mainnet.

**Why it matters:** a future maintainer who adds fields, dependencies, or
unrolls code in this contract (or any contract near this size) could
silently cross a hard deploy-time wall with no advance warning beyond
trial and error on their own stand — there is no documented way to query
the limit, only to discover it by hitting it. Critically, **adding ABI
surface (new methods, new types) shrinks the same budget just as surely
as adding code does** — this was invisible under the original,
wasm-only framing.

**Evidence:** `SPEC.md` §1; `REVIEW.md` TASK J entry (original) and
TASK O entry (correction); the original spike (`spike/sizebisect/`) and
the TASK O re-bisection spike (`spike/sizebisect-real/`), both
throwaway and never committed.

## 12. What is the real headroom of the Settle/Restore callback at the true worst case, and does the removed check's own cost fit within it? — RESOLVED: the ≥2× rule was unsatisfiable on the real numbers and was dropped; a faster callback dispatch path now gives ≈1.6–1.7× more margin

**Resolved.** Gate 3c TASK 3/M4 reported a 1-row+1-pending Settle
boundary of K=590 safe/610 traps, fee-independent across six orders of
magnitude, and left open whether the real worst case (252 rows + 32
pending) resolves cleanly and what headroom it has. Gate 3d TASK M5
answers both — **and in the process found TASK M4's own K=590/610
figure does not reproduce under repeated, controlled re-testing**
(this round's own bisection of the identical configuration: K=16,140
safe/16,160 traps, roughly 26× higher; three differential checks ruled
out an actor-body-size or seed-dispatch-code explanation; logged as a
correction to TASK M4's own REVIEW entry, cause not confirmed since
that spike is already deleted). The fee-independence finding itself
(no measurable effect from 1 to 2,000,000 shells) was re-confirmed,
just at the corrected, much higher boundary.

**The real numbers, bisected directly, no theoretical calibration:**
Settle at the true 252-row + 32-pending worst case: K≈8,400 (8,350
resolves, 8,440 traps). Restore at the same state: K≈8,275 (8,200
resolves, 8,350 traps) — measurably lower than Settle, since Restore
also writes back into the ledger. The removed O(n²) per-depositor
check's own real K-equivalent cost, measured directly (not the
"1,024 comparisons" theoretical estimate every earlier round's own
headroom arithmetic used) by re-enabling it on a spike and finding how
far it moves the boundary at a 1-row ledger (where there is headroom
to measure a finite number): **≈13,190** — roughly 13× the old
calibration. **Headroom = K-boundary(252+32) ÷ K-cost(removed check):
Settle ≈0.64×, Restore ≈0.63×. The ≥2× decision rule this section has
applied since the very first dummy-K sweep (TASK J) fails outright** —
confirmed independently by the direct observation that re-enabling the
check at the true worst case traps even with zero additional padding
(K=0).

**Why it matters:** this is not merely "headroom is lower than
thought" — the removed check's own real cost exceeds the entire
measured margin at the true worst case. This is the real, measured
explanation (in the same K-equivalent units as every headroom figure
in this repository) for why removing that check was necessary rather
than optional (`docs/PLATFORM_FINDINGS.md` P20/P25): restoring it
would not erode margin, it would already break the real worst case.

**The ≥2× rule was dropped as unsatisfiable, not relaxed.** Measured
per-row cost (≈28 K/row) and per-pending cost (≈24 K/pending) mean no
combination of lowering either `MAX_LEDGER_ENTRIES` or
`MAX_PENDING_WITHDRAWALS` buys enough margin against a ≈13,190-unit
fixed cost to reach 2× — even the near-empty 1-row+1-pending case only
has ≈16,140 units of spare K, short of the ≈26,380 a 2× margin against
that fixed cost would need.

**Resolution: a callback fast-dispatch path (Gate 3e TASK F1/F2), now
in production, measured at ≈1.6–1.7× the prior callback cost at the
true worst case.** Rather than touch either cap, the callback path
(`handlers::handle_callback_fast`, `state::VaultState::fast_callback`)
now validates the header and the entire pending section exactly as
before, but no longer decodes or re-validates the *ledger* section's
own content — only its declared length. A gate check (same session,
same build family, bisected) found this buys ≈13,000–14,000 spare
K-units for Settle and ≈12,250–13,000 for Restore at the true
252-row + 32-pending worst case, against a freshly re-measured
baseline of ≈7,600–7,625 (Settle) and ≈7,525–7,600 (Restore) — ≈1.7×
and ≈1.6×. (That baseline re-measurement itself came in ≈9% below
Gate 3d's own 8,350/8,440 and 8,200/8,350 figures — ordinary
session-to-session noise, not the same category of error as TASK
M4's, and reported as a range rather than chased further.) The
accepted trade-off: a corrupted ledger row no longer blocks a
Settle/Restore; the next `DEPOSIT`/`WITHDRAW`, which still validates
the full state unconditionally, catches it there instead — see
`SPEC.md` §1 for why this is judged acceptable.

**What remains genuinely open:** what fuel a real external caller
supplies on mainnet, and whether `--fuel-fee-shells` (a local-stand
CLI flag) has any mainnet equivalent at all — this was always a
separate question from the headroom measurement itself, and was shown
fee-independent, so the real caller's own fuel supply doesn't change
the answer above, absent evidence otherwise.

**Evidence:** `SPEC.md` §1; `REVIEW.md` entries 43, the Gate 3c TASK
3/M4, Gate 3d TASK M5, and Gate 3e TASK F1/F2 entries;
`docs/PLATFORM_FINDINGS.md` P24/P25/P26; `spike/vault-m2/`,
`spike/actor-m2/`, `spike/vault-m4/`, `spike/actor-m4/`,
`spike/vault-m5/`, `spike/vault-m5strip/`, `spike/actor-m5/`,
`spike/vault-f1/` (all throwaway, all deleted after use, Hard Rule 5).
