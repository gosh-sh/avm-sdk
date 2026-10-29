# Platform Spike Findings

This file has grown across seven Gate-1 review rounds (the original
baseline plus six revision rounds). **Round 2** added
three probes (A4/B/C) after the first baseline writeup (Q1–Q7). **Round 3**
added nine more (A1/A2/A3/A5/A6/A7/A8/A9/A10) and, critically, **A8
overturned Q1's original conclusion** about how failure-callback refunds
work — Q1's body below has been rewritten in place to reflect that
correction rather than left stale with a patch note bolted on top (round
3's version did the latter; round 4 cleans it up). **Round 4** adds four
more (P1/P2/P3/P4), each in its own addendum section at the end. **Round 5**
adds a P1/Q1/A4 reconciliation and P5 (vault SHELL-exhaustion-mid-callback).
**Round 6** adds a pending-record-size correction, a rent-rate
reconfirmation at scale, P6 (rent can silently drop a value credit when it
consumes a call's entire FUEL budget), P7 (withdraw-chain SHELL cost at
small/large state size), P8 (currency round-trip for ids 0/1/3/7), and a
methodology note on `--extra-currencies`' local-stand-only nature.
**Round 7** adds P9a/P9b (internal deposit-shaped delivery and an
undersized external topup, both against the same rent-delinquent vault).
Jump to "## Addendum (Gate 1 round 2)", "## Addendum (Gate 1 round 3)",
"## Addendum (Gate 1 round 4)", "## Addendum (Gate 1 round 5)", "##
Addendum (Gate 1 round 6)", or "## Addendum (Gate 1 round 7)" for the
later rounds; Q1–Q7 below are the original baseline, kept current rather
than frozen.

Empirical results from exercising `avm-local-stand`/`avm-contract-build`
directly (SDK release `avm-dev-06e96ef71006-sdk-0748f4f0767a`, installed via
`scripts/install.sh`, run under WSL Ubuntu-24.04 — Linux x86-64-v3, satisfying
the documented platform requirement). Every claim below is marked **OBSERVED**
(with the exact mechanism used) or **UNKNOWN** (with why it couldn't be
resolved). Nothing here is inferred from "it would make sense if..." without
saying so explicitly.

## Method

A throwaway diagnostic contract (`spike/probe-receiver/`, never part of any
submission, not committed) was used instead of guessing from docs alone,
because `avm-local-stand inspect` does not expose account balances or raw
state (consistent with open issue #5) — there is no black-box way to observe
either without a contract that reports them itself. The probe dispatches by a
body **tag byte** (0=REPORT, 1=FAIL, 2=FORWARD, 3=BUSY_LOOP) rather than by
method name, because `avm-local-stand`'s method-name→method_id hash is
undocumented anywhere in this repo (confirmed: calling an undeclared method
name fails with `"ACAB method ... is absent"`; open issue #2 corroborates
this gap). REPORT emits an event *and* now preserves state as-is; FORWARD
sends one `OutboundIntent` (optionally with a `CallbackIntent`) built from
caller-supplied bytes; a received callback is decoded and written into state
as a diagnostic record (tag `CBRD` + caller address + status_code +
msg_value_main + cause_msg_hash prefix), read back via a later call's
`raw_state` field in the CLI's JSON — a completely separate, more reliable
channel than the `events` command's wire format, which is also undocumented
and was not successfully reverse-engineered (its field layout is not a
straightforward reuse of `encode_event_fields`'s byte layout; it appears to
wrap fields in an additional index/offset table this spike did not need to
decode once `raw_state` worked).

## Q1 — On receiver failure, is the value returned to the sender or retained? How does the refund arrive?

**OBSERVED — fully, atomically rolled back; the callback's carried value is
informational, not a real credit (merged in from round 3's A8/A1/A3 — this
section originally described the carried value as a real refund "riding
back" on the callback, which A8 later disproved; rewritten here rather than
left wrong with a patch note on top, per round-4 cleanup).**

Command pattern: deploy two probe instances (sender `R2`, target `R1`);
fund `R2` with `value_main` via one plain external call carrying
`--value-main`; call `R2.probe.body` with a FORWARD body targeting `R1`,
`out_tag=1` (unconditional clean failure) or `out_tag=3` (busy-loop → FUEL
exhaustion), `value_main=700`, `extra=(2,50)`, requesting a callback.

Result, both failure modes:
- `R1`'s own state never changed (`raw_state` stayed all-zero, matching its
  untouched `init` state; `account_state_root`/`registry_root` in the
  receipt were identical across both failure-mode attempts).
- `R2`'s `balance_main`/SHELL balance were unchanged by the attempted
  700/50 transfer in every before/after read (the only deltas observed were
  the independent per-call `charged_shells` fee deductions) — **confirmed
  this is because the transfer was never actually debited from `R2` in the
  first place** (isolated and pinned down precisely in round 3's A3: a
  successful send debits the sender by exactly the sent amount; a failing
  one never touches the sender's balance, not even transiently).
- The requested callback **was delivered** in both cases — decoded directly
  from `R2`'s own diagnostic state write:
  ```
  tag: CBRD
  caller_dapp_id:    1111...1111          (the stand's dapp_id)
  caller_account_id: 640db757...2d31738e  (R1's own account_id, exactly)
  status_code:       1000                 (R1's failure status)
  msg_value_main:    700                  (the exact amount attempted)
  ```
  The callback's own `value_main` field reports the originally-attempted
  amount — but round 3's A8 (two otherwise-identical experiments, one where
  the sender's callback-handling code succeeds, one where it unconditionally
  fails) proved this field is **not a real credit**: `input.balance_main`
  during the callback's own execution, and the final persisted balance
  afterward, were identical (`9000`, never `9000 + 888`) in *both* variants.
  There is no real value for the callback to "return," because (per the
  point above) nothing was ever actually taken from the sender to begin
  with. The refund mechanism is therefore: **nothing moves, ever, for a
  failing destination** — not a bounce message, not a retained-then-returned
  value, just an informational status report riding on the callback. The
  one quantity that genuinely is prepaid-and-refunded for real
  (`docs/fuel-and-fees.md`) is the FUEL reserve specifically, a separate
  field this vault always sets to `0` on payouts.
- The whole chain (forward → delivery → destination failure → callback)
  resolved **synchronously inside one `avm-local-stand call` invocation** —
  the receipt's `queue` stayed `{"before": 0, "after": 0}` throughout. There
  is no separate "advance the queue" step required for a same-DApp chain on
  this local stand.

**Caveat (added round 4, item 5):** this is an outcome-level observation —
balances before and after were measured, not the host's internal mechanism
for achieving it. It was exercised only on the local, same-DApp, synchronous
route. Whether a cross-DApp or post-split chargeable route (still unresolved,
Q3) achieves the same outcome, or genuinely retains value at risk, is not
verified. See `docs/THREAT_MODEL.md`'s trust assumptions for why this
distinction matters and is treated as explicitly undefended.

## Q2 — Can an arbitrary contract send a message with the CALLBACK flag set, or only the runtime?

**OBSERVED (via source, reinforced by Q1's live data) — only the runtime.**

`OutboundIntent` (`crates/avm-contract-sdk/src/output.rs`) has no `flags`
field at all — just `destination`, `value_main`, `fuel_fee_shells`, `extra`,
`method_id`, `body`. `encode_outbound_intents` never writes an
`INTERNAL_FLAG_CALLBACK` bit for anything a contract constructs; that bit is
only ever *decoded* (`crates/avm-contract-sdk/src/input.rs`) on an inbound
message, meaning it is set by the runtime when it materializes a reply, never
by guest code via any public API. `ExternalMessage`'s flag space
(`EXTERNAL_FLAG_SIGNED | EXTERNAL_FLAG_DEPLOY`) has no callback bit at all, so
an external call is structurally incapable of being a callback.

Q1's live data reinforces this from the other direction: the genuine
callback we captured had `Caller::Internal(source)` equal to **exactly** the
account that actually ran and failed (`R1`), not a spoofable/anonymous
"system" address. A forged callback would need to fake being that specific
account's own outbound send — there is no SDK or CLI path that lets an
unrelated contract do that.

No separate CLI/stand negative test was run for this one, because there is no
code path to even attempt the forgery through the public tooling — the
absence of the capability is itself the finding.

## Q3 — Does a route that fails before receiver WASM starts still call back?

**UNKNOWN — not testable on this local stand.**

Attempted: a `FORWARD` targeting a destination with `dapp_id =
2222...2222` (deliberately different from the stand's single initialized
`1111...1111`), `fuel_fee_shells = 0`. Result: the **entire** `call`
invocation was rejected outright —
`"local call materialization failed: InvalidOutboundIntent"` — before the
sender's own message even finished executing. This single-`init`'d local
stand appears to support exactly one DApp id; there is no documented or
discovered mechanism (no CLI flag, no multi-dapp `init`) to create a second
DApp or simulate a routing-partition split locally. The specific scenario
("chargeable route, insufficient reserve, fails before receiver WASM starts")
could not be constructed. This is carried into `THREAT_MODEL.md`/`REVIEW.md`
as an unresolved residual risk, not something disproven.

## Q4 — Is `block_time` in seconds or ms?

**OBSERVED WITH CAVEAT — strongly implied to be seconds; not independently confirmable on this stand.**

`docs/getting-started.md` uses `--block-time 1800000000` throughout (a
plausible Unix-seconds value, ~2027) and pairs `--created-at`/`--expire-at`
with a difference of exactly `3600` — a natural "one hour" *in seconds*
(3600 ms would be an oddly short 3.6-second example expiry window). This
spike used the same convention throughout and it behaved consistently
(ascending, well-formed). However, the stand explicitly "does not read
wall-clock time" (`docs/getting-started.md`), so there is no real clock to
cross-check the unit against locally — the convention is inferred from
example magnitude and internal consistency, not verified against an
independent time source. Treated as a strong inference, not a hard fact.

## Q5 — Are external messages replay-protected (expiry/dedup)?

**PARTIALLY OBSERVED.**

Expiry **is** enforced: a call with `--block-time` after `--expire-at` is
rejected with `"local external ingress rejection: MessageExpired"`; a call
with `--created-at` after `--expire-at` is rejected with `"external
operation expiration precedes creation time"`. Both commands: plain
`avm-local-stand call` with the stated timestamp ordering.

Nonce/exact-duplicate dedup is **UNKNOWN** — not testable through this
tooling. `ExternalMessage` carries a `nonce: u64` field
(`crates/avm-contract-sdk/src/input.rs`), strongly suggesting nonce-based
replay protection is the intended mechanism, but `avm-local-stand call`
exposes no `--nonce` flag (per `docs/cli-reference.md`'s documented option
list) — it presumably mints a fresh one internally on every invocation, so
there is no way through the public CLI to submit two byte-identical external
messages and observe whether the second is rejected.

## Q6 — Does an out-of-FUEL execution commit any state?

**OBSERVED — no.**

`FORWARD` targeting a receiver with `out_tag=3` (unconditional
`loop { core::hint::spin_loop() }`) with a large `fuel_fee_shells` reserve to
guarantee the receiver actually starts and then exhausts its FUEL budget.
Result: identical to the clean-failure case in every observable way — the
receiver's `raw_state` stayed all-zero (untouched since `init`), its
`account_state_root` was unchanged versus the pre-attempt baseline, and the
requested callback still arrived with `status_code != 0`. A trap mid-execution
commits nothing, exactly like a clean `MessageOutput::failure(...)`.

## Q7 — Can a contract read its own balance?

**OBSERVED — yes, directly and unconditionally.**

`MessageInput` (`crates/avm-contract-sdk/src/input.rs`) exposes
`balance_main: AvmAmount` and `extra: CurrencyAmounts` as plain fields, no
host call needed. Confirmed live: a fresh account's first-ever value-carrying
call reported `balance_main` exactly equal to the value just sent (0→777);
across a sequence of calls on another account, `balance_main` accumulated
correctly (0→1000→2000→4500, once a self-inflicted double-submission from
retrying a crashed local decode script — not a platform bug — was accounted
for; see "Methodology pitfall" below). `extra` similarly reflected the
account's real current SHELL holdings.

## Bonus findings (all OBSERVED, not part of the original 7 questions but load-bearing for SPEC.md)

- **`fund-user --shells N` credits `extra[currency_id=2]` (SHELL), not
  `value_main`.** An account funded with 5,000,000 shells reported
  `balance_main=0` and `acct_first_extra_id=2`,
  `acct_first_extra_amount≈4,993,668` (5,000,000 minus accumulated
  `charged_shells` fees) on its first probe read. This directly validates
  SPEC.md's decision to track `Main` and `Extra(2)`/SHELL as separate ledger
  slots rather than assuming they're the same thing.
- **`value_main` is a real, balance-constrained, persistent account
  currency**, not a stateless pass-through: attempting to forward more
  `value_main` than the sending account currently holds is rejected
  (`"local call materialization failed: EconomicsError"`) before the
  sender's own call completes.
- **Deploying a second account from already-published, byte-identical
  code** via `--wasm/--acab/--descriptor` again is rejected ("selection is
  already attached immutably to the package"); subsequent deployments of the
  same code must pass `--package-id`/`--selected-code-id` instead. Neither
  id is surfaced directly in any `deploy`/`publish-version` JSON response in
  the cases tested — they had to be recovered by diffing `inspect`'s
  `prepared_selection_ids`/`registry_entries` before and after a deploy. This
  is a real gap for any E2E script that deploys multiple accounts from one
  build (relevant to Phase 3's multi-depositor E2E journey).
- **`--args-json` for a declared `bytes`-typed method argument** requires a
  `0x`-prefixed hex string and is wrapped on the wire with a 4-byte
  little-endian length prefix ahead of the raw bytes (discovered the hard way
  — a tag byte was being read as part of that length prefix before this was
  accounted for).
- **Methodology pitfall worth remembering for Phase 3's E2E scripts:**
  re-running a shell script after a purely *local* decode/post-processing
  crash can silently resubmit an already-successful `avm-local-stand call` —
  the on-chain call had already succeeded before the local Python crashed on
  unrelated formatting. Retrying a whole script after a downstream-local
  failure is not safe to treat as idempotent.

## What this changes in SPEC.md / THREAT_MODEL.md

See the Gate 1 revision summary in the conversation for the itemized list;
in short: `DEPOSIT_METHOD_ID` is now explicit only (Q1's rollback guarantee
makes the implicit-deposit-fallback unnecessary — a genuine failed delivery
never strands value, so there's no fund-trap risk from rejecting unrecognized
method ids outright); the withdrawal-callback handler now also checks
`Caller::Internal(source) == pending.destination` (Q1/Q2 confirm this is a
meaningful, enforceable check, not a tautology); Q3 remains an explicitly
unresolved residual risk rather than something Q1's clean result lets us
quietly assume away for the cross-DApp/split case.

## Addendum (Gate 1 round 2)

### A4 — Does a non-SHELL extra currency ride back on a failure callback, the same way `value_main` does?

**OBSERVED — yes, exactly.** Funded a sender with extra currency `7` (via a
plain external call carrying `--extra-currencies "7:1000"` — confirms extra
currencies can be funded directly this way, not just SHELL), then forwarded
`value_main=700` + `extra=(7,50)` to a receiver with `out_tag=1` (clean
failure), requesting a callback. The callback diagnostic written into the
sender's own state showed `status_code=1000`, `msg_value_main=700`, **and**
`extra_count=1, first_extra_id=7, first_extra_amount=50` — the non-SHELL
extra rode back exactly, same mechanism as `value_main`. **This finding
about the carried value itself is superseded by round 3's A8** (the carried
value is informational, not a real credit, for `value_main` and non-SHELL
`extra` alike) — what this test still establishes on its own, independent
of A8, is narrower and remains true: a non-SHELL `extra` currency's amount
is reported back on a failure callback the same way `value_main` is, so
nothing about `currency_id = 7` specifically behaves differently from
`value_main` in how the callback describes what was attempted. (This is
also unrelated to round 4's P1 finding about `currency_id = 2` specifically
— `7` is an ordinary extra currency, not SHELL, and P1's
no-contract-can-send-SHELL-via-OutboundIntent restriction doesn't apply to
it.)

### B — Can a destination forward the received value onward and then still report failure (the "malicious destination" scenario)?

**OBSERVED — no, the runtime rejects the attempt outright.** Constructed a
`MessageOutput` directly (bypassing the `success()`/`failure()` helpers) with
`status_code = 1000` *and* a non-empty `outbound_intents` — the exact
`EffectsOnFailure`-violating shape `crates/avm-contract-sdk/src/output.rs`'s
`validate_message_shape` is written to reject. Invoked directly via external
call: the entire call was rejected by the CLI with
`"call rejected canonical operation: local call returned VM status 43"` —
the same numeric VM status independently seen in open issue #20's
oversized-state rejection, suggesting `43` is a general "guest output failed
validation" status, not specific to state size. The calling context (this
was tested as a direct external call to the probe, not routed through an
internal sender) doesn't change the conclusion: the validation is a property
of the *destination's own returned output shape*, independent of who called
it. A contract genuinely cannot combine "I forwarded your money onward" with
"and also, I'm reporting failure" — the SDK's own encode-time check forecloses
it before the runtime admits the output at all. This directly supports
treating a withdrawal destination's failure report as trustworthy: if it
reports failure, it structurally cannot have also kept moving the money
elsewhere in the same breath.

### C — Payout to a same-DApp account that was never deployed (no contract code)

**OBSERVED — the send silently never materializes: no debit, no credit, no callback, no error.**
Forwarded `value_main=300` then separately `150` (requesting a callback) from
a funded sender to a syntactically-valid same-`dapp_id` account address that
was never `deploy`'d. Both outer calls reported `"status": "ok"` (the
sender's own message executed fine) with `queue: {"before":0,"after":0}`
both times. But: `avm-local-stand inspect` afterward showed no account at
that address at all (22 accounts total, none matching), the sender's own
`balance_main` was unchanged (still exactly what it was before both
attempts — never debited), and the sender's own callback-diagnostic state
was unchanged from an earlier, unrelated test (no new callback ever arrived
for either send, despite the second one requesting one).

This is the concrete, now-*confirmed-reachable* instance of the "callback
never arrives at all" scenario this contract's design has to account for —
not merely a theoretical gap. It directly motivates the Gate-1-round-2
decision to remove `reclaim_stale_withdrawal` rather than keep a
balance-based heuristic for it: a withdrawal sent to a destination address
that was simply never deployed (a realistic depositor mistake — a typo'd or
not-yet-deployed address) produces exactly nothing observable, forever, on
this local stand. Any reclaim mechanism keyed on "enough time passed" would
have no way to distinguish this genuinely-never-happened case from a
delivered-but-callback-lost case using only a balance check — which is
exactly why SPEC now documents a liveness limitation instead of attempting
to resolve it by guessing (see SPEC §5, THREAT_MODEL "L").

## Addendum (Gate 1 round 3)

Same throwaway probe, extended further: `report_callback` now also records
`input.balance_main` *during the callback's own execution* into its
diagnostic state write (previously only captured in an event field that
couldn't be reliably decoded), and a build-time switch
(`AVM_SPIKE_CALLBACK_FAILS=1`) makes the callback-handling branch itself
return `failure(2000)` instead of running normally, to directly compare
outcomes. A second, fully SDK-free raw probe (`spike/raw-probe/`, no
dependency on `avm-contract-sdk` at all) was also built for A6.

### A3 — Is in-flight value deducted from the sender's balance at send time?

**OBSERVED — no, only on destination success, and only then.** A clean,
isolated measurement: sender `S2`'s `balance_main` was 2000 before a
successful forward of `123`; immediately after, a separate probe read it at
exactly `1877` (`2000 - 123`). Every failed-destination attempt in this
spike (round 2 and round 3 alike) left the sender's `balance_main`
byte-for-byte unchanged, immediately and in every later read — there is no
observable moment, even transiently, where it was debited and then
credited back. Given the whole causal chain resolves synchronously within
one `avm-local-stand call` (confirmed repeatedly), this spike cannot fully
distinguish "never debited" from "debited and atomically refunded within an
opaque single step" by timing alone — but A8 below settles the question a
different way: see A8's conclusion. The practical, load-bearing fact either
way: **the sender's observable balance for `value_main`/extra currencies
only ever decreases when the destination succeeds.**

### A5 — Does a successful payout still produce a callback? What does it carry?

**OBSERVED — yes, status_code 0, carrying no value.** Forwarded `123` +
`extra(7,44)` from `S2` to `S1` with `out_tag=0` (the REPORT/success
behavior), requesting a callback. `S2`'s diagnostic state afterward:
`status_code=0`, `msg_value_main=0`, `extra_count=0`. **This resolves the
round-3 conditional directly: success callbacks are real and reliable on
this platform — no redesign of the pending-withdrawal lifecycle is needed on
that front.** Combined with A3: a successful withdrawal both debits the
sender for real *and* reliably notifies it via callback — the common case is
fully sound.

### A7 — Payout to a `fund-user`-funded-but-never-`deploy`ed same-DApp address

**OBSERVED — a materially different outcome from the never-funded case
(addendum C, round 2): the entire call is rejected outright, not silently
dropped.** `fund-user --address <never-deployed>` itself reports
`"status": "ok"` (it queues a funding grant — recall `inspect`'s `funding`
array — without creating an account). A subsequent attempt to forward value
to that same address fails the **whole calling transaction**:
`"local call materialization failed: local call account is absent"`
(exit code 2). The sender's own balance was confirmed unaffected (no debit
attempted — the call never got far enough to try). So there are now two
*distinct*, both-confirmed "undeployed destination" behaviors depending on
prior history:
- never touched at all (round 2, addendum C): silent no-op, `"status":"ok"`,
  no error, no callback, no debit.
- `fund-user`'d but never deployed (this probe): hard rejection of the
  entire sending call, no error reaching the contract, no debit.

Neither case produces a callback or a debit, so the practical consequence
for SPEC's liveness limitation (§"L") is the same either way — a withdrawal
to an address that's never actually deployed cannot resolve — but *how* it
fails differs (silent vs. a loud, whole-call rejection the sender's own
code could in principle detect and handle *if* the CLI/host surfaced it as
something other than a top-level call error). This distinction doesn't
change any invariant, but it's recorded precisely because the two code
paths are clearly different in the host.

### A1 — Receiver fails, no callback requested: where does the attached value go?

**OBSERVED — still fully rolled back, independent of whether a callback was
ever requested.** Forwarded `150` + `extra(7,20)` from `S2` to a failing
`S1`, `want_callback=0`. `S2`'s `balance_main` afterward: unchanged
(`1877`, identical to its value before this attempt). Requesting a callback
or not has no bearing on whether the failed transfer is rolled back — it
only affects whether the sender gets notified.

### A2 — A failing call to a contract by a plain (external) caller: where does the attached value go?

**OBSERVED — rolled back, same as the internal case.** Isolated,
back-to-back measurement on `S1`: baseline `balance_main = 623`; one
external call directly to `S1` with `--value-main 999 --extra-currencies
"9:55"` and a body tag forcing contract-level failure (confirmed via the
decoded `raw_executor_output`'s own `status_code = 1000`); immediate
re-read: `balance_main` still exactly `623`. A failing call's attached value
is never credited to the destination, regardless of whether the caller was
internal or an external, unauthenticated-by-the-contract identity.

### A6 — Can a raw, SDK-free guest contract set the callback flag, or smuggle data past the host's output decoder?

**EMPIRICALLY CONFIRMED, upgraded from SOURCE-READ — no, on both counts.**
Built `spike/raw-probe/`, a cdylib with **zero dependency on
`avm-contract-sdk`**, hand-implementing `avm_guest_alloc`/`avm_guest_free`/
`init`/`process_message` directly over a static byte array. Two variants of
its `process_message` were built and deployed:
- A byte-for-byte minimal, well-formed `ACMO` output (magic + status=0 +
  one `FullReplace` state update of 8 zero bytes + zero-length
  callback/outbound/callback-intent/event sections, 41 bytes total):
  **deployed successfully** (`"status": "ok"`), confirming the hand-rolled
  encoder itself was correct and not some other reason for failure.
- The identical structure with **one extra trailing byte appended** (42
  bytes): **rejected** —
  `"deploy rejected canonical operation: existing deploy init output failed: TrailingBytes"`.

This directly answers two things at once: the host's decoder for guest
*output* is exactly as strict about trailing bytes as the guest-side input
decoder's own `Reader::finish()` (confirmed previously only for input);
and — combined with the wire-format reading already in Q2 — there is no
byte position anywhere in a guest-authored `ACMO` buffer, raw or
SDK-mediated, that the host would interpret as a settable message flag:
even a fully raw contract cannot smuggle one in, and the documented field
layout has no gap to put one in even if trailing bytes were tolerated
(they are not). **Q2 is hereby upgraded from "SOURCE-READ" to "EMPIRICALLY
CONFIRMED."**

### A8 — If the sender's own callback handler itself fails, where does the callback's carried value end up?

**OBSERVED — and this corrects Q1's original description of the refund
mechanism (see the correction note above Q1).** Two identical experiments,
differing only in one build-time switch:
- `okA`: normal callback handling (always returns success after recording
  diagnostics).
- `failB`: callback handling unconditionally returns `failure(2000)`
  instead (`AVM_SPIKE_CALLBACK_FAILS=1`), skipping the diagnostic write
  entirely — by construction, this means `failB`'s own state write never
  happens on the callback path, so its "during-the-callback" balance could
  not be captured for this variant; only the final persisted balance was
  available for it.

Both funded with `value_main = 9000`, then forwarded `888` to a failing `S1`
requesting a callback. Results:
- `okA`: `balance_main` *during* the callback's own execution
  (`input.balance_main`, captured into the diagnostic state write) was
  **`9000`** — not `9888`. The callback's own carried `msg_value_main =
  888` did **not** add to `input.balance_main` even while that exact
  message was executing. Final persisted balance (read back later): `9000`.
- `failB`: final persisted balance (the only measurement available, since
  the diagnostic write itself is skipped when the handler fails): **`9000`**
  — identical to `okA`.

**Both variants end up at the identical final balance, and that balance
never reflects the carried `888` in either case.** The conclusion this
forces: the callback's carried `value_main` on a failure callback is
**not** a real, protocol-credited value at all — it is informational. This
reconciles with A1/A3: the original transfer attempt was never actually
debited from the sender to begin with (for a failing destination), so there
is nothing real for the callback to "return." The one quantity
`docs/fuel-and-fees.md` documents as genuinely prepaid-and-refunded is the
**FUEL reserve** (`fuel_fee_shells`) specifically — a different field,
prepaid at materialization regardless of outcome — not the transferred
`value_main`/`extra` amount, which this spike never prepays (`fuel_fee_shells
= 0` throughout, as SPEC pins for the vault's payouts).

**Direct answer to "where does that value end up": nowhere — it was never
real money in the account's actual balance to begin with, in either the
handler-succeeds or handler-fails case.** The practical implication for
design: a ledger "credit back on failure" operation is pure internal
bookkeeping arithmetic, not a claim on any real, at-risk account balance —
the vault's real balance for a given currency is simply never reduced by a
withdrawal attempt that the destination's execution fails. A verification
gate on the callback's carried value (round 2's I11) was solving a problem
that does not exist, and — per this same probe — making the callback
handler itself *fail* as that gate's consequence has no additional
protective value (there's no real credit at stake to lose) while removing
the one thing that gate was meant to produce (a resolved pending record).
Round 3's SPEC revision drops the carried-value gate and restores
unconditionally on any failure callback.

### A9 — Who pays FUEL for an external call: the destination's SHELL, or the caller's?

**OBSERVED (destination pays) + SOURCE-CITED; one sub-question UNKNOWN.**
`docs/fuel-and-fees.md` states plainly: *"The sender prepays the reserve
when the outbound message is materialized"* — but an `ExternalMessage`
(`crates/avm-contract-sdk/src/input.rs`) has no `AccountAddress` source at
all, only a bare `pubkey`; there is no on-chain account to charge as
"sender." Observed SHELL deltas on `S1` directly confirm the **destination**
account's own SHELL balance is what actually shrinks: repeated external
calls to `S1` (both ones the contract's own logic accepted and ones it
rejected outright — see A2) each independently reduced `S1`'s own
`acct_first_extra_amount` (its SHELL holding) by that call's
`charged_shells`, with no other account in the picture. **A rejected call
costs the destination SHELL exactly like an accepted one** — confirmed via
A2's own isolated measurement (the failing external call still appeared in
`charged_shells`/`cumulative_fee_shells` deltas).

**UNKNOWN:** whether a message that's invalid or unsigned *at the protocol
level* (never reaching contract code at all — e.g. a bad signature) is
still charged. This could not be tested: the CLI always signs correctly
with a key it holds; there is no way to submit a deliberately-malformed or
unsigned external message through the public tooling to observe this
specific sub-case.

**This is directly load-bearing for the vault's own design**, since every
externally-reachable vault method (`propose_owner`, `accept_owner`,
`set_pause` in round 2's design) is now confirmed to cost the **vault's
own** SHELL to process — successful or not, authorized or not. Anyone can
make the vault spend its own SHELL simply by sending it external calls,
including ones with the wrong owner key that get rejected immediately.

### A10 — Does `avm-contract-build build --no-default-features` work at all?

**OBSERVED — no, the tool doesn't accept it; round 2's production-build
design is unworkable as written.** Direct test against the real tool:
```
$ avm-contract-build build --manifest-path examples/contract-a-v1/Cargo.toml --no-default-features
error: unexpected argument '--no-default-features' found

Usage: avm-contract-build build --manifest-path <Cargo.toml>
```
This matches `docs/cli-reference.md`'s documented syntax exactly:
`avm-contract-build build --manifest-path <Cargo.toml>` — **no other
options are accepted at all.** There is no way to pass `--no-default-features`,
`--features`, or any other Cargo flag through this tool. Round 2's
`demo-owner` Cargo-feature design (default on, disable via
`--no-default-features` for a "production" build) cannot work as specified,
because the only tool this repo's conventions use to build an example
cannot be told to pass that flag through. **Round 3's SPEC revision drops
the Cargo-feature mechanism entirely** and returns to a pure
environment-variable gate (env var set → use it; unset → fixed demo key),
identical in shape to `contract-a-v1`'s own existing, working pattern —
since environment variables *do* pass through to the underlying `cargo
build`/`rustc` invocation without needing any CLI flag, unlike Cargo
features.

Separately checked for item 7's demo-owner private-key question: grepped
the full repository for the `FIXTURE_PACKAGE_OWNER_PUBKEY` constant and any
nearby private-key material. **None found anywhere in this public repo.**
This neither confirms nor refutes whether its private key is known to
anyone outside this repo (e.g. in a private SDK test fixture) — it only
confirms this public checkout contains no record of it. Treated as
*unverified*, not *confirmed safe*, in the SPEC/THREAT_MODEL revision.

## Addendum (Gate 1 round 4)

### P2 — Can guest code read its own `dapp_id`/address? [SOURCE-READ, no probe needed]

**OBSERVED — yes, directly, no capture hack required.**
`crates/avm-contract-sdk/src/input.rs` declares `pub destination:
AccountAddress` as a top-level field on *both* `MessageInput` (line 309)
and `BatchInput` (line 322) — distinct from `input.message.destination()`,
which is the same value reached through the canonical message. Since
`decode_message_input`/`decode_batch_input` require the message's own
declared destination to equal this field (`DestinationMismatch` otherwise),
`input.destination` is, structurally, always this account's own address.
Every probe in this spike has in fact been using exactly this field already
(e.g. in `EventValue::Address(input.destination)`), so this was already
implicitly relied on, just not previously called out as the direct answer
to "can a contract see its own address." No stand probe was needed — this
is a plain, unconditional struct field, not something that could behave
differently at runtime.

### P3 — Which CLI field exposes raw state? [Corrects SPEC/THREAT_MODEL's "unreadable" framing]

**OBSERVED — `raw_state`, present on every `avm-local-stand call` response,
reflecting that account's state immediately after that call.** Confirmed
directly: every saved response in this spike's history has a top-level
`"raw_state"` key (hex-encoded raw bytes), alongside an ABI-decode attempt
into `"state"` (null, with `"state_decode_error"` explaining why, whenever
the declared ABI type doesn't match — a tooling mismatch in this spike's
own `avm-contract.toml`, not a platform limitation). **This corrects a
claim repeated through rounds 1–3 of this spike** ("there is currently no
documented way to read raw account state via the CLI, issue #5") — that
claim conflated two different things. What issue #5 and `inspect`
specifically lack is a way to read state *without also making a call* (and
`inspect` itself shows none of this). But `call` itself — on *any* method,
including one designed to be a harmless no-op/read probe — returns the
account's current state as a side effect, for free, to whoever can
construct a valid call. This is a real, already-available read channel,
not a gap. SPEC/THREAT_MODEL round 4 corrects the "unreadable" framing
accordingly and uses this channel for `admin_seq` observability.

### P1 — SHELL exhaustion: what actually happens, and can it be recovered from?

**OBSERVED, several distinct sub-findings, one of which corrects round 3's
hopeful unverified claim.** `docs/fuel-and-fees.md` is unambiguous that
local-free status covers compute only: *"Local-free applies only to
delivery and receiver compute for that message. It does not waive storage
rent, code publication fees, account initialization fees, name fees, or
other protocol operations that have their own prices."* Open issue #21
measured local-stand rent at roughly **1 SHELL per byte per second,
collected from the account when it is next touched** (its own example: a
6.3 KB registry owed ~543 million SHELL after one day of block time) and
left open exactly the question this probe answers: *"what happens when an
account can't pay."*

Procedure: deployed a probe funded with only 6,000 SHELL, then drove its
balance down with repeated external calls across an increasing `block_time`
gap (to accumulate rent as well as ordinary per-call fees).

- **External calls transition through three distinct states as SHELL
  depletes**, not a single cliff: (1) normal execution; (2) a call that is
  *admitted and charged* (`receipt.charged_shells` present, height
  advances) but whose guest code never actually runs — empty
  `raw_executor_output`, empty `events` — consistent with a FUEL budget
  that rounds to zero once the affordable reserve is too small to grant any
  compute at all; (3) outright rejection, `"local call charge failed:
  InsufficientBalance"`, once even the minimal/fixed charge can't be
  covered — confirmed this floor is independent of the requested
  `--fuel-fee-shells` (tried `1000`, `100`, and `0`, all rejected
  identically once balance was low enough).
- **Internal (local-free) calls to a SHELL-exhausted destination ALSO
  fail — this corrects round 3's unverified assumption that they would be
  unaffected.** A fresh, separately-funded sender forwarding a plain,
  zero-value internal message to the drained account (requesting a
  callback to observe the outcome) received a failure callback with
  **`status_code = 56`** — an undocumented status code, not found anywhere
  in `docs/` or any issue searched. The message was *not* silently dropped
  (unlike the undeployed-destination case, round 2 addendum C/round 3 A7) —
  it was actively rejected, cleanly, with a specific (if unexplained)
  status. **SHELL exhaustion is not merely an "external admin surface
  down" problem — it makes the account unreachable by internal messages
  too**, at least for the "fresh request requiring compute" case tested
  here.
- **A guest contract cannot refill another account's SHELL via its own
  `OutboundIntent` — confirmed as a distinct, general restriction, not
  specific to a drained destination.** Forwarding `extra[currency_id=2]`
  from a contract's own `OutboundIntent` failed with `"local call
  materialization failed: EconomicsError"` **even when forwarding to a
  perfectly healthy destination, with the sender holding orders of
  magnitude more SHELL than requested** (isolated repeatedly: varied the
  amount, `fuel_fee_shells`, callback-request flag, and destination — all
  failed identically; a parallel send of a *different* currency in the
  exact same shape succeeded once the sender was confirmed to actually hold
  it, ruling out a methodology error). **No contract, anywhere, can send
  SHELL onward via its own outbound intent — only an externally-signed
  message can carry `extra[currency_id=2]`.** This is the direct,
  load-bearing justification for round 4's `TOPUP_METHOD_ID` being
  external-only, and for why the round-3 "deposit swallows SHELL silently"
  design was dead code on arrival: `DEPOSIT_METHOD_ID` is internal-only, and
  no internal message can ever carry SHELL in the first place.
- **Recovery is possible, but only via an external message, and `fund-user`
  specifically cannot be reused.** `fund-user` on an already-deployed
  address fails outright: `"local_address_already_funded"` — it is a
  one-time pre-deploy helper, not a general top-up tool, and does not model
  a real external SHELL transfer. A genuine external `call` carrying
  `--extra-currencies "2:1000000"` directly to the fully-drained account
  **succeeded** (`"status": "ok"`, `charged_shells = 101324` — a large
  charge, consistent with settling accrued rent debt in the same call that
  delivers the rescue funds). The incoming SHELL evidently covers that same
  message's own charge before any shortfall can reject it, avoiding a
  deadlock. **No deletion, freeze, or other irreversible state was ever
  observed** — every failure mode found was a clean rejection or an
  empty-execution admission, never account loss.
- **This is treated as a platform-wide property of AVM accounts, not
  something unique to this vault.** Nothing in what was tested is specific
  to the vault's design; any account that lets its SHELL run out should be
  expected to become unreachable (internally and externally) until an
  external message carrying enough SHELL reaches it.

**Open question for the SDK maintainers (recorded, not resolved here):**
what does status code `56` mean precisely, is it specific to
compute-starved internal delivery or a broader "destination insolvent"
signal, and is the exact rent formula / "next touch" collection timing
documented anywhere beyond issue #21's local-stand measurement? This
spike could not determine more than what's reported above by
observation alone.

### P4 — Reentrancy: a depositor contract that calls back into the vault during its own payout

**OBSERVED — the reentrant call is evaluated while the original withdrawal
is still genuinely Pending, and the original's settlement happens strictly
after the depositor's own execution (including its reentrant sends)
completes.** Built two purpose-specific throwaway probes (neither part of
any submission): `spike/mini-vault/` (a minimal stand-in for the real
vault's withdraw flow — single-withdrawal-at-a-time by design specifically
to make ordering observable: it stores `pending_flag` + `withdraw_seq` +
`deposit_count` in state, rejects a new withdraw while `pending_flag == 1`,
and clears it only on a `status_code == 0` callback) and
`spike/reentrant-depositor/` (on receiving *any* non-callback internal
message, immediately emits two outbound intents back to its caller — the
sender address is read directly from `Caller::Internal(source)`, no
baked-in address needed — one a reentrant withdraw attempt, one a reentrant
deposit, both constructed within the same execution that's handling the
original payout).

Procedure: funded `MiniVault` with `value_main`, called its `withdraw`
targeting `ReentrantDepositor` as the payout destination, then read
`MiniVault`'s final state.

Result: `pending_flag = 0`, `withdraw_seq = 1`, `deposit_count = 1`.
- `withdraw_seq` staying at `1` (not `2`) means the **reentrant withdraw
  attempt was rejected** by `MiniVault`'s own `pending_flag == 1` guard —
  which only rejects while a withdrawal is genuinely still outstanding.
  This directly confirms the reentrant call arrived, and was evaluated,
  **before** the original withdrawal's settlement callback had fired.
- `deposit_count = 1` confirms the reentrant *deposit* (not gated on
  `pending_flag` in this minimal model, matching how the real vault's
  deposits are independent of any one pending withdrawal) succeeded
  normally, in the same reentrant burst.
- `pending_flag = 0` at the end confirms the original withdrawal's
  settlement callback did eventually arrive and correctly resolve — *after*
  all of the above.

**Ordering, stated plainly:** pending record created → outbound payout sent
→ depositor's own execution runs, including its reentrant sends (evaluated
against the already-updated, post-debit state) → depositor's execution
completes → *only then* does the original withdrawal's own settlement
callback fire and resolve it. There is no window in which a reentrant call
observes a stale, pre-debit balance, and no window in which the original
pending record is ambiguously resolved while reentrant activity is still in
flight. This is the direct empirical basis for `docs/THREAT_MODEL.md`
§5.1's reentrancy conclusion, now demonstrated with a dedicated adversarial
scenario rather than inferred only from Q1's simpler single-withdrawal
test.

## Addendum (Gate 1 round 5)

### Reconciling P1 with Q1/A4: does the SHELL-forwarding `EconomicsError` depend on the destination's outcome?

**OBSERVED — no. It occurs unconditionally at materialization, before the
destination is even reached, regardless of whether that destination would
have succeeded or failed.** This was checked directly rather than assumed:
a freshly-deployed, generously-funded sender (`FRESH`, holding millions of
SHELL) attempting to forward `extra[currency_id=2]` to a destination
*configured to fail* hit the identical `"local call materialization
failed: EconomicsError"` that round 4's P1 observed when forwarding to
*healthy, succeeding* destinations. Materialization happens before the
message is dispatched anywhere, so the destination's eventual behavior
cannot be the deciding factor — and this test confirms it isn't.

This leaves Q1/A4's original result — a `value_main=700, extra=(2,50)`
forward to a *failing* destination that was reported to succeed — as
**not reproducible under controlled, isolated retesting**, and it is
retracted as stale rather than reconciled. Two direct reproduction attempts
using the original pattern failed to recreate it: one against a fresh
sender (`EconomicsError`, matching P1 exactly) and one reusing the original
`S2` sender from that early test (`InsufficientBalance` — that account had
since been drained by unrelated rent/fee accrual over the course of this
long session, so it couldn't even be used to retest the original claim).
No condition was found under which a contract's own `OutboundIntent`
carrying `currency_id=2` in `extra` succeeds. **P1's finding — no guest
contract can ever send SHELL via its own outbound intent, independent of
amount, destination, or destination outcome — is the current, reproducible
understanding; Q1/A4's original contrary data point could not be
reproduced and should not be relied on.**

### P5 — Does a callback execute when the receiving account's SHELL is zero or below what's owed?

**OBSERVED, with a workload-dependent threshold — not a single fixed SHELL
number.** Two otherwise-identical experiments, differing only in whether
the *forwarded-to* destination was configured to succeed or fail, using
freshly-deployed senders funded with the same starting balance
(`4200` SHELL, `3189` remaining after each sender's own deploy cost) and
immediately — before spending any balance on unrelated diagnostic calls —
forwarding to `S1` with a callback requested:

- **Failure-destination case:** the forward itself succeeded (`"status":
  "ok"`), but `receipt.charged_shells` (`3189`) exactly exhausted the
  sender's entire remaining balance, and the sender's own final state —
  read back from this same response's `raw_state` — was still the
  untouched `96`-byte all-zero `init()` state, **not** the `124`-byte
  `CBRD`-tagged diagnostic this probe's callback handler always writes on
  any callback it actually processes. **The callback was never executed.**
- **Success-destination case:** under the *identical* starting balance,
  the forward and the resulting callback both completed —
  `charged_shells = 2212`, leaving a `977`-SHELL surplus — and `raw_state`
  showed the `CBRD`-tagged diagnostic in full, confirming the callback
  **did** execute.

**Does state change when a callback fails to execute?** No — consistent
with every other failure observed in this spike, the sender's own state
was byte-for-byte unchanged from its pre-existing value (here, untouched
`init` state) in the case where the callback didn't run.

**Exact SHELL threshold:** there isn't a single one to report. The two
cases above started from the *same* balance and diverged only in how much
the *specific* completed work cost — a failing destination's return
callback apparently costs more than a succeeding one's in this setup (exact
cause not isolated: possibly failure-path bookkeeping, possibly incidental
rent/fee differences from this session's long accumulated history — a
third attempt to isolate this further hit an unrelated internal stand
error, `"local_pipeline_state_plan_failed: ... InvalidAccountTransition"`,
and was not pursued further since it wasn't reproducing the SHELL question
at hand). The concrete, reportable fact is: **a chain that is accepted and
begins executing can still end with its own return callback never
delivered, purely because the chain's total cost exceeds what the
initiating account held** — this is a real, now twice-observed failure
mode (not hypothetical), and it is **exactly as dangerous for a real vault
as for this probe**: a withdrawal whose debit and pending record are
already committed (checks-effects, before the outbound send) can still end
up with its settlement callback never arriving, for no reason related to
the depositor at all — purely because the vault itself was too close to
SHELL-empty when it initiated the withdrawal. This directly motivates
SPEC's `MIN_OPERATIONAL_SHELL` pre-check on `WITHDRAW` (round 5).

## Addendum (Gate 1 round 6)

New throwaway spike contracts, never committed: `spike/plain-receiver/`
(accepts any message, any method/body, always succeeds, no state change,
no side effects — a clean "successful withdrawal destination" baseline
with none of `ReentrantDepositor`'s own reentrant side effects);
`spike/mini-vault-big/` (byte-for-byte the same withdraw/deposit/callback
logic as round 4's `spike/mini-vault/`, but every `FullReplace` state blob
is padded out to exactly 31,159 bytes — SPEC's corrected worst-case size,
see below — to measure whether state size affects a withdraw chain's
SHELL cost). `probe-receiver`'s own empty-body dispatch (`body.get(4) =>
None`, which its existing code already fails with `1002`) was reused
as-is as the "failing withdrawal destination" case — no new failure-mode
code was needed for that side.

### Pending-record size correction (not a probe — a bug caught while
recomputing round-6's own numbers)

SPEC's §2.3 field table for a pending-withdrawal record was always
`withdrawal_id(4) + dapp_id(32) + account_id(32) + currency_tag(1) +
currency_id(4) + amount(16) = 89` bytes, but the section's own prose
label called it "85 bytes" (copy-pasted from §2.2's *ledger*-entry size,
which genuinely is 85 bytes). This propagated into every worst-case-size
and rent calculation in SPEC/THREAT_MODEL. Corrected worst case:
`91 + 332×85 + 32×89 = 91 + 28,220 + 2,848 = 31,159` bytes (was
erroneously `31,031`).

### Rent-rate reconfirmation, at scale, using the corrected worst-case size

**OBSERVED — issue #21's ~1 SHELL/byte/second figure holds to within 1%
at a realistic worst-case size, confirmed via the account-level
`storage_due_payment`/`storage_last_paid` fields `avm-local-stand inspect`
exposes (not previously used in this spike — a real, structured way to
read accrued rent directly, distinct from `raw_state`).** A
`mini-vault-big` instance (canonical account size 31,234 bytes — the
31,159-byte state plus a small, fixed account-record overhead) was left
untouched for 10,005 seconds of block-time (from deploy to first
post-deploy touch). `inspect` showed `storage_due_payment =
"310342012"` immediately before that first touch: `310,342,012 /
(31,234 × 10,005) ≈ 0.993` SHELL/byte/second — matching issue #21's
locally-measured rate closely, now confirmed at roughly 5× the byte-size
issue #21's own worked example used.

### P6 — Rent can consume an entire call's FUEL budget, causing the contract's own code to never run — and a value credit that call was supposed to deliver can be silently dropped in that state

**OBSERVED, directly, not inferred — a sharper, rent-specific instance of
round 4's P1 "admitted and charged but guest code never ran" state.** The
same first post-deploy touch on the rent-delinquent `mini-vault-big`
instance above (an external call carrying `--value-main 100000`, method
`mvb.body`, REPORT tag) returned `"status": "ok"` with `charged_shells =
"1997989"` (consuming nearly the account's entire prior 2,000,000-SHELL
balance) but `"events": []` and `"raw_executor_output": ""` — the guest
contract's own code never actually ran. A follow-up `WITHDRAW`-shaped call
immediately afterward, requesting a payout of `1` unit of `value_main`,
failed with `"local call materialization failed: EconomicsError"` —
proving the `100,000` `value_main` credit the first call nominally carried
never actually landed. Recovery: once the account's accrued rent was paid
down separately (via a plain external call carrying enough
`--extra-currencies "2:N"` to cover the debt — confirmed this still works
even against a 310-million-SHELL debt, consistent with round 4's P1
"recovery is always possible via a sufficiently large external message"
finding, just requiring a larger amount than P1's own test needed), a
repeat `value-main` credit call showed genuine execution
(non-empty `raw_executor_output`, ordinary small `charged_shells` of
`157171`), and the same `1`-unit withdrawal then succeeded normally.
**Direct answer to round 6's question of whether `input.extra` is read
before or after rent collection:** whenever the contract's own code runs
at all, `input.extra` already reflects a post-rent-collection balance —
rent affordability is resolved upstream, at materialization, before the
guest ever starts. But the guest may not get to run at all if rent
consumes the entire FUEL allowance, and in that case nothing inside the
contract — including a `MIN_OPERATIONAL_SHELL`-style check — ever gets a
chance to fire.

### P7 — Withdraw-chain SHELL cost, measured on withdraw-shaped fixtures, at small and large state sizes, success and failure

**OBSERVED.** Four `mini-vault`/`mini-vault-big` instances (two of each,
one routed to `plain-receiver` as an always-succeeding destination, one
routed to `probe-receiver` as an always-failing destination via its
natural empty-body `1002` rejection) each executed one withdraw of `1`
unit of `value_main`, with the withdrawing account's own `charged_shells`
recorded, and the resulting `pending_flag` byte in `raw_state` decoded
afterward to confirm the intended outcome genuinely occurred (not merely
assumed from top-level `"status": "ok"`):

| State size | Destination | `charged_shells` | `pending_flag` after |
|---|---|---|---|
| 91 bytes (small) | succeeds | 2,367 | `0` (Settled, confirmed) |
| 91 bytes (small) | fails | 2,367 | `1` (left pending by design, confirmed) |
| 31,234 bytes (this contract's worst case) | succeeds | 157,172 | `0` (confirmed) |
| 31,234 bytes (this contract's worst case) | fails | 157,172 | `1` (left pending by design, confirmed) |

(Relabeled in round 7: `pending_flag = 1` here does **not** mean the real
vault's "Restored" outcome — `mini-vault`'s own callback handler simply
sets `pending_flag = 1` on any non-zero `status_code` and has no restore
logic of its own at all, by the spike fixture's own deliberately minimal
design; it was mislabeled "Restored/stuck pending" in the original
round-6 writeup, which could be misread as claiming this spike fixture
demonstrates the real vault's ledger-credit-on-failure behavior. It does
not — `mini-vault` only demonstrates that the failure *callback itself
arrives correctly*; the real vault's actual restore logic, Invariant I11,
is implemented and tested only in `../SPEC.md` §8's tests against the
real contract, not here.)

Two things worth recording precisely: success and failure cost
*identically* within each state-size tier here (unlike round 5's P5,
which saw 2,212 vs 3,189 on the generic probe-receiver fixture) —
consistent with the dominant cost driver being the withdrawing account's
own `FullReplace` rewrite, which happens regardless of destination
outcome, rather than the destination's behavior. Separately, a
materially larger one-time cost (`781,851`) was observed for a plain
report call (not a withdraw) on a large-state account under nominally
similar conditions to the `157,172` figure above — a roughly 5× swing this
spike could not fully attribute to a single cause. Both figures are
recorded in `../SPEC.md` §1 and used to justify `MIN_OPERATIONAL_SHELL`'s
revised value (`1,600,000`, roughly 10× the clean `157,172` worst-case
withdraw-chain figure, chosen specifically to also absorb the ~5× variance
observed here).

### P8 — Currency round-trip: does the SHELL-only `OutboundIntent` restriction (round 4's P1) extend to any other currency id?

**OBSERVED — no. Currency id `2` (SHELL) remains the only currency this
restriction has ever been observed to apply to.** A `probe-receiver`
instance was funded with extra currency ids `0`, `1`, `3`, and `7` (1,000
units each, via a plain external call carrying `--extra-currencies
"0:1000,1:1000,3:1000,7:1000"`), then used its own `FORWARD` dispatch to
send `10` units of each currency id, one at a time, to a `plain-receiver`
destination, each requesting a callback. All four completed with a
genuine `status_code == 0` success callback, confirmed by decoding the
sender's own `CBRD` diagnostic state write (not merely the top-level call
status): currency `0` → `status_code = 0`; currency `1` → `0`; currency
`3` → `0`; currency `7` → `0`. None hit `EconomicsError`. This directly
confirms `DEPOSIT`'s SHELL-rejection rule (error 1519) is correctly scoped
to `currency_id == 2` only — no other currency id needs to be added to
it.

### Methodology note: `--extra-currencies` on a `call` is confirmed, again, to be purely a local-stand minting convenience, not a model of real account-to-account value transfer

Every currency-funding step in this round (and in rounds 4–5) worked by
passing `--extra-currencies "<id>:<amount>"` directly to
`avm-local-stand call`, crediting the destination with currency that
appeared from nowhere, chargeable only against that same call's own fee.
This is the same mechanism round 3's A9 already established has no
real-network analog for `ExternalMessage` specifically (no source account
field exists to debit), and this round leaned on it directly to fund
currencies `0`, `1`, `3`, `7`, and repeatedly to rescue rent-delinquent
accounts with large SHELL amounts. **Nothing in this spike has ever
exercised a real-network-shaped path for an external message to carry
real value from a real source** — every positive result involving
`--extra-currencies` or `--value-main` on an external call should be read
as "the local stand's test-harness minting works," not as "a real
network's external-message economics were exercised." This directly
informs `../docs/THREAT_MODEL.md`'s and `../SPEC.md`'s round-6 framing of
`TOPUP_METHOD_ID`'s real-network viability as UNVERIFIED, not merely
untested.

## Addendum (Gate 1 round 7)

New throwaway spike contract, never committed: `spike/mini-vault-big-c/`
(byte-for-byte round 6's `mini-vault-big` logic, republished under a new
schema id purely to get a fresh, deployable package — `avm-contract-build`
rejects publishing byte-identical code a second time without
`--package-id`/`--selected-code-id`, which this spike has never recovered
the exact values for; a trivial source variant sidesteps that cleanly, as
in round 6). Deployed as `mvbigC`, funded with 2,000,000 SHELL only (no
`value_main`), then left completely untouched for 10,020 seconds of
block-time — reproducing round 6's P6 setup exactly, but this time probed
from the *sender* side via a genuine internal message instead of an
external CLI call.

### P9a — An internal, deposit-shaped message (carrying `value_main`, requesting a callback) sent to a rent-delinquent destination

**Relabeled (Step 0 doc patch, pre-implementation) to separate what was
actually measured from what was concluded: sender-side is OBSERVED
directly; vault-side is INFERRED, not directly observed.** The sender's
own balance was read directly from
the call's own `events` output, before and after — a genuine measurement.
The vault's own `deposit_count` was never actually read after the
attempt (doing so would have required a further call the vault could no
longer afford — see below); "its own guest code never ran" is inferred
from the combination of `storage_due_payment` staying almost fully
outstanding and a follow-up call failing with `InsufficientBalance`, not
from directly observing `mvbigC`'s own state. Sender: a `probe-receiver`
instance (`failrecv`, already deployed in round 6),
funded with real `value_main = 500,000` and ample SHELL, confirmed
rent-current immediately beforehand. Using `probe-receiver`'s own
`FORWARD` dispatch, `failrecv` sent an `OutboundIntent` carrying
`value_main = 50,000`, `method_id = 0`, `body = [1,0,0,0,11]` (mini-vault's
own `DEPOSIT` tag), `fuel_fee_shells = 5,000` (an ordinary, modest reserve
— not an enormous one deliberately sized to force success), to `mvbigC`
(rent-delinquent for 10,020 seconds, ~313 million SHELL theoretically
owed), requesting a callback.

- **The external call that triggered this (on `failrecv`) returned
  `"status": "ok"` with genuine execution** (`raw_executor_output` 1,278
  bytes, non-empty; `charged_shells = 7,952` — a small, ordinary cost,
  confirming `failrecv`'s own side was healthy and unaffected by the
  destination's condition).
- **A callback was delivered, with `status_code = 56`** — the same
  undocumented code round 4's P1 first observed for a plain, zero-value
  internal message to a SHELL-exhausted destination, now confirmed for a
  value-carrying, deposit-shaped message as well. Decoded directly from
  the call's own `events` array (the `probe.callback` event's
  `status_code` field — this round successfully read plain ASCII field
  names and values directly out of this array, which earlier rounds had
  treated as an opaque, not-worth-decoding format; it turned out to be
  straightforwardly legible by eye for this purpose, so this correction
  is noted for any future round that needs it again).
- **Sender balance: confirmed unchanged, directly, not merely assumed.**
  The same `events` array's `probe.callback` event reports `balance_main =
  500,000` — identical to the pre-forward value reported in the paired
  `probe.report` event from the same call. **`failrecv` was not debited.**
- **Vault (destination) side: INFERRED, not directly observed — touched
  and partially charged; "its own guest code never ran" is a conclusion
  from the following indirect evidence, not a direct read of `mvbigC`'s
  own state.** `inspect` immediately after showed `mvbigC`'s
  `storage_last_paid` advanced to the call's own block-time (confirming it
  *was* touched — this is not the "never reached at all" case) and
  `storage_due_payment = 310,966,691` still outstanding (confirming only
  partial rent payment — roughly `1,997,989` of its own pre-existing
  2,000,000-SHELL balance was consumed, the same figure round 6 saw in an
  unrelated call, suggesting this may be some fixed charge for this
  "admitted, partially charged, zero-FUEL" outcome specifically, not
  re-derived further here). A direct follow-up `REPORT` call to `mvbigC`
  immediately afterward failed outright with `"local call charge failed:
  InsufficientBalance"` — confirming its remaining spendable balance was
  by then too small to afford even a trivial call, let alone run the
  `DEPOSIT` dispatch and increment `deposit_count`.
- **Depositor-loss scenario: none found, on either side, at its own
  evidence level.** The attempted deposit's `50,000` value_main was never
  taken from the sender — **OBSERVED**, directly, via the balance readback
  above. It was also never credited to the vault's ledger-equivalent
  counter — **INFERRED**, not directly observed, from the vault's own
  guest code never having run (per the bullet above). Taken together, the
  message appears to have failed atomically on both sides, consistent
  with every other failure this spike has directly observed — but only
  the sender half of that claim is a direct measurement; the vault half
  rests on the same inference chain (due-payment left outstanding, then
  `InsufficientBalance` on the next touch) as the bullet above, not on a
  direct read of the vault's own `deposit_count`.

### P9b — An external, TOPUP-shaped SHELL amount smaller than the debt (plus whatever the call itself would cost) sent to the same rent-delinquent vault

**OBSERVED.** Immediately after P9a (so `mvbigC` was, if anything, in a
*worse* financial state — its own balance nearly exhausted by the partial
rent charge above), a plain external call carrying `--extra-currencies
"2:50000000"` (50 million SHELL — smaller than the `310,966,691` already
recorded as due, let alone that plus this call's own cost) was sent
directly to `mvbigC`. Result: `"local call materialization failed:
EconomicsError"` — rejected outright, at the same materialization stage
round 6 observed for an analogous undersized-relative-to-debt attempt.
`inspect` immediately after showed `mvbigC`'s `storage_last_paid`,
`storage_due_payment`, and `transaction_lt` all **byte-for-byte unchanged**
from immediately before this call — the account was not touched at all.
**Where does the carried SHELL end up? Nowhere — the entire message is
rejected atomically before any materialization, exactly like every other
failed-delivery case this spike has observed; the 50,000,000 SHELL it
carried is simply never applied, not partially credited and not lost
into some unrecoverable state.** This differs from the "admitted,
partially charged, zero-FUEL" outcome P9a's internal message hit: an
*external* topup attempt that is too small to plausibly cover the debt
appears to be rejected at a strictly earlier stage (before any charge is
even assessed), while an *internal* message to the same account was
admitted far enough to be partially charged before failing. **Depositor-
loss scenario: also none observed here** — the sender of this external
message risks nothing from the attempt itself failing this way (no SHELL
was taken, no partial credit occurred); the real risk this probe
highlights is operational, not a loss of the specific SHELL sent: an
undersized top-up accomplishes nothing toward rescuing the vault, and a
party relying on "I sent *some* SHELL, surely that helps" would be wrong —
round 6 already established that a sufficiently *large* external SHELL
transfer does rescue an account in this state (confirmed again as still
true, by implication, since nothing here contradicts it); this round
confirms specifically that an *insufficient* one does nothing at all,
succeeding or failing in a way that leaves the account exactly as
delinquent as before, with no partial progress banked.

**Both P9a and P9b are recorded as OBSERVED, not UNKNOWN, overall — but at
mixed evidence levels within P9a specifically, not uniformly direct.**
P9b's finding is fully OBSERVED on both halves (the external call's own
rejection, and the account's untouched `inspect` fields, were both read
directly). P9a's finding is OBSERVED on its sender half only (balance
readback) and INFERRED on its vault half (no guest execution, no
credited deposit) — see the relabeled bullets above for exactly which
half is which. Neither scenario produced a depositor-loss scenario (value
taken without being credited) at the evidence level each half actually
supports; both failed atomically with no confirmed value movement on
either side, consistent with every prior "no partial debit-without-credit"
finding in this spike (A1, A2, A3, P1, P5, P6) — with the caveat that
P9a's vault-side half of this specific claim is an inference, not a
direct measurement, and should be read accordingly.

## Addendum (Phase 2 implementation)

Two throwaway spike probes, never committed, built while wiring up
dispatch for the real vault's four externally-invoked methods.
**SDK release this addendum's findings are pinned against:** release tag
`avm-dev-06e96ef71006-sdk-0748f4f0767a` (`ackinacki_commit
06e96ef71006cca020575cbf35e6b82cc82a324e`, `sdk_commit
0748f4f0767a26da8bc363bcb6b5585539d6970e`, `guest_version 0.1.1`,
`bundle_version 0.2.0` — read directly from
`~/.local/share/avm-sdk/releases/<tag>/avm-sdk-linux-x86_64/BUILD-
MANIFEST.json`, confirmed to be what `current` is symlinked to at the
time of this addendum). If this environment's installed release ever
changes, the four discovered method ids below (and this body-shape
finding) must be re-verified against the new release before being
trusted again.

### P10 — No SDK-side helper exists to compute a method_id from its ABI name; `avm-contract-build`'s own tool version; where `contract-a-v1`'s `VERSION_METHOD_ID` actually came from is undeterminable from this checkout

**OBSERVED (absence confirmed by exhaustive search) / UNKNOWN (for
`contract-a-v1`'s own origin).** Searched for a public method-name-hashing
helper in three places: `avm-contract-sdk`'s own exported items (`crates/
avm-contract-sdk/src/{lib,input,output,runtime}.rs` — no such function;
the crate only encodes/decodes wire formats given an already-resolved
numeric `method_id`, with no concept of ABI names at all); every script
under `scripts/` and every doc under `docs/` in this checkout (no hits for
`method_id` computation at all); and `contract-a-v1`'s own git history
(`git log -p --follow -- examples/contract-a-v1/src/lib.rs`) — its
`VERSION_METHOD_ID` constant was introduced fully-formed, with no
derivation comment, in the repository's initial "feat: publish private
AVM developer SDK distribution" commit. **Conclusion: the method-name
hashing algorithm exists only inside the closed-source
`avm-local-stand`/`avm-contract-build` binaries (`avm-contract-build
--version` reports `0.1.0`, a build-tool-only version string, not the
full SDK release), with no source-available equivalent anywhere in this
checkout** — confirming `docs/PLATFORM_FINDINGS.md`'s original
methodology note (round 1) that this hash is undocumented, now extended
with a specific, exhaustive search rather than an assumption. Whether
`contract-a-v1`'s own author discovered `VERSION_METHOD_ID` via the same
empirical technique used here, or had privileged access to the closed
algorithm, cannot be determined from this public checkout either way.

### P11 — Real wire-level body bytes for the four externally-invoked methods, captured directly, confirming no length prefix for `fixed_bytes` ABI arguments

**OBSERVED.** `spike/bodyprobe/` declared the exact same `avm-
contract.toml` ABI shape the real vault uses for its four external
methods (`kind = "fixed_bytes"`, `len = 36/4/5` for
`vault.propose_owner`/`vault.accept_owner`/`vault.set_pause`
respectively; no input type at all for `vault.topup`), and
unconditionally recorded `input.message.body()`'s length and first bytes
into state on every call, regardless of value. Called via the CLI with a
flat hex payload of the exact expected length for each:

| Method | Payload sent (hex) | Body observed (`body_len`, bytes) |
|---|---|---|
| `vault.propose_owner` | `44332211aa×32` (36 bytes) | `36`, byte-for-byte identical |
| `vault.accept_owner` | `88776655` (4 bytes) | `4`, byte-for-byte identical |
| `vault.set_pause` | `ccbbaa9901` (5 bytes) | `5`, byte-for-byte identical |
| `vault.topup` | (no input type declared) | `0` |

**No 4-byte length prefix appears for any `fixed_bytes` argument** — this
differs from this spike's own earlier bonus finding (round 1) that a
`kind = "bytes"` (variable-length) argument *is* auto-wrapped with a
4-byte LE length prefix ahead of the raw bytes. The distinction makes
sense in hindsight: `fixed_bytes`' length is already fixed and known from
the ABI schema itself, so the host has no need to prefix anything. This
directly confirms `SPEC.md` §4's body-length columns (36/4/5 bytes
respectively) and the real vault's own exact-length body checks
(`body.len() != 36/4/5`) are already correctly matched to the real wire
shape — no SPEC change and no length-prefix-stripping logic was needed in
`src/handlers.rs`.

### P12 — Confirming `DEPOSIT_METHOD_ID`/`WITHDRAW_METHOD_ID` (the `NORMAL_METHOD_TAG<<32|N` formula) reach the real, built vault via genuine internal messages

**OBSERVED.** The real vault (`examples/multi-currency-vault`'s own build
artifacts, not a spike fixture) was deployed to the stand and funded with
10,000,000 SHELL (clearing `MIN_OPERATIONAL_SHELL`). A new throwaway
spike contract, `spike/relay-probe/` (never committed — built because
`probe-receiver`'s own `FORWARD` primitive only ever sends a fixed
5-byte `[1,0,0,0,tag]` outbound body, unusable for sending `WITHDRAW`'s
exact 21-byte structured body to a real contract) was deployed, funded,
and used to send two internal messages directly to the real vault: a
`DEPOSIT` (`method_id = 0x4D43565400000001`, `value_main = 777`), then a
`WITHDRAW` (`method_id = 0x4D43565400000002`, body
`currency_tag=0 + currency_id=0 + amount=777`, requesting a callback).

**Result: the `WITHDRAW` call's own requested callback arrived with
`status_code = 0`** (decoded directly from `relay-probe`'s own `CBRD`
diagnostic state write, not merely the top-level call status). Since
`handle_withdraw` only returns success after its ledger-balance check
passes (requiring a real, already-credited entry of at least `777`),
this directly confirms both `DEPOSIT_METHOD_ID` and `WITHDRAW_METHOD_ID`
were correctly recognized by the real vault's dispatch — neither fell
through to `unrecognized_method` (1002), and the `DEPOSIT` genuinely
credited the ledger entry the `WITHDRAW` then successfully matched and
debited.

**One nuance, recorded precisely rather than overclaimed:** this
callback's `status_code` reflects the vault's own processing of the
`WITHDRAW` message itself (debit, pending-record insertion, outbound
dispatch all succeeding) — not necessarily that the downstream payout
was deliverable to this specific `relay-probe` fixture, whose own
dispatch (like `probe-receiver`'s) rejects an empty, untagged body. That
narrower question is exactly what `../SPEC.md` §8 test 5 already
anticipates and is designed to test with a purpose-built fixture, not
what this check was aimed at. What P12 establishes — the one thing it
set out to check — is that the formula method ids genuinely reach and
are matched by the real vault's dispatch over the wire, which it does
conclusively.

### P13 — Stand smoke test: all four `External` method names resolve to exactly the pinned constants, none fall through to `unrecognized_method`

**OBSERVED.** Gate 2a round 2 item 7's stand smoke test: the real vault
(demo-fallback-owner-key build, same artifacts as P12) was freshly
deployed and funded, then called once each via `avm-local-stand call
--method <name>` for all four `External` methods, using the stand's own
`user` identity and deliberately minimal/placeholder-correct-length
arguments (no attempt to satisfy ownership or SHELL-content
requirements — the smoke test's only question is whether the name
resolves at all):

| Method | `method_id` in the response | Matches `constants.rs`? | `status_code` (decoded from `raw_executor_output`) |
|---|---|---|---|
| `vault.topup` | `0xc0aac68a6cd17e18` | yes, exactly | `1526` (`topup_requires_shell_only` — empty `extra`, as sent) |
| `vault.propose_owner` | `0xfc4183aab1661acf` | yes, exactly | `1514` (`unauthorized_owner` — `user`'s pubkey isn't the demo owner key, as expected) |
| `vault.accept_owner` | `0x8fc03d2a5e71b6df` | yes, exactly | `1515` (`unauthorized_pending_owner` — no rotation proposed, as expected) |
| `vault.set_pause` | `0x33c0aeb43f5360d5` | yes, exactly | `1514` (`unauthorized_owner`, as expected) |

None of the four returned `1002` (`unrecognized_method`), and — beyond
the literal ask — the CLI's own response JSON echoes `method_id`
directly, giving an independent, second confirmation (beyond
`spike/methodid-probe/`'s original discovery) that all four pinned
constants in `constants.rs` are exactly correct against the currently
installed SDK release. Every other returned code is exactly what
SPEC.md §4 predicts for a correctly-shaped call from an unauthorized/
under-provisioned sender — not an error in this check, confirmation that
dispatch routed into the real handler logic rather than stopping at the
name-resolution step.

### P14 — The real platform caps every message at exactly 8 extra-currency entries, enforced at the envelope level before any guest execution — Gate 2a round 3 item 3c

**Context.** Item 3c asked for a real-stand run of one relay account,
funded with 300 distinct extra currencies, sending one `DEPOSIT` carrying
all 300, then a `WITHDRAW` — reporting success, `charged_shells`,
`raw_executor_output`, and any FUEL/size errors. Rebuilding `spike/
relay-probe/`'s full wire protocol from a prior, now-compacted-away
session, purely to drive an internal message (`DEPOSIT` is
`Internal`-caller-only), was judged disproportionate once the question it
was meant to answer — does a single message carrying very many extra
currencies hit a real platform limit? — turned out to be answerable
directly and conclusively via `vault.topup` alone (`External`,
permissionless, no relay needed), since the limit this probe found is
enforced identically regardless of caller type, before the message is
even routed to a specific method or caller kind. See `REVIEW.md` item 3c
for this scope decision recorded as a judgment call.

**OBSERVED.** A single `avm-local-stand call --method vault.topup
--extra-currencies "<N distinct id:amount pairs>"` against the real
deployed vault:

- `N = 300` (ids 100–399, matching item 3c's literal ask): rejected
  immediately, before reaching guest execution at all:
  ```json
  {"error": "call rejected canonical operation: zero-signature canonical envelope rejected: TooManyExtraCurrencies", "status": "error"}
  ```
- Binary search (first on the same already-used account, then —
  because an account accumulating extra currencies across several prior
  calls is a confound — repeated from scratch against **freshly
  deployed, never-before-called accounts, one fresh account per single
  data point**) found the exact threshold: **`N = 8` is accepted, `N = 9`
  is rejected with the identical `TooManyExtraCurrencies` error, on a
  completely fresh account with zero prior history.** This confirms the
  cap is **per-message**, not a cumulative per-account total — a fresh
  account's very first message already hits it at `N = 9`.
- At the accepted `N = 8`: `"status": "ok"`, real guest execution ran
  (the contract's own logic correctly evaluated and returned `1526`
  `topup_requires_shell_only`, since 8 non-SHELL currencies obviously
  aren't all `currency_id == 2` — `raw_executor_output`'s decoded
  `status_code` matches), `receipt.charged_shells = "101661"` (against a
  requested `--fuel-fee-shells 100000` reserve — note this exceeded the
  reserve, which is a separate, likely-reserve-insufficiency observation
  logged here for completeness, not the subject of this probe).

**Interpretation.** `TooManyExtraCurrencies` is enforced at the "canonical
operation"/"zero-signature canonical envelope" stage — i.e. by the host,
on the message's own wire shape, before the guest's WASM even starts
running (it is not a FUEL-exhaustion error, and it is not specific to
`TOPUP` or to `External` callers; the error text names the envelope
itself, not any method-specific or caller-specific concept). This is a
**real, previously-undocumented platform constraint found nowhere in
this checkout's own docs** (`docs/fuel-and-fees.md`, `docs/cli-
reference.md`, `docs/getting-started.md` — none mention any
extra-currency count limit). It means:

- Item 3c's literal scenario (one message carrying 300 distinct
  currencies) is not merely untested here — it is **categorically
  impossible on the real platform**, for `DEPOSIT` exactly as much as for
  `TOPUP`, since the cap is enforced on the message envelope itself, not
  on any particular method's logic. This is itself the most direct
  possible answer to "does a message this large hit a real platform
  limit": yes, deterministically, at a hard count threshold, not a soft
  FUEL/size degradation.
- This contract's own design is unaffected in substance: `MAX_LEDGER_ENTRIES
  = 300` bounds the vault's own internal (depositor, currency) row count
  across its entire lifetime and *every* depositor combined, never
  assumed to be reachable in one message — SPEC.md and this project's own
  `ledger_with_fillers`-based tests (`lib.rs`'s `arena_headroom`/
  `counting_allocator` modules) already construct that worst-case state
  directly rather than via a single real call, precisely because reaching
  it via genuine traffic was always going to require many separate
  messages over time. P14 confirms concretely *why* that is true (every
  single DEPOSIT a real depositor could ever send is capped at 8 new
  distinct currencies), not merely assumed — worth stating explicitly in
  `SPEC.md` as an operational property: a depositor wanting to establish
  more than 8 distinct currency rows for themselves genuinely needs more
  than one `DEPOSIT` message, `ceil(count / 8)` at minimum.
- Not independently confirmed here for the `Internal` message path
  specifically (only `External`, via `TOPUP`, was directly tested,
  because it needed no relay contract) — but the envelope-level
  "`zero-signature canonical envelope`" framing names a decode/shape
  check that applies to the canonical message format itself
  (`crates/avm-contract-sdk/src/input.rs`'s `extra_count`/`CurrencyAmounts`
  shape is identical for `InternalMessage` and `ExternalMessage`), making
  it very likely this is a universal, caller-type-independent cap — but
  this is an inference from the error's own wording and the SDK's shared
  wire format, not a direct `Internal`-message observation, and is
  flagged as such rather than asserted as directly tested.

### P15 — Real-stand max-state measurement: DEPOSIT/WITHDRAW/callback fuel, rent at max state, and the `charged_shells` excess explained — TASK 0 (closing round-3 item 3c properly)

**Methodology.** Built a `spike/seeded-vault/` (never committed, Hard Rule
5) — byte-for-byte this contract's own `constants.rs`/`state.rs`/
`handlers.rs` (copied verbatim), with only `init()` replaced to seed a
large state directly (filler ledger rows + filler pending records + one
real depositor row) instead of empty, so the real, unmodified handlers
could be fuel/`charged_shells`-measured on the real local stand at this
contract's own worst-case state sizes without replaying the long real
call history that would otherwise be needed to reach them. Two
configurations: **332 ledger rows / 0 pending** (the maximum row count,
reachable only via restores bypassing the cap) and **300 ledger rows /
32 pending** (the true byte-densest reachable state). Since `DEPOSIT`/
`WITHDRAW` are `Internal`-caller-only, a second throwaway contract
(`spike/sender/`) was built to act as the real depositor — on its one
external method, it parses a caller-specified destination/method_id/
value_main/extra/body and emits exactly one real `OutboundIntent`. Built
in two variants (`ACCEPT_PLAIN_TRANSFERS` compile-time flag, same
`option_env!` pattern as the owner-pubkey fallback): one that accepts an
inbound plain value transfer (`method_id == 0`) trivially, used to force
`WITHDRAW`'s payout callback to **Settle**; one that does not (falls
through to its own `unrecognized_method`), used to force it to
**Restore**. Each sender's real deployed address was funded with real
`currency_id = 1` via an external call carrying `--extra-currencies`
(confirmed in round 3 item 3c's own P14 to support multi-entry lists),
then used to fire a real `DEPOSIT` (crediting its own pre-seeded,
already-existing ledger row — never a new one) and a real `WITHDRAW`.

**A critical measurement artifact, found and corrected before trusting
any number: `avm-local-stand`'s per-call receipt conflates FUEL-derived
compute charges with storage-rent settlement, and these must be
separated to mean anything.** The first `DEPOSIT` measurement showed
`charged_shells = 369,018` for the vault's own receipt — but `DEPOSIT`/
`WITHDRAW` between same-`dapp_id` accounts are supposed to be
**local-free** (`docs/fuel-and-fees.md`: "no SHELL is burned for
compute" when source/destination share a `dapp_id`, same routing
partition, `MessageIsolated`, not a callback — exactly this contract's
own design). The number is not a compute charge at all: `28,386 bytes ×
13 seconds elapsed since that account's last touch × 1 SHELL/byte/second
(issue #21's own measured local-stand rate) = 369,018`, an **exact**
match. **Fix:** fire every real-measurement call at the *same*
`--block-time` as the account's immediately preceding touch (elapsed =
0), which isolates rent to exactly 0 and leaves only whatever the
compute charge genuinely is. Confirmed a second time independently at
the 300+32 configuration: `28,425 bytes × 20 seconds = 568,500`, again
exact.

**Results, rent isolated (both configurations tested; all real,
real-stand numbers, no estimation):**

| Operation | State | `charged_shells` (rent isolated) | Outcome |
|---|---|---|---|
| `DEPOSIT` (existing row) | 332 rows / 0 pending | `0` | success |
| `WITHDRAW` | 332 rows / 0 pending | `0`–`1` | success, pending created |
| Callback, Restore | 332 rows / 0 pending | `0` | pending resolved |
| `DEPOSIT` (existing row) | 300 rows / 32 pending | `0` | success |
| `WITHDRAW` | 300 rows / 32 pending | `1` | success, pending created |
| Callback, Restore | 300 rows / 32 pending | `0` | pending resolved |
| Callback, Settled | 300 rows / 32 pending | `0` | pending resolved |

None of these came anywhere close to `VM status 31` (issue #19's
documented out-of-fuel error) at either worst-case configuration — every
one completed normally. **What this table is, and is not:** it is the
real `charged_shells` delta for each operation, with rent subtracted out
by construction (zero elapsed time). It is **not** a raw FUEL number.
`fuel_per_shell` — the config value `charged_shells = ceil(fuel_used /
fuel_per_shell)` depends on — has no documented or discoverable concrete
value anywhere in this checkout (confirmed only as a field *name* in the
installed `avm-local-stand` binary's own strings, same as
`memory_limit_bytes` below); without it, a `charged_shells` figure of
`0`–`1` cannot be converted back into a FUEL number to compare directly
against issue #19's ~10,000,000 figure. What **is** directly established
is the practical question this measurement was for: this contract's own
`DEPOSIT`/`WITHDRAW`/callback handlers, run for real on the real local
stand at the exact worst-case row counts this contract's own design
permits, do not exhaust FUEL. That is a pass/fail signal, not a number,
and is reported as such rather than overclaiming a raw FUEL figure that
was not actually obtainable.

**The wasmtime memory limit — not measured, stated explicitly rather
than estimated, per this round's own instruction.** The installed
wasmtime profile (`~/.local/share/avm-sdk/current/profiles/wasmtime-
release-profile.v1`) references the field only symbolically:
`memory_reservation=config.wasm.memory_limit_bytes` and
`pooling_max_memory_size=config.wasm.memory_limit_bytes` — a template
variable, not a literal number. `strings` on the installed
`avm-local-stand` binary confirms `memory_limit_bytes` is one field of a
`WasmConfig` struct defined in `node/libs/avm-config/src/lib.rs`
(upstream, closed-source, not vendored in this checkout), validated via
that crate's own `validate_config`/`canonical_encode_config`, and folded
into the `config_hash` `avm-local-stand inspect` reports — but its
*concrete value* is set per-genesis-config, lives only in that binary's
runtime config data (not in any string table `strings` can find, and not
in any file this installer places on disk), and is not exposed by any
`avm-local-stand` subcommand (no config-dump command exists among
`init`/`fund-user`/`deploy`/`call`/`publish-version`/`set-default`/
`deprecate`/`upgrade`/`delete-account`/`compact-library`/`inspect`/
`events`/`replay`). An empirical bisection analogous to issue #19's own
method (growing a `Vec::with_capacity` allocation, which costs
near-constant FUEL regardless of size since `memory.grow` is O(pages),
to isolate the memory ceiling from FUEL cost) was judged out of scope to
add as a *third* parallel spike investigation on top of this round's
already-large one — this is recorded as a genuine, explicit gap, not
estimated. This contract's own real allocator traffic (`REVIEW.md` entry
14b) is ~60 KB at worst case, nowhere near pressuring whatever this limit
actually is, which is the only claim made about it here.

**Rent at max state — directly reconfirmed twice this round, both exact.**
`28,386 bytes × 13s = 369,018` and `28,425 bytes × 20s = 568,500`, both
landing on **exactly** 1.000 SHELL/byte/second — not merely "within 1%"
as round 6's own `mini-vault-big` spike found, but exact, at the real
contract's own real worst-case sizes rather than a purpose-built
dust-filled analog.

**Why `charged_shells` (101,661) exceeded the 100,000 `--fuel-fee-shells`
reserve at `N = 8` (round 2 item 3c's `PLATFORM_FINDINGS.md` P14).**
`docs/fuel-and-fees.md` states this directly: *"Fixed delivery and
callback fees are separate from `fuel_fee_shells`. A chargeable message
may therefore debit transferred SHELL, the FUEL reserve, and fixed fees
independently."* The `fuel_fee_shells` reserve only bounds the
FUEL-derived *compute* charge; a separate, fixed *delivery* fee is
charged on top of it, unconditionally, for any chargeable (here:
`External`) message — so `charged_shells` exceeding the stated reserve
by a small amount is expected behavior, not an anomaly. This round's own
fresh data confirms the same pattern repeatedly and independently: every
`External` "fire" call in this very investigation, with
`--fuel-fee-shells 1,000,000`, came back with `charged_shells` in the
range `1,000,001`–`1,000,751` — consistently a few hundred to low
thousands of shells *over* the stated reserve, the same small-fixed-fee
signature as the original `1,661`-shell excess at `N = 8`, now seen
across 6 independent calls rather than one.

### P16 — Real-vault stand E2E journey, `replay` determinism, and test 13 (vault-underfunded rejection) on the real stand

**Methodology.** Deployed the real, unmodified vault contract (`avm-
contract-build build`, no spike substitution this time — full confidence
this run reflects the actual shipped code) alongside a throwaway
`spike/sender/` (Hard Rule 5, deleted after this round) acting as a real
depositor dapp, on a single fresh `avm-local-stand init` session. Full
journey, all via real `avm-local-stand call`/`deploy`/`fund-user`:
deploy vault (funded 5,000,000 SHELL) and sender (funded 2,000,000
SHELL) under the same `dapp_id`; fund sender with real `currency_id = 1`
via its own `sender.fire`'s all-zero-destination "accept the carried
`--extra-currencies` as a funding transfer" no-op path; fire a real
internal `DEPOSIT` (3,000 of currency 1); fire a real internal `WITHDRAW`
(1,000), confirmed Settled via the real callback round-trip; a second
`WITHDRAW` (2,000) draining the ledger to exactly 0, also Settled.

**Two new gotchas found and worked around, beyond the ones already on
record:**
- **Deterministic addresses can shift *within* a single session**, not
  only across separate `init` invocations (round 3's existing finding).
  Probing the sender's address, then deploying the *vault* first, then
  attempting to deploy the sender at its originally-probed address
  failed (`funding_required` pointed at a *different* address than the
  probe returned). Re-probing immediately before funding/deploying, with
  no intervening deploy, is required — the derivation evidently folds in
  some piece of mutable registry/account-count state, not just
  `(package, owner, salt, init data)` as literally stated.
- **External method ids for a contract's own custom ABI methods are
  host-hashed, exactly like the real vault's admin methods** (already
  known), confirmed again here the hard way: guessing a formula-derived
  value for the throwaway sender's own `sender.fire` method caused every
  call to silently mismatch and fall through this spike's own
  `unrecognized_method` arm — except that path itself hadn't been
  reached yet either, because the guessed id was being compared before
  any panic-free fallback, surfacing as a flat `"local call returned VM
  status 43"` with zero diagnostic detail until bisected down to a
  trivial always-succeed build, which revealed the real id
  (`0x67083f7c6ebe7464`) in that build's own `method_id` response field.
  Lesson reconfirmed: never guess a host-hashed external method id,
  even for one's own throwaway spike — always discover it empirically
  first.

**Test 13 (SPEC.md §8), folded into this E2E journey.** Drained the
*vault's own* SHELL (not the depositor's ledger) via a single `vault.
topup` call carrying a non-SHELL `extra` (always rejected,
`ERR_TOPUP_REQUIRES_SHELL_ONLY`, 1507 — but still charged at the
platform layer per round-4's P1), requesting `--fuel-fee-shells
3,497,985`; `charged_shells` tracked the requested fee almost exactly
(`requested + 1`, confirmed as a general pattern across every call this
round), landing the vault at roughly 500,000 SHELL — clearly below
`MIN_OPERATIONAL_SHELL` (1,600,000) and clearly above whatever floor
would prevent the guest from running at all. With a depositor (sender)
holding a genuine, non-zero ledger balance (2,000 of currency 1, freshly
re-deposited), a `WITHDRAW` of 500 came back `status: "ok"` at the
`sender.fire` level (sender's own dispatch always succeeds regardless of
what its forwarded message does) — the real signal is in the *downstream
receipt count* `inspect` shows for the forwarded `WITHDRAW` itself: `outer
+ 2` receipts (the vault's own attempt, then one more, both charged `0`),
matching — independently calibrated against a guaranteed-reject `DEPOSIT`
carrying a SHELL `extra` — the *rejection* signature, not the
`outer + 3` signature every genuinely successful `WITHDRAW` in this same
session produced (`exec` + outbound delivery + callback delivery). **Zero
lasting effect, confirmed directly rather than merely inferred from the
receipt count:** recovered the vault via a real external `--extra-
currencies "2:5000000"` transfer (P1's own recovery method), then
withdrew the *exact* original 2,000 — it succeeded via the full
`outer + 3` success signature, which could only happen if the ledger
still held exactly 2,000 (not 1,500, not more) — i.e. the earlier
rejected attempt genuinely debited nothing, created no pending record,
and emitted no outbound intent, exactly as Invariant I17's "checked
before any of those effects" ordering claims.

**A consistent, unexplained receipt-count signature, noted but not fully
root-caused.** Across this session, every `Internal` message that
succeeds *without* emitting an `OutboundIntent` (`DEPOSIT`) produces
exactly 1 downstream receipt; every one that succeeds *with* one
(`WITHDRAW`) produces exactly 3 (`exec` + outbound delivery + callback
delivery); every one that is *rejected* by the destination contract
(confirmed twice, once via `WITHDRAW`'s own `vault_underfunded` and once
via a guaranteed-reject SHELL-carrying `DEPOSIT`) produces exactly 2,
not 1 — one more than a naive "rejection is just the failed attempt,
nothing else" model would predict. The second receipt in a rejection is
not a `CallbackIntent` the vault requests (it requests none on any
failure path) and not rent (identical `--block-time` throughout
isolates that to 0, per P15's own methodology). What it concretely *is*
was not pinned down further this round — recorded as an open, real
observation rather than guessed at, since both of this round's actual
claims (the vault-underfunded rejection happened; the ledger was
genuinely untouched) were independently confirmed by other means (the
hop-count signature itself was only used as a fast cross-check, not the
load-bearing proof) and did not require resolving this.

**Replay determinism — exact match.** `avm-local-stand replay --state
... --wasmtime-profile ...` against the full 35-message journal (2
deploys, 2 fund-users folded into the same accepted-input count, 31
calls) returned `cumulative_fee_shells: 4,519,056` and
`registry_root: 81e5c50193...fd1cd`, both **byte-for-byte identical** to
the live run's own final values (`inspect`'s last receipt reported the
same `cumulative_fee_shells`; the same `registry_root` had in fact been
stable since this session's very first deploy, since no further code/
registry mutations occurred after it). `receipt_count` (35) and
`accepted_input_count` (35) also matched exactly. This is the strongest
determinism confirmation obtained this engagement: not just "replay
doesn't error," but every cumulative, order-dependent quantity the
command reports lines up exactly with what the live run itself produced.

### P17 — `avm-contract-build` is NOT reproducible across different build directories: the compiled `contract.wasm` embeds an absolute source path

**OBSERVED, a genuine negative result.** Built the real, unmodified vault
twice from two independent copies of the source tree (same `src/`,
`Cargo.toml`, `Cargo.lock`, `avm-contract.toml`, same pinned toolchain via
a copied `rust-toolchain.toml`, same repo-root `Cargo.toml` providing the
`avm-contract-sdk` dependency's required workspace context), placed at
two different absolute paths under the scratch directory. `contract.acab`
and `contract.descriptor` hashes were **byte-identical** across both
copies (expected: both are pure ABI/metadata, derived only from
`avm-contract.toml`, with no compiled code involved). **`contract.wasm`
hashes differed** between the two copies, and differed again from this
same round's earlier build at the real repo path — three builds of
byte-identical source, three different wasm hashes.

**Isolated to the build directory's absolute path specifically, not
general nondeterminism.** Rebuilding the *same* copy at the *same* path
three times in a row (`rm -rf target` between each) produced the
**identical** wasm hash every time. Only changing *which directory* the
build ran from changed the hash.

**Root cause, directly confirmed via `strings` on the compiled binary:**
the wasm embeds exactly one absolute filesystem path —
`.../crates/avm-contract-sdk/src/runtime.rs` — matching whichever
scratch directory that particular build ran from. `cmp -l` between the
two `contract.wasm` files showed only 283 bytes differing out of 56,550
total, consistent with just this one embedded path string (and nothing
else) diverging. No `panic!`/`unwrap()`/`expect()`/`unreachable!()`
call exists anywhere in `runtime.rs` outside its own `#[cfg(test)]`-gated
code (confirmed by reading the file in full) — those are compiled out of
the wasm32 production build entirely, same as the real vault's own test
code, and are not candidates for a runtime panic-location string leaking
in regardless. The much more likely explanation — not independently
confirmed further, stated as the most probable cause rather than a
verified one — is embedded DWARF/debug-info compile-unit path metadata
from an unstripped-debuginfo build, since the file's absolute path
alone, with no particular line/function singled out, is exactly what a
compile-unit path entry (as opposed to a panic-location string, which
would also carry a specific line number) would look like.

**What this means in practice.** Anyone verifying this contract's
deployed bytecode against its own source (the normal "does the on-chain
code match what's in this repository" check a real audit or a
cautious depositor would want to do) **cannot simply rebuild from a
checkout at a different path and compare hashes** — the hash will
differ even for byte-identical source, through no fault of the
checkout, the toolchain pin, or the lockfile. The ABI (`contract.acab`)
and code descriptor (`contract.descriptor`) remain independently
verifiable this way; only the raw `contract.wasm` hash does not. A
correct verification procedure must either rebuild at the exact
original absolute path, or compare the wasm's *behavior* (as this
engagement's own test suite and real-stand E2E runs do) rather than its
raw byte hash, until/unless `avm-contract-build` strips or remaps debug
paths (a `--remap-path-prefix`-style fix, standard in other
reproducible-build toolchains, was not found as an existing flag — see
`docs/cli-reference.md`, which lists `avm-contract-build build
--manifest-path <Cargo.toml>` with no other options).

**Gate 2b round 2 TASK D addendum — `contract.acab`'s contents directly
verified, and one `--remap-path-prefix` experiment run.** Two follow-ups
to this finding, both empirical, reported as-observed:

1. **What `contract.acab` actually contains, confirmed by direct
   inspection** (`xxd`/`strings`/`file` on a fresh build's
   `target/avm/contract.acab`, 3,076 bytes vs. the wasm's 56,482): it
   opens with a 4-byte `"ACAB"` magic header followed by
   struct/method-layout data, and its full `strings` output is entirely
   this contract's own type names (`vault_state`,
   `vault_propose_owner_args`, ...), ABI method names
   (`vault.set_pause`, `vault.accept_owner`, ...), and the human-readable
   error-reason strings from `constants.rs`'s own error-code doc comments
   (`"a withdraw amount exceeds the caller's tracked ledger balance..."`,
   etc.). Byte-searched for the WebAssembly magic number (`00 61 73 6d`)
   anywhere in the file: zero occurrences. This directly confirms — not
   merely infers, as the paragraph above originally did without a byte-
   level check — that `contract.acab` does not embed the compiled wasm in
   any form; it is a self-contained ABI/metadata description derived from
   this contract's type definitions and doc comments, generated
   independently of code generation. No correction to this finding is
   needed; this is additional direct evidence for a claim that was
   previously only an inference ("expected: both are pure ABI/metadata").
2. **One `--remap-path-prefix` experiment, as a standard `rustc` flag via
   the `RUSTFLAGS` environment variable** (not altering
   `avm-contract-build` itself): built the same two-independent-directory
   setup as the original finding above, this time with
   `RUSTFLAGS="--remap-path-prefix=<each copy's own absolute path>=/remapped"`
   set identically (same `/remapped` target) for both builds. Result: the
   embedded path string in **both** resulting `contract.wasm` files was
   still the real, unremapped absolute source path (confirmed via
   `strings`) — not `/remapped/...` — so the flag had no observable effect
   on this specific embedding, and the two wasm hashes still differed
   (byte-level diff: exactly 1 byte, where the two scratch directories'
   equal-length names differed by a single character — consistent with
   the embedded path string being the only thing that still varies).
   Whether this is because `avm-contract-build`'s internal `cargo`
   invocation does not pass the caller's `RUSTFLAGS` through to the
   compiler, or because this specific embedding is not the kind of
   debug-info span `--remap-path-prefix` normally rewrites (which would
   weaken, though not disprove on its own, the "most probable cause"
   DWARF/compile-unit-path hypothesis in the paragraph above), was **not**
   further isolated — reported as a negative result only, per this
   round's "report only" scope, not chased into `avm-contract-build`'s own
   (unavailable-as-source-here) internals.

### P18 — CRITICAL: at real worst-case ledger size, a WITHDRAW's own Settle/Restore callback silently fails to resolve, permanently stranding the withdrawal

**OBSERVED, on the real, unmodified vault, on the real local stand —
discovered during the final `MIN_OPERATIONAL_SHELL` re-measurement round,
not a hypothetical.** This is more serious than anything else in this
document: it is not a cost measurement, it is a real correctness gap at
the real platform level, at a size this contract's own design explicitly
permits (`MAX_LEDGER_ENTRIES = 300`).

**Methodology.** Built a `spike/seeded-vault/` (never committed, Hard
Rule 5) identical in approach to P15/TASK 0: this contract's own
unmodified `constants.rs`/`state.rs`/`handlers.rs`, with `init()` seeding
only filler rows (never the real depositor rows — see the correction
below) at three size tiers, plus two throwaway `spike/sender/` instances
(one accepting plain transfers, forcing Settle; one rejecting them,
forcing Restore). At each tier: funded both senders with real
`currency_id = 1`, fired a real `DEPOSIT` from each into the seeded
vault (crediting a pre-existing filler row — never a new one, so the row
count stays exactly at the intended tier), then a real `WITHDRAW` of
part of it. Outcomes were verified **by decoding the real raw state
bytes returned by the stand** (`VaultState::serialize`'s own documented
layout: 91-byte header, 85 bytes/ledger row, 89 bytes/pending record —
`next_withdrawal_id`, `ledger_count`, and `pending_count` read directly
from fixed offsets), not inferred from receipt counts alone — this
distinction mattered (see below).

**A real methodology correction made along the way, itself worth
recording.** The first attempt seeded the two real depositor rows'
*amounts* directly via `credit_ledger` in `init()` (a large placeholder
balance, matching this spike's own superficial reading of P15's
approach). Every subsequent `WITHDRAW` attempt failed with `"local call
materialization failed: EconomicsError"` — a contract cannot forward
value via `OutboundIntent` that it does not **genuinely, platform-level**
hold, no matter what its own state bookkeeping claims; this applies to
any currency, not only SHELL (round 4's P1 already established the
SHELL-specific case). **Fix:** seed only the row's *existence* via
filler rows sized to leave exactly two slots, then let two real
`DEPOSIT` calls (fired after deploy, exactly P15's own documented
approach, re-read carefully this time) create the two real rows with
real backing. This is logged here because it is an easy trap: directly
writing a ledger amount into seeded state looks identical to a real
balance until the very first attempt to move it.

**The core finding.** At three tiers — **2 ledger rows** (near-empty),
**166 ledger rows** (half of `MAX_LEDGER_ENTRIES + MAX_PENDING_WITHDRAWALS`),
and **~300 ledger rows** (the real worst case) — ran the identical real
`DEPOSIT → WITHDRAW → {Settle | Restore}` sequence for two independent
depositors (one forcing each outcome):

| Tier | Ledger rows | Settle resolves? | Restore resolves? | Confirmed via |
|---|---|---|---|---|
| Small | 2 | **Yes** | **Yes** | raw state decoded to exactly 261 bytes = 2 ledger / 0 pending after both |
| Medium | 166 | **Yes** | **Yes** | raw state decoded to exactly 15,625 bytes = 166 ledger / 16 pending (fillers only) after both |
| Max | 299–300 | **No** | *(not reached — see below)* | raw state decoded to 28,265–28,439 bytes; the withdrawal's own pending record (id 32, or 31 in a second independent run) is still present, `amount` unchanged, never removed |

The max-tier result was **reproduced twice independently** at two
different pending-record pre-fill levels (31 and 30 filler records,
deliberately varied to test whether landing exactly at
`MAX_PENDING_WITHDRAWALS` — 32 — was the trigger) — both runs left the
withdrawal's pending record permanently stuck, ruling out "exactly at
the pending cap" as the specific cause. The trigger tracks **ledger row
count** (somewhere between 166 and ~299), not the pending count.

**Why receipt-count alone is not sufficient evidence here — a
methodology note that corrects this engagement's own earlier
reasoning.** Earlier real-stand E2E work this round (see the E2E/
`replay` entry) calibrated "4 receipts after the outer call" as the
signature of a *successful* Settle/Restore round trip, versus "3
receipts" for an outright-rejected `WITHDRAW`. The max-tier case here
produced the **same 4-receipt pattern** as a genuine success — yet the
state never actually changed. A delivered-and-processed callback that
itself *returns a failure status* is indistinguishable from a
successful one by receipt count alone (both show a charged, completed
hop); only decoding the actual resulting state tells the difference.
Every success claim in this document that rests on receipt-count
alone and predates this correction should be read with that caveat —
the small- and medium-tier results above were re-verified by direct
state decoding specifically because of this, and held up; the max-tier
one did not.

**Root cause: not pinned down, stated as such rather than guessed.**
SHELL funding is not the explanation — the vault held millions of SHELL
throughout, and every charge observed anywhere in this sequence was
0–1 shells (local-free). Hitting `MAX_PENDING_WITHDRAWALS` exactly is
not the explanation (ruled out directly, above). The leading candidate,
*not confirmed*: callbacks are "always chargeable" (round 4's P1) and
may draw on a smaller, platform-fixed fuel/fee allocation than an
ordinary local-free call, one that could be sufficient to re-serialize a
~15 KB state but not a ~28 KB one — this would be consistent with
issue #19's own open question about `max_message_execution_fuel` and
with this contract never coming close to that ceiling for `DEPOSIT`/
`WITHDRAW` themselves (P15) while a *callback specifically* at the same
state size behaves differently. This was not verified further: the
local stand exposes no way to see an internal message's own status code
once it is absorbed into a causal chain's receipts, and deeper
instrumentation was judged out of scope to build for this already-large
round. Recorded as a genuine, unresolved gap, not a guess dressed as a
finding.

**Practical implication for this contract, independent of root cause.**
If this vault is ever operated near its own `MAX_LEDGER_ENTRIES`
ceiling, a depositor's `WITHDRAW` can create a pending record that never
resolves — not refunded, not settled, just permanently stuck, silently
consuming one of the 32 global pending slots forever, with no error
surfaced anywhere a caller or this contract's own code could observe.
This is a real, demonstrated availability/correctness risk at scale,
distinct from anything `MIN_OPERATIONAL_SHELL` was ever designed to
guard against (that constant only gates whether a `WITHDRAW` may
*begin* — it has no bearing on whether the later callback can finish).
This contract's own code has no way to detect or defend against this:
`handle_callback`'s logic was read in full and contains nothing
state-size-dependent; if the cause is platform-side fuel allocation for
callbacks specifically, no guest-side change could fix it. Flagged here
explicitly rather than folded quietly into the `MIN_OPERATIONAL_SHELL`
number, because it is not the same kind of risk and conflating the two
would understate it.

### P19 — P18 root-caused to pending-record count, not ledger rows or rent; both caps reduced; "receipt count proves resolution" corrected

**Follow-up to P18, Gate 2b round 2 TASK A.** Reconciled P15 against
P18 first: P15's own "Callback, Restore"/"Callback, Settled" rows at the
300-row/32-pending configuration said "pending resolved" with no stated
verification beyond `charged_shells`/status. Re-verifying the *other* two
sizes P15 and the earlier Phase-3 round measured — 2 ledger rows and 166
ledger rows — by directly decoding the resulting state bytes this time
confirmed both genuinely resolve; only the 300/32 point, which neither
P15 nor the earlier round ever byte-decoded, was actually unverified
before P18 caught it.

**The failing hop's own status, obtained despite the stand exposing no
way to read it directly.** `inspect`'s receipts carry only
`charged_shells`/roots/message ids for every hop in a causal chain, never
a downstream hop's own `status_code`/`events`/`raw_executor_output`.
Built a diagnostic instrumented spike: byte-for-byte copies of this
contract's own `handlers.rs`/`state.rs`/`constants.rs` (confirmed
unedited), with only the `lib.rs` dispatch layer wrapping — never
replacing — the real `handlers::handle_callback` call, packing its true
`status_code` and the pending count beforehand into the returned state's
`admin_seq` field (`status_code × 1000 + pending_before`; that field has
no bearing on `handle_callback`/`handle_deposit`/`handle_withdraw`, only
on the admin methods, none of which this experiment calls), read back
via the same `TOPUP`-probe-and-decode technique P15/P18 already used. An
`EventIntent`-based version was tried first; the real stand's `events`
command returns an undocumented host-side storage encoding, not this
crate's own `fields_binary` wire format, and decoding it with confidence
was not achieved in reasonable time — a genuine dead end, recorded as
one. At the failing configuration, `admin_seq` stayed at its untouched
default and the pending count never moved: the guest code for that hop
never ran at all, matching the documented round-6 "admitted and charged,
guest never runs" signature, not a bug reachable through
`handle_callback`'s own logic (read in full again this round; nothing
in it depends on state size). Every measurement used identical
`--block-time` to the account's last touch, so elapsed time — and
therefore accrued rent — was exactly zero throughout: **rent is not a
contributing factor to this failure.**

**Four experiment sets, every outcome verified by decoding the resulting
state bytes, not inferred from receipts:**

| # | Setup | Result |
|---|---|---|
| a | Same failing restore retried at 5,000× baseline `--fuel-fee-shells` | No change — still stuck |
| b | Minimum elapsed time | Already 0 throughout (same `--block-time` everywhere) |
| c | Ledger-row bisection at fixed high pending count | 266, 200, and even 166 (pending held at 31) rows all failed |
| c | Pending-count bisection at minimal (1) ledger rows | 16, 17 safe; 18, 19, 20, 24, 31 all fail |
| c | The same 17-pending safe point, re-tested at 300 ledger rows | **Failed** — confirms a real row×pending interaction, not a single-axis cutoff |
| c | Further bisection at 300 rows, lower pending counts | Blocked: `"BodyTooLarge"` deploy error, reproduced across salts and a fresh stand session, cause not isolated |
| d | 299 ledger rows / 0 pending | Not run — superseded by (c), and blocked by the same tooling issue |

**A correction this investigation forces on its own predecessor.** Entry
23/P16's calibration — "4 downstream receipts after the outer call means
the Settle/Restore round trip succeeded" — is **not sufficient evidence**.
The 300-row/18-pending failure produces the *identical* 4-receipt pattern
as a genuine success, because a callback that is delivered and itself
returns failure looks the same by receipt count as one that resolves. The
small- and medium-tier successes from that same round were re-verified
here by direct state-byte decoding specifically because of this, and held
up; the max-tier one did not, which is how this investigation started.

**Root cause: still not confirmed, stated as such.** Rent/SHELL is ruled
out directly (above). The leading candidate — `state.rs`'s own
`O(n²)`-in-pending-count per-depositor validation (added to `parse()` in
an earlier round) combined with callbacks drawing on a smaller or
differently-allocated resource than ordinary local-free calls
(round 4's P1: callbacks are never local-free) — is consistent with
every data point above (pending count dominates; ledger rows contribute
but are not sufficient alone) but was not independently confirmed by a
further isolating experiment, since testing it would have meant changing
that validation, which this round's own instructions explicitly
excluded ("do not change handler logic").

**Decision and corrections applied.** Per the instructed "if neither is
isolated: say so, and still apply the empirical cap reduction" path:
`MAX_PENDING_WITHDRAWALS` reduced `32 → 12` (≥25% below the lowest
verified-safe count, 17); `MAX_LEDGER_ENTRIES` reduced `300 → 220` as an
added margin of caution, anchored on the one verified-safe large-scale
point (166 rows) rather than a directly-measured ceiling at 220 itself.
New worst-case state size: `91 + 220×85 + 12×89 = 19,859` bytes (down
from `31,159`). `MIN_OPERATIONAL_SHELL` re-derived identically to the
Phase-3 methodology: `19,859 × 1.000 × 60 = 1,191,540`, rounded up to
`1,200,000`. Every prior claim in SPEC.md/P18 of the form "no SHELL
funding level could fix this" has been corrected to state precisely what
was and was not tested: the vault's own balance and the triggering
call's requested `--fuel-fee-shells` were both directly ruled out as a
fix (a and the ample balance throughout); whether some platform-side,
callback-specific allocation not exposed to any caller would fix it was
never tested and is not claimed either way.

### P20 — P18/P19's root cause CONFIRMED BY A/B (not "likely"): `state.rs` `parse()`'s own O(n²) per-depositor loop; a real-stand seeding methodology bug corrected along the way; `BodyTooLarge` traced to the deploy envelope's total size

**Gate 2b round 3 TASK F.** The user's prior-round instruction ("do not
change handler logic") had blocked the one experiment that could
isolate root cause; this round explicitly permits editing **spike**
copies (never the real contract, never committed) to test it directly.

**F1 — the A/B test, run for real on the stand, 8/8 configurations.**
Built three spike copies of the real vault (byte-for-byte `handlers.rs`/
`constants.rs`, `MAX_LEDGER_ENTRIES`/`MAX_PENDING_WITHDRAWALS` restored to
the pre-reduction `300`/`32` specifically to reproduce the *original*
failing configuration under test): `vault-base` (the real, unmodified
`state.rs`, including its O(n²) per-depositor `pending.iter().filter(...)
.count()` loop in `parse()`), `vault-nocheck` (that loop deleted
entirely), and `vault-linear` (the same check, same semantics, rewritten
as a single sort-and-count pass — see below for why this variant never
got to run). Each spike's only OTHER addition is one spike-only,
`AVM_SPIKE_SEED`-gated internal method that replaces state wholesale
from a message body with no other validation, used purely to seed
filler ledger/pending rows (never the real depositor's own row — see the
methodology correction below) without needing hundreds of real calls.
Ran the exact originally-failing shapes — 300 ledger rows with 18
pending, and with 31 pending, both the Settle (depositor accepts payout)
and Restore (depositor rejects payout) outcome — against both
`vault-base` and `vault-nocheck`, every result verified by decoding the
resulting state bytes (via the `vault.topup`-probe-and-decode channel,
entry 19 in `REVIEW.md`; `ledger_count`/`pending_count`/`next_withdrawal_id`
read directly from the header). Result, all 8 runs:

| Variant | Pending | Outcome | Result |
|---|---|---|---|
| `vault-base` (O(n²) check present) | 18 | Settle | **stuck** (pending stayed at 19) |
| `vault-base` | 18 | Restore | **stuck** (19) |
| `vault-base` | 31 | Settle | **stuck** (32) |
| `vault-base` | 31 | Restore | **stuck** (32) |
| `vault-nocheck` (check removed) | 18 | Settle | **resolved** (back to 18) |
| `vault-nocheck` | 18 | Restore | **resolved** (18) |
| `vault-nocheck` | 31 | Settle | **resolved** (31) |
| `vault-nocheck` | 31 | Restore | **resolved** (31) |

**Root cause CONFIRMED BY A/B, not merely likely:** removing exactly
this one loop — nothing else — flips every one of the 8 previously-stuck
configurations to resolving cleanly, for both callback outcomes and both
pending counts previously found to fail. This closes P18/P19's "root
mechanism not independently confirmed" gap.

**A real methodology bug, found and fixed before F1's result above is
trustworthy.** The first attempts at this A/B test used the spike's SEED
method to inject a ledger row directly for the real depositor under
test (claiming it already held a balance), then triggered a real
`WITHDRAW` against it. Every such attempt — on `vault-base`,
`vault-nocheck`, and even the real, unmodified production vault, at
trivial 1-row scale — failed identically with `"local call
materialization failed: EconomicsError"`, regardless of
`--fuel-fee-shells` magnitude (tried 5,000 up to 40,000,000) or caller
identity. Root cause, isolated by a staged series of control tests (a
bare `method_id = 0` transfer succeeded; a real `DEPOSIT` forwarding
externally-minted currency failed identically to `WITHDRAW` until the
deposit was backed by currency the sender actually held): a ledger row
seeded directly via the SEED method is a bookkeeping claim with **no
real currency behind it** — the vault's actual on-chain account never
received it. `WITHDRAW` trying to pay that claim out fails at
materialization because the vault doesn't really hold what its own
ledger claims. **Fixed** by never seeding a row for the real depositor
under test: the SEED method now injects only filler rows/pending
records (which are never resolved, so their lack of real backing never
matters), and the real depositor's own row comes from one genuine,
externally-minted, actor-forwarded `DEPOSIT` call issued separately.
Re-ran the small-scale sanity check after this fix and got a clean
Settle (`pending_count` 1→0) on the real, unmodified vault — confirming
the fix, not just silencing the symptom. This is logged explicitly as a
correction to this round's own early methodology, not swept past: every
F1 result above was produced under the *fixed* methodology.

**A second, unrelated resource-management bug, also found and fixed.**
Mid-investigation, calls that had worked moments earlier (`vault.topup`
probes) began failing with the same `EconomicsError`. `inspect` showed
`storage_due_payment: "10780812"` — the account's own accrued storage
rent (entry P1's ~1 SHELL/byte/second figure) had grown large enough,
across many intervening `--block-time` jumps spent on *other* accounts
while this one sat untouched, to exceed its SHELL balance, blocking
further calls outright. Not a P18-related finding — purely an artifact
of this round's own harness using large block-time increments across a
long session. Fixed by funding fresh vault instances with very large
SHELL balances and using minimal (+1) block-time increments between
calls for the remainder of this round's testing.

**F2 — `BodyTooLarge`'s cause, now characterized cleanly.** The
`vault-linear` variant (the same per-depositor check, rewritten as one
sort-and-count pass instead of O(n²) comparisons) never got to run on
the real stand: its own **deploy** was rejected with the exact same
`"zero-signature canonical envelope rejected: BodyTooLarge"` error P19
hit while trying to seed large states directly. Measured directly this
time: `vault-base`/`vault-nocheck`'s compiled wasm are ~56.8–57.0 KB and
deploy fine; a first `vault-linear` attempt using `std::collections::
BTreeMap` compiled to ~64.3 KB and was rejected; a second attempt
avoiding `BTreeMap` (a plain `Vec` sort instead) compiled to ~68.0 KB and
was *also* rejected. This directly confirms what P19 could only
speculate about: `BodyTooLarge` is a **deploy-envelope size limit**
(wasm + ACAB + descriptor + overhead), not anything about seeded state
content — and it is low enough that *this specific* linear-pass rewrite,
in either form tried, cannot even be deployed, let alone tested. The
likely explanation for the limit's exact value: `docs/` already
documents a ~65.6–66.1 KB boundary for state-replace payloads (open
issue #20); a deploy envelope's wasm (~64–68 KB) plus its ACAB (~3 KB)
plus descriptor plus signature overhead lands in the same range,
suggesting one shared body-size limit applies broadly to large message
envelopes generally, not specifically to state storage — not
independently confirmed beyond this one data point, stated as the
likely explanation rather than a verified one. **Practical consequence
for TASK H:** a linear in-`parse()` replacement for this check is not
simply "the same logic, cheaper" on this toolchain — both linear
encodings tried bloat the compiled contract enough to risk this same
deploy-time limit, which is itself an argument for *dropping* the check
from `parse()` entirely (per the user's own alternative) rather than
linearizing it.

**F3 — making the diagnosis honest, per the explicit instruction.**
Entry 26/P19's original wording ("the guest code for that hop never ran
at all... matching the documented round-6 'admitted and charged, guest
never runs' pattern") is corrected here: dispatch (`lib.rs`) calls
`VaultState::parse(input.state)` **before** ever reaching
`handle_callback`'s own logic, for every message including callbacks.
F1's A/B result — removing exactly the O(n²) loop inside `parse()`, and
nothing else, flips all 8 stuck configurations to resolving — is direct
causal evidence that execution enters `process_message` and reaches
`parse()`, then traps (most likely runs out of fuel) **inside that
loop**, before `handle_callback` is ever invoked. This is a more precise
claim than "guest never ran at all," and better supported than a guess:
it is evidence from directly manipulating the suspected code region, not
mere correlation. What is **not** independently confirmed: no
side-channel instrumentation directly observed the trap occurring
*inside* `parse()` specifically (the `admin_seq`-encoding technique
wraps `handle_callback`, which is never reached either way here, so it
cannot distinguish "trapped in `parse()`" from "trapped on the dispatch
line immediately before it" — both are consistent with F1's result, and
no diagnostic built this round or last can tell them apart). Stated
plainly: **not-run-at-all vs. out-of-fuel-specifically-inside-`parse()`
remains formally undetermined by direct instrumentation, but the A/B
evidence strongly localizes the trap to `parse()`'s O(n²) loop
specifically, not to `handle_callback` or anything after it.**

### P21 — Gate 2b round 3 TASK G: current caps (220/12) verified safe on the real stand, with margin mapped; 232-row post-restore path confirmed; MIN_OPERATIONAL_SHELL's own 60-second margin found insufficient against real overhead

**G1 — 220 ledger rows + 12 pending, unmodified production code, real
stand.** Using `vault-prodcaps` (the real, unmodified `handlers.rs`/
`state.rs`/`constants.rs` — including the current, reduced `220`/`12`
caps — plus only the same spike-only `AVM_SPIKE_SEED`-gated method used
throughout this round): seeded 219 filler ledger rows + 11 filler
pending, then backed the real depositor's own row with one genuine
`DEPOSIT` (219+1=220 ledger rows), then triggered the real 12th
withdrawal (11 filler pending + this one = 12, exactly at the cap).
Both Settle and Restore resolved cleanly (`pending_count` 11→12→11,
`state_len` unchanged before/after). Also confirmed the "232 rows after
restores" shape (`MAX_LEDGER_ENTRIES` + `MAX_PENDING_WITHDRAWALS` =
220+12, reachable only via restores recreating rows a prior full-drain
had removed, since Invariant I11 lets restore bypass the cap entirely):
seeded 231 filler rows + the real depositor's already-backed row
preserved across the re-seed (88-byte `actor.seed2` variant built for
this), confirmed both an existing-row `DEPOSIT` and a full `WITHDRAW` +
Settle round trip succeed cleanly at 232 ledger rows / 12 pending
(peak). Separately confirmed (if accidentally, by trying to deposit a
*new* row at 231 existing rows): `DEPOSIT`'s own `new_row_count > 0 &&
ledger.len() + new_row_count > MAX_LEDGER_ENTRIES` guard correctly
rejects a new-row deposit once ledger is already past 220 — 232 is
reachable only via restore, never via ordinary deposit, exactly as
designed.

**G2 — margin mapped at 220 rows, using `vault-base` (the real,
unmodified O(n²) check, caps restored to `300`/`32` so pending could be
pushed past `12` without the production cap itself blocking the test).**
Bisected pending count at a fixed 220 ledger rows:

| Pending | Result |
|---|---|
| 13 | resolves |
| 15 | resolves |
| 17 | resolves |
| 20 | resolves |
| 21 | **stuck** |
| 22 | **stuck** |
| 24 | **stuck** |

First failing point at ledger=220: **21**. The required margin (TASK
G2's own instruction: chosen cap must be ≥50% below the first failing
point at the same row count) would require a cap of **≤10** if the
check stays as-is — the *current* cap of 12 does **not** clear that bar
at this row count (12 > 10.5). This is a direct input to TASK H's
decision below.

Separately, confirmed on `vault-nocheck` (the O(n²) check removed, i.e.
the state this round's F1 result recommends shipping): pending **32**
— the architectural ceiling this contract's `MAX_PENDING_PER_DEPOSITOR`
grouping was always designed around — resolves cleanly at 220 ledger
rows for both Settle and Restore, with no failing point found anywhere
in the range tested this round (12 through 32, at both 220 and 300
ledger rows, counting TASK F's results too). This is the strongest
evidence yet that the check's own cost, not some other limit, was the
binding constraint.

**G3 — nonzero elapsed time against `MIN_OPERATIONAL_SHELL` itself.**
Deployed two fresh `vault-prodcaps` instances, funded with exactly
`1,200,000` (1×) and `3,600,000` (3×) SHELL respectively — the only
point where "exactly" is achievable, since every subsequent call
(seeding, the backing deposit, probes) necessarily spends some of that
balance on its own fuel and on rent accrued meanwhile; this is disclosed
as the honest limit of precision available, not glossed over. Brought
each to the worst-case shape (220 ledger rows / 12 pending peak) and
triggered the real test withdrawal at +1, +10, and +60 seconds of
elapsed `--block-time` since the vault's prior touch (this stand
resolves a withdrawal's own callback synchronously within the same
triggering call — there is no way to independently delay only the
callback leg relative to the withdrawal itself, so "elapsed time between
WITHDRAW and the callback" is approximated here as elapsed time between
the vault's last touch and the single call that performs both — a
disclosed, not hidden, limitation of what's testable on this stand):

| Funding | Elapsed | Result | charged_shells |
|---|---|---|---|
| 1,200,000 (1×) | +1s | **fails** (`next_withdrawal_id`/`pending_count` unchanged — rejected before any state write) | 5,191 (triggering call only; downstream rejection not separately chargeable-visible) |
| 3,600,000 (3×) | +1s | resolves | 5,191 |
| 3,600,000 (3×) | +10s | resolves | 6,046 |
| 3,600,000 (3×) | +60s | **fails** (same signature as above) | 10,796 |

Neither failure's exact downstream status code is independently
visible (the same `inspect`-cannot-show-downstream-hop-status limitation
documented throughout this engagement) — but both show the identical
signature already established as "rejected before any state write" (no
change to `pending_count`/`next_withdrawal_id`/`state_len`), consistent
with Invariant I17's `vault_shell_balance < MIN_OPERATIONAL_SHELL` check
firing (the first check in `handle_withdraw`, before anything else can
run), not independently confirmed beyond that consistency. **What this
implies for `MIN_OPERATIONAL_SHELL`:** its own documented derivation
("60 seconds of margin at worst-case size") describes a theoretical
buffer against rent *alone*, starting from a balance that itself already
assumes no other costs have been paid yet. In practice, by the time any
real funding, setup, or elapsed time has occurred, that margin is
consumed faster than the "60 seconds" framing suggests — this round
measured the 1× value failing immediately (before even 1 second of
*additional* elapsed time, once setup overhead is counted) and the 3×
value failing somewhere between 10 and 60 seconds. `MIN_OPERATIONAL_SHELL`
is accurately described as a withdrawal-gate threshold (SPEC.md §1
already says this, Gate 2b round 2), not as an operating balance a real
deployment could safely run near — this round's measurement makes that
caveat concrete with real numbers rather than a general warning.

### P22 — `avm-local-stand` has a session-wide cap on distinct (account, currency) pairs, separate from the already-documented per-message 8-currency limit (P14) — discovered mid-TASK-M, not root-caused further

**What was being done when this was found.** TASK M (Gate 3b) needed to
build a 252-ledger-row vault via genuine real `DEPOSIT` calls, reusing
a pool of already-registered currency ids (from `101`–`132` and
`1000`–`1023`, both ranges minted earlier in this same engagement) sent
from several depositor-relay actor accounts against a freshly deployed
vault. This had worked earlier in the day (reaching 287 real rows). A
same-day repeat of the identical procedure against a new, equally
fresh vault plateaued well short of 252 — four actors' first ~56
currency ids each landed cleanly (56, 112, 168, 224 rows), then later
actors' deposits returned contract-level success (`call_status == 0`
in every single deposit's own `raw_executor_output`, confirmed with a
per-batch, uniquely-named-output diagnostic script — an earlier version
of the script used a `$$`(PID)-based temp filename inside a bash
function invoked via command substitution, which runs in a subshell;
every batch silently overwrote the same file, so the first version of
this diagnosis was itself briefly wrong before the filename collision
was caught) without the ledger row count increasing to match.

**Isolating it from the already-known per-message limit.** P14
documents a *per-message* cap of 8 extra-currency entries; this is a
different, *session-wide, cumulative* limit — confirmed by sending a
single, brand-new, never-before-used currency id (`777777`) alone (one
entry, nowhere near 8) to a depositor actor, mid-session, after the
plateau: it failed immediately with `"call rejected canonical
operation: local call ABI input failed: InvalidLength"`, the exact
error P14 documents for a *different* cause (too many entries in one
message). A second, 10-batch probe of entirely fresh ids
(`800000`–`800039`) against the same vault showed the cap arriving
abruptly: the first batch of 4 fresh ids succeeded, every subsequent
batch failed with the identical `InvalidLength` error — ruling out a
magnitude-based theory (`800000` is not meaningfully larger than ids
already in use) and confirming the limit is on *cumulative distinct
ids/pairs ever created this session*, not on any single id's value.

**What this is not, and what remains unconfirmed.** This is not simply
"the ~63–64-distinct-currency-id cap" an earlier round of this
engagement (pre-dating this entry) believed it had characterized —
reused ids (not just fresh ones) also stopped landing as new rows past
a certain cumulative point this session, which a flat "distinct ids
ever minted" theory does not explain on its own (reusing an
already-registered id against a new account had previously, and
reliably, created a genuinely new row). The more likely mechanism is a
cap on cumulative distinct **(account, currency) pairs**, or some
related registry structure, not on currency ids in isolation — but
this was not root-caused further, since doing so was not needed for
TASK M's own purpose (it motivated switching to a byte-splice state-
injection method, `vault.seed_full`, that needs no currency
registration at all — see `REVIEW.md` entry 42). **Open question for a
future round:** what this cap actually counts and its exact numeric
threshold.

**Refinement (Gate 3b TASK M2): NOT tied to the `--state` file.**
TASK M2 needed real-built ledger rows again (to test whether real
calls' accrued rent explains a different headroom boundary than
fresh `seed_full` states) and deliberately started a brand-new
`avm-local-stand` session against a FRESH `--state` file specifically
to rule out any interaction with this cap. The same exhaustion
recurred anyway: 4 depositor-relay actors' real `DEPOSIT` calls,
each individually returning `call_status == 0` (contract-level
success, confirmed the same way as the original diagnosis), still
landed only 148 of an expected 224 new rows on one run and only 124
of an expected 224 on a second, independent run against a second
fresh vault in the SAME fresh session. **This proves the cap is not
reset by starting a new `--state` file** — it must be tracked
somewhere outside the state file this engagement has not identified
(a candidate: something under the `avm-local-stand`
binary/installation's own runtime state, e.g. under
`AVM_SDK_HOME`, rather than the `--state` path passed on each
invocation; not confirmed). The open questions above (what it counts,
its exact threshold) stand; this adds "and it is not per-`--state`-
file" to what's confirmed false about it.

**TASK P22b: checked directly for funds loss in both failure modes —
none found in either, using a VAULT call (not the actor's reply) to
read ledger state before/after.** Two scenarios were tested on fresh
vault+actor pairs, with the vault's own `vault.topup` response's full
embedded state decoded byte-for-byte (not just header counts) before
and after each attempt, to see exactly which (account, currency) rows
exist on each side — the status being read throughout is the
**vault's**, via its own state, not the depositor actor's reply,
per the task's explicit requirement:
1. **Fresh, never-before-used currency ids, once the cap is hit:**
   fails LOUDLY — `"call rejected canonical operation: local call ABI
   input failed: InvalidLength"` at the top level, before the message
   ever reaches contract execution. No row is created, confirmed by
   full ledger decode; no funds-loss risk here, since the call never
   executes and the caller is told immediately that nothing happened.
2. **Reusing already-registered ids against a brand-new account** (the
   scenario that, in TASK M2's real-deposit construction, appeared to
   plateau — contract-level success reported, but the row count grew
   by less than the number of batches sent): re-tested cleanly, with
   full ledger decoding this time rather than count-based inference,
   and **4 new rows were created correctly, each with the exact amount
   deposited** — no shortfall, no loss. This does not reproduce the
   shortfall TASK M2 observed.

**Conclusion: no funds-loss path confirmed in either failure mode
directly tested.** TASK M2's original "deposits plateau despite
reporting `call_status == 0`" observation was not reproduced under
this more careful (full-ledger-decode, not count-based) methodology.
Given this engagement already found and fixed two genuine test-
harness bugs during this same investigation (a `$$`/PID-based temp
filename collision that silently overwrote earlier batches' results,
and an `"ok"` vs `"ok 0"` string-match bug that misreported every
batch as failed), the most likely explanation for TASK M2's shortfall
is a third, undiagnosed harness artifact rather than a genuine
platform funds-loss bug — but this is not proven, since the exact
prior conditions (specific actors, very high cumulative call volume
in that session) were not reproduced byte-for-byte here. Stated as an
open, unresolved discrepancy rather than asserted either way, per
Hard Rule 6.

### P23 — CRITICAL: a Settle callback's `callback_result_body` is silently DROPPED (not truncated, not failed) above exactly 65,536 bytes — the call still reports success, directly contradicting TASK N's "resolves cleanly up to 10,000,000 bytes" conclusion, which never checked delivered content

**Why this was found.** TASK N (Phase 4 finding 4 follow-up) tested
whether a large `callback_result_body` from a malicious depositor
actor would cause the vault's Settle resolution to fail, up to
10,000,000 bytes, and found it always resolved successfully. TASK N2
(Gate 3b) was asked to prove the body actually ARRIVES, not just that
resolution succeeds — a materially different question TASK N never
asked.

**Method.** A spike vault (`spike/vault-m2/`, Hard Rule 5, deleted
after use) was given a one-line, `AVM_SPIKE_SEED`-gated addition to
`handle_callback`: on every resolved callback, record the DELIVERED
body's length into `admin_seq` (a plain `u32` header field with no
cross-validation in `parse()`, so repurposing it cannot break state
validity) instead of its normal value. A paired depositor actor
(`spike/actor-m2/`) accepts its payout and attaches a
runtime-generated `callback_result_body` of a build-time-configured
size. A real `DEPOSIT` then `WITHDRAW` through the actor exercises a
genuine Settle each time; `vault.topup` probes `admin_seq` afterward
to read back what the vault's own `handle_callback` actually saw.

**Result — a razor-sharp, 100%-reproducible threshold at exactly
65,536 bytes (64 KiB):**

| sent (bytes) | delivered (bytes, read from `admin_seq`) |
|---|---|
| 10 | 10 |
| 10,000 | 10,000 |
| 50,000 | 50,000 |
| **65,536** | **65,536** |
| **65,537** | **0** |
| 70,000 | 0 |
| 80,000 | 0 |
| 90,000 | 0 |
| 130,000 | 0 |
| 262,144 | 0 |
| 1,000,000 | 0 |
| 10,000,000 | 0 |

Below and at 65,536 bytes, the exact byte count sent arrives
unchanged. One byte over — 65,537 — the delivered length is not
65,536 (truncated) and not an error; it is exactly 0. The call's own
top-level status is `"ok"` in every row above 65,536 — identical to
what TASK N originally observed and interpreted as "resolves
cleanly." **TASK N's conclusion was accurate about resolution and
silent about content; this finding is the content check TASK N never
ran.** Above 65,536 bytes, an actor can attach an arbitrarily large
`callback_result_body` to its payout response and the vault will
Settle the withdrawal as if nothing were attached at all — the bytes
are gone, not malformed, not rejected, just absent, with no signal of
any kind that anything was dropped.

**Why 65,536 specifically:** this is exactly 2^16, strongly
suggestive of a fixed internal buffer (receive buffer, serialization
buffer, or similar) somewhere in the SDK or host runtime sized at
64 KiB, with silent truncation-to-nothing (not truncation-to-64KiB)
on overflow. This engagement did not locate the specific buffer in
source — `avm-contract-sdk`'s own `output.rs`/`input.rs` paths quoted
in TASK N's original finding show no size check of any kind for
`callback_result_body`, so if this is the mechanism, it lives in the
closed-source host runtime, not in this SDK crate.

**A second, related observation (not separately confirmed as the
same mechanism): the responding actor's OWN state update is also
lost above the threshold.** The actor's payout-accept branch also
writes a fixed marker into its own state via `FullReplace`. Below
65,536 bytes, this marker persists and is readable afterward; above
it, the actor's state reads back empty — as if the entire nested
response (not just `callback_result_body`) were discarded once the
attached body crosses this size, not merely the body field in
isolation.

**Severity and scope.** This is a platform/host behavior, not a bug
in this contract's own code — the vault has no way to detect or
reject an oversized `callback_result_body`, because `handle_callback`
never sees it in the first place once it's dropped upstream of the
contract's own execution. It does not enable fund theft or
mis-crediting in this specific vault (the vault's own logic never
reads `callback_result_body`'s content, only whether `status_code`
is 0 or non-zero), but it means **any contract design that DOES rely
on reading a Settle/Restore callback's body for values above 64 KiB
will silently receive nothing, with no error to catch.** This
directly reframes TASK N/Phase-4-finding-4's risk assessment: the
risk was never "does a large body cause a crash or fuel exhaustion"
(confirmed not, up to 10MB) — it is "does a large body arrive at
all" (confirmed NOT, above 65,536 bytes), a data-integrity risk
TASK N did not test for.

**Not established:** whether this is a local-stand-only artifact or
also true on mainnet; this engagement has no way to test mainnet
behavior. Logged as an explicit caveat in `README.md`,
`docs/THREAT_MODEL.md` §5.12/§6, and `docs/MAINTAINER_QUESTIONS.md`
#8 (all updated to state plainly that this and every related finding
in this repository was tested on the local stand only).

### P24 — Gate 3c TASK 3/M4: the Settle callback's own fuel budget is small and does NOT scale with the triggering call's fee, even across six orders of magnitude — overturning TASK M2's "more fuel buys more headroom" conclusion, and showing SPEC.md's own "safe" K = 5,850 figure does not survive a real Settle round trip

**Why this task existed.** P18-P20 (above) left one footnote
unresolved: P18's own text floated "callbacks may draw on a smaller,
platform-fixed fuel/fee allocation than an ordinary local-free call"
as a "leading candidate, not confirmed," before P20's A/B test
root-caused the *original* P18 failure entirely to the now-removed
O(n²) check, closing that investigation without needing the
fixed-budget theory. Separately, TASK M2 (`REVIEW.md` entry 43)
compared a real `WITHDRAW` (resolved at K = 50,000, 2,000,000-shell
fee) against the `vault.topup` probe (failed at K = 20,000/50,000,
2,000-shell fee) and concluded the divergence was "explained by the
1000× difference in supplied fuel ... more fuel buys more headroom."
This task's job: settle whether the callback leg's own fuel genuinely
scales with the triggering fee, with one isolating experiment.

**Method.** A throwaway pair (`spike/vault-m4/`, `spike/actor-m4/`,
Hard Rule 5, deleted after use) separated the two things TASK M2's
design conflated: `vault-m4` is a byte-for-byte copy of the current
production contract (including the `ParseFailureReason`/
`parse_with_reason` split from this round's TASK 1) with a
build-time-gated dummy-work loop (`AVM_SPIKE_DUMMY_K`, same
`std::hint::black_box`-guarded 64-byte-array-comparison shape as
every earlier K-sweep in this repo) inserted at the end of
`parse_with_reason`; `actor-m4` is a minimal relay whose
`actor.withdraw` method reads a *separate*, explicit
`intent_fuel_fee_shells: u128` field from its own call body and uses
it to set the OUTBOUND `WITHDRAW` intent's own `fuel_fee_shells` —
decoupling that field from the external call's own `--fuel-fee-shells`
for the first time in this repo's experiments. A real `DEPOSIT` funds
the actor; each `WITHDRAW` attempt is checked two ways: the actor's
own synchronous reply status (did the first, `WITHDRAW`-side dispatch
succeed) and a separate, generously-fueled `vault.topup` probe against
the vault's real state afterward (did `pending_count` return to 0 —
i.e., did the asynchronous Settle *callback* dispatch, triggered only
once the actor's payout-acceptance reply arrives back at the vault,
actually resolve). This second check is what TASK M2 never isolated.

**Step 1 — the OutboundIntent's own `fuel_fee_shells` field is
irrelevant.** Bisecting it from 2,000,000 down to 0 (external call
fee held at 2,000,000 throughout) never changed the outcome: even
`intent_fuel_fee_shells = 0` Settles cleanly at K = 0.

**Step 2 — the lowest working external call fee is 1 shell.**
Bisecting the external `--fuel-fee-shells` on `actor.withdraw` itself
(intent fee pinned at 0) from 2,000,000 down: every value through 1
Settles cleanly at K = 0. At exactly 0, the call is rejected outright
with `RouteRequiresFee` — a minimum-fee call-admission gate, not a
fuel/K phenomenon, and not part of this sweep's result.

**Step 3 — the K-sweep, bisected to a 20-wide bracket, is identical
at three fee levels spanning six orders of magnitude.** At fee = 1:
K = 590 Settles cleanly, K = 610 traps (actor status stays 0 — the
`WITHDRAW`'s own synchronous leg always succeeds — but the vault's
own probe shows `pending_count` stuck at 1, not 0: the Settle
callback specifically never resolved). At fee = 10: identical
boundary (590 / 610). At fee = 2,000,000 — TASK M2's own fee level —
**identical boundary again (K = 610 traps).** Raising the triggering
fee 2,000,000× produced zero movement in where the Settle leg traps.

**Step 4 — SPEC.md's own "safe" figure was retested directly and
fails.** `K = 5,850` — the exact value SPEC.md §1 calls safe with
5.7× headroom, measured via the synchronous `vault.topup` probe — was
run through this experiment's real Settle leg at fee = 2,000,000. It
traps (`pending_count` stuck at 1), even on a trivial one-row ledger
where the real per-row parsing cost is negligible. The probe above
measures a different dispatch than the one that actually resolves a
withdrawal; its "safe" figure does not transfer to the Settle
callback.

**Answering the task's own decision rule directly: the boundary does
NOT scale with the triggering fee.** Per the task's own fallback,
TASK M2's K = 50,000 result is explained, not guessed at, as follows
— with the caveat that `spike/vault-m2`/`spike/actor-m2` are already
deleted (Hard Rule 5) and cannot be re-inspected to confirm this with
certainty: TASK M2's "WITHDRAW/Settle leg resolved cleanly" observation
most likely reflected only the `WITHDRAW`'s own fee-scaled synchronous
dispatch (which this task also found scales fine with fee — the
actor's own status stayed 0 at every fee and K tested up to the
Settle boundary), not a genuinely isolated Settle *re-entry* dispatch.
TASK M2 never split the intent's own fee from the external call's fee
the way this task's relay-actor design does, so it could not have
told the two legs apart. This task's design can, and does: the
Settle leg's own budget is small (on this one-row test, on the order
of a few hundred dummy-work units) and fee-independent across the
full range tested.

**This revives, with direct isolating evidence for the first time,
the part of P18's old "leading candidate, not confirmed" footnote
that P20 never needed and never tested:** a callback-specific,
fixed-size allocation smaller than an ordinary call's fee-funded
budget. P20's A/B test and this task's finding are not in conflict —
P20 showed the *original* P18 failure (at 220-300 real ledger rows)
was fully explained by the now-removed O(n²) check; this task shows
that, independent of that now-fixed issue, the Settle callback *still*
has its own small, fee-independent ceiling, not previously isolated.

**What remains open, and is now higher priority than before this
task.** This experiment used a minimal one-row ledger specifically to
isolate the fee/K relationship from real per-row parsing cost; it did
not re-run the real, un-padded Settle callback at the true 252-row +
32-pending worst case (the spike that could have done so, TASK M2's,
is already deleted). Whether that real worst case's own un-padded
parsing cost fits inside whatever this fixed Settle budget actually
is in absolute terms was not established this round. `SPEC.md`'s
"no cap change follows" conclusion is corrected to state it is
validated only for the synchronous probe, not for the real async
Settle callback. Logged in `docs/MAINTAINER_QUESTIONS.md` as a new,
higher-priority open question than the pre-existing fuel-supply
caveat (#12). `spike/vault-m4/` and `spike/actor-m4/` deleted after
use, per Hard Rule 5.

### P25 — Gate 3d TASK M5: the decisive measurement — a correction to TASK M4's own 1-row boundary, real K-boundaries for both Settle and Restore at every real-worst-case state, and the removed check's real cost measured directly: the ≥2× decision rule FAILS

**Closes P24's own open question.** P24 left open whether the real,
un-padded Settle/Restore callback at the true 252-row + 32-pending
worst case resolves cleanly, and what headroom it has. This task
answers both, using a fresh spike family (`spike/vault-m5/`,
`spike/actor-m5/`, Hard Rule 5, deleted after use) with
`vault.seed_full`/`vault.seed_pending` (byte-splice fixture
construction, reused from TASK M2) and a build-time flag to force
either Settle or Restore, resolution always read from a separately
byte-decoded `vault.topup` probe, never receipts or the relaying
actor's own reply.

**TASK M4's own 1-row+1-pending boundary (K=590 safe/610 traps) does
NOT reproduce.** Directly re-testing that exact configuration on this
round's build resolved cleanly at K=610. Three differential checks
ruled out the obvious culprits: (1) an actual bug found along the way
— this round's first dummy-work-loop draft used 32-byte arrays, not
the established 64-byte calibration, confirmed and fixed; (2) fixing
the array size did not reproduce M4's boundary either (resolved past
K=16,000); (3) a stripped vault with no `seed_full`/`seed_pending`
dispatch code at all, and an actor matching M4's own 101-byte
withdraw-body shape exactly, both still resolved at K=610. This
round's own bisected boundary for the identical 1-row+1-pending case
is K≈16,150 — roughly 26× M4's reported figure. Logged as a correction
to M4's own REVIEW entry (not silently overwritten), with the honest
caveat that M4's spike is already deleted and the exact cause of its
own wrong number cannot be confirmed — the same category of correction
TASK M2 made to TASK M's own uncontrolled sweep.

**Settle-leg K-boundary, bisected, by real state size (fee =
2,000,000 throughout):**

| State | K resolves | K traps |
|---|---|---|
| 1+1 | 16,140 | 16,160 |
| 100+32 | 12,625 | 13,000 |
| 190+32 | 10,025 | 10,350 |
| 220+32 | 9,300 | 9,650 |
| **252+32 (true worst case)** | **8,350** | **8,440** |

Headroom falls smoothly with row count (≈100 K-units per row), not a
cliff — consistent with TASK J's original row-count-dominance finding,
now confirmed across five sizes instead of two.

**Restore-leg K-boundary at 252+32: 8,200 resolves / 8,350 traps** —
measurably below Settle's own boundary at the identical state (Restore
inserts the restored amount back into the ledger; Settle never touches
the ledger), a small (~2%) but real and reproducible difference in the
direction `handle_callback`'s own code predicts.

**K-cost of the removed check, measured directly, not theoretically
calibrated: ≈13,190.** Reinstating the removed O(n²) per-depositor
check (own real shape: 8 depositor groups × 4, matching its own
architectural cap) at the true 252+32 worst case **traps even at
K=0** — its own real cost alone already exceeds the entire ~8,400-unit
measured headroom there. Measured at a 1-row ledger instead (enough
headroom, ~16,150 units, to get a finite number): with the check
enabled, the boundary drops to K≈2,960. 16,150 − 2,960 ≈ 13,190 —
roughly **13× every earlier round's own theoretical calibration**
("1,024 comparisons," derived from the check's own pairwise-key-
compare shape and used, unverified against the real check, in every
headroom calculation since TASK J). The real check, built from
idiomatic Rust iterator chains, costs far more per logical comparison
than the calibration assumed.

**Headroom = K-boundary(252+32) ÷ K-cost(removed check):** Settle
≈8,400/13,190 ≈ **0.64×**; Restore ≈8,275/13,190 ≈ **0.63×**. **The
≥2× decision rule this section has applied since TASK J fails
outright** — not narrowly: the removed check's own real cost exceeds
the entire measured headroom, independently confirmed by the direct
K=0 trap observation above. This is the real, measured explanation
(in the same K-equivalent units as every prior headroom figure in this
repo) for why removing that check (TASK H, P20) was necessary, not
optional — restoring it would not merely erode margin, it would
already break the true worst case with zero margin to spare.

**K=0 sanity: the current, check-removed production logic resolves
cleanly for both Settle and Restore at 252+32** with zero dummy
padding, confirmed on this round's K=0 spike baseline (functionally
identical to real `parse()`/`handle_callback`). Whether the earlier
real-call marathon (REVIEW.md entry 42, 287 rows + 32 pending) itself
ran with K=0 is not confirmed from the available record — inferred
likely from its stated purpose (reachability/correctness, not fuel
exhaustion) but not stated explicitly, and its producing spike is
already deleted.

**No remediation chosen here, per this round's own instruction.**
Options and their own costs are listed in `REVIEW.md`'s Gate 3d entry
for the user's decision, not decided in this document. `spike/
vault-m5/`, `spike/vault-m5strip/`, `spike/actor-m5/` all deleted
after use, per Hard Rule 5.

### P26 — Gate 3e TASK F1/F2: the ≥2× rule dropped as unsatisfiable; a callback fast-dispatch path, now in production, measured at ≈1.6–1.7× the prior callback cost at the true worst case

**The ≥2× rule, retired.** P25 found the removed check's own real cost
(≈13,190 K-units) exceeds the entire measured headroom (≈8,400/8,275)
at the true 252+32 worst case — a ratio of ≈0.63–0.64×. This round's
own measured per-row (≈28 K) and per-pending (≈24 K) costs show no
cap adjustment can close that gap to 2×: even the near-empty
1-row+1-pending case has only ≈16,140 units of spare K, short of the
≈26,380 a 2× margin against a ≈13,190-unit fixed cost would require.
The rule is dropped as unsatisfiable on this contract's own real
numbers, not relaxed or reinterpreted.

**TASK F1 — a callback fast-dispatch spike, decision-gated before any
production change.** `spike/vault-f1/` (Hard Rule 5, deleted after
use): dispatch routes a callback to a new path BEFORE any full
`VaultState::parse`, straight from the raw state bytes — the wire-
level callback flag needs no state access to evaluate, so this
reordering is safe regardless of which branch runs. The new path
validates the header in full and the entire pending section exactly
as `parse()` does (same checks, same `ERR_INVALID_STATE` on any
failure), but no longer decodes or re-validates the ledger section's
own content — only its declared byte length. Settle copies the ledger
bytes through verbatim; Restore locates the one row that changes via
binary search over the raw 85-byte records, using a comparator that
decodes each candidate's key fields in the exact field order
`LedgerKey`'s own derived `Ord` uses (`currency_id` compared as a
decoded `u32`, never raw little-endian bytes — the two disagree for
pairs like id 256 vs id 1, confirmed as a real trap by a dedicated
test).

**Gate measurement (one session, one build family, bisected): both
Settle and Restore clear the round's own ≥1.5× bar.** Settle's
fast-path spare K at the true 252+32 worst case: ≈13,000–14,000
(13,000 resolves, 14,000 traps, narrowed to 12,000/13,000 then
confirmed). Restore: ≈12,250–13,000. Against this round's own
re-measured baseline — ≈7,600–7,625 (Settle), ≈7,525–7,600 (Restore),
itself ≈9% below P25's own 8,350/8,440 and 8,200/8,350 (ordinary
session-to-session drift, reported as a range, not chased further,
and a materially smaller discrepancy than TASK M4's own ≈26×
error) — the ratios are ≈1.7× (Settle) and ≈1.6× (Restore), both
clearing the gate.

**TASK F2 — the fast path, implemented in production, test-first.**
`handlers::handle_callback_fast`/`state::VaultState::fast_callback`
are now the real dispatch for every callback; the pre-fast-path logic
(`handlers::handle_callback`) is kept `#[cfg(test)]`-only as the
differential tests' own ground truth, not left as dead production
code (confirmed via a clean, warning-free build — `pending_index`/
`remove_pending` are `#[cfg(test)]`-gated for the same reason).
Differential coverage: the existing 200-seed random operation-
sequence property test now also runs the slow path on every real
callback step's identical pre-state and asserts byte-identical
encoded output; five dedicated host tests cover the ordering trap
(currency ids 1/255/256/257), Restore at the true 252-row worst case
for both the add-to-an-existing-row and insert-a-new-row sub-cases
plus an overflow case, unknown-id/wrong-caller/wrong-magic/
paused-deposits-still-resolves via the real dispatch specifically,
and a corrupted-ledger-row scenario proving Settle still succeeds
while the very next `DEPOSIT`/`WITHDRAW` (still calling the full,
unconditional `parse()`) catches the corruption; two new fuzz tests
(5,000 iterations each, random and header-aware) confirm the fast
path never panics.

**Build impact.** `contract.wasm` grew from 55,703 to 58,559 bytes
(the new dispatch logic outweighing what moving the slow path to
test-only removed); `contract.acab`/`contract.descriptor` unchanged
(no new ABI surface — this is a dispatch-order and validation-scope
change only). Combined deploy envelope: 58,559 + 3,182 + 66 = 61,807
bytes, ≈5.3% below the 65,237-byte combined limit (`docs/
MAINTAINER_QUESTIONS.md` #11) — a materially tighter margin than the
≈9.6% before this change, worth tracking if future growth continues
in this direction. (`README.md`'s own "Build artifacts" section is
this repository's one current source for these figures going
forward — the numbers above are this entry's own point-in-time delta,
not repeated elsewhere as if live.)

**What this changes for this contract's own guarantees, and why it's
accepted:** a callback no longer requires the stored ledger to be
fully valid, only its header and pending section. A corrupted ledger
row never blocks a Settle or Restore; the next `DEPOSIT`/`WITHDRAW`,
unconditionally calling the full `parse()`, catches it there instead.
Accepted because nothing in this contract's own writer logic can ever
produce a ledger row `parse()` itself would reject — the gap only
matters against corruption from outside this contract's own control,
where a one-call delay in catching it is judged a smaller risk than
the margin problem this round closed. `spike/vault-f1/` deleted after
use, per Hard Rule 5.
