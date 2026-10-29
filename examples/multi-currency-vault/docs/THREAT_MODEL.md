# Multi-Currency Vault — THREAT MODEL

This is a standalone document: every section is written out in full, with
no "unchanged from round N" references to earlier drafts. It analyzes
`../SPEC.md` (also standalone) for exploitability, grounded in
`PLATFORM_FINDINGS.md`'s empirical results against the real
`avm-local-stand`. `MAINTAINER_QUESTIONS.md` (same directory) consolidates
every open question this spike raised against the SDK itself, in priority
order. Nothing here claims the contract is "secure" or
"audited" — this records what was tested, what was not, and what residual
risk remains open, per this engagement's Hard Rule 6.

## 1. Scope and methodology

In scope: the multi-currency vault contract described in `../SPEC.md` —
its state machine, its entry points, and every invariant it claims —
I1–I17, including the I4-family sub-invariants I4b, I4c, and I4d, which
refine I4's caller-matching guarantee for the callback path specifically
and are cited by their own letters throughout this document and
`../SPEC.md` §7–§9.
Out of scope: the AVM platform/runtime itself (closed-source; this
repository's own `README.md` states "production AVM runtime source remains
in the private Acki Nacki repository"), the `avm-contract-sdk` crate's own
internal correctness (treated as a trusted dependency, though several of
its specific validation behaviors are cited below because they directly
bound what this contract can or cannot be attacked through), and any other
example contract in this repository.

Methodology: every claim below is backed by one of three things, and each
claim states which: (a) a specific empirical result recorded in
`PLATFORM_FINDINGS.md`, obtained by running a throwaway, never-committed
spike contract against the real `avm-local-stand`/`avm-contract-build`
tools; (b) a specific source citation from this repository's own
`avm-contract-sdk` crate; or (c) a reasoned argument from the contract's own
specified logic in `../SPEC.md`, clearly labeled as analysis rather than an
observed or sourced fact. Nothing below is asserted on the strength of "this
seems unlikely" alone.

## 2. Adversary model

The adversary is assumed to control: any number of external signing keys;
any number of same-DApp accounts, including accounts that are themselves
arbitrary, adversary-written contracts (not merely externally-owned-style
accounts) — meaning any depositor can run arbitrary logic in response to
receiving a payout, and that logic can itself be adversarial; the timing
and ordering of every message the adversary's own accounts send, within
whatever constraints the platform itself enforces; and, in the owner
compromise scenario specifically (§5.6), the vault's own `owner_pubkey`
private key.

The adversary is assumed **not** to control: the AVM platform/runtime
itself (its message-routing, fee-charging, and rollback-on-failure
behavior are trusted, subject to the explicit caveats in §3); any other
depositor's private key; or this contract's own deployed code (there is no
upgrade path — see `../SPEC.md` §1 — so there is no "malicious upgrade"
scenario to consider at all).

## 3. Trust assumptions — explicit, with confidence level

These are the platform-level guarantees this contract's safety depends on.
Each is stated with how strong the evidence behind it actually is, because
"the contract is correct assuming the platform behaves as expected" is a
materially different claim from "the contract is correct, full stop," and
collapsing that distinction would violate Hard Rule 6.

- **A failed message delivery never debits the sender — CONFIRMED,
  empirically, on the one route this spike could reach.**
  `PLATFORM_FINDINGS.md` probes A1 and A3: a successful send debits the
  sender by exactly the sent amount; a failing send never touches the
  sender's balance, not even transiently (verified via clean before/after
  balance reads, not inferred from a single after-the-fact state). This
  was tested only on the local, same-DApp, synchronous delivery route —
  see the atomicity caveat below for why that scope matters.
- **The receiving account's `balance_main`, both during the callback's own
  execution and in the final persisted state afterward, excludes the
  callback's own carried value — in both the success and the failure case
  — CONFIRMED, empirically, as an outcome, not asserted as a claim about
  the underlying mechanism.** (Reworded in round 6 to state only what was
  actually observed — see below.) `PLATFORM_FINDINGS.md` probe A8: two
  otherwise-identical experiments, differing only in whether the receiving
  account's own callback-handling code reported success or failure for
  that callback, produced an identical real `balance_main` for that
  account in both cases — the carried value was reflected in neither.
  This is stated at the outcome level deliberately: the probe measured
  `balance_main` before, during, and after, in both variants, and found no
  difference attributable to the carried value — it did not, and could
  not, observe *why* the host produces that outcome (whether the value was
  "never really there," "credited and immediately reversed," or something
  else entirely is not something a guest-side probe can distinguish; the
  host's own accounting logic is closed-source, per §1 of this document).
  This is the single most load-bearing finding for this contract's
  design: it is what makes "restore unconditionally, with no verification
  gate" safe rather than reckless given the *observed outcome*
  (`../SPEC.md` §1 explains the full reasoning; §5.2 below analyzes the
  attack this closes off) — a claim this document is careful to ground in
  what was measured, not in an assumed mechanism behind it.
- **No guest contract can ever send SHELL (`extra[currency_id=2]`) through
  its own `OutboundIntent` — CONFIRMED, empirically, independent of
  destination health or outcome.** `PLATFORM_FINDINGS.md` probe P1,
  reconciled directly in round 5 against an earlier, apparently
  contradictory result: a fresh, well-funded sender forwarding SHELL-extra
  to an explicitly-failing destination still hit `EconomicsError` at
  materialization — the same unconditional failure as every other SHELL-
  forwarding attempt tested, healthy destination or not. The original
  contradictory data point could not be reproduced under any controlled
  retest found and is retracted as stale, not reconciled with a
  mechanism — see `PLATFORM_FINDINGS.md`'s round-5 addendum for the full
  retraction writeup.
- **SHELL exhaustion is an account-wide outage condition, not merely an
  external-surface condition — CONFIRMED, empirically.**
  `PLATFORM_FINDINGS.md` probe P1: an internal message to a SHELL-exhausted
  destination fails too (observed as a failure callback carrying an
  undocumented `status_code = 56`), and every external call costs the
  destination SHELL to process regardless of authorization outcome, even
  one rejected immediately for the wrong key.
- **The host's guest-output decoder has no gap through which a callback
  flag, or any other flag, could be smuggled — CONFIRMED, empirically,
  pushed past source-reading alone.** `PLATFORM_FINDINGS.md`: a fully
  SDK-free, hand-rolled raw contract was built specifically to attempt
  appending one extra trailing byte after an otherwise byte-for-byte-
  correct output; the host rejected it outright (`"TrailingBytes"`),
  confirming the decoder is exactly as strict as the documented wire
  format implies.
- **A `MessageOutput` cannot combine a non-zero status code with any
  outbound/callback/event effect — CONFIRMED, empirically, matching the
  SDK's own `EffectsOnFailure` source check.** `PLATFORM_FINDINGS.md`
  addendum B: a depositor fixture built specifically to both keep a
  received payout and report failure for it was rejected by the runtime
  before ever being admitted (`"VM status 43"`).
- **Atomicity of a failing transfer was verified as an outcome, on one
  route, not proven as a mechanism that holds everywhere — UNVERIFIED
  beyond that one route, named explicitly rather than assumed silently.**
  Every claim above about "never debited," "fully rolled back," etc. rests
  on observing account balances before and after a call on the local,
  same-DApp, synchronous delivery route — never on reading the host's own
  accounting logic, which is closed-source. Whether a cross-DApp or
  post-split chargeable route (§6, the "L"/split-liveness question)
  produces the same outcome is unverified either way. §8's simulator
  includes a dedicated "retain" branch to demonstrate concretely what
  breaks in this contract's own accounting if this assumption is ever
  wrong somewhere this spike had no way to reach, and to state plainly
  that no code path in this contract would catch it if so.
- **Inbound internal messages (deposit/withdraw requests, as opposed to
  their resulting callbacks) are delivered exactly once — UNVERIFIED,
  assumed as an ordinary platform trust boundary, not independently
  re-implemented.** No documentation anywhere in this repository states
  whether AVM guarantees exactly-once delivery for internal messages. This
  contract does not defend against duplicate delivery of an original
  request (doing so would require remembering every processed message's
  hash forever, which conflicts directly with the bounded-state
  requirement, and no other example contract in this repository defends
  against it either) — §6 states the consequence of this assumption being
  wrong plainly, as a named, accepted residual risk, not a silent gap.
- **Whether the platform itself deduplicates an exact replayed external
  message is unverified either way — not load-bearing for this contract
  regardless of the answer.** `PLATFORM_FINDINGS.md` Q5. This contract
  does not rely on the answer: `admin_seq` (`../SPEC.md` §1, Invariant I13)
  makes its own independent, contract-level replay-rejection guarantee for
  every owner/pending-owner call, so a platform-level answer either way
  changes nothing about this contract's own safety here.
- **Mainnet's storage-rent formula and its long-arrears account behavior
  are undocumented and unverified.** `../SPEC.md` §2.4 restates this
  spike's own local-stand rent measurement (open issue #21, roughly 1
  SHELL/byte/second) in terms of this contract's own worst-case state
  size, and explicitly flags that mainnet's actual formula, and what
  happens to an account whose rent arrears grow very large relative to its
  balance over a long period, are both unknown — recorded as an open
  question for the maintainers, not resolved here.

## 4. Attack surface inventory

Every externally or internally reachable entry point, exactly as specified
in `../SPEC.md` §4: the callback-handling branch (gated on the wire-level
callback flag, reachable only in response to a message this contract
itself sent), `DEPOSIT_METHOD_ID` (`Internal`, same-DApp only),
`TOPUP_METHOD_ID` (`External`, permissionless), `WITHDRAW_METHOD_ID`
(`Internal`, same-DApp only), `PROPOSE_OWNER_METHOD_ID`/
`ACCEPT_OWNER_METHOD_ID`/`SET_PAUSE_METHOD_ID` (`External`, owner- or
pending-owner-gated), and the fallthrough for every other method id
(`unrecognized_method`, no effect). There is no `TRANSFER` method and no
`VERSION` method — both were removed during this specification's earlier
rounds and are analyzed in §5.5 and §5.9 respectively as *removed* attack
surface, since understanding why they were removed is itself part of this
contract's threat model.

## 5. Per-attack analysis

### 5.1 Unauthorized withdrawal (stealing another depositor's funds)

**Attack:** craft a `WITHDRAW` call that debits a ledger entry belonging to
someone other than the caller.

**Defense:** Invariant I4 — a `WITHDRAW` call is only ever evaluated
against the ledger entry keyed by `Caller::Internal(source)` on the inbound
message itself, read from the message's own authenticated caller field, not
from anything the body supplies. There is no body field naming "whose
balance to debit" at all — the only choice the caller has is which
currency and how much, always against their own entry. An adversary cannot
name another depositor's entry as the target because no such field exists
to name it in.

**Residual risk:** none identified beyond the platform-level trust
assumption that `Caller::Internal` itself cannot be forged by a message
that did not genuinely originate from the claimed account — this is a
platform guarantee this contract has no way to independently verify and
treats as foundational, consistent with every other example contract in
this repository making the same assumption about `Caller`.

### 5.2 Forged settlement via a fabricated or malicious callback

**Attack:** get a withdrawal double-credited, or credited without the
funds ever actually leaving the vault, by controlling or spoofing the
callback that resolves it.

**Defense, layered:** First, no guest contract can construct a
callback-flagged message at all — confirmed in §3 above (`OutboundIntent`
has no `flags` field whatsoever, and the raw-contract trailing-byte probe
found no gap in the host's output decoder either). Second, even a genuine
callback is only accepted if `Caller::Internal(source)` on that callback
message equals the specific pending record's own depositor exactly
(Invariant I4b, `../SPEC.md` §5/§7) — an adversary who controls some
*other* account cannot deliver a callback that resolves a withdrawal
belonging to a different depositor, because that callback's caller would
not match. Third, a related but distinct attempt — crafting a plain,
non-callback message whose `method_id` happens to numerically equal some
real `withdrawal_callback_method_id(id)`, hoping to reach settlement
logic without ever having to forge the unforgeable callback flag itself —
is closed by Invariant I4c: settlement is gated strictly on
`input.message.is_callback()`, checked before any method-id-based
dispatch, so a method-id collision alone, with the flag unset, falls
through to ordinary dispatch and fails as `unrecognized_method` (1002);
`../SPEC.md` §8 test 1 is the direct regression test. Fourth, and most
importantly, resolution never trusts the callback's own carried value at
all (Invariant I11) — restore always credits exactly the amount this same
withdrawal's own debit removed, never a value read from the callback —
which closes off the specific attack a verification gate was once designed
to catch (over-crediting by sending a callback claiming a larger carried
value than was ever real), and which §3's trust-assumption writeup shows
was never a real risk to begin with, since a callback's carried value was
confirmed to never be a real credit regardless.

**Residual risk:** none identified under the trust assumptions in §3. If
the "callback caller is forgeable" or "callback flag is smuggleable"
assumptions were ever wrong, this would become exploitable — both were
tested directly, not merely assumed, as cited above.

### 5.3 Ledger-row exhaustion as a denial of service against new depositors

**Attack:** fill `MAX_LEDGER_ENTRIES` so no new depositor/currency pair can
ever get a row, denying service to everyone else.

**Defense:** none, by design — this is the one DoS vector this contract
accepts as a permanent, documented limitation rather than defending
against, because every mechanism considered for reclaiming an unwanted row
would require seizing a depositor's funds without their consent, which
Invariant I5 forbids categorically. With `TRANSFER` removed (§5.5), the
attacker can only fill the ledger using their *own* funds, across many
self-controlled addresses — so the cost of this attack is bounded by
however much the attacker is willing to permanently lock up as dust (one
unit of some currency per occupied row, times up to 220 rows), not merely
bounded by gas/fuel the way a typical blockchain DoS might be. This is
stated plainly in `../SPEC.md` §1 as a permanent limitation of a
conservative, non-upgradeable v1 cap — not expected to self-resolve, and
not something a larger cap would meaningfully fix (a larger cap only
raises the attacker's own lockup cost proportionally, while raising this
contract's worst-case state size and therefore its rent exposure — see
§5.9).

**Residual risk:** accepted and documented, not mitigated. A future
revision with an upgrade path could introduce a bonded reclaim mechanism
(e.g., a long inactivity timeout combined with a minimum-deposit floor
below which a row can be swept to some neutral destination) — out of
scope for this non-upgradeable v1.

### 5.4 Pending-withdrawal slot exhaustion

**Attack:** an earlier design's sharpest finding — fill
`MAX_PENDING_WITHDRAWALS` by withdrawing small amounts to many distinct,
never-deployed addresses, permanently occupying every pending slot with no
possible resolution (an undeployed destination was confirmed, per
`PLATFORM_FINDINGS.md` addendum C, to produce no debit, no callback, and no
error at all — silently unresolvable forever).

**Defense:** this attack cannot even be expressed against the current
specification. `WITHDRAW`'s body carries no destination argument at all
(`../SPEC.md` §4) — every withdrawal necessarily targets the caller's own,
already-existing account, which has, by definition, already proven it
exists and can send messages, because it is the very account that just
sent the withdraw request. There is no way to name an arbitrary,
potentially-undeployed address as a withdrawal's destination anywhere in
this contract. `../SPEC.md` §8 test 4 is the direct regression test
confirming this attack cannot be constructed.

**Residual risk:** the per-depositor cap (`MAX_PENDING_PER_DEPOSITOR = 4`)
still means a single depositor can have at most 4 of their own
withdrawals outstanding simultaneously; this is a self-imposed limit on
the depositor's own concurrency, not an attack surface against anyone
else, since a depositor can only ever occupy slots with their own pending
withdrawals, to their own eventual benefit or loss.

**Restored from an earlier round (dropped during consolidation, reinstated
in round 6): the *global* `MAX_PENDING_WITHDRAWALS` cap (32) is a shared
resource across every depositor, and a small number of simultaneously
active depositors can fill it legitimately, without any griefing intent at
all.** Unlike the undeployed-address attack above, this does not require
any destination trickery — it only requires ordinary use at a modest
scale: 8 distinct, perfectly legitimate depositors, each at their own
`MAX_PENDING_PER_DEPOSITOR` limit of 4 concurrently outstanding
withdrawals (`8 × 4 = 32`), fill the global cap entirely. A 9th depositor's
*first* withdrawal attempt — even for an amount they genuinely hold, with
no malicious destination involved anywhere — is then rejected with
`pending_capacity_exceeded` (1509) purely due to global capacity, not
because of anything wrong with their own request. This is self-resolving
in the way §5.3's dust-ledger-exhaustion is not: every one of those 32
pending withdrawals is a real, resolvable request against a real
destination (unlike the attack this section exists to close), so ordinary
callback traffic drains the global count back down as those withdrawals
settle or restore, and the 9th depositor can succeed once a slot frees up.
It is named here as a capacity/availability limitation, not a security
defect — distinguishing it clearly from both §5.3 (permanent, attacker-
funded, never self-resolving) and this section's main undeployed-address
attack (which this specification closes outright, not merely bounds).

**Added in round 7 — the self-resolving framing above assumes every
pending withdrawal eventually settles or restores; that assumption breaks
if any of them hit "L" instead (§6), and the consequence is severe enough
to spell out explicitly rather than leave as a footnote to §6.** A pending
record that falls into "L" — via either route: the same-DApp-but-split
case, or the SHELL-starved dropped callback this round's P9 also directly
observed for an *internal*, deposit-shaped message (not just a
withdrawal payout) — never drains from `pending_count`, by design (there
is no reclaim mechanism; `../SPEC.md` §2.3). Unlike the 8×4 scenario
above, stuck records of this kind **accumulate instead of resolving**:
each one permanently occupies one of the 32 global slots forever. The
global cap is small enough that this does not require many occurrences —
**32 stuck pending records, accumulated over time from any mix of
depositors and either "L" route, permanently disable `WITHDRAW` for
every depositor in the vault, not only the ones whose own withdrawals got
stuck.** This is a materially worse consequence than "that one
depositor's funds are stuck" (already named in §6): it is a single,
shared point of failure that a small, slow trickle of unlucky or
attacker-engineered deliveries can eventually trip for the entire vault,
and once tripped there is no recovery path in this contract at all — not
even for depositors whose own withdrawals were never affected.

**Added Gate 2b round 3 TASK H — the collusion-cost question this
section's own "small, slow trickle" framing invites but did not
previously answer: how many accounts, and at what cost, could
*deliberately* engineer this outcome rather than wait for it to happen
by accident?** `MAX_PENDING_WITHDRAWALS` (32) ÷ `MAX_PENDING_PER_
DEPOSITOR` (4) = **8 colluding accounts**, each driving itself to its
own per-depositor cap with withdrawals engineered to land on "L" (never
resolving) rather than genuinely settling or restoring, fills the
global pending-withdrawal capacity entirely and permanently, for every
depositor in the vault, with no fewer than 8 distinct accounts able to
do it (fewer accounts can occupy at most `4 × (accounts)` slots each,
by the same per-depositor cap that bounds ordinary use in the paragraph
above).

**Cost to execute, order-of-magnitude, not a precise bound.** The
capital placed at risk is not lost, only locked: each of the 8 accounts
needs a genuine ledger balance to withdraw against (`../SPEC.md` §4's
balance check is real, not bypassable), and that balance is never
burned by this attack — it sits, correctly tracked, in a ledger row
this contract's own accounting still recognizes; the attacker's
*lockup* cost is exactly the sum of what they chose to deposit, fully
theirs again if this vault is ever abandoned or upgraded with a reclaim
path. The *real*, non-recoverable cost is the SHELL spent on fuel for
the handful of real calls each account needs (deploy, one `DEPOSIT`,
one `WITHDRAW`) plus whatever is spent engineering the "L" outcome
itself (either triggering a same-DApp split at the right moment, or
deliberately under-funding the account so its own incoming-payout
execution starves for FUEL — `docs/PLATFORM_FINDINGS.md` P1's directly
observed "admitted, charged, zero-FUEL" signature). This round's own
real-stand experiments (TASKS F–G) measured ordinary `deploy`/`DEPOSIT`/
`WITHDRAW` calls at roughly 3,000–15,000 `charged_shells` each — on that
order, 8 accounts × a handful of calls each puts the real, burned cost
of this attack at roughly **a few hundred thousand SHELL**, a rough
estimate from this round's own measurements, not an independently
re-verified attack cost (actually engineering the "L" outcome
specifically, as opposed to a genuine Settle/Restore, was not attempted
this round — SHELL-starving an account precisely enough to hit zero-FUEL
rather than simply failing outright is itself nontrivial to land, per
round 6's own addendum). **This is cheap relative to what it buys**: a
few hundred thousand SHELL (small next to this contract's own measured
rent burn rate, `docs/PLATFORM_FINDINGS.md` P21/`../SPEC.md` §1 — over a
billion SHELL per day at worst-case state size) permanently disables
`WITHDRAW` for every depositor in the vault, forever, with no recovery
mechanism. This asymmetry — low, bounded attacker cost against an
unbounded, permanent denial of service for everyone — is the single
most important consequence of this section, more important than the
exact SHELL figure, which is deliberately not overstated as precise.

### 5.5 Dust griefing via a transfer-to-arbitrary-address method

**Attack, as it existed in an earlier design:** a `TRANSFER` method let any
depositor move ledger balance to any other same-DApp address, including
one that never deposited anything and has no relationship to the sender.
Repeated to up to `MAX_LEDGER_ENTRIES` distinct never-before-seen
addresses, this would consume ledger capacity with no way for anyone to
reclaim it, since Invariant I9 only removes a row when its own owner drains
it, and nothing forces an unwilling recipient to do that.

**Defense:** `TRANSFER` does not exist in this contract. Method id 3 —
what would have been `TRANSFER_METHOD_ID` — is permanently retired, not
reassigned (`../SPEC.md` §3), specifically so this document and this
contract's tests can never become confused about what that id once meant.
Removing the feature entirely, rather than adding a guard to it (e.g.
"recipient must already hold a row"), was the deliberate choice — such a
guard does not prevent dust being left on a row its owner never asked to
receive more of, it only slightly narrows who can be targeted.

**Residual risk:** none from this specific vector — it has been removed
entirely rather than mitigated. The self-funded ledger-exhaustion vector in
§5.3 remains, but that is a materially different, and strictly less
severe, attack: an attacker can only ever spend their own funds to occupy
capacity, never force an unwilling party into holding a dust balance they
never consented to.

### 5.6 Owner key compromise

**Attack:** an attacker obtains `owner_pubkey`'s private key (including, in
the worst case, by relying on the fact that it was never rotated away from
the fixed, publicly-known demo fallback — `../SPEC.md` §1) and attempts to
use it to steal, redirect, or seize depositor funds.

**Defense:** structural, not procedural — the owner role has no entry
point, direct or indirect, that can ever move, redirect, or seize any
depositor's ledger balance, under any circumstance (Invariant I5). The
entire capability `owner_pubkey` grants is exactly: pause deposits, and/or
rotate the owner (`../SPEC.md` §1, in full). An attacker holding this key
can grief by pausing deposits indefinitely and by taking over future owner
rotations, but cannot touch a single unit of any depositor's funds.
Existing withdrawals and the callback-settlement branch that resolves them
are entirely unaffected by pause state (Invariant I14) — an attacker who
seizes the owner key cannot even strand funds that are already mid-
withdrawal when they act.

**Residual risk:** the demo fallback key's private-key secrecy is
explicitly unverified, not confirmed safe (`../SPEC.md` §1) — a repository-
wide search found no private key material paired with the fixed fallback
constant in this public checkout, which proves only that this repo doesn't
contain it, not that nobody else does. This is why the README (`../SPEC.md`
§10) must state loudly that any real deployment must set
`AVM_EXAMPLE_VAULT_OWNER_PUBKEY` to a freshly-generated real key. Given the
structural defense above, even a fully compromised owner key is a
griefing/control risk only, never a funds-theft risk.

A related, non-adversarial consequence worth stating plainly (Gate 2a
round 2 item 8, corrected in round 3 item 5; `../SPEC.md` §1 in full): a
vault built with the fixed fallback key cannot have `PROPOSE_OWNER`/
`ACCEPT_OWNER`/`SET_PAUSE` genuinely exercised on a real
`avm-local-stand` at all, since no stand identity holds that key's
private half — `avm-local-stand init` generates fresh random identities
each time and can only ever sign as one of them, never as an arbitrary
externally-supplied key. This is narrower than claiming nobody anywhere
holds that key, which remains exactly as unverified as `../SPEC.md` §1
already states; real admin testing requires rebuilding with
`AVM_EXAMPLE_VAULT_OWNER_PUBKEY` bound to one of the stand's own
`init.json` identities first, the same pattern `docs/getting-started.md`
already documents for `contract-a-v1`'s analogous upgrade-authority key.
This is an availability/testability property, not a security weakness in
either direction.

### 5.7 Reentrancy during payout delivery

**Attack:** a depositor whose own account is an adversarial contract, upon
receiving a withdrawal payout, immediately sends new messages back to the
vault — a new withdraw and/or a new deposit — within the same execution,
attempting to exploit a stale or inconsistent intermediate state (e.g.
double-spending a balance that should already be debited, or forcing
unbounded recursive execution).

**Defense:** checks-effects-interactions ordering, confirmed directly
against the real platform, not merely assumed from the pattern's general
reputation. `PLATFORM_FINDINGS.md` probe P4 built a `ReentrantDepositor`
fixture (a minimal, SDK-based contract whose `process_message` handler,
upon receiving any non-callback internal message, immediately constructs
and returns two new outbound intents back to the sender — a reentrant
withdraw attempt and a reentrant deposit — in that same execution, before
its own success is even reported back as a callback) against a minimal
`MiniVault` stand-in (whose own withdraw flow debits/marks pending *before*
sending the payout, exactly mirroring this specification's own ordering).
The observed, directly-confirmed ordering: the reentrant withdraw was
evaluated against the already-correctly-debited balance (not a stale,
pre-debit balance), and the original withdrawal's own pending record
remained unsettled for the full duration of the depositor's execution,
including its reentrant sends, resolving only afterward. This specification
requires this contract's own debit and pending-record insertion to be
committed as part of the same state update that is already final before
the outbound payout is even sent (`../SPEC.md` §5, explicit
checks-effects-interactions framing) — the same ordering `MiniVault`
exhibited. `../SPEC.md` §8 test 11 describes the direct Phase 3
regression test for this exact scenario against the real vault, but
**is recorded there as NOT DONE** (Gate 2b round 3 TASK I) — building it
requires genuinely new spike engineering (a depositor fixture whose
`method_id = 0` handler constructs two outbound intents, not one, from
within the same sanctioned `AVM_E2E_FAILURE_FLOW` pattern tests 2/5 use),
and this round's time went to TASK F/G's real-stand A/B investigation
instead. What actually exists today, closing most but not all of the
gap a missing test would leave: **probe P4 itself**
(`docs/PLATFORM_FINDINGS.md`), which directly observed this exact
ordering on the real platform using the `ReentrantDepositor`/`MiniVault`
fixtures described above, and a **host-level unit test** asserting the
same checks-effects-interactions ordering within this contract's own
`handle_withdraw` implementation (debit and pending-record insertion
committed before the outbound payout intent is constructed). Neither is
a substitute for a dedicated end-to-end regression test built against
the real, deployed vault and a real reentrant depositor; that gap is
open, not silently assumed closed.

A second variant — a depositor that re-withdraws *on every single payout
it ever receives*, attempting to force unbounded recursive execution
rather than merely testing ordering once — was not independently re-run
against the platform as a sixth probe; it follows analytically from
Invariant I8 (the pending-withdrawal caps) combined with P4's already-
confirmed ordering, rather than requiring new empirical evidence. This is
stated as an analytical conclusion, not an empirical one, and `../SPEC.md`
§8 test 12 is required specifically because "a cap exists on paper" and
"the real implementation correctly enforces it under genuinely recursive
pressure" are different claims, and only a dedicated implementation test
can verify the second one.

**Residual risk:** none identified beyond the general trust assumption
that the platform's own execution ordering (one execution fully completing,
including any outbound sends and recursive deliveries it causes, before
that execution's own result becomes visible to its caller as a callback)
is correctly and consistently enforced — which is a platform guarantee,
not something this contract could verify further on its own, but which was
tested directly here rather than merely assumed.

### 5.8 Vault SHELL exhaustion stranding a withdrawal mid-flight

**Attack, or more precisely, platform-level hazard this specification
defends against rather than exploits:** a withdrawal is accepted (ledger
debited, pending record created) and then its own settlement callback
never arrives, purely because the vault itself was too close to
SHELL-empty when it initiated the withdrawal — through no fault of the
depositor, and with no malicious actor required at all; ordinary vault
operation under-provisioned for its own SHELL needs is sufficient.

**Finding, directly confirmed (`PLATFORM_FINDINGS.md` probe P5):** in a
controlled experiment, a forward-and-callback-request chain to a failing
destination consumed a sender's *entire* remaining SHELL balance, and the
sender's own callback-handling code never ran at all, confirmed by the
complete absence of the diagnostic state write that code always performs
when it does run. An otherwise identical chain to a succeeding destination,
started from the same balance, completed normally with SHELL to spare.
Since a withdrawal's debit and pending-record creation are committed
*before* the outbound payout is even sent (§5.7's checks-effects-
interactions ordering), this means a withdrawal can be fully, successfully
accepted by this contract's own logic, and then simply never resolve,
through no defect in the withdrawal request itself.

**Defense:** Invariant I17 — `WITHDRAW_METHOD_ID` checks the vault's own
current real SHELL holding (`input.extra`, filtered for `currency_id ==
2`, i.e. the account's own actual balance — `crates/avm-contract-sdk/src/
input.rs` line 313, paired with `input.balance_main` at line 312 — never
anything ledgered, since SHELL is never ledgered — Invariant I10, and never
the inbound message's own carried `extra`, which is a different field
entirely: `CanonicalMessage::extra()`, lines 275–280) against
`MIN_OPERATIONAL_SHELL` (`1,500,000` as of Phase 4, correcting Gate 2b
round 3 TASK H's `1,300,000`; see below) before doing anything
else, and refuses to even start a withdrawal if the vault is below that
threshold (error 1528, `vault_underfunded`), with absolutely no effect in
that case — no debit, no pending record, no outbound intent. `../SPEC.md`
§8 tests 13 and 14 are the direct regression tests, at two different
levels deliberately: **test 13 is a coarse stand (E2E) test** — it drains
a test vault's own SHELL below the threshold via genuine external calls
on the real `avm-local-stand` and confirms rejection with no state
change, landing *somewhere* below the threshold rather than at an exact
value; **test 14 is a host-level unit test with a crafted `input.extra`**
— the real stand has no way to land a vault's SHELL on one exact value
on demand, so the exact boundary (`MIN_OPERATIONAL_SHELL - 1` rejects,
`MIN_OPERATIONAL_SHELL` itself does not, and an absent SHELL entry means
exactly zero) is instead checked by constructing a synthetic
`MessageInput` directly against the contract's own `process_message`.

**Residual risk, explicitly not fully closed, stated plainly rather than
implied solved — sharpened considerably in round 6.** `MIN_OPERATIONAL_SHELL`
is a heuristic safety margin, not a formula-derived guarantee. Round 6
measured the actual withdraw-chain cost using fixtures shaped like this
contract's own flow (not the generic forward/callback probe used in round
5): `2,367` SHELL for a small-state (91-byte) chain, identically for
success and failure; `157,172` SHELL for a chain at this contract's own
31,234-byte worst-case state size, again identically for success and
failure (`docs/PLATFORM_FINDINGS.md`, round-6 addendum). A separate,
nominally-similar large-state call was observed costing `781,851` — a
roughly 5× swing versus the `157,172` figure for reasons this spike could
not fully pin down. These round-6 figures come from generic fixtures, not
this contract's own implementation; they motivated the round-7 placeholder
value (historical: `1,600,000`), since superseded — see below and
`../SPEC.md` §1 for the final, real-vault-measured derivation
(`1,500,000` as of Phase 4; an intermediate `1,300,000` figure from Gate
2b round 3 is also now historical, superseded). Beyond ordinary
variance, round 6 found a qualitatively
new and more severe failure mode: **a severely rent-delinquent large-state
account can have a call admitted and charged while its own contract code
never runs at all, and a value credit that call was supposed to deliver can
be silently dropped in that state** — observed directly (§5.9) by driving a
large-state spike account far enough behind on rent that a call paying down
the debt consumed its entire FUEL allowance, returning `"status": "ok"`
with no genuine execution (`"events": []`, `"raw_executor_output": ""`) and
no actual value credit, confirmed by a follow-up withdrawal attempt failing
with `EconomicsError`. If the vault's own account were ever to fall this
far behind on rent, `MIN_OPERATIONAL_SHELL` provides no protection at
all — the check is inside the contract's own logic, and this failure mode
is precisely the contract's own logic never running. This specification
does not claim to solve that case — it only closes the common, easily-
reached case of a vault that is simply low on SHELL from ordinary
operation, and states this boundary explicitly so a reader does not
mistake Invariant I17 for a stronger guarantee than it actually provides.
If the vault's own account has accrued a very large storage-rent debt from
having gone untouched for a long time while holding a large ledger (§5.9),
the true SHELL amount required to see a new withdrawal's own callback
through to completion could, in principle, exceed even the final
`1,500,000` threshold, since rent is collected lazily and its mainnet
long-arrears behavior is itself unverified (§3, §5.9).

**Round 7 directly tested the depositor-facing side of this same hazard —
a `DEPOSIT`, not a `WITHDRAW`, sent to a rent-delinquent vault — and found
no depositor-loss scenario.** A deposit-shaped internal message sent to a
deliberately rent-delinquent large-state spike account came back with a
failure callback carrying `status_code = 56` (the same code §5.8 and
round 4's P1 associate with SHELL/rent exhaustion), and the sender's own
`balance_main` was confirmed, by direct readback, unchanged — the
attempted deposit was never taken (`docs/PLATFORM_FINDINGS.md`, round-7
addendum, P9a). This closes off one way this hazard could have been
worse than already described: a depositor retrying a failed deposit loses
nothing in the attempt, even against a severely delinquent vault. A
separate, undersized external top-up attempt against the same account
was rejected outright with no SHELL credited anywhere either (P9b) — so
neither tested path produces a scenario where value is taken without
being credited. This was `MIN_OPERATIONAL_SHELL`'s required
re-measurement target as of round 7 — **since completed**: Phase 3 and
Gate 2b round 3 (TASK H) directly measured the real, implemented vault
and re-derived the constant, no longer provisional — further corrected
at Phase 4 (fresh-context adversarial review, finding 1) to its current
final value, `1,500,000` (`../SPEC.md` §1).

### 5.9 Storage-rent-driven state growth, cap justification, and the retired VERSION method

**Attack / hazard:** a large `FullReplace` state costs more to store, and
accrues storage rent (open issue #21) proportional to its size and the time
since it was last touched; an oversized state, left untouched, could in
principle accrue enough rent debt to threaten the vault's own solvency or
liveness (feeding directly into §5.8's hazard).

**Analysis — figures corrected twice: at Gate 2b round 3 (TASK H), and
again at Phase 4 (fresh-context adversarial review, finding 1).** This
section's own worst-case figures (`332`/`300`/`31,159`) were never
updated through Gate 2b round 2's cap reduction, a gap Gate 2b round 3
caught and fixed (logged in `REVIEW.md`) — but that fix itself still
understated the true worst case, as Phase 4 then found. `../SPEC.md`
§2.4 derives this contract's current worst-case state size as **24,359
bytes**: `91` (header) `+ 252 × 85` (ledger entries) `+ 32 × 89`
(pending records) `= 91 + 21,420 + 2,848 = 24,359`. The **252 rows** come
from `MAX_LEDGER_ENTRIES + MAX_PENDING_WITHDRAWALS` (220 + 32): every one
of the 32 possible pending-withdrawal restores bypasses the new-entry
cap (§1) and can recreate a row beyond it. Gate 2b round 3 reasoned that
reaching 252 rows requires every pending withdrawal to have *already
resolved* (via restore), and so used **220 rows + 32 pending = 21,639
bytes** as the worst case instead of 252 rows + 0 pending (`91 + 252 ×
85 = 21,511` bytes, smaller than 21,639 since a pending record, 89
bytes, costs more than a ledger row, 85 bytes). **Phase 4 found that
reasoning incomplete**: once 252 rows exist with 0 pending, a *partial*
withdrawal — debiting a row to something other than exactly zero —
opens a new pending record **without removing its row** (Invariant I9
only removes a row a debit brings to exactly zero, and `handle_withdraw`
never checks `MAX_LEDGER_ENTRIES`, only the two pending caps). All 32
pending slots can therefore be refilled on top of all 252 rows
simultaneously — `src/handlers.rs`'s
`true_worst_case_state_is_252_rows_plus_32_pending` host test
constructs this exact sequence and confirms it reaches 24,359 bytes,
`2,720` bytes (`32 × 85`) larger than the previous `21,639`-byte figure.
`24,359` (252 rows + 32 pending), not `21,639` or `21,511`, is this
contract's true documented worst case.

This section restates that worst case in rent terms using issue #21's
locally-measured ~1 SHELL/byte/second figure: roughly 2.1 billion SHELL
per day of accrued rent if the vault's state were at its absolute worst
case and went completely untouched for a full day (historically, the
`21,639`-byte figure gave ~1.87 billion). Round 6 reconfirmed
the ~1 SHELL/byte/second figure directly at scale, not merely by citing
the issue: a dedicated 31,234-byte spike account left untouched for
10,005 seconds accrued `storage_due_payment = 310,342,012`, i.e.
`≈0.993` SHELL/byte/second (`docs/PLATFORM_FINDINGS.md`, round-6
addendum). `MAX_LEDGER_ENTRIES` is deliberately kept at 220 (reduced
from the original 300 at Gate 2b round 2, for a reason unrelated to rent
— see `../SPEC.md` §1/§2.4 and `docs/PLATFORM_FINDINGS.md` P18–P21 — and
left there rather than restored, since a smaller worst-case state helps
the separate `MIN_OPERATIONAL_SHELL` margin finding those same documents
describe) rather than lowered in response to the rent number in this
section specifically, for three stated reasons (repeated here because
they are a threat-model conclusion, not merely a spec design note):
actual rent scales with the account's *actual* current state size, not
with the cap it's permitted to reach, so a lightly-used vault is unaffected
by what the cap is set to; the one scenario where the cap size matters for
rent (§5.3's deliberately-filled ledger) is already an accepted, permanent
limitation independent of the exact cap chosen, and shrinking the cap only
makes that same griefing cheaper to trigger, in exchange for a
proportionally smaller worst-case rent bill — not an obviously favorable
trade; and the local stand's measured rent rate is explicitly not known to
reflect mainnet pricing or long-arrears behavior at all, so optimizing the
cap against a number that may not apply to a real deployment risks solving
the wrong problem. The open question for the maintainers — mainnet's exact
rent formula, and what happens to an account whose arrears grow very large
relative to its balance over a long period — is recorded in `../SPEC.md`
§2.4 verbatim (now the *second* maintainer question as of round 6 — see
below and `../SPEC.md` §10 for the question promoted ahead of it), not
resolved here, because it cannot be resolved from this repository's own
documentation.

**New in round 6: rent can consume an entire call's FUEL budget, causing
the contract's own code to never run — and, when that happens, a value
credit the call was supposed to deliver can be silently lost.** This was
observed directly, not inferred: a large-state spike account left
untouched long enough to accrue a large rent debt had a subsequent call
(intended to both pay down that debt and credit it a `value_main` amount)
return `"status": "ok"` with `charged_shells` consuming almost its entire
prior balance, yet `"events": []` and `"raw_executor_output": ""` — the
same "admitted and charged, but guest code never actually runs" state
`docs/PLATFORM_FINDINGS.md` P1 first identified in round 4 for plain SHELL
exhaustion, now confirmed to be reachable via rent arrears specifically,
not only via a directly-drained balance. A follow-up withdrawal attempt
confirmed the intended `value_main` credit from that call never actually
landed (it failed with `EconomicsError` for insufficient balance); a
repeat of the same credit, issued once the account's rent was already
current and the contract's code demonstably ran (non-empty
`raw_executor_output`), succeeded normally. **This answers round 6's
question of whether `input.extra` is read before or after rent
collection: whenever the contract's own code runs at all, rent
affordability has already been resolved upstream of it, so `input.extra`
reflects a post-rent-collection balance — but in the worst case the
contract may not get to run at all, and Invariant I17's own
`MIN_OPERATIONAL_SHELL` check never has a chance to fire either, since
that check is itself inside the contract's logic.** This is a sharper,
directly-observed instance of the residual risk already named in §5.8,
not a new, separate one — recorded here because it was found while
investigating this section's own rent hazard specifically, and because it
is the clearest evidence yet that `MAX_LEDGER_ENTRIES` and
`MIN_OPERATIONAL_SHELL` reduce exposure to this class of failure without
eliminating it.

**TOPUP's real-network viability is itself unverified — named explicitly
as its own residual risk, distinct from the rent questions above.** Every
`TOPUP_METHOD_ID` call this spike ever exercised was funded via
`avm-local-stand call --extra-currencies "2:N"`, a flag this same spike
also used to mint *ordinary, non-SHELL* currencies into test accounts out
of nothing for unrelated probes (§5.1's and this round's currency
round-trip test, among others) — nothing distinguishes it, on the local
stand, from a pure test-harness minting convenience. `ExternalMessage`
(`crates/avm-contract-sdk/src/input.rs` lines 211–225, previously cited in
§3's trust-assumption list via finding A9) carries no `AccountAddress`
source at all, only a bare `pubkey` — so there is no structural account
for a real network to debit the carried SHELL from. If a real network has
no actual mechanism for an external message to carry real SHELL value
(as opposed to the local stand's minting convenience), `TOPUP_METHOD_ID`
would be unusable there, and this vault would have no way to replenish
its own SHELL once genesis funding is exhausted. Given Invariant I17, the
consequence is a **permanent withdrawal outage, not a funds-theft or
funds-loss event**: ledger balances stay exactly intact, but no
`WITHDRAW` call could ever succeed again once the vault's SHELL falls
below `MIN_OPERATIONAL_SHELL`. This is recorded in `../SPEC.md` §10 as the
first and most important open maintainer question this specification
raises — more fundamental than the rent-formula question above, since an
unanswerable rent question is moot if the replenishment path it concerns
turns out not to exist on a real network at all.

**Relation to the retired `VERSION_METHOD_ID`:** an earlier design included
a permissionless, external, no-op version-probe method. It was removed
once `PLATFORM_FINDINGS.md` probe P1 established that every external call
costs the destination account SHELL to process, regardless of
authorization outcome — a permissionless, non-essential probe method is
therefore pure, unmitigated SHELL-drain exposure (anyone, at any time, for
free to themselves, can cost the vault SHELL) with no offsetting benefit,
directly working against the §5.8 hazard this specification otherwise
tries to defend against. This is recorded here, not only in `../SPEC.md`
§3, because it is a direct instance of this section's own threat
(unnecessary attack surface that drains the exact resource §5.8 depends
on) having already been found and removed once.

**Residual risk:** named explicitly in §5.8 above (rent-driven unbounded
SHELL requirement) and not independently resolvable from within this
contract's own logic — it is bounded by policy (the fixed cap) and by
heuristic (`MIN_OPERATIONAL_SHELL`), not by a provable ceiling.

### 5.10 Forged settlement via a chosen destination

**Attack, as it would apply to a design with a caller-chosen withdrawal
destination (which this contract does not have):** name an attacker-
controlled address as the destination of someone else's withdrawal, or
construct a withdrawal that appears to settle in the attacker's favor.

**Defense:** moot by construction, not by a runtime check — `../SPEC.md`
§1 and §4: there is no destination argument anywhere in `WITHDRAW`'s body.
Every withdrawal's payout goes to exactly the account that called
`WITHDRAW`, which is also exactly the account whose own ledger entry was
debited (Invariant I4; the payout-destination guarantee specifically is
Invariant I4d). An attacker cannot be named as the destination of
a withdrawal that is not their own, because no field exists through which
to name any destination at all. This closes an entire category of
forged-settlement attempts by construction, the same way §5.4's undeployed-
destination DoS is closed by construction rather than by a guard.

**Residual risk:** none from this vector specifically. The narrower,
still-open liveness questions this choice does *not* resolve — what
happens on the same-DApp-but-split route, and separately, what happens
when the vault's own SHELL is exhausted mid-chain — are tracked in §6 as
"L" routes 1 and 2, since both are liveness questions (does the
withdrawal ever resolve), not forged-settlement questions (can someone
else claim it) — those are different failure modes and this section
addresses only the second.

### 5.11 Admin-call replay

**Attack:** capture and resend a previously-successful, signed
`PROPOSE_OWNER`/`ACCEPT_OWNER`/`SET_PAUSE` call, attempting to re-trigger
its effect (e.g. re-pausing deposits after they were deliberately
unpaused, or re-proposing an owner rotation long after it was superseded).

**Defense:** Invariant I13 — every such call must supply the header's
current `admin_seq` value exactly, checked before the call has any other
effect; `admin_seq` advances by exactly one, via `checked_add`, on every
successful admin call. A byte-identical replay of an old call carries a
now-stale sequence value and fails with `stale_admin_sequence` (1524)
unconditionally, regardless of how much time has passed or how many other
admin calls have occurred since, and regardless of whether the underlying
platform message transport itself ever deduplicates the replay (§3's Q5
trust assumption is explicitly not load-bearing here, for exactly this
reason). `../SPEC.md` §8 test 10 covers both the single-replay case (replay
an old pause call after unpausing) and a longer rotation-chain case
(A→B→A, then replay A's very first, long-superseded propose-B call).

**Residual risk:** none identified — this is a self-contained, contract-
level guarantee that does not depend on any platform behavior being a
particular way.

### 5.12 A third "L" route: a large callback body from a malicious depositor contract could exhaust the guest arena before this contract's code even runs

**Found at Phase 4 (fresh-context adversarial review, finding 4).
Mechanism confirmed by reading `crates/avm-contract-sdk/src/runtime.rs`
directly; whether it is reachable on the real platform is NOT
confirmed, and is recorded here as open rather than asserted either
way, per Hard Rule 6.**

**Mechanism.** `GuestArena<N>` (`runtime.rs`) is a fixed-size,
bump-allocated arena holding exactly two live allocations at once: the
raw inbound message bytes (allocated by the host, via the exported
`alloc` function `export_message_contract!` wires up, *before*
`dispatch_message` is even called) and this contract's own output
(allocated by `write_output` *after* `process_message` runs). Both must
fit in `N` bytes simultaneously — `allocate()` returns `0` if
`input_len + output_len > N`, which `dispatch_message_inner` treats as
total failure (the function returns `0` with no further detail). This
contract declares `arena_bytes = 262_144` (`src/lib.rs`), deliberately
reduced from the SDK's `8 MiB + 16` default to comfortably cover this
contract's own worst-case *legitimate* traffic (24,359-byte state, §2.4)
with margin — a decision made before this specific threat was
considered.

A depositor who receives the vault's withdrawal payout controls a
`CallbackIntent`-bearing message and can set a non-empty
`callback_result_body` in their own response (`output.rs`'s
`validate_message_context` permits this whenever the inbound message
carries a callback request, which the vault's payout always does); that
body becomes the `body` of the CALLBACK message delivered back to the
vault. This contract's own `handle_callback` never reads that body (it
reads `status_code` from the callback header only, by design — §5), but
the body's *bytes* still have to be allocated and written into the
arena as part of the raw inbound message before `handle_callback` ever
runs, independent of whether the handler logic reads them. A
sufficiently large body — roughly `208,700`+ bytes, given this
contract's own 24,359-byte worst-case state and a minimal output — would
exhaust the 262,144-byte arena and make `handle_callback` never run at
all. Since the real platform never redelivers a callback it has already
sent once (the same exactly-once-delivery trust assumption §3 states for
every callback), a withdrawal whose resolving callback is dispatched
this way would be stranded exactly like the other two "L" routes below —
permanently pending, consuming one of only 32 global pending slots.

**Why this is recorded as open, not fixed or dismissed.** Resolving it
requires one fact not available anywhere in this repository's `docs/`:
whether the platform itself caps message body size (independent of the
deploy-time wasm-envelope limit, §1, and the `FullReplace` state-size
ceiling, §2.4 — neither of which is documented as applying to an
arbitrary message's body). If such a cap exists and is smaller than
~208,700 bytes, this mechanism is real but never reachable. The
mechanism itself cannot be exercised by a host-level unit test either:
`GuestArena::base_pointer` uses the real WASM linear-memory pointer
under `cfg(not(test))`, so `dispatch_message` itself cannot be driven
from a 64-bit host test at all — only `handle_callback` directly,
bypassing the arena entirely. Verifying this would require a real-stand
run with an actual oversized callback body from a malicious depositor
fixture, which this round's time did not go to. Raising `arena_bytes`
back toward the SDK default was considered and deliberately not done:
it would reverse a previous, deliberate, measured sizing decision on
the basis of an unconfirmed threat, trading a 32×-larger committed
memory footprint (with its own undocumented resource/fee implications)
for protection against a risk this repository cannot confirm is real.

**TASK N (Gate 3b) tested this directly on the real stand — the
mechanism did NOT reproduce, at any size tried, up to 10,000,000
bytes.** A malicious-depositor spike (`spike/actor-bigbody/`, never
committed) accepted the vault's real withdrawal payout and attached a
`callback_result_body` of increasing size (generated at runtime, not
baked into the actor's own wasm, so the actor's deploy size stays
constant regardless) to its own success response. Byte-decoded
before/after probes of the real, unmodified vault (not a spike)
confirmed the withdrawal's Settle callback resolved cleanly —
`next_withdrawal_id` advanced and `pending_count` returned to 0 — at
**every** size tested: 50,000 / 200,000 / 262,144 (the vault's own
declared `arena_bytes`) / 1,000,000 / 5,000,000 / 10,000,000 bytes. No
failure, no stuck pending record, at any size, including sizes **38×
larger** than the vault's declared 262,144-byte arena and far beyond
the originally estimated ~208,700-byte threshold.

**This contradicts the theoretical mechanism above — not because the
mechanism was misread, but because no test size large enough to
exercise it ever actually reached `handle_callback`.** The mechanism
was derived directly from reading `GuestArena`/`dispatch_message_inner`
in `avm-contract-sdk`'s own source (§5.12's earlier text, unedited
above, remains an accurate description of what that source appears to
say), and the exact same source-reading correctly predicted TASK O's
deploy-envelope combined-total finding elsewhere in this round. **TASK
N2 (below) explains the gap: on the local stand, a `callback_result_body`
above exactly 65,536 bytes is never delivered at all** (`docs/
PLATFORM_FINDINGS.md` P23) — silently dropped before this contract's
code ever runs, regardless of arena size — and 65,536 bytes is itself
well under a quarter of this contract's 262,144-byte arena. So every
size TASK N tried above 65,536 bytes tested a body that was already
reduced to 0 bytes long before the arena could ever be stressed by it;
the theoretical mechanism was never actually put to the test at a size
capable of triggering it, on this platform. What `arena_bytes`
constrains for an inbound callback message that genuinely IS large
enough to matter — on mainnet, or for a message shape other than this
one — remains the open question, not "why doesn't the reading match
observation."

**Residual risk: downgraded from "open, plausible" to "tested
extensively; not reproduced because no delivered body on the local
stand is ever large enough to exercise the mechanism."** No
fund-stranding risk was found within the tested range. See
`../SPEC.md`'s maintainer questions (`docs/MAINTAINER_QUESTIONS.md`
#8) for the specific, now-updated question this raises for the SDK
maintainers.

**TASK N2 (Gate 3b) found something TASK N never checked for: whether
the callback body actually ARRIVES, not just whether resolution
succeeds — and the answer is no, above exactly 65,536 bytes.** TASK N
confirmed the Settle resolves cleanly at every size tested, including
10,000,000 bytes, and concluded (accurately, as far as it tested) that
no arena-exhaustion crash occurs. It never inspected what
`handle_callback` actually received. TASK N2 added a one-line,
spike-only hook recording the delivered body's exact length (see
`docs/PLATFORM_FINDINGS.md` P23 for full method and data) and found a
razor-sharp, 100%-reproducible threshold: a body of exactly 65,536
bytes (2^16) arrives byte-for-byte; a body of 65,537 bytes arrives as
0 bytes — not truncated to 65,536, not an error, just silently
absent — while the call's own top-level status remains `"ok"`,
identical in both cases. **"Resolves cleanly" and "the body arrived"
are different claims, and this repository's own TASK N conclusion
conflated them.** This specific vault's `handle_callback` never reads
`callback_result_body`'s content (only `status_code`), so this does
not create a fund-theft or mis-crediting path here — but it means the
arena-exhaustion risk this section spent several paragraphs analyzing
was never the operative risk at all: a malicious depositor cannot
crash or stall the callback by attaching a huge body, but anything
that vault logic future-written to actually *read* that body would
silently receive nothing above 64 KiB, with no signal that anything
was dropped. See `docs/PLATFORM_FINDINGS.md` P23 for the full
sent-vs-delivered table and `docs/MAINTAINER_QUESTIONS.md` #8 for the
updated open question this raises.

### 5.13 A callback no longer requires the full stored ledger to be valid

**Background.** Gate 3d (TASK M5) measured the Settle/Restore
callback's own real fuel headroom at the true 252-row + 32-pending
worst case and found the decision rule this contract's cap choices had
relied on since the earliest dummy-fuel measurement (`REVIEW.md`,
TASK J onward) — that the callback must tolerate at least 2× its
already-characterized "current work" before failing — is
unsatisfiable on this contract's own real, measured per-row and
per-pending costs, for any combination of the two caps
(`docs/PLATFORM_FINDINGS.md` P25/P26). The rule was dropped as
unsatisfiable, not relaxed.

**The change.** Gate 3e (TASK F1/F2) replaced the callback dispatch
path with one that validates the header and the entire pending section
exactly as `parse()` always has, but no longer decodes or
re-validates the *ledger* section's own content (currency shape,
nonzero amounts, sort order) — only its declared byte length, already
covered by the same checked-arithmetic total-length check `parse()`
performs. **Stated plainly: the Settle/Restore step no longer checks
that each stored ledger row's bytes decode to a valid currency/amount
— only that the ledger section's total byte count matches the
header's own declared `ledger_count`. The very next `DEPOSIT` or
`WITHDRAW`, which still calls the full `parse()`, would catch any
corruption this gap let through.**

**Measured callback-leg numbers (bisected; fee = 2,000,000; `K` is
dummy-work fuel padding, bisected to within 25–2,000 units depending
on state size):**

| Config | State | Spare K (resolves) | Bracket width (resolves→traps) |
|---|---|---|---|
| Baseline, Settle | 1+1 | ≈15,775–16,150 | ≈375 |
| Baseline, Settle | 100+32 | 12,625 | 375 |
| Baseline, Settle | 252+32 | 7,600 | 25 |
| Baseline, Restore | 252+32 | 7,525 | 75 |
| Fast path, Settle | 1+1 | 15,000 | 2,000 |
| Fast path, Restore | 1+1 | 15,000 | 2,000 |
| Fast path, Settle | 252+32 | 13,000 | 1,000 |
| Fast path, Restore | 252+32 | 12,250 | 750 |

Fast path over baseline at the true 252+32 worst case: Settle
13,000/7,600 ≈ **1.71×**, Restore 12,250/7,525 ≈ **1.63×** — spare K
falls with row count for both paths, confirming per-row cost
(≈28 K/row) dominates over per-pending cost (≈24 K/pending) either
way; the fast path's own advantage is negligible at 1 row and grows
with row count, consistent with its design (it specifically removes
per-row ledger decode/validation cost). A same-session,
same-build-family re-measurement of the prior round's own 252+32
baseline found it ≈9% below that round's own figures (7,600/7,625 vs
8,350/8,440 for Settle) — reported as ordinary session-to-session
drift, not chased further, since it is far smaller than an earlier
round's own ≈26× measurement error and the *relative* conclusions
(row-count dominance, Restore costing more than Settle) held
unchanged across both measurements. Full bisection methodology and
every intermediate state size: `docs/PLATFORM_FINDINGS.md` P25/P26.

**Error precedence, stated explicitly.** A callback with a bad *shape*
(wrong caller, wrong magic tag, not an internal message, no callback
header at all) is rejected with `ERR_INVALID_CALLBACK` (1513) before
`fast_callback` ever runs — these checks are on the message itself,
not the state, and happen in `handle_callback_fast` ahead of any
state-validity check. A callback that passes those shape checks but
whose *state* bytes fail validation (bad header, malformed pending
section) returns `ERR_INVALID_STATE` (1502) from inside
`fast_callback`. An unknown withdrawal id (pending section is valid,
but no record matches) returns `ERR_WITHDRAWAL_NOT_FOUND` (1510),
also from inside `fast_callback`, after the full pending-section scan.
So the real precedence is **1513 (shape) before 1502/1510 (state
content)** — unchanged from the pre-fast-path ordering, just now
enforced by two separate functions instead of one.

**Contrast with `DEPOSIT`/`WITHDRAW`'s own failure mode — atomicity,
not just validation scope.** `DEPOSIT` and `WITHDRAW` are unchanged by
this round: both still call the full, unconditional `parse()` on
every invocation, and both can still fail on fuel exhaustion while
doing so. That failure is atomic by Invariant I6 — the AVM's own
message-processing model guarantees nothing is written to storage
unless the call completes and returns success, so a fuel failure
partway through leaves the stored state exactly as it was; no funds
move, nothing is stranded, and the caller can simply retry. A
callback's own failure mode is different in kind, not degree: a
callback is the *only* step that resolves a pending withdrawal, so if
it fails for any reason (fuel exhaustion included — this is exactly
what the Gate 3d/3e headroom measurements were bisecting), the
pending record is never removed and the funds it reserved are
strandable (§5.8 names this directly). The fast path's own narrower
validation scope does not change this contrast in either direction —
it exists specifically to keep the callback's own fuel margin
comfortable, so this exact permanent-stranding failure mode stays
rare in practice, not to introduce it.

**What this means for an adversary.** Nothing changes for an attacker
who cannot write to this contract's own stored state directly — every
path that can ever write a ledger row (`handle_deposit`'s
`credit_ledger`, `handle_withdraw`'s `debit_ledger`/`insert_pending`,
and now `fast_callback`'s own Restore-leg `credit`/insert) always
produces output that would itself pass `parse()`'s full validation;
this contract's own writer logic cannot produce a `parse()`-rejecting
ledger row. The gap only matters if the stored state were corrupted by
something **outside** this contract's own control (a platform-level
storage fault, a future code-upgrade path this spec does not cover,
or any other out-of-band mutation) **between** a `WITHDRAW` and its
own resolving callback. In that specific window: a Settle now succeeds
against a corrupted ledger (copying the corrupted bytes through
verbatim, never inspecting them), where the pre-Gate-3e logic would
have rejected the whole call with `ERR_INVALID_STATE` instead.
Restore's own binary search over a corrupted (non-sorted) ledger could
in principle land on the wrong row or insert a duplicate, rather than
reject outright — not independently tested at the byte level this
round (the host test suite confirms Settle's own behavior directly;
Restore's behavior under ledger-level corruption specifically, as
opposed to the already-tested pending-section corruption, was not
separately constructed).

**Why this is accepted rather than treated as a new vulnerability.**
The corruption this gap exposes is not reachable through this
contract's own message-processing logic at all — it requires a
mechanism this spec has no visibility into to begin with. The very
next `DEPOSIT` or `WITHDRAW` after such a callback, which still calls
the unconditional, full `parse()` exactly as before, catches the same
corruption and returns `ERR_INVALID_STATE` — so the practical
consequence of this gap is, at most, a one-call delay in detecting a
corruption class this contract cannot itself cause, not a new way for
an adversary operating only through this contract's own message
surface to cause or exploit one
(`tests::fast_path::corrupted_ledger_content_settle_succeeds_then_
next_op_rejects` demonstrates both halves of this directly).

**Residual risk:** whether a corrupted-but-not-fully-invalid ledger
could ever arise from a cause this specification has not considered,
and what a Restore specifically does when its own binary search
operates against such a ledger, remain open — logged here rather than
asserted away.

## 6. Unresolved residual risks — named, not solved

These are open questions this specification does not resolve, listed here
together so a reviewer does not have to hunt through every section above
to find them. Each is deliberately left open rather than papered over with
an unverified claim, per Hard Rule 6.

- **"L", route 1 of 2 — the same-DApp-but-split liveness case.**
  `PLATFORM_FINDINGS.md` Q3: whether a chargeable route that fails before
  the receiving contract's own WASM execution even starts still produces a
  callback at all is unresolved — the local stand's synchronous,
  same-partition delivery route never exercises this path, and every
  route this spike could actually construct was same-DApp and resolved
  synchronously. If such a route exists and never calls back, a
  withdrawal sent along it would remain pending forever, with no
  mechanism in this contract to recover it (`../SPEC.md` §2.3 states
  plainly that no reclaim mechanism exists for exactly this scenario, by
  deliberate choice, since a balance-based heuristic recovery was
  considered and rejected — §1's reasoning: the vault's balance for a
  currency is shared across every depositor's entries, so a bare balance
  check proves nothing about any one pending withdrawal specifically).
  This is the first of two liveness gaps this specification accepts
  as open rather than closing — see the next bullet for the second (a
  third, theorized route was tested directly and not reproduced — see
  the note later in this list).
- **"L", route 2 of 2 — a SHELL-starved dropped callback, distinct from
  the split-route case above.** Added in round 6, directly observed rather
  than hypothesized: §5.8's probe showed a withdraw-shaped chain can be
  *accepted and begin executing* and then have its own return callback
  never delivered, purely because the chain's total SHELL cost exceeds
  what the initiating account held — observed on two separate occasions
  with as much as a 5× difference in cost between nominally similar calls
  (`docs/PLATFORM_FINDINGS.md`, round-6 addendum). `MIN_OPERATIONAL_SHELL`
  (Invariant I17) narrows how often this can happen by requiring a wide
  margin above every measured cost before a withdrawal is even allowed to
  start, but it is explicitly a heuristic, not a proof (§5.8) — if the
  vault's real chain cost at the moment of a given withdrawal exceeds
  `MIN_OPERATIONAL_SHELL`'s margin (whether from ordinary variance or from
  accrued rent, §5.9), the withdrawal can still end up permanently pending
  via this route, with the exact same "no recovery mechanism" consequence
  as the split-route case: `../SPEC.md` §2.3's reasoning for not
  implementing a balance-based reclaim heuristic applies identically here,
  since this route's drop looks, from the contract's own state, exactly
  like the split-route's drop — there is no way for the contract to tell
  the two apart, and no way to safely recover either one without resolving
  the ambiguity the carried-value trust assumption (§3) already rules out
  relying on. **Both routes accumulate rather than resolve, and §5.4 now
  states the consequence explicitly: as few as 32 stuck pending records,
  from either route, permanently disable `WITHDRAW` for every depositor
  in the vault, not only the ones directly affected.**
- **Tested and NOT reproduced — a malicious depositor's oversized
  callback body exhausting the guest arena before this contract's code
  runs (§5.12).** Theorized at Phase 4 (fresh-context adversarial
  review, finding 4) directly from `avm-contract-sdk`'s own
  `GuestArena`/`dispatch_message` source, and initially recorded here as
  a third "L" route. **TASK N (Gate 3b) tested it directly on the real
  stand and found no failure at any size up to 10,000,000 bytes** — 38×
  the vault's own declared 262,144-byte arena. The withdrawal's Settle
  callback resolved cleanly every time, byte-decoded and confirmed. Not
  because the source-reading was wrong, but because (per the next
  bullet, P23) no `callback_result_body` above 65,536 bytes is ever
  actually delivered on the local stand, so no size TASK N tried ever
  reached a length capable of exercising the mechanism at all — see
  §5.12 for the full explanation. Kept here, demoted from an active "L"
  route to this note, so the investigation and its outcome are both on
  the record rather than the original theory being silently removed.
- **New finding, not a route — a `callback_result_body` above exactly
  65,536 bytes is silently dropped (not truncated, not an error), a
  risk TASK N's own test above did not check for.** `docs/
  PLATFORM_FINDINGS.md` P23: TASK N2 (Gate 3b) recorded the actual
  delivered body length (not just whether resolution succeeded) and
  found a razor-sharp threshold at 2^16 bytes. Does not create a
  fund-theft/mis-crediting risk in this specific vault, which never
  reads that body's content — but establishes that **no result in
  this repository about callback-body size (§5.12, P23, this bullet)
  has been tested beyond the local stand; mainnet behavior for any of
  it is not established** (`docs/MAINTAINER_QUESTIONS.md` #8).
- **Exactly-once delivery of inbound request messages.** §3's trust
  assumption: if the platform ever delivers a `DEPOSIT` or `WITHDRAW`
  request twice, this contract will process it twice (double-crediting or
  double-debiting), since it keeps no message-hash-based deduplication
  state (doing so would require unbounded memory, conflicting directly
  with the bounded-state requirement). This is a named consequence of an
  explicit trust assumption, verified by the simulator's "duplicated
  original request messages" branch (§8) to confirm it behaves exactly as
  predicted rather than in some unexpected third way, not something this
  contract defends against.
- **External message nonce-replay deduplication at the platform level.**
  `PLATFORM_FINDINGS.md` Q5, genuinely unresolved either way, but
  explicitly not load-bearing for this contract (§5.11) — `admin_seq`
  supplies an independent, contract-level guarantee regardless of the
  answer.
- **Atomicity as a mechanism, beyond the one route tested.** §3's final
  trust assumption: every "never debited"/"fully rolled back" claim in
  this document was verified as an outcome on the local, same-DApp,
  synchronous route, never proven as a mechanism that holds on every
  possible route this platform supports. §8's simulator "retain" branch
  demonstrates concretely what this contract's own accounting looks like
  if this assumption is ever wrong somewhere unreachable by this spike,
  and states plainly that no code path here would catch it.
- **`TOPUP_METHOD_ID`'s real-network viability — new in round 6, and the
  top-priority open question of this whole document.** §5.9: every
  `TOPUP` call this spike ever exercised was funded through
  `avm-local-stand call --extra-currencies`, a flag already known to be a
  pure local-stand minting convenience for non-SHELL currencies elsewhere
  in this spike, and `ExternalMessage` has no source account for a real
  network to debit the carried SHELL from (`crates/avm-contract-sdk/src/
  input.rs` lines 211–225). If a real network has no actual mechanism for
  this, `TOPUP_METHOD_ID` is unusable there, and the consequence (via
  Invariant I17) is a permanent withdrawal outage once genesis funding
  runs out — ledger balances remain exactly intact; funds are stuck, not
  stolen or lost. Recorded as the first maintainer question in
  `../SPEC.md` §10, ahead of the rent-formula question below, because an
  unanswerable rent question is moot if this one has no answer.
- **Mainnet storage-rent formula and long-arrears account behavior.**
  §5.9/§5.8: the local stand's measured rent rate and lazy-collection
  policy are not confirmed to match mainnet, and what happens to an
  account whose rent debt grows very large relative to its balance over a
  long period is not documented anywhere in this repository. Recorded as
  the second open maintainer question, in `../SPEC.md` §2.4, not resolved
  here. Round 7 adds a third sub-question in the same area: whether *any*
  touch resets mainnet's stated 30-day inactivity clock, or only a touch
  that results in a genuine charge or genuine guest execution — round 7's
  P9b observed a touch that failed outright at materialization with no
  charge at all (`docs/PLATFORM_FINDINGS.md` round-7 addendum), and it is
  unknown whether that kind of touch would count on a real network. A
  fourth sub-question, added at the start of implementation: this spike's
  rent probes all read accrued-but-uncollected debt via
  `avm-local-stand inspect`'s `storage_due_payment`/`storage_last_paid`
  fields — an explicitly local-stand debugging command — and nothing in
  this repository says whether a real network exposes any equivalent way
  for a keeper to read a live account's accrued rent debt before it
  becomes delinquent.
- **The demo owner fallback key's private-key secrecy.** §5.6: unverified,
  not confirmed safe, by construction of what a repository search can and
  cannot prove.

## 7. Method-id-0 safety — full citation

This contract pins every withdrawal payout to `method_id = 0`, `body = []`
(`../SPEC.md` §3). Whether this is actually safe to use — rather than
colliding with some reserved meaning the SDK gives to `method_id = 0` —
was verified by reading the exact source, not merely asserted.
`crates/avm-contract-sdk/src/input.rs`'s `decode_internal_message` function
contains this check:

```rust
if let Some(callback) = callback {
    if read_u64_at(header, 8)? == 0 && callback.status_code == 0
        || callback.status_code != 0 && !body.is_empty()
    {
        return Err(DecodeError::InvalidValue);
    }
}
```

`read_u64_at(header, 8)` reads the message's own `method_id` field. This
rejects `method_id == 0` **only when the message itself is a callback**
(`callback` is `Some(...)`) **and** that callback reports success
(`status_code == 0`). The withdrawal payout message this contract sends is
never itself a callback — it is an ordinary forward call that *requests*
one via a separate `CallbackIntent` — so its own `callback` field is always
`None` on the way out, and this check cannot apply to it. On the reply
side, every `withdrawal_callback_method_id(id)` value this contract uses is
tagged with `CALLBACK_MAGIC_TAG` in its upper 32 bits (`../SPEC.md` §3) and
therefore can never equal `0` either, so this restriction cannot bite there
either. This was established conclusively by reading the quoted source
directly — there was no need for an additional positive test on the real
stand, since the payout message path never has `callback.is_some()` in the
first place, so there is no behavior left to observe beyond what the
source already shows.

## 8. Phase 3 verification strategy: a model-based simulator with an adversarial fake environment

Beyond ordinary property/fuzz testing of random, valid operation sequences
checked against Invariants I1–I17 (including I4b/I4c/I4d) after every step, Phase 3 implements a
swappable fake "AVM environment" specifically to exercise behaviors this
threat model depends on that are properties of *this one local stand*,
empirically observed, not platform specifications proven to hold
everywhere — directly addressing the atomicity gap named in §3 and §6:

- **Refund vs. retain on destination failure, both branches.** This spike's
  own probes only ever observed "never debited in the first place" for a
  failing destination on the one route actually reachable. The simulator's
  fake environment must be able to produce *both* outcomes so this
  contract's logic is exercised against the branch it does not expect, not
  only the branch this spike happened to observe.
- **The "retain" branch must demonstrate the Invariant I1 break concretely,
  and the test must label it explicitly undefended, not "handled."** Under
  this branch (value stays credited to a destination that still reports
  failure), the test must assert `sum(ledger) + sum(pending)` for the
  affected currency comes to *exceed* the account's real balance —
  Invariant I1, genuinely broken — and the test's own documentation must
  state plainly that no code path in this contract catches this: the
  carried-value verification gate that would once have caught exactly this
  case was deliberately removed from this design, specifically because the
  real platform, on the one route this spike could reach, does not exhibit
  this behavior (§3, probe A8). This demonstration is included on purpose
  so the gap stays visible rather than silently assumed away.
- **Dropped callbacks** — a withdrawal's payout is requested and no
  callback of any kind ever arrives, modeling §6's "L" scenario directly.
  The simulator must confirm the affected withdrawal correctly remains
  pending forever with no incorrect auto-resolution, and — the part a
  single-withdrawal test cannot show — that this permanently-pending state
  does not corrupt or block correlation for any *other*, unrelated
  withdrawal's own callback handling, under randomized conditions with
  multiple concurrent withdrawals, some stuck and some resolving normally.
- **Duplicated callbacks** — the same callback, or a second callback
  referencing the same `withdrawal_id`, delivered twice. The simulator must
  confirm the second is absorbed as `withdrawal_not_found` (1510) once the
  first has already resolved and removed the pending record — never a
  double-credit or double-restore (Invariant I3, I7).
- **Duplicated original request messages** — a deposit or withdraw request
  delivered twice by the simulated environment. The simulator must confirm
  the outcome matches §6's named trust-assumption consequence exactly
  (double-crediting or double-debiting) — recorded as a known, predicted
  consequence of an explicit trust assumption, not discovered as a silent
  surprise during testing.
- **A failing callback handler.** The simulator must confirm this
  contract's own callback-handling logic never deliberately returns
  failure once the depositor-match check (Invariant I4b) passes, and must separately
  demonstrate concretely what would be lost if it ever did: a withdrawal
  whose resolution is thrown away with no second chance, exactly as probe
  A8 found for a callback handler that deliberately fails. This keeps the
  "never deliberately fail the handler" design rule enforced by a test, not
  merely documented in prose.

## 9. Summary table

| # | Attack | Section | Status |
|---|---|---|---|
| 1 | Unauthorized withdrawal | §5.1 | Closed — no field exists to name another depositor's entry |
| 2 | Forged/fabricated callback settlement | §5.2 | Closed — flag unforgeable, caller-matched, value never trusted |
| 3 | Ledger exhaustion (self-funded dust) | §5.3 | Accepted, permanent, documented limitation |
| 4 | Pending-slot exhaustion via undeployed addresses | §5.4 | Closed — attack cannot be expressed (no destination field) |
| 5 | Dust griefing via arbitrary-recipient transfer | §5.5 | Closed — TRANSFER removed entirely |
| 6 | Owner key compromise | §5.6 | Bounded — griefing/control only, never funds-theft |
| 7 | Reentrancy during payout delivery | §5.7 | Closed — checks-effects-interactions, confirmed live |
| 8 | Vault SHELL exhaustion stranding a withdrawal | §5.8 | Real trade-off, not fully mitigated — a free (anyone-can-trigger) SHELL drain costs the vault nothing to inflict but buys a free withdrawal outage until someone refills it; I17 only raises the drain's cost, it does not remove the outage; `MIN_OPERATIONAL_SHELL` is now a final, real-vault-measured value (`1,500,000`, §1, Phase-4-corrected for the true 252-row/32-pending worst case), no longer provisional — but remains a heuristic margin, not a proof, against rent-driven long-arrears growth (§5.9) |
| 9 | Rent-driven unbounded state growth | §5.9 | Bounded by policy/heuristic; mainnet formula unverified; can itself drive #8's drain for free via accrued rent |
| 10 | Forged settlement via chosen destination | §5.10 | Closed — moot by construction (no destination field) |
| 11 | Admin-call replay | §5.11 | Closed — admin_seq, platform-independent |
| L | Same-DApp-but-split liveness (route 1) | §6 | Open — unresolved, no recovery mechanism, by choice |
| L | SHELL-starved dropped callback (route 2, new in round 6) | §6 | Open — same no-recovery consequence as route 1, triggered by #8/#9 instead of a routing split |
| — | TOPUP real-network viability | §5.9, §6 | Open, top maintainer question — if unanswerable, #8's outage becomes permanent and unrefillable (funds stuck, not stolen) |
| — | Exactly-once inbound delivery | §6 | Open trust assumption, consequence named and simulated |
| — | External nonce-replay dedup | §6 | Open, not load-bearing |
| — | Atomicity beyond one route | §6 | Open trust assumption, "retain" branch demonstrates the gap |
| — | Mainnet rent formula / long arrears | §6 | Open maintainer question (second priority, after TOPUP viability) |
| — | Demo key secrecy | §6 | Unverified by construction |
