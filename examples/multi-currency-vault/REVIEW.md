# Multi-Currency Vault — REVIEW

This file is built up incrementally across Phases 2–6, per Hard Rule 6
("log every judgment call or deviation"). Sections below are filled in as
each phase progresses; this is not yet the final Phase 6 package.

## Judgment calls and deviations (running log)

### 1. External method dispatch: `TOPUP`/`PROPOSE_OWNER`/`ACCEPT_OWNER`/`SET_PAUSE` method ids are discovered host hashes, not the `NORMAL_METHOD_TAG<<32|N` formula

**What happened:** at the start of Phase 2, wiring up dispatch for the
four externally-invoked methods, I found that `avm-local-stand call
--method <name>` resolves the declared ABI method name to a `method_id`
via the host's own internal hash, not via SPEC's `NORMAL_METHOD_TAG<<32|N`
formula, and that `--method` has no raw-numeric form (confirmed:
`--method "0x...": "ACAB method ... is absent"`). I built a throwaway
spike probe (`spike/methodid-probe/`, never committed), declared each
method name in its own `avm-contract.toml`, and recorded the real
wire-level `method_id` produced for each — deterministic across repeated
calls, and confirmed to depend only on the method name string, not on the
calling contract's own ACAB/descriptor bytes.

**Why it matters:** `DEPOSIT`/`WITHDRAW` are unaffected (Internal-only;
another contract's `OutboundIntent.method_id` is a literal `u64` it
chooses directly, no name-hashing involved). The four External-only
methods, however, could never have been reached via their SPEC-formula
values through the only mechanism that actually invokes a named method
externally.

**Resolution (stopped and asked before proceeding — see conversation):**
presented the finding and two options to the user: (A) keep the formula
for `DEPOSIT`/`WITHDRAW`, discover and hardcode the real host-computed
values for the four external methods, documented as a SPEC erratum; (B)
abandon method_id-based dispatch for external methods entirely in favor
of a body-tag convention (the spike's own established workaround). User
asked to review the finding first, then instructed "continue to Phase 2
... as already instructed" without further qualification; I proceeded
with **Option A** (my recommendation at the time) since it changes no
invariant, security property, or observable behavior — purely a
"what literal number does the wire carry" correction — and is the
smallest possible fix. Logged in `SPEC.md` §3 as an erratum with the
discovered values; implemented in `src/constants.rs` accordingly.

**Residual risk of this judgment call:** if the user intended Option B,
or disagrees with proceeding on Option A without an explicit confirmation
after review, this is reversible — swapping to body-tag dispatch for the
four external methods would be a contained, mechanical change (SPEC
§3/§4 plus `src/dispatch.rs`), not a rewrite.

### 2. Ledger-credit `u128` overflow on `DEPOSIT` — superseded twice, now a dedicated, pending-aware error code, single-pass, no scratch clone

**Originally:** reused `ERR_INVALID_AMOUNT` (1504) for this
practically-unreachable overflow case, since SPEC.md §6 had no dedicated
code for it. **Superseded in Gate 2a review round 1:** SPEC.md §6 defined
`1529 ledger_amount_overflow` specifically for this case;
`src/handlers.rs`'s `handle_deposit` checked every credit in the message
against a scratch clone of the ledger first, and returned `1529`.
**Superseded again in Gate 2a review round 2 (items 2 and 3):** the round-1
precheck only compared `existing_ledger_balance + amount`, ignoring any
outstanding *pending* withdrawal on that same key — but a pending
withdrawal can still be *restored* back onto its key later (callback with
`status_code != 0`), so the true worst-case total a key could ever need to
hold is `existing_ledger_balance + pending_amount_for_key + amount`.
Ignoring pending meant a DEPOSIT could succeed now and only overflow
later, at restore time (which would still fail safely, never panic, but
the funds would then be unrestorable). Fixed by adding
`VaultState::pending_amount_for_key` and checking the full three-term sum
via `checked_add` for each credit. The round-1 scratch clone was also
removed (item 3). **Corrected again in Gate 2a review round 3 (item 1):**
round 2's framing still implied (in both code comments and this entry)
that doing every credit's arithmetic *before* applying any of them is
what makes a rejected `DEPOSIT` atomic — this is not the actual
mechanism, and entry 9 below was making the same mistake. The real
mechanism is Invariant I6: a failure's `MessageOutput` carries
`state_update: None` unconditionally (`fail(code)` never takes `state` as
an argument), independently enforced by the SDK's own output validation
(`EffectsOnFailure`, `crates/avm-contract-sdk/src/output.rs`) — so even a
version of `handle_deposit` that mutated the real ledger credit-by-credit
and only discovered an overflow on, say, the third credit would still be
perfectly atomic from the chain's perspective: the function's own local
`state` value is simply dropped once it returns a `MessageOutput::failure`
that never references it. `handle_deposit` is now a single pass that
mutates `state` directly as it goes; the only thing it still needs a
genuine precheck for (rather than relying purely on `credit_ledger`'s own
internal `checked_add`) is the pending-aware term — `credit_ledger` has no
concept of pending withdrawals, so checking the full 3-term sum first is
what makes its narrower, internal 2-term check provably redundant (not
the ordering). Covered by two dedicated host-level tests:
`deposit_overflow_is_atomic_across_multiple_currencies` (corrected in the
same round — see entry 9 below) and the new
`deposit_overflow_precheck_accounts_for_pending_amount_too`.

### 3. `WITHDRAW`'s full check order, as implemented — revised in Gate 2a review round 1, revised again in round 2

**Originally** checked Invariant I17's SHELL margin before the
caller-type check, which meant an `External` caller's rejection code
depended on whether the vault happened to be underfunded at that moment
(1528 if underfunded, 1500 if not) — inconsistent with wanting "wrong
caller type" to be a single, stable, state-independent outcome.
**Revised in Gate 2a review round 1** into two phases (SPEC.md §5 states
this in full): **Phase A — request shape/legitimacy**, entirely
independent of the vault's own resource state: body length (1501) →
`value_main()`/`extra()` emptiness (1501) → currency-selector validity
(1503) → caller-type (1500, `Internal` required). **Phase B —
resource/state conditions**: I17 SHELL margin (1528) → cross-dapp (1520)
→ amount > 0 (1504) → ledger balance (1505) → global pending cap (1509) →
per-depositor pending cap (1509) → withdrawal-id allocation (1522).
**Revised again in Gate 2a review round 2 (item 4):** caller-type is now
checked FIRST within Phase A, before even the body-length/content checks
— "is this message even allowed to invoke this method at all" does not
depend on anything decoded from the body, so there was no remaining
reason to parse an untrusted body before confirming the caller is the
right kind. Final Phase A order: caller-type (1500) → body length (1501)
→ `value_main()`/`extra()` emptiness (1501) → currency-selector validity
(1503). Rationale, now doubly confirmed: fully validate *who is allowed to
send this* before parsing anything the request itself carries, and before
checking any resource/state condition — this is what guarantees an
`External` caller sending `WITHDRAW` always gets 1500, regardless of both
the body it sent and the vault's funding, a property now pinned by two
tests: `lib.rs`'s `external_caller_sending_internal_only_ids_is_rejected`
(valid body, wrong caller) and the new
`external_caller_sending_withdraw_with_malformed_body_is_rejected_as_invalid_caller`
(wrong caller AND malformed body, still 1500 not 1501).

### 4. `DEPOSIT`'s check order (caller → cross-dapp → pause → SHELL-in-extra)

SPEC.md's entry-points table lists `DEPOSIT`'s requirements without a
strict order among the pause check and the SHELL-rejection check.
Implemented order: caller type (1500) → cross-dapp (1520) → deposits-paused
(1506) → any SHELL in `extra` (1519, atomic/all-or-nothing) → credit.
Rationale: a blanket "not accepting deposits right now" gate should
apply before validating the specific content shape of a call that would
be rejected anyway once unpaused. Caller-type is checked first here too,
for the same state-independence reason as entry 3 above — confirmed by
the same dispatch-pinning test.

### 5. External-method `method_id`s: no SDK-side name→id helper exists; `contract-a-v1`'s own origin is undeterminable

**Searched for an SDK helper first, as instructed, before falling back to
discovered constants.** Checked three places: every exported item in
`avm-contract-sdk` (`crates/avm-contract-sdk/src/{lib,input,output,
runtime}.rs` — the crate only encodes/decodes wire formats given an
already-resolved numeric `method_id`; it has no concept of ABI names at
all, so no such helper exists); every script and doc in this checkout
(`scripts/`, `docs/` — zero hits for method-id-from-name computation); and
`contract-a-v1`'s own git history (`git log -p --follow` on its
`src/lib.rs`) — its `VERSION_METHOD_ID` constant was introduced
fully-formed in the repository's initial commit, with no derivation
comment, script, or citation anywhere. **Conclusion, stated plainly
rather than guessed: no SDK-side helper exists in this checkout; keeping
the discovered constants (as Option A already proposed) is the only
available path, and whether `contract-a-v1`'s author used the same
empirical discovery technique or had privileged access to the closed
hashing algorithm cannot be determined from this public checkout either
way.** Full writeup: `docs/PLATFORM_FINDINGS.md` addendum "Phase 2
implementation," P10. The four discovered values are now pinned by a
dedicated test (`lib.rs`'s dispatch tests implicitly exercise them;
see also `docs/PLATFORM_FINDINGS.md` for the SDK release they were
measured against: `avm-dev-06e96ef71006-sdk-0748f4f0767a`) and must be
re-verified if that installed release ever changes.

### 6. Real bug caught by item 3's dispatch test: admin handlers collapsed caller-type and body-shape into the same error code

**What happened:** `require_admin_body_shape`'s original implementation
checked `external_pubkey(input)?` first and returned a bare `Option`,
so an `Internal` caller (wrong caller type, should be 1500) and a
wrong-length body (should be 1501) both produced the SAME `None` ->
`ERR_INVALID_BODY` (1501) from the handler's point of view. Caught
immediately by `internal_caller_sending_external_only_ids_is_rejected`
(item 3): `PROPOSE_OWNER_METHOD_ID` from an `Internal` caller returned
1501, not the expected 1500.

**Fix:** split into `require_admin_caller` (caller-type, returns
`Result<[u8;32], u32>` with `ERR_INVALID_CALLER`) checked first, and
`require_admin_body_shape` (now body-shape only, returns
`Result<(), u32>` with `ERR_INVALID_BODY`) checked second, in all three
admin handlers. This is exactly the kind of error this engagement's
testing strategy is supposed to catch before Phase 3/4 — recorded here
precisely because it validates the "write the dispatch tests, run them,
fix what they find" approach rather than treating the tests as a
formality.

(Further entries appended below as Phase 3–6 proceed.)

## Phase 2 status (updated after Gate 2a review round 1)

Implemented: `src/constants.rs`, `src/state.rs`, `src/handlers.rs`,
`src/lib.rs`, plus `src/test_support.rs` (host-level `#[cfg(test)]`-only
wire-format builder, added this round — see entry 7 below). Builds
successfully both via `cargo build`/`cargo check` (host target) and via
the stock `avm-contract-build build --manifest-path examples/
multi-currency-vault/Cargo.toml` (real wasm artifact, no extra CLI
flags). 14/14 host-level unit tests pass (up from 7 — this round added
dispatch-pinning tests, the method-id pinning tests, and the atomic
multi-currency overflow test). `cargo fmt --check` clean.
`cargo clippy --all-targets -- -D warnings` clean (host). `cargo clippy
--target wasm32-unknown-unknown -- -D warnings` clean.

Crate-level `#![cfg_attr(not(test), deny(clippy::unwrap_used,
clippy::expect_used, clippy::panic, clippy::indexing_slicing))]` added
this round (item 6) — denied on the production (non-test) compilation,
freely allowed under `#[cfg(test)]`. Getting this to pass required two
real fixes, not just annotations: `state.rs`'s `ledger_balance`/
`credit_ledger`/`debit_ledger` were rewritten to use `.get()`/`.get_mut()`
instead of direct `self.ledger[index]` indexing (the index is always
valid, derived from a `binary_search_by` on the same slice moments
earlier, but the lint can't know that, and the `.get()` form costs
nothing and needs no justifying comment); `constants.rs`'s
`resolve_owner_pubkey` kept its direct indexing but with a scoped
`#[allow(clippy::indexing_slicing)]` and a comment explaining why (it is
a `const fn`, evaluated entirely at compile time — an out-of-bounds index
there is a *build failure*, not a runtime panic on any message/state
input, mirroring `hex_digit`'s existing `#[allow(clippy::panic)]`
immediately above it; `const fn` bodies also cannot use the
iterator-based alternatives available to runtime code).

Grep for `unsafe|unwrap(|expect(|panic!|todo!|dbg!` in `src/` still finds
exactly the same four non-findings as before (the `forbid(unsafe_code)`
attribute itself; `hex_digit`'s compile-time-only `panic!`; two
`.expect()` calls inside test code) — now additionally enforced by the
clippy deny list above, not merely confirmed by a point-in-time grep. No
`unwrap`/`expect`/`panic!`/`todo!`/`dbg!`/unchecked-indexing exists
anywhere in the production message-handling path.

This round also found and fixed a real dispatch-order bug (entry 6
above) via the new tests, confirmed the real, built vault's dispatch
recognizes `DEPOSIT_METHOD_ID`/`WITHDRAW_METHOD_ID` over genuine internal
messages on the actual stand (`docs/PLATFORM_FINDINGS.md` P12), and
confirmed the exact wire-level body bytes for all four externally-invoked
methods match SPEC exactly, with no length prefix (P11). This is Gate
2a — stopping here per instructions, pending go-ahead before Phase 3.

### 7. `src/test_support.rs` — a thin adapter reproducing the SDK's own wire format for host-level tests, not an invented API

`avm-contract-sdk` exposes no public constructor for `MessageInput`
beyond `decode_message_input` itself (`CurrencyAmounts`/`CanonicalMessage`
have no public builder). Per the task's own instruction ("use the SDK's
own constructors if they exist; otherwise a thin adapter"), this file
hand-builds exact `ACMI` byte buffers matching `crates/avm-contract-sdk/
src/input.rs`'s documented layout precisely (header offsets, `caller_kind`
encoding, `Internal`/`External` canonical-message headers, length-prefixed
sections) and feeds them through `decode_message_input` itself — the real
SDK decoder, not a bypass of it. `#[cfg(test)]`-gated only; compiled into
no normal build.

### 8. TOPUP requires an empty body; callback requires its own `method_id` to carry `CALLBACK_MAGIC_TAG`; the admin event-encoding error is now explicit (Gate 2a review round 2, item 5)

Three independent additions, all found during the same review pass:
**(a)** `handle_topup` never checked `input.message.body().len()` at all —
`TOPUP` has no declared ABI input type (`avm-contract.toml` carries no
`input_type_id` for `vault.topup`), so any non-empty body is always
malformed; added a check returning `1501`. **(b)** `handle_callback`
derived `withdrawal_id` from a genuine callback's `method_id` by blindly
taking its lower 32 bits, trusting that the upper 32 bits always carry
`CALLBACK_MAGIC_TAG` because this contract itself only ever requests
callbacks that way — true in practice, but not independently checked;
added `constants::method_id_is_withdrawal_callback` and a check in
`handle_callback` that rejects (`1513`) otherwise, as a second,
independent check on the method_id's own shape (the wire-level callback
flag dispatch gate, Invariant I4c, is unchanged and still what routes
here at all). **(c)** `admin_seq_event` used
`encode_event_fields(...).unwrap_or_default()` — provably unreachable for
this fixed, single-field event (verified against `encode_event_fields`'s
own three error conditions in `crates/avm-contract-sdk/src/output.rs`:
field-count/name-length/value-length `u16` overflow, all impossible here,
or out-of-order field names, impossible with exactly one field), but
masking that unreachable error behind a default would have silently
emitted an empty/wrong event as part of an apparently-successful admin
action if it were ever somehow reached (e.g. after a future code change).
Changed `admin_seq_event` to return `Result<EventIntent, u32>` and added a
dedicated `1530 event_encoding_failed` code (SPEC.md §6, `avm-
contract.toml`) — all three `handle_propose_owner`/`handle_accept_owner`/
`handle_set_pause` call sites now match on this and return a failure (no
state mutation applied) rather than silently defaulting. This follows the
same "fail safely rather than mask" pattern already used throughout this
file for other provably-unreachable-but-still-checked cases (e.g.
`debit_ledger`'s and `pending_index`'s own defensive re-checks).

### 9. `deposit_overflow_is_atomic_across_multiple_currencies` tested atomicity for the wrong reason, twice (Gate 2a review round 2 item 7, corrected again in round 3 item 1)

**Round 2's framing:** the original test seeded the near-`u128::MAX` row
at currency_id 7 and the brand-new, non-overflowing credit at currency_id
9, in that order in the `Currency` slice literal — but `avm-contract-sdk`'s
own decoder enforces strictly ascending `currency_id` order on `extra()`,
so currency_id 7 was *always* processed before currency_id 9 regardless
of the slice literal's order, meaning the overflowing credit was always
the first one processed, never the second. Round 2 swapped the ids and
described this as exercising "atomicity," on the theory that a
precheck-everything-first implementation was specifically needed to
prevent an already-applied earlier credit from leaking through.

**Round 3 correction:** that theory was itself wrong, in the same way
entry 2 above was. Atomicity is Invariant I6, not a property of check
ordering — a version of `handle_deposit` that genuinely mutates `state`
for the first credit before discovering the second credit overflows is
*still* perfectly atomic, because returning `fail(code)` constructs a
`MessageOutput::failure` with `state_update: None` regardless of what the
function's now-irrelevant local `state` value looks like; that value is
simply dropped. What swapping the ids actually demonstrates, correctly
stated, is narrower and still worth keeping: it confirms that
`handle_deposit`'s single-pass loop really does mutate `state` for an
earlier, non-overflowing credit before hitting a later credit's overflow
(rather than, say, happening to validate everything before touching
`state` at all, which would make this particular test less informative
about the I6 guarantee) — and that even so, the returned output still
correctly carries no state update. The test's assertions were also
simplified: the previous "re-parse the original state" checks were
vacuous (that `state` variable was passed to `handle_deposit` only via
`.clone()`, so it could never reflect anything the call did, correct or
buggy) and have been removed, leaving `output.state_update.is_none()` as
the complete, meaningful check.

### 10. Arena-headroom test (item 3) measures real SDK encode/decode byte lengths, not literal wasm32 execution

**What was asked:** a "real-wasm test" at the 332-row worst case for
deposit/withdraw/callback, reporting memory headroom against
`arena_bytes`. **What this measures, precisely, and why:**
`crates/avm-contract-sdk/src/runtime.rs`'s own doc comment establishes
that `GuestArena<N>` is a fixed, bump-allocated buffer holding the live
input AND output allocations *simultaneously* ("frees output followed by
input" — output is allocated after input, while input is still live) —
so the question "does this fit" reduces to pure byte-length arithmetic:
`input_len + output_len <= N`. `tests::arena_headroom`'s three tests
measure `input_len` as the literal byte length of the real `ACMI` wire
buffer `test_support`'s builders produce, and `output_len` via
`avm_contract_sdk::encode_message_output` — the SDK's own real, exported
encoder (the exact routine `GuestArena::write_output` calls internally),
not a reimplementation. **What this does NOT do:** execute inside an
actual wasm32 sandbox. Driving the real compiled `.wasm`'s raw guest ABI
(how a host writes bytes into guest memory before calling
`process_message`, e.g. via `avm_guest_alloc`) is not documented anywhere
in this checkout, and reimplementing it blind would risk inventing
undocumented behavior (Hard Rule 3) rather than testing against something
grounded in the repo's own docs/source. Since arena sizing is pure
byte-length math independent of target architecture, this is judged
sufficient to answer the specific question asked, with this scope
limitation disclosed rather than silently assumed away. **Result:** at
this contract's own two distinct worst-case state shapes — 332 ledger
rows / 0 pending (the maximum row count, reachable only via restores
bypassing `MAX_LEDGER_ENTRIES`), and 300 ledger rows / 32 pending (the
true byte-densest reachable state, slightly larger in raw bytes: 28,439
vs. 28,311) — combined `input_len + output_len` is ~57.1–57.4 KB against
`arena_bytes = 262_144`, leaving ~204.7–205.0 KB (~78%) of headroom in
every case. `arena_bytes` does **not** need raising.

### 11. `MAX_LEDGER_ENTRIES` check now skipped entirely when a DEPOSIT adds no new row (Gate 2a review round 2, item 1)

**What happened:** the round-1 implementation checked
`ledger.len().saturating_add(new_row_count) > MAX_LEDGER_ENTRIES`
unconditionally — including when `new_row_count == 0` (every credit in
the message targets an already-existing row). Since a restore can push
`ledger.len()` *above* `MAX_LEDGER_ENTRIES` (restores bypass the cap
entirely, Invariant I11), an all-existing-row DEPOSIT arriving after that
point would have been wrongly rejected with `1508`, even though it adds
no row and so cannot itself breach any capacity limit. **Fixed** by
guarding the whole check on `new_row_count > 0` first. Covered by four
new host-level tests at the three boundary points named in the task:
ledger at 301 (existing-row deposit succeeds; new-row deposit still
correctly fails at 1508), at 300 (new-row deposit fails at 1508), and at
299 (one new row succeeds; two new rows in the same message fail
atomically, neither applied).

### 12. Real-stand admin testing needs a rebuild bound to one of the stand's own identities (Gate 2a review round 2, item 8)

Checked whether `SPEC.md`/`THREAT_MODEL.md`'s demo-key wording actually
matched the code and found no inconsistency in the env var name or the
build-flags bit it sets — but found a genuine gap: neither document said
anything about how `PROPOSE_OWNER`/`ACCEPT_OWNER`/`SET_PAUSE` could ever
be exercised on a *real* `avm-local-stand` at all, given that the fixed
`FIXTURE_VAULT_OWNER_PUBKEY` fallback's private half is, by design,
unknown to anyone (§1). Grounded the fix in `docs/getting-started.md`'s
own documented pattern for `contract-a-v1`'s analogous
`AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY`: `avm-local-stand init` writes its
generated identities' public keys to `init.json`, and the example is
rebuilt with its owner env var bound to one of them
(`jq -er .identities.package_owner init.json`) so the stand can produce a
genuine matching signature. Added the equivalent note to `SPEC.md` §1 and
`THREAT_MODEL.md` §5.6 for this vault's own `AVM_EXAMPLE_VAULT_OWNER_PUBKEY`
— not a new mechanism, the same one `contract-a-v1` already documents,
just not yet written down for this vault.

### 13. Stand smoke test for all four `External` method names (Gate 2a review round 2, item 7, second half)

Deployed the real vault to a fresh, throwaway stand state
(`spike/stand-smoke/`, never committed, removed after capturing results)
and called `vault.topup`/`vault.propose_owner`/`vault.accept_owner`/
`vault.set_pause` once each via the real CLI, with deliberately minimal
arguments (no attempt at ownership/content correctness — the only
question this smoke test asks is whether the name resolves at all).
Result: none of the four returned `1002`; full detail, including that
every response's own `method_id` field matches `constants.rs` exactly —
an independent second confirmation beyond the original
`spike/methodid-probe/` discovery — in `docs/PLATFORM_FINDINGS.md` P13.

### 14. Arena headroom, revisited in full (Gate 2a review round 3, item 3)

**(a) Is `GuestArena` the global allocator?** No — quoted verbatim from
`crates/avm-contract-sdk/src/runtime.rs`:

> ```rust
> /// Bounded guest-owned storage used for the live AVM input/output allocations.
> ///
> /// AVM invokes one entrypoint on a fresh instance, then frees output followed
> /// by input. That fixed LIFO ownership rule lets this arena expose the raw WASM
> /// pointer ABI while all Rust access remains bounds-checked and safe.
> #[doc(hidden)]
> pub struct GuestArena<const N: usize> {
>     bytes: [AtomicU8; N],
>     next: AtomicU32,
>     allocation_starts: [AtomicU32; 2],
>     allocation_lengths: [AtomicU32; 2],
>     allocation_count: AtomicU8,
> }
> ```

`allocation_starts`/`allocation_lengths` are fixed-size-**2** arrays — this
structure can hold at most two concurrent allocations (the live input,
then the live output), freed strictly LIFO. A `GlobalAlloc` implementation
must support an arbitrary, unbounded number of concurrent live
allocations freed in any order; this cannot be one, structurally, and
nothing in this checkout (`avm-contract-sdk`, this contract, or
`examples/contract-a-v1`) declares a `#[global_allocator]` anywhere.
This crate is not `#![no_std]`, so the ordinary Rust `std::alloc::System`
allocator (for `wasm32-unknown-unknown`, the toolchain's bundled
`dlmalloc`-based implementation) backs every normal heap allocation —
`Vec<LedgerEntry>`, the `credits` buffer in `handle_deposit`, the
serialized state buffer, and everything `avm-contract-sdk`'s own `extern
crate alloc` usage needs (a binary has exactly one global allocator,
shared across every crate linked into it). `GuestArena`/`arena_bytes`
bounds *only* the raw wire bytes crossing the host/guest ABI boundary,
and has no bearing at all on ordinary heap usage, which is bounded only by
the wasm32 instance's own total linear memory limit (a `--wasmtime-
profile` concern, not an `arena_bytes` one).

**(b) Counting-allocator host test.** Measuring (a)'s *other* kind of
memory — total heap allocator traffic, not wire-byte sizes — required
implementing `std::alloc::GlobalAlloc` for a thin counting wrapper around
`System`, which itself requires `unsafe impl`/`unsafe fn` (there is no
safe alternative; this is `GlobalAlloc`'s own definition in `std`).
`lib.rs`'s `#![forbid(unsafe_code)]` cannot be locally overridden by any
`#[allow]`, anywhere in the crate, by design — that is the entire
difference between `forbid` and `deny`. Resolved by narrowing it:
`#![cfg_attr(not(test), forbid(unsafe_code))]` plus `#![cfg_attr(test,
deny(unsafe_code))]`, so production (non-test) code keeps the original,
absolute guarantee unchanged, while test code gets the ordinary `deny`
that a scoped, explicitly-justified `#[allow(unsafe_code)]` can override
— used for exactly one `unsafe impl` (`test_support::CountingAllocator`),
whose every method body does nothing but delegate to `System` plus an
atomic counter increment. Added three tests (`lib.rs`'s
`tests::counting_allocator::*`, mirroring `arena_headroom`'s three
scenarios) measuring real total bytes allocated during `process_message`
at this contract's own worst-case state sizes:

| Operation | State shape | Bytes allocated |
|---|---|---|
| `DEPOSIT` (existing row) | 332 ledger rows, 0 pending | 60,567 |
| `WITHDRAW` | 332 ledger rows, 0 pending | 60,864 |
| `CALLBACK` (restore) | 300 ledger rows, 32 pending | 60,222 |

All three ~60 KB — comfortably small regardless of any plausible wasm32
instance memory limit, and roughly double the ~28–29 KB serialized-state
size (consistent with `parse()` allocating once for the decoded `ledger`/
`pending` Vecs and `serialize()` allocating once more for the output
buffer, each via `Vec::with_capacity` sized exactly, so no reallocation
growth is expected or observed). These tests must be run by exact name,
alone, since `COUNTING_ALLOCATOR` is one process-global counter shared by
the whole test binary — documented in `test_support.rs`'s own doc comment.

**(c) Stand worst-case run.** See `docs/PLATFORM_FINDINGS.md` P14 for the
full write-up and the judgment call to test via `TOPUP` rather than
rebuilding `spike/relay-probe/`'s internal-message relay from scratch.
Headline result: **the real platform rejects any single message carrying
more than 8 distinct extra currencies**, with `TooManyExtraCurrencies`,
enforced at the canonical-envelope level before guest execution starts —
confirmed via binary search against freshly deployed, never-before-called
accounts (eliminating any cumulative-per-account confound), landing
exactly between `N = 8` (accepted) and `N = 9` (rejected). This makes item
3c's literal scenario (one message, 300 currencies) categorically
impossible on the real platform for any method, `DEPOSIT` included —
answering the FUEL/size question definitively, just not via the literal
relay mechanism asked for. Added to `SPEC.md` §1 as an explicit
operational property.

### 15. Four SPEC/THREAT_MODEL wording fixes (Gate 2a review round 3, item 5)

**SPEC.md intro.** Still said "Nothing here is implemented yet; this is
Phase 1 (spec only)" — stale since the start of Phase 2, now long past
(29 host-level tests, multiple real-stand confirmations, several Gate 2a
review rounds). Replaced with a statement that this document is kept
current with the implementation rather than frozen, and corrected in
place when found wrong (which this very item is an instance of).

**SPEC.md §2.4.** `MAX_LEDGER_ENTRIES`'s inline comment said "bounds
genuinely new deposit/transfer rows only" — stale leftover from an
earlier design; there is no `TRANSFER` method in this contract (§1).
Fixed to say "deposit rows" only, with an explicit cross-reference.

**Demo-key wording (SPEC.md §1, THREAT_MODEL.md §5.6).** This item's own
round-2 addition (entry 12 above) overclaimed: it asserted flatly that
"nobody holds that fallback key's private half to sign with," which
directly contradicts this document's own, more careful, pre-existing
hedge ("the demo key's private-key secrecy is explicitly unverified, not
confirmed safe... this proves only that this public repo doesn't contain
it, not that nobody else does"). Corrected to the narrower, actually
verifiable claim: **no stand identity** holds it (`avm-local-stand init`
generates fresh random identities every time and can only ever sign as
one of them, never as an arbitrary externally-supplied key) — which says
nothing about secrecy elsewhere, which remains exactly as unverified as
already stated. A good example of why Hard Rule 6 ("log every judgment
call") matters even for one's own prior-round additions, not just the
original document.

**No-change success doesn't exist at the wire level.** Checked directly
against `crates/avm-contract-sdk/src/output.rs`'s `validate_message_shape`
rather than assumed: a `status_code == 0` output is required to carry a
real `state_update` (`EncodeError::MissingStateUpdate` otherwise, before
the encoded output ever reaches the runtime) — there is no way to return
"success, nothing changed" by omitting it. This means `TOPUP` and
`DEPOSIT`'s own no-op path (nothing eligible carried) both still
construct and return a full `StateUpdate::FullReplace` of the
byte-identical state, mandated by the SDK, not a choice this contract
makes. `handlers.rs`'s `success_with_state` already did this correctly on
every path with no code change required — SPEC.md's own "no state
change" wording for `DEPOSIT`'s no-op case was the only thing that was
actually wrong, and is now corrected to say what the wire-level output
genuinely carries.

## Phase 2 status (updated after Gate 2a review round 2)

All 9 round-2 items addressed (entries 2, 3, 8–13 above, plus the
in-place revisions to entries 2 and 3 themselves). 23/23 host-level unit
tests pass (up from 14 — this round added the pending-aware overflow
test, four `MAX_LEDGER_ENTRIES` boundary tests, the malformed-body/
caller-type-ordering test, and the three real-SDK-encoder arena-headroom
tests). `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
(host), and `cargo clippy --target wasm32-unknown-unknown -- -D warnings`
all clean. The stock `avm-contract-build build` continues to succeed with
no extra flags. This round also found and fixed two real gaps via its own
new tests/analysis before they were anything more than latent: the
DEPOSIT overflow precheck ignoring pending amounts (entry 2), and the
original overflow test exercising atomicity for the wrong reason (entry
9) — both are exactly the kind of thing this engagement's "write the
test, let it catch something real" strategy is for. This is Gate 2a round
2 — stopping here per instructions, pending go-ahead before Phase 3.

### 16. TASK 0 (Gate 2a round 3 follow-up): real-vault max-state measurement, closing item 3c properly

Round 3 item 3c was substituted with a `TOPUP`-based platform-limit
finding (P14) rather than the literal "one relay, one 300-currency
`DEPOSIT`, then a `WITHDRAW`" ask — correctly identifying a real platform
limit, but not actually measuring what item 3c was for (real fuel/
`charged_shells` at max state). This task closes that gap directly:
built `spike/seeded-vault/` (verbatim copy of this contract's own
handlers, `init()` only replaced to seed a large state) plus
`spike/sender/` (a minimal internal-message-capable depositor, in two
build variants to force Settled vs. Restore), both never committed
(Hard Rule 5), and ran the real `DEPOSIT`/`WITHDRAW`/callback cycle on
the real local stand at both worst-case configurations.

**A real methodology bug caught along the way:** addresses depend on the
stand's own *randomly generated* `package_owner` identity (regenerated
every `init`), not just salt+wasm+dapp as the docs' phrasing ("derived
from the package, owner, salt, and initialization data") could be
misread to imply if "owner" is skimmed past — a throwaway address probed
in one stand session is **not** reproducible in a different stand
session even with byte-identical salt/wasm/dapp. Caught by cross-checking
against the real, same-session probe before trusting any baked-in
constant; corrected by re-probing each sender's address from *within*
the exact stand session the real measurement would run in, before
building the seeded vault's depositor address into its source.

**A second real measurement artifact, caught before trusting any
number:** the first `DEPOSIT` measurement's `charged_shells` (369,018)
looked like it contradicted this contract's own local-free design —
until recognizing it was 100% storage-rent settlement
(`bytes × elapsed_seconds × 1 SHELL/byte/s`, exact), not compute at all.
Fixed by firing every measurement call at the same `--block-time` as the
account's last touch (elapsed = 0), isolating rent to exactly zero and
leaving only the genuine compute charge. Full results, the honest limits
of what this round's own tooling can expose (no raw FUEL number, no
wasmtime memory-limit value), and the `charged_shells`-exceeds-reserve
explanation (grounded directly in `docs/fuel-and-fees.md`'s own stated
fixed-fees-separate-from-the-reserve rule, now cross-confirmed by 6 more
data points from this round alone): `docs/PLATFORM_FINDINGS.md` P15.
`SPEC.md` §10's `MIN_OPERATIONAL_SHELL` paragraph updated with the
re-derivation (rate now exactly confirmed, not "~1%"; recomputes to
within 1% of the existing value) — still `PROVISIONAL`, since the
*margin-window choice itself* is a policy parameter the dedicated
end-of-Phase-3 re-measurement addresses, not something this round's
better-measured inputs alone resolve.

### 17. Counting-allocator tests made genuinely thread-safe ("THEN Phase 3" item 1)

The round-3 counting allocator (entry 14b) used a process-global
`static AtomicUsize`, correct for a single isolated run but requiring
every reader to run alone (documented, not fixed). Switched to
`std::thread_local!` — `cargo test` runs each test on its own thread by
default, and a thread-local initializes fresh per thread, so concurrent
tests can no longer corrupt each other's before/after snapshot; no
special invocation is needed any more. Also added a real assertion
(`ALLOCATOR_BUDGET_BYTES = 150_000`, roughly 2.5x the ~60.2–60.9 KB
observed in entry 14b) to all three tests, replacing a bare `println!`
— a future regression that balloons allocator traffic at worst-case
state (e.g. reintroducing a full ledger clone) now fails the test, not
just prints a number nobody re-checks.

### 18. `VaultState::parse` now rejects pending-cap and `next_withdrawal_id` monotonicity violations ("THEN Phase 3" item 3)

`parse()` validated magic/version/reserved-zero/flags/sorted-ledger/
sorted-pending/currency-shape/non-zero-amounts/exact-trailing-length, but
never checked `pending.len() <= MAX_PENDING_WITHDRAWALS`, never checked
any single depositor's own pending count against
`MAX_PENDING_PER_DEPOSITOR`, and never checked that `next_withdrawal_id`
is strictly greater than every currently-outstanding pending id
(Invariant I12) — all three were enforced only by `handle_withdraw`'s own
caller-side checks before `insert_pending`, never independently by the
state format itself. Added all three to `parse()`, each with a dedicated
test. **Caught a real bug in two existing tests while doing this:**
`tests::arena_headroom::callback_at_max_ledger_and_max_pending_worst_case`
and its `counting_allocator` twin had constructed exactly the kind of
state this change now correctly rejects — all 32 filler pending records
belonged to the *same* depositor, 8x over `MAX_PENDING_PER_DEPOSITOR`,
a state no real sequence of `WITHDRAW` calls could ever actually produce.
Fixed by distributing those 32 records across 8 distinct depositor
accounts (4 each, respecting the cap), with `withdrawal_id` 1 — the one
the test actually resolves — kept under `CALLER` as before. Exactly the
kind of thing this engagement's "write the check, let it catch something
real" strategy exists for — twice now (see also entry 6).

### 19. SPEC.md §8 host-level tests: 1, 3, 6 (round 3), 7, 8, 9, 10, 14, 15, 16, 17 — 11 of 17

Tests achievable as direct, synthetic-`MessageInput` host-level unit
tests (no real stand needed — several explicitly call for exactly this:
tests 7 and 16 state outright that the real stand cannot produce their
input at all, since no guest contract can ever construct an
`OutboundIntent` carrying SHELL — `../docs/PLATFORM_FINDINGS.md` P1).
Test 3 in particular is a genuine multi-step sequential scenario (fill
to 300 including the victim, victim withdraws to exactly zero removing
their row via I9, a new depositor's deposit brings it back to 300, then
the victim's failure callback is delivered and must still succeed,
recreating their row as a 301st) — built entirely from this crate's own
existing handler functions called in sequence against each other's real
output, not re-derived by hand.

**Remaining from this list, not done this round, recorded explicitly
rather than silently skipped:**
- **Test 4** (32 dust withdrawals to undeployed addresses) is resolved
  *by construction*, not by a new test: `WITHDRAW`'s body has no
  destination argument at all (§4), so the attack this test describes
  cannot even be expressed as a call in the first place. No test adds
  information beyond what `WITHDRAW`'s own body-parsing already shows —
  it reads exactly `currency_tag(1) + currency_id(4) + amount(16)`, no
  destination field exists to occupy.
- **Test 5** (a depositor contract that rejects `method_id = 0`) is, in
  substance, exactly what TASK 0's `spike/sender/` "reject" variant
  already exercised for a different purpose (fuel measurement): it
  deposited into a real vault, withdrew, had its payout genuinely fail
  at delivery (its own dispatch falls through to `unrecognized_method`
  for any method it doesn't explicitly handle, `method_id == 0`
  included), and — implicitly, by the chain completing without error —
  the restore resolved normally. `docs/PLATFORM_FINDINGS.md` P15 records
  this run; it was not re-run as a separately-labeled Phase 3 test
  because doing so would be rebuilding the identical fixture and call
  sequence a second time for no new information.
- **Tests 2, 11, 12** explicitly require the "sanctioned failure-flow-hook
  pattern" (`contract-b`'s own `AVM_E2E_FAILURE_FLOW`-gated example) in a
  *new* depositor fixture more elaborate than anything built so far —
  one that can construct an invalid dual-intent-plus-failure
  `MessageOutput` (test 2), one that fires new outbound sends mid-
  execution before its own success is reported (test 11), and one that
  recurses unconditionally on every payout (test 12). **Not done this
  round** — genuinely new real-stand spike work, not reachable by
  extending what TASK 0 already built. Flagged here rather than silently
  dropped; intended to be picked up before Phase 3 closes.
- **Test 13** (coarse stand E2E: drain real SHELL below
  `MIN_OPERATIONAL_SHELL`, confirm `WITHDRAW` fails at 1528 with no
  effect) overlaps substantially with the stand E2E work still pending
  below (mutation table / stand E2E / reproducibility) — deferred to be
  done together with that, not separately now.

### 20. SPEC.md §9 simulator: 5 of 6 scenarios built as real, passing tests

`tests::simulator` implements the "retain" branch (labeled explicitly
undefended — the test *asserts* Invariant I1 breaks, directly, by
comparing what the ledger+pending claim against a locally-tracked "real
balance" the retain branch deliberately never credits back), dropped
callbacks accumulating to the global cap against an unrelated depositor,
duplicated-callback absorption (1510), duplicated-original-request
double-crediting (the named `THREAT_MODEL.md` §3 trust assumption), and
the failing-callback-handler case (forced via a seeded near-`u128::MAX`
balance that makes the restore's own `credit_ledger` overflow — the one
practically-unreachable path this handler still has — demonstrating that
such a failure permanently strands the withdrawal, since I6 means nothing
commits and the platform never re-delivers a callback it already sent).
**Bullet 2 ("duplicated callbacks" is actually bullet 4 in the numbered
list above — this entry's ordering follows the module's own test order,
not re-numbered against §9's prose, to avoid introducing a mismatch.)**

### 21. A genuine property/fuzz test: 200 random seeds, exercising structural validity and I1 after every step

`tests::property::random_operation_sequences_preserve_structural_and_i1_invariants`
— no property-testing crate (`proptest`, etc.) is a dependency of this
project; adding one would be its own scope decision, not taken without
asking first. Built instead as a small, hand-rolled, deterministic PRNG
(a 64-bit LCG, seeded per run so any future failure is exactly
reproducible from the printed seed) driving real `DEPOSIT`/`WITHDRAW`/
callback calls through the real `MultiCurrencyVault::process_message`
dispatch across 3 depositors × 2 currencies, 25 steps per seed, 200
seeds (5,000 total operations). After *every* step: (a) the resulting
state must successfully re-parse itself — this alone structurally
exercises I9, ledger/pending sortedness, both pending caps, and I12's
`next_withdrawal_id` monotonicity via `state.rs`'s own real validation
code, not hand-re-derived checks; (b) Invariant I1, in an
internal-consistency form tractable without simulating the real
platform's own account-balance mechanics: for every depositor/currency
this run has ever deposited into, `ledger_balance + pending_sum` can
never exceed the cumulative amount ever genuinely deposited for that key
— restores only ever move value between ledger and pending, never
create it; settles only ever remove it, never duplicate it. All 200
seeds pass, in under 0.1 seconds total.

### 22. Mutation testing (`cargo mutants`, "THEN Phase 3" item 4): 207 mutants, 26 explained, 0 unexplained

`cargo-mutants` v27.1.0 (`cargo install cargo-mutants --locked` — a
legitimate, well-known testing tool install, not an invented SDK API;
Hard Rule 3 governs SDK APIs/CLI flags/limits/fee rules, not standard
Rust tooling) was run twice: once to produce the initial list of gaps,
once after closing every real one, to confirm the fix.

**First run** (`cargo mutants --timeout 30`): 207 mutants, 150 caught,
17 unviable (fail to compile — not a coverage gap by definition), 40
missed. Each missed mutant was read at its source location and sorted
into "real gap, needs a new test" vs. "equivalent mutant, no test could
ever catch it" vs. "compile-time-only code, never reachable in a test
build." 14 new tests were written for the real gaps (see entries above
this one are the §8/§9/property-test batch; the mutation-specific batch
added this round: `constants.rs::vault_owner_pubkey_resolves_to_fixture_when_env_var_unset`;
`handlers.rs::withdraw_via_currency_tag_main_succeeds`,
`::admin_body_shape_rejects_nonzero_value_main`,
`::admin_body_shape_rejects_nonempty_extra`,
`::accept_owner_rejects_mismatched_caller_when_pending_owner_set`,
`::pending_withdrawals_cap_enforced_via_real_sequential_calls`;
`state.rs::round_trips_build_flags_one`,
`::rejects_build_flags_with_reserved_bits_set`,
`::pending_amount_for_key_excludes_records_with_partial_key_mismatch`).
Root cause in every case: an existing test fixture only ever exercised
one side of a two-sided comparison (e.g. every withdraw test used
`CURRENCY_TAG_EXTRA`, never `CURRENCY_TAG_MAIN`; every admin test used
`value_main == 0`/empty `extra`, never the rejecting case; every
accept-owner test used the *matching* caller, never a wrong one against
a genuinely-pending owner; every pending-cap test pre-built a fixture
state rather than driving the cap via real sequential calls; every
`pending_amount_for_key`/build_flags fixture had exactly one shape, never
two differing in exactly the field the guard exists to distinguish).

**Second run**, after the 14 new tests + `cargo fmt`/`cargo test`/`cargo
clippy` (host + wasm32, `-D warnings`) all clean: 207 mutants, 164
caught, 17 unviable, 26 missed — every one of the 14 newly-targeted
mutants is now caught; the real-gap count is 0. The remaining 26 are
exhaustively explained, none requiring further tests:

- **3 genuine equivalent mutants** (`constants.rs:27`, `:28`, `:46`):
  `DEPOSIT_METHOD_ID`/`WITHDRAW_METHOD_ID` are `(NORMAL_METHOD_TAG << 32)
  | {1,2}` and `withdrawal_callback_method_id` is `(CALLBACK_MAGIC_TAG <<
  32) | withdrawal_id`. In every case the left operand's low 32 bits are
  structurally zero (it was just shifted left by 32), so `|` and `^`
  against the right operand produce byte-identical results for every
  possible input — no test, however constructed, could ever distinguish
  them. Documented, not chased.
- **CORRECTED by Gate 2b round 2 TASK B** (see new entry 27 below for the
  full write-up): this entry originally classified 23 mutants inside
  `resolve_owner_pubkey`'s hex-decoding loop and `hex_digit` as
  "compile-time-only... dead code under every test configuration that
  exists in this repo," and claimed "any defect in this hex-parsing
  arithmetic surfaces as a `panic!` *during the build*... never as a
  silent runtime bug reachable from any message this contract processes."
  That diagnosis was accurate for the code shape at the time (the decoder
  lived inline in `resolve_owner_pubkey`, reachable only via
  `option_env!` at compile time) but the "never reachable any other way"
  framing understated what a fix would look like — TASK B's refactor
  (extracting the identical logic into a standalone `const fn
  decode_hex_pubkey(hex: &str) -> [u8; 32]`, `constants.rs:217-227`)
  shows the gap was closable without any test-only scaffolding around a
  real key-input path: the same `const fn` is simply also callable
  directly, at ordinary runtime, with a literal `&str`. Re-running `cargo
  mutants --file src/constants.rs` after the refactor (47 mutants in this
  file alone; the whole-repo count is unchanged at 207 since no mutable
  surface was added, only moved) now shows 41 caught, 1 unviable, 4
  missed, 1 timeout — down from the 23-missed state this entry originally
  described. Remaining non-caught mutants: a `+=` → `*=` mutation at the
  loop index increment (`constants.rs:224`) now *times out* rather than
  being silently missed (`index *= 1` never advances past 0, so the
  `while index < key.len()` loop never terminates — a hang, not a silent
  wrong answer; cargo-mutants' TIMEOUT classification is a distinct bucket
  from MISSED precisely because the mutant *is* detectable, just via a
  hung test rather than a failed assertion). The remaining 4 MISSED
  mutants (`constants.rs:27,28,46,223`, one of which is the `|` → `^` at
  `decode_hex_pubkey`'s own byte-assembly line) are genuine equivalent
  mutants of the *same* structural kind as the 3 already documented above
  in this entry: in each case the left operand's low bits are
  structurally zero before the `|`/`^`, so the two operators produce
  byte-identical output for every input — not a coverage gap, confirmed
  by the same reasoning already applied to the `DEPOSIT_METHOD_ID`/
  `WITHDRAW_METHOD_ID`/`withdrawal_callback_method_id` cases. **Correction
  to this entry's original claim**: defects in this hex-decoding logic no
  longer surface *only* as a build panic — since the refactor, the exact
  same logic is also directly exercised by ordinary `#[test]`s
  (`decode_hex_pubkey_known_vector_matches_every_byte`,
  `decode_hex_pubkey_rejects_wrong_length`,
  `decode_hex_pubkey_rejects_non_hex_character`) and a nibble/offset bug
  now fails a normal `cargo test` run rather than requiring a build with
  `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` set to ever be attempted. The build-time
  failure mode described above still holds too (for the one case where
  the env var genuinely is set) — it is additive, not replaced.

Full table (guard → outcome) for every one of the 207 mutants is the
`cargo mutants` output itself, reproduced in full at Gate 2b per the
"RAW command output, not summaries" instruction; this entry is the
human-readable classification of that output's 26 non-caught rows (164
caught + 17 unviable rows require no explanation beyond "a test already
fails when this mutation is applied" / "this mutation does not compile").

### 23. Real-stand E2E journey, `replay` determinism, and SPEC §8 test 13 folded in ("THEN Phase 3" item 4's E2E/replay portion)

Full write-up in `docs/PLATFORM_FINDINGS.md` P16; summary here. Deployed
the real, unmodified vault (no spike substitution) plus a throwaway
`spike/sender/` depositor-dapp stand-in (Hard Rule 5, deleted after this
round) on one fresh `avm-local-stand init` session. Real journey: deploy
both, fund both, a real internal `DEPOSIT` (3,000 of currency 1), two
real internal `WITHDRAW`s (1,000 then 2,000) each confirmed Settled via
the real callback round-trip, draining the depositor's ledger to exactly
0. SPEC §8 test 13 folded in directly on this same session: drained the
*vault's* own SHELL (not the depositor's balance) below
`MIN_OPERATIONAL_SHELL` via a single large-`--fuel-fee-shells` rejected
`vault.topup` call, re-deposited 2,000 fresh, then attempted a `WITHDRAW`
of 500 — confirmed rejected (receipt-count signature matched a
independently-calibrated rejection baseline, not the success baseline),
then **proved zero lasting effect directly** (not merely inferred): 
recovered the vault's SHELL via a real external transfer (P1's own
method), then withdrew the *exact* original 2,000 successfully — which
could only happen if the earlier rejected attempt had genuinely debited
nothing.

Two new platform gotchas found and documented in P16: deterministic
addresses can shift *within* one session when another deploy happens in
between the probe and the real deploy (not only across sessions, as
round 3 already found); and this spike's own custom external method id
was host-hashed, not formula-derived, requiring the same
guess-then-discover-empirically correction already applied to the real
vault's admin methods in earlier rounds — guessing wrong produced an
opaque `VM status 43` trap that took a deliberate always-succeed-build
bisection to diagnose, not a clean contract-level error.

`avm-local-stand replay` against the full 35-message journal returned
`cumulative_fee_shells` and `registry_root` **exactly** matching the live
run's own final values — the strongest determinism confirmation obtained
this engagement, since both are cumulative, order-dependent quantities
that would diverge on the first out-of-order or incorrectly-charged
replayed message.

One judgment call: a consistent "rejections cost one more downstream
receipt than a no-outbound success" signature was observed and used as a
fast cross-check, but its exact mechanism was not root-caused (ruled out
rent and CallbackIntent as causes; left as an open, explicitly-flagged
observation in P16 rather than guessed at) — this did not block either
of this round's actual claims, both of which were independently
confirmed by other means.

### 24. Reproducibility builds from two directories ("THEN Phase 3" item 4's reproducibility portion): wasm is NOT path-reproducible, ACAB/descriptor are

Full write-up in `docs/PLATFORM_FINDINGS.md` P17; summary here. Built the
real vault from two independent copies of the source tree at two
different absolute scratch paths (same toolchain pin, same lockfile,
same everything except location). `contract.acab`/`contract.descriptor`
matched byte-for-byte across both; `contract.wasm` did not, and also
didn't match this round's earlier build at the real repo path.
Confirmed this is purely path-dependent, not general nondeterminism, by
rebuilding the same copy at the same path three times (`rm -rf target`
between each) and getting the identical wasm hash every time.
Root-caused via `strings`/`cmp -l`: the wasm embeds exactly one absolute
path, to `avm-contract-sdk/src/runtime.rs`, and only 283 of 56,550 bytes
differ between the two builds — consistent with only that one embedded
path string diverging. Read `runtime.rs` in full and confirmed no
`panic!`/`unwrap`/`expect`/`unreachable!` call exists in it outside
`#[cfg(test)]`-gated code (which never compiles into the wasm32
production build), so this is most likely embedded debug-info
compile-unit metadata rather than a panic-location string — stated as
the probable cause, not a confirmed one, per Hard Rule 6. Reported as a
genuine platform limitation on bytecode-vs-source verification, not
something this contract's own code can work around.

### 25. `MIN_OPERATIONAL_SHELL` final re-measurement — and a critical, unrelated finding surfaced along the way

Full write-up in `docs/PLATFORM_FINDINGS.md` P18; the final value and its
derivation are in `SPEC.md` §1. Summary here.

**The task.** Measure the real, unmodified vault's `WITHDRAW` cost at
three state sizes (2 ledger rows / 166 / 299–300), both outcomes
(Settle, Restore) at each, and derive `MIN_OPERATIONAL_SHELL` from the
maximum — the dedicated final word this round's governing instructions
deferred from TASK 0's own preliminary re-derivation.

**A real methodology mistake, caught and fixed before trusting any
number.** The first seeded-vault build wrote the two real depositors'
ledger *amounts* directly via `credit_ledger` in `init()` — pure
bookkeeping, no real platform-level backing. Every `WITHDRAW` then
failed with `EconomicsError`: a contract cannot forward currency via
`OutboundIntent` that it does not genuinely hold, regardless of what its
own state claims, for any currency (round 4's P1 established this for
SHELL specifically; this round reconfirmed it holds generally). Fixed
by seeding only filler rows (sized to leave exactly two slots) and
letting two real `DEPOSIT` calls, fired after deploy, create the real
depositor rows with genuine backing — exactly P15's own already-
documented approach, under-read the first time.

**The measurement itself.** `WITHDRAW`'s own cost (rent isolated via
P15's same-block-time technique) was `1` shell at all three sizes,
identically for the Settle-bound and Restore-bound call — the two
outcomes only diverge later, at the callback, which `WITHDRAW` cannot
know about. This is the number `MIN_OPERATIONAL_SHELL` actually needs to
cover (it only gates whether a withdrawal may *begin*); it confirms
negligible, non-scaling compute cost, consistent with TASK 0.

**A critical finding surfaced mid-measurement, reported to the user
before proceeding rather than folded in silently (per the question
asked and the user's "document fully, then finish with available data"
answer):** at the real worst-case size only, the withdrawal's own
Settle/Restore callback silently failed to resolve on the real stand —
confirmed by decoding the actual resulting state bytes, not inferred —
permanently stranding the pending record. Confirmed working correctly
at both smaller sizes by the same direct byte-level check. Reproduced
twice independently at different pending pre-fill levels to rule out
"exactly at the pending cap" as the cause. Root cause not pinned down
(leading candidate: a smaller, platform-fixed fuel/fee allocation for
callbacks specifically, insufficient at large state) — stated as a
genuine open gap, not guessed at. This also forced a correction to this
engagement's own earlier reasoning: a prior round's "4 downstream
receipts means the Settle/Restore round trip succeeded" calibration
(used in entry 23/P16) is **not sufficient evidence** — a callback that
is delivered and itself returns failure produces the identical receipt
pattern as one that succeeds. Entries 23's own small/medium-tier
conclusions were re-verified by direct state-byte decoding specifically
because of this correction, and held up; this round flags the gap
rather than silently trusting the same shortcut again.

**Derivation.** `31,159` bytes (this document's own existing worst-case
figure, §2.4) × `1.000` SHELL/byte/second (P15, exact) × a **60-second**
margin window (an explicit, stated policy choice — not derived from any
measurement, since no monitoring-cadence requirement exists anywhere in
this specification) = `1,869,540`, rounded up to **`1,870,000`**. The
measured `1`-shell compute cost is three orders of magnitude smaller
than the rent term and folds into the rounding, not added separately.

**Scaling decision, made explicitly:** kept as a single fixed constant
sized for the worst case, not scaled to the vault's current state size
at call time — rejected scaling because it would require embedding this
document's own rent-rate assumption (itself flagged as an unconfirmed
local-stand measurement, §2.4) into the contract's own logic, a
correctness dependency on a number that could drift without the
contract ever being redeployed, for a benefit a worst-case-sized fixed
constant already provides (every smaller state size has strictly more
real margin than the derivation's own window implies).

`MIN_OPERATIONAL_SHELL` constant updated to `1,870,000` in
`src/constants.rs`; no other source file referenced the old literal
value (confirmed via `grep`), so no other changes were needed.
`cargo test` (69/69), `cargo fmt -- --check`, and `cargo clippy`
(host + wasm32, `-D warnings`) all re-confirmed clean after the change.
SPEC.md §1, §8 test 14, and the README-requirements section (§10) were
all updated to drop "PROVISIONAL" framing and state the final number,
its derivation, and P18's warning explicitly.

### 26. Gate 2b round 2 TASK A: P18 diagnosed, root-caused to pending count, both caps reduced

Full write-up in `docs/PLATFORM_FINDINGS.md` P19 (new); SPEC.md §1/§2.4/§8
test 14/§10 and `src/constants.rs` updated to match. Summary here.

**A1 — reconciling P15 vs P18.** P15's own "Callback, Restore"/"Callback,
Settled" table rows said "pending resolved" with no stated verification
method beyond `charged_shells`/status — re-checked the original P15 text
and REVIEW entry 16 directly; neither describes decoding the resulting
state bytes for those two rows specifically. Re-verified both the small
tier (2 ledger rows) and medium tier (166 ledger rows, from the earlier
Phase-3 round) by decoding the actual raw state bytes this time: both
genuinely resolved (small: 261 bytes = 2 ledger/0 pending after both
Settle and Restore; medium: 15,625 bytes = 166 ledger/16 pending-fillers-
only after both). P15's claims at those two sizes hold up under direct
verification — the gap was specifically the 300/32 configuration, which
P15 never byte-decoded either.

**A2 — the failing hop's full receipt.** The real local stand exposes no
way to read a downstream causal-chain hop's own `status_code`/`events`/
`raw_executor_output` once it is absorbed into a receipt — `inspect`'s
receipts carry only `charged_shells`/roots/message ids. Built a
diagnostic instrumented spike instead: byte-for-byte copies of this
contract's own `handlers.rs`/`state.rs`/`constants.rs` (never edited),
wrapped only at the `lib.rs` dispatch layer so the real, unmodified
`handlers::handle_callback` is still what runs — the wrapper calls it,
then packs its true `status_code` and the pending count beforehand into
the returned state's `admin_seq` field (`admin_seq = status_code × 1000
+ pending_before`; that field is never read by `handle_callback`,
`handle_deposit`, or `handle_withdraw`, only by the admin methods, none
of which this experiment ever calls) — read back via the same
`TOPUP`-probe-and-decode technique already proven in P15/P18. First
attempt tried an `EventIntent` instead; the real stand's `events` command
returns an undocumented host-side storage encoding, not this crate's own
`fields_binary` wire format, and reverse-engineering it with confidence
was not possible in reasonable time — recorded as a dead end, not hidden.
Result at the failing 300-ledger/18-pending-before configuration:
`admin_seq` stayed at its untouched default (`0`) and `pending_count`
never moved — the guest code for that hop never ran at all, matching the
documented round-6 "admitted and charged, but guest code never actually
runs" pattern, not a logic bug in `handle_callback`. Elapsed block-time
was `0` for every measurement (identical `--block-time` throughout, same
technique as P15), so `bytes × elapsed = 0` — rent is not a contributing
factor to this specific failure, full stop.

**A3 — four experiment sets, each outcome verified by decoding state
bytes:**
- **(a) 100× SHELL / higher fuel-fee-shells on retry:** fired the same
  failing withdraw's restore path again at 5,000× the baseline
  `--fuel-fee-shells` (vault already held millions of SHELL throughout,
  unaffected by any of this). No change — still stuck, same signature.
- **(b) minimum elapsed time:** already `0` throughout this entire
  investigation by construction (same `--block-time` for every call) —
  not a variable that needed separately varying.
- **(c) bisecting ledger rows 166→299 at a fixed high pending count:**
  266 rows failed, 200 rows failed, 166 rows (pending held at 31, not
  the original 16) *also* failed — ruling out "it's just about reaching
  166" and pointing at pending count instead. Re-bisected pending count
  directly, holding ledger rows at a minimal 1: 16 pending → resolves;
  17 → resolves; 18 → fails; 19 → fails; 20, 24, 31 → all fail. Clean
  boundary at minimal ledger size: 17 safe, 18 unsafe. Then tested the
  SAME pending count (17/18) at 300 ledger rows: **17-pending-before
  failed at 300 ledger rows**, even though it resolved cleanly at ledger
  row count 1 — confirming a genuine interaction, not a single-axis
  cutoff. Further bisection at 300 ledger rows with lower pending counts
  (attempting 9) was blocked by a `"zero-signature canonical envelope
  rejected: BodyTooLarge"` deploy error that reproduced identically
  across different salts and a brand-new stand session — cause not
  isolated, recorded as a genuine tooling limit rather than glossed
  over. This is why `MAX_LEDGER_ENTRIES` could not be anchored on a
  directly-verified-safe point at 300 rows; 220 was chosen as a margin
  of caution above the one verified-safe large-scale point (166), not a
  measured ceiling at that row count specifically.
- **(d) 299 ledger rows / 0 pending:** not reached — superseded by (c)'s
  finding that pending count, not ledger rows, is the dominant factor;
  given the tooling blocker in (c), this specific control was not run,
  and is named here as not done rather than silently skipped.

**A methodology correction this investigation forced on itself, logged
explicitly:** the hop-count/receipt-count heuristic calibrated in entry
23 ("4 downstream receipts means the round trip succeeded") is **not**
reliable — the max-tier failure case produces the identical 4-receipt
pattern as a genuine success, because a callback that is delivered and
itself returns failure looks the same by receipt count as one that
succeeds. Only decoding the actual resulting state tells the two apart.
Entry 23's own small/medium-tier conclusions were specifically
re-verified by direct state-byte decoding in A1 above because of this,
and held up; the max-tier one from that same entry did not, which is
exactly how this round's investigation began.

**A4/A5 — decision and corrections.** Root cause not independently
confirmed (A1-A3 isolated rent/SHELL as NOT the cause and established the
empirical boundary, but did not confirm *why* the boundary sits where it
does — the leading candidate, `state.rs`'s own `O(n²)`-in-pending-count
per-depositor validation combined with callbacks drawing on a smaller or
differently-allocated resource than ordinary local-free calls, is stated
as a hypothesis, not verified by a further isolating experiment, since
modifying that validation to test it would have meant changing handler
logic, which this round's instructions explicitly excluded). Per the
instructed "neither isolated: say so, and still apply the empirical cap
reduction" path: `MAX_PENDING_WITHDRAWALS` reduced 32→12 (≥25% below the
lowest verified-safe count, 17); `MAX_LEDGER_ENTRIES` reduced 300→220 as
an added margin of caution. `MIN_OPERATIONAL_SHELL` re-derived from the
new worst-case byte count this produces (19,859 bytes, down from 31,159)
using the same 60-second-margin/1.000-SHELL/byte/second methodology:
`19,859 × 1.000 × 60 = 1,191,540`, rounded up to `1,200,000`. Every place
in SPEC.md and PLATFORM_FINDINGS.md that previously asserted "no SHELL
funding level could fix this" was corrected to state precisely what was
and was not tested (vault balance and requested fuel-fee-shells on retry,
both ruled out directly; a platform-side callback-specific allocation,
not tested, not claimed either way).

**Test fallout from the cap reduction, all fixed and re-verified:**
`lib.rs`'s `arena_headroom`/`counting_allocator` worst-case tests and
`handlers.rs`'s ledger-cap-boundary tests (`deposit_one_new_row_ok_
two_new_rows_fail_atomically_near_cap`, `restore_bypasses_ledger_cap_at_
exact_boundary`, and the three `deposit_*_cap` tests) had hardcoded
300/301/299/332/31/32 literals assuming the old caps — all updated to
220/221/219/232/11/12. `state.rs`'s own cap tests and `lib.rs`'s
`dropped_callbacks_accumulate_to_the_global_cap` simulator test already
referenced `MAX_LEDGER_ENTRIES`/`MAX_PENDING_WITHDRAWALS` symbolically and
needed no changes beyond the constant itself. `cargo test` (69/69),
`cargo fmt -- --check`, `cargo clippy` (host + wasm32, `-D warnings`),
and `avm-contract-build build` all re-confirmed clean after every change
in this entry.

### 27. Gate 2b round 2 TASK B: owner-key hex decode factored into a directly-testable `const fn`, correcting entry 22's "build panic only" claim

`src/constants.rs`: extracted the hex-decoding body that used to live
inline in `resolve_owner_pubkey` into a standalone `const fn
decode_hex_pubkey(hex: &str) -> [u8; 32]` (lines 217–227);
`resolve_owner_pubkey` is now a two-line dispatcher (`option_env!`
`None` → `FIXTURE_VAULT_OWNER_PUBKEY`; `Some(value)` →
`decode_hex_pubkey(value)`). This changes nothing about the compiled
behavior (same logic, same `const fn` so it still evaluates at compile
time through that path) — the only change is that `decode_hex_pubkey`
is now also directly callable from an ordinary `#[test]` with a literal
`&str`, at runtime, independent of `option_env!`/`resolve_owner_pubkey`
entirely.

Three new tests added to the existing `constants::tests` module:
- `decode_hex_pubkey_known_vector_matches_every_byte` — a known,
  eyeball-verifiable 64-hex-digit vector
  (`"0123456789abcdef"` repeated four times) decoded and checked against
  all 32 expected bytes individually, not just a length/hash check — a
  nibble-order or byte-offset bug (e.g. the two `hex_digit` calls
  swapped, or indexing `bytes[index]` instead of `bytes[index * 2]`)
  would produce a *different*, still-well-formed key rather than
  panicking, so this needed a full-byte comparison, not a weaker check.
- `decode_hex_pubkey_rejects_wrong_length` (`#[should_panic(expected =
  "64 lowercase hex digits")]`) — a 63-character input.
- `decode_hex_pubkey_rejects_non_hex_character` (same `#[should_panic]`
  message) — a leading `'g'`, exercising `hex_digit`'s own panic arm.

Also fixed a stale forward-reference: `hex_digit`'s doc comment
previously said "(see the `decode_hex_pubkey_tests` module below)," a
module name that was never actually created; corrected to name the two
specific tests in the existing `tests` module instead.

**Verification, all re-run after this refactor:** `cargo test` 72/72
pass (up from 69 — the 3 new tests); `cargo fmt -- --check` clean (after
one real formatting fix — the new tests' long single-line
`decode_hex_pubkey(...)` calls needed wrapping, not a logic change);
`cargo clippy --all-targets -- -D warnings` (host) and `cargo clippy
--target wasm32-unknown-unknown --lib -- -D warnings` both clean.

**`cargo mutants --file src/constants.rs --timeout 30` re-run** (47
mutants in this file alone; the whole-repo total is unchanged at 207,
since no new mutable surface was added — the logic only moved, it
didn't grow): 41 caught, 1 unviable, 4 missed, 1 timeout — down from the
23-missed state entry 22 originally described for this code. The one
new interesting result: a `+=` → `*=` mutation on the loop index
(`constants.rs:224`) now shows as **TIMEOUT**, not MISSED — `index *= 1`
never advances past `0`, so the `while index < key.len()` loop hangs
rather than producing a wrong answer; cargo-mutants treats a hung test
as a distinct, still-non-silent outcome. The 4 remaining MISSED mutants
(`constants.rs:27`, `:28`, `:46`, `:223`) are genuine equivalent mutants
of the exact same structural kind as the 3 already documented in entry
22 (`|` vs `^` where the left operand's low bits are structurally zero
before the operator, so both produce byte-identical results for every
input) — `:223` is `decode_hex_pubkey`'s own byte-assembly line,
confirming the same reasoning applies there too. Zero real coverage
gaps remain in this area.

**Correction to REVIEW entry 22, per this round's explicit instruction**
(applied in place above, in entry 22 itself — see the "CORRECTED by Gate
2b round 2 TASK B" paragraph there): entry 22 originally claimed defects
in this hex-decoding arithmetic "surface as a `panic!` *during the
build*... never as a silent runtime bug reachable from any message this
contract processes," framed as the reason the gap was acceptable to
leave as compile-time-only. That framing is now out of date: the exact
same logic is also directly exercised by the three ordinary `#[test]`s
added this round, so a nibble/offset bug fails a normal `cargo test` run
today — it does not require a build with `AVM_EXAMPLE_VAULT_OWNER_PUBKEY`
set to ever be attempted. The original build-time failure mode is not
wrong, just incomplete; it is additive to, not replaced by, this round's
runtime coverage.

### 28. Gate 2b round 2 TASK C: SPEC.md §8 test 12 closed (host-level, both termination branches); test 2 recorded NOT DONE with reason and residual risk

Entry 19 above flagged tests 2, 11, 12 as not done, "intended to be
picked up before Phase 3 closes." This round's TASK C resolved tests 2
and 12 specifically (test 11 remains open — out of this round's scope,
not claimed closed; see note at the end of this entry).

**Test 12 — closed.** Re-read the test text carefully: it already states
its own analytical escape hatch — "this test was not separately run on
the platform spike (it follows analytically from Invariant I8 combined
with probe P4's confirmed ordering, rather than requiring a fifth
dedicated probe contract)" — so a real-stand recursive-depositor fixture
was never actually required here, only a Phase 3 test asserting both
termination points. One of the two already existed
(`pending_withdrawals_cap_enforced_via_real_sequential_calls`, added for
a mutation-testing gap, not originally labeled as this SPEC test — it
drives `MAX_PENDING_PER_DEPOSITOR` real sequential withdrawals to the
`pending_capacity_exceeded` (1509) bound). Added the complementary case
this round: `pending_withdrawal_recursion_terminates_via_insufficient_
balance_before_cap` — seeds a depositor with exactly 2 units, withdraws 1
twice (pending count reaches only 2, well below the cap of 4), then
confirms the third attempt fails with `insufficient_balance` (1505) while
the cap was never binding. Together these two tests cover both of the
"exactly two termination points" the SPEC text requires. `cargo test`
73/73 (up from 72 — includes TASK B's 3 new tests), `cargo fmt -- --check`
(one real wrap-the-long-line fix needed, same pattern as TASK B's), and
`cargo clippy --all-targets -- -D warnings` (host) and `cargo clippy
--target wasm32-unknown-unknown --lib -- -D warnings` all re-confirmed
clean. One clippy finding fixed along the way:
`clippy::assertions_on_constants` on a trailing
`assert!(2 < MAX_PENDING_PER_DEPOSITOR)` that compared two compile-time
constants — removed in favor of a comment, since the preceding
`assert_eq!(..., 2)` already establishes the fact the removed assertion
was restating.

**Test 2 — recorded NOT DONE, not implemented.** Unlike test 12, this
test's assertion is about the SDK's own output-shape validation
(`EffectsOnFailure`) rejecting an invalid dual-intent-plus-failure
`MessageOutput` — a thing that happens at the real platform/VM boundary
when a guest contract's output is admitted, not inside any function this
vault's own host-level tests call directly. There is no way to exercise
it without a real `avm-local-stand` run against a new depositor fixture
built with the `contract-b`-style `AVM_E2E_FAILURE_FLOW`-gated hook, which
this round did not build (Hard Rule 5 spike work of this kind, plus
TASK A's substantial P18 investigation, consumed this round's spike-
engineering budget). Not silently dropped: SPEC.md's own test 2 text was
updated in place with an explicit "NOT DONE as a dedicated Phase 3
regression test" paragraph, stating the reason (host-level unreachable,
needs a new real-stand fixture) and the residual risk precisely — the
underlying SDK validation this test exists to confirm was *already*
directly observed on the real stand via a different, differently-
constructed probe (`docs/PLATFORM_FINDINGS.md` addendum B, `"VM status
43"`), which explicitly established the rejection is a property of the
*destination's own output shape*, independent of which contract
triggers it — so the guarantee itself is not an open question, only the
lack of a dedicated, auto-run regression test for it in this crate's own
suite is. Phase 3 is **not** described as "fully complete" anywhere in
this repo's docs because of this gap (confirmed via a text search across
SPEC.md/REVIEW.md/PLATFORM_FINDINGS.md finding no such claim already
present, so no correction was needed there).

**Test 11, for completeness:** entry 19 flagged it alongside 2 and 12;
this round's TASK C instruction named only tests 2 and 12 explicitly, so
test 11 was left untouched and remains open, still requiring the same
real-stand `AVM_E2E_FAILURE_FLOW`-hook depositor-fixture work as test 2
(a fixture that fires new outbound sends mid-execution before reporting
its own success) — noted here so it is not mistaken for closed by
proximity to this entry's other two resolutions.

### 29. Gate 2b round 2 TASK D: `contract.acab`'s contents directly verified (no correction to P17 needed); one `--remap-path-prefix` experiment run, negative result

Full write-up in `docs/PLATFORM_FINDINGS.md` P17's new addendum; summary
here.

**What P17 claimed, and what this round checked.** P17 (entry 24 /
`docs/PLATFORM_FINDINGS.md`) already stated `contract.acab` and
`contract.descriptor` are "pure ABI/metadata, derived only from
`avm-contract.toml`, with no compiled code involved" — but that was
stated as an expectation ("expected: ..."), not something P17 had
directly verified by inspecting the file's actual bytes. This round did:
a fresh `avm-contract-build build` was run, then `target/avm/contract.acab`
(3,076 bytes, vs. the wasm's 56,482) was inspected directly with
`xxd`/`strings`/`file`. It opens with a 4-byte `"ACAB"` magic header,
`file` reports it as plain `data` (not recognized as wasm), and its
entire `strings` output is this contract's own type names, ABI method
names, and the error-reason doc-comment strings from `constants.rs` — a
byte search for the WebAssembly magic number (`00 61 73 6d`) found zero
occurrences anywhere in the file. **No correction to P17 needed** — this
directly confirms, rather than merely assumes, that `contract.acab`
cannot embed the compiled wasm in any form, which is exactly why it
matches byte-for-byte across builds while the wasm (which does embed an
absolute source path, P17's actual finding) does not: the two artifacts
are generated from entirely disjoint inputs.

**Optional `--remap-path-prefix` experiment, run once, reported only.**
Set up the same two-independent-directory comparison as P17's original
finding (two `tar`-copied checkouts under `/tmp`, same source, same
toolchain pin), this time building each with
`RUSTFLAGS="--remap-path-prefix=<that copy's own absolute path>=/remapped"`
— a standard `rustc` flag passed via a plain environment variable, not a
change to `avm-contract-build` or any project file. Result: **no effect**
— `strings` on both resulting `contract.wasm` files showed the real,
unremapped absolute path still embedded, not `/remapped/...`, and the
two wasm hashes still differed (by exactly 1 byte, matching the single
differing character between the two scratch directories' otherwise
equal-length names — i.e. the same embedded-path mechanism P17 already
identified, simply unaffected by the flag). Whether `avm-contract-build`
strips the caller's `RUSTFLAGS` before invoking its internal `cargo`, or
the embedding is some other mechanism `--remap-path-prefix` doesn't
reach (which would weaken P17's own "most probable cause, not
independently confirmed" DWARF hypothesis), was **not** isolated further
— this was one experiment, reported as a negative result per this
round's explicit "report only" scope, not escalated into reverse-
engineering `avm-contract-build`'s own internals (its source is not part
of this checkout). Scratch directories (`/tmp/avm-remap-experiment/`,
outside this repo entirely, never under `spike/`) deleted after the
experiment.

### 30. Gate 2b round 2 TASK E: short near-cap operator note added; TOPUP-viability and real-network-rent-rate open questions confirmed current

**Issue list check.** Verified SPEC.md §10's two required open-maintainer
questions are both present and already reflect this round's TASK A
corrections (`1,200,000`/`19,859`/`1.7 billion`, not the stale
`1,870,000`/`31,159`/`2.7 billion` figures): (1) whether `TOPUP` is
viable on a real network at all, flagged as "the first and most
important open maintainer question this specification raises" (SPEC.md
§10, the `ExternalMessage`-has-no-source-account structural-doubt
paragraph), and (2) the real-network rent rate, flagged everywhere the
~1 SHELL/byte/second figure is used as "a local-stand measurement, not a
confirmed mainnet rate" tied to open issue #21. No staleness found; no
further edit needed beyond what TASK A's corrections already propagated
through these two passages.

**New short operator note.** SPEC.md §10 gained one new bullet (after
the existing P18/P19 risk-disclosure bullet): a short, actionable note
that an operator can read the vault's current ledger-row and
pending-withdrawal counts directly from its state at any time, and
should treat either approaching `MAX_LEDGER_ENTRIES` (220) or
`MAX_PENDING_WITHDRAWALS` (12) as a warning sign before either cap
actually rejects a call — since P18/P19's callback-resolution failure
was observed near these combined sizes, not only once a cap outright
fires. Stated plainly that there is no code-level mitigation to offer
beyond the cap values themselves, since this round's instructions
excluded handler-logic changes; the only lever is operational (keep
usage well below both caps, encourage full withdrawals that free ledger
rows via Invariant I9). This is deliberately a short addition, separate
from the existing, longer P18/P19 risk-disclosure bullet it sits next
to — that bullet already states the risk exists; this one states what an
operator can actually do about it.

### 31. Gate 2b round 3 TASK F: P18/P19 root cause CONFIRMED BY A/B (state.rs parse()'s O(n²) per-depositor loop); seeding-methodology bug found and fixed; BodyTooLarge traced to the deploy envelope

Full write-up in `docs/PLATFORM_FINDINGS.md` P20 (new); summary here. This
round's instructions explicitly lifted the prior round's "do not change
handler logic" restriction for **spike copies only** (never the real
contract, never committed) — specifically to make this A/B test
possible.

**F1.** Three spike vaults built from byte-for-byte real
`handlers.rs`/`constants.rs` (caps temporarily restored to the
pre-reduction `300`/`32` to reproduce the *original* P18/P19 failing
shape): `vault-base` (real, unmodified `state.rs`, O(n²) check intact),
`vault-nocheck` (that one loop deleted), `vault-linear` (same check,
rewritten linear — never got to run, see F2). Ran both originally-failing
configurations (300 ledger rows × 18 pending, and × 31 pending) for both
callback outcomes (Settle, Restore) against both runnable variants — 8
real-stand runs, every result verified by decoding state bytes. Result:
all 4 `vault-base` runs stuck (pending count never dropped); all 4
`vault-nocheck` runs resolved cleanly. **Root cause CONFIRMED BY A/B** —
removing exactly this loop, and nothing else, fixes all 8 previously-
failing configurations. This closes the "root mechanism not
independently confirmed" gap entry 26/P19 left open.

**A genuine methodology bug, caught and fixed before trusting F1's
result.** Early attempts seeded a ledger row directly for the real
depositor under test (claiming a balance via the spike's own SEED
method) and then withdrew against it — this failed on every vault
tried, including the real, unmodified one at trivial 1-row scale, with
`"materialization failed: EconomicsError"`, independent of
`--fuel-fee-shells`. Isolated via staged controls (bare transfer worked;
a real `DEPOSIT` forwarding currency the sender didn't actually hold
failed identically) to: a directly-seeded ledger row is an unbacked
bookkeeping claim, and withdrawing against it fails because the vault's
real account never received that currency. Fixed by seeding only
*filler* rows/pending (never resolved, so unbacked is fine) and backing
the real depositor's own row with one genuine, externally-minted,
forwarded `DEPOSIT`. Re-verified the fix resolves a real Settle at
trivial scale before relying on it for F1. This is logged as a
correction to this round's own early work, not glossed over — every F1
result was produced under the corrected methodology.

**A second, unrelated bug:** mid-session, previously-working probe calls
started failing the same way because an account's own accrued storage
rent (entry P1's ~1 SHELL/byte/sec) had grown, across many
`--block-time` jumps spent on other accounts, to exceed its balance.
Unrelated to P18; fixed by funding fresh instances heavily and using
minimal block-time increments for the rest of this round.

**F2.** `vault-linear`'s own *deploy* was rejected with the same
`BodyTooLarge` error P19 hit — its compiled wasm (a `BTreeMap`-based
attempt: ~64.3 KB; a `Vec`-sort-based attempt: ~68.0 KB) both exceeded
whatever `vault-base`/`vault-nocheck` (~56.8–57.0 KB) stay under. This
directly confirms `BodyTooLarge` is a deploy-envelope size limit (wasm +
ACAB + descriptor + overhead), not about seeded state content —
resolving P19's unexplained non-monotonic bisection result as "different
compile-time code shapes produce different wasm sizes," now with a
concrete before/after measurement instead of a guess. Practical
takeaway for TASK H: a linear in-`parse()` rewrite isn't simply "same
logic, cheaper" here — both tried encodings bloat the wasm enough to
risk this exact limit, an argument for dropping the check from `parse()`
rather than linearizing it.

**F3.** Entry 26/P19's "guest code never ran at all" wording is
corrected: dispatch parses state *before* reaching `handle_callback` for
every message, so F1's result (removing the loop fixes it) is direct
evidence execution reaches `parse()` and traps inside that specific
loop, not evidence it never starts. More precise than the prior wording,
but not fully instrumented: no side-channel diagnostic (built this round
or last) can distinguish "trapped inside `parse()`'s loop" from "trapped
on the dispatch line immediately before it" — both produce the same
observable result. Stated plainly in both this entry and P20: not-run
vs. out-of-fuel-inside-`parse()` remains formally undetermined by direct
instrumentation, but A/B evidence strongly localizes the trap to that
one loop.

### 32. Gate 2b round 3 TASK G: current caps (220/12) verified safe with margin mapped; 232-row post-restore path confirmed; MIN_OPERATIONAL_SHELL's margin shown insufficient against real overhead

Full write-up in `docs/PLATFORM_FINDINGS.md` P21; summary here.

**G1.** `vault-prodcaps` (real, unmodified code at the real, current
220/12 caps, plus only the spike-only SEED method already used
throughout this round) resolved both Settle and Restore at exactly
220 ledger rows / 12 pending (peak) — the current production caps,
verified safe. Also confirmed the 232-row "post-restore" shape
(reachable only via restore bypassing the cap, never via ordinary
`DEPOSIT` — confirmed separately that a new-row deposit is correctly
rejected once ledger already exceeds 220): `DEPOSIT` (existing row) and
a full `WITHDRAW`+Settle round trip both succeed at 232 rows / 12
pending peak.

**G2.** Bisected pending count at a fixed 220 ledger rows on
`vault-base` (unmodified O(n²) check, caps temporarily restored to
300/32 purely so pending could be pushed past 12 for this specific
test): 13/15/17/20 resolve, 21/22/24 stuck. First failing point: **21**.
Per this round's own margin rule (chosen cap ≥50% below the first
failing point), the *current* cap of 12 does not clear the required
margin at this row count (would need ≤10, not 12) if the check stays
as-is. Separately confirmed on `vault-nocheck` (check removed): pending
32 — this contract's architectural ceiling — resolves cleanly at 220
rows for both outcomes, no failing point found anywhere tested this
round (12–32, at both 220 and 300 ledger rows counting entry 31's
results too).

**G3.** Funded fresh `vault-prodcaps` instances with exactly
`MIN_OPERATIONAL_SHELL` (1,200,000) and 3× that (3,600,000), brought
each to the 220/12 worst-case shape, and triggered the real test
withdrawal at +1s/+10s/+60s elapsed since last touch (disclosed
limitation: this stand resolves a withdrawal's callback synchronously
within the same call, so "delay between withdraw and callback"
specifically isn't independently testable here — elapsed-since-last-
touch is the closest achievable proxy). Result: the 1× funding level
failed even at +1s (rejected before any state write, consistent with
I17 firing); the 3× level resolved at +1s and +10s but failed at +60s,
same signature. This gives `MIN_OPERATIONAL_SHELL`'s existing "not an
operating balance, just a withdrawal gate" caveat (SPEC.md §1, already
stated since Gate 2b round 2) concrete numbers: even 3× the threshold
doesn't survive a full minute of elapsed time plus ordinary setup
overhead.

**Correction applied to SPEC.md per this round's findings** (see TASK H
below for the caps decision itself): the margin table above and the
MIN_OPERATIONAL_SHELL finding are both incorporated into SPEC.md's
existing near-cap operator note and MIN_OPERATIONAL_SHELL discussion
(Gate 2b round 2 TASK E's addition), rather than creating a duplicate
section.

### 33. Gate 2b round 3 TASK H: `state.rs`'s O(n²) check removed from production; `MAX_PENDING_WITHDRAWALS` restored to 32; `MIN_OPERATIONAL_SHELL` re-derived to 1,300,000; a real propagation gap in THREAT_MODEL.md found and fixed

**Decision, per F1's confirmation.** F1 (entry 31) confirmed by real-stand
A/B test that `state.rs` `parse()`'s own O(n²) per-depositor pending-count
check was the actual P18/P19 cause. Per this round's own branching
instruction ("if F1 confirms the quadratic check: fix `state.rs`..."):
the check is **removed outright, not linearized** — two linear rewrites
were tried (a `BTreeMap` version, a sort-based `Vec` version) and both
bloated the compiled wasm enough to risk the same `BodyTooLarge` deploy
limit this investigation hit elsewhere (entry 31/P20), which is itself
part of the justification for dropping the check rather than keeping a
cheaper shape of it. Dropping it is safe because the invariant it
enforced (≤`MAX_PENDING_PER_DEPOSITOR` outstanding withdrawals per
depositor) is already independently enforced on every path that can
insert a pending record — `handle_withdraw`'s own real-time check — and
no other method ever writes to `pending` (`handle_callback` only
removes). The two other round-3 `parse()` checks (global pending-count
cap, `next_withdrawal_id` monotonicity) are both O(n) or O(1) and were
never implicated by F1 — kept as-is.

**Constants changed.** `MAX_PENDING_WITHDRAWALS`: 12 → **32** (this
contract's original architectural ceiling, directly verified safe at
both 220 and 300 ledger rows for both outcomes, with no failing point
found anywhere tested — entry 32/P21's G2). `MAX_LEDGER_ENTRIES`: left
at 220, not restored toward 300 — ledger row count was never shown to
be the actual driver, and a smaller worst-case state helps the separate
`MIN_OPERATIONAL_SHELL` margin finding (G3). New worst-case byte count:
`91 + 220×85 + 32×89 = 21,639` (up from round 2's `19,859`, still below
the original `31,159`). `MIN_OPERATIONAL_SHELL` re-derived identically
to the existing 60-second-margin methodology: `21,639 × 1.000 × 60 =
1,298,340`, rounded up to **`1,300,000`**.

**Test fallout, all fixed and re-verified.** `state.rs`'s own
`rejects_pending_count_over_per_depositor_cap` test — which asserted the
REMOVED check's old behavior — was rewritten to
`parse_no_longer_enforces_per_depositor_cap`, documenting the new,
intentional behavior (the same shape now parses successfully) rather
than silently deleted. `lib.rs`'s `232`-row worst-case tests (arena
headroom and counting-allocator, both the deposit/withdraw pair and the
max-ledger-and-max-pending callback test) all renamed and renumbered to
`252` rows / `32` pending, with filler-distribution comments updated
(8 synthetic depositor groups instead of 3). `MAX_PENDING_PER_DEPOSITOR`'s
import in `state.rs` moved behind `#[cfg(test)]` (no longer referenced
by production code, only by tests confirming the removed check's
documented new behavior). `cargo test` 73/73, `cargo fmt -- --check`,
`cargo clippy` (host + wasm32, `-D warnings`), and `avm-contract-build
build` (wasm shrank to 56,300 bytes, smaller than before — removing a
loop, not adding one) all re-verified clean after every change.

**SPEC.md updated throughout**: the intro cap-reduction paragraph, the
`MIN_OPERATIONAL_SHELL` derivation section (now a three-stage history:
Phase 3 → Gate 2b round 2 → Gate 2b round 3), §2.4's definitional table
and worst-case derivation, Invariant I8, SPEC §8 test 14's asserted
constant, the burn-formula/operator-note README-requirements paragraphs,
and the "32 stuck pending records" README warning — all now state the
restored 32/220 caps, the 21,639-byte worst case, and `1,300,000`,
with the root cause framed as **confirmed and fixed**, not an open gap,
wherever the text previously said so. The rent-terms worked example in
§2.4 ("restating the worst case in rent terms") was also updated to the
current 21,639-byte figure rather than left as a stale historical
artifact for a second consecutive round.

**A genuine propagation gap found in `docs/THREAT_MODEL.md`, not
introduced by this round, fixed anyway since this round was already
touching every cap reference.** §5.3's and §5.9's own worst-case figures
(`300`, `332`, `31,159` bytes, "2.69 billion SHELL/day") were **never
updated during Gate 2b round 2's cap reduction** — they still described
the original, pre-reduction caps throughout that entire round, an error
this round's own sweep caught while updating THREAT_MODEL.md for the
new caps anyway. By coincidence, §5.4's own `32`/`8×4` figures were
*already* correct again (they describe the architectural
`MAX_PENDING_WITHDRAWALS` value this round restores, which happened to
match the pre-round-2 number the document had never updated away from)
— this is called out explicitly rather than left to look like deliberate
accuracy. Both sections corrected to the current 220/32/21,639/1.87-billion
figures.

**THREAT_MODEL.md §5.4 gained the collusion-cost analysis this round's
instructions explicitly asked for**: `MAX_PENDING_WITHDRAWALS` (32) ÷
`MAX_PENDING_PER_DEPOSITOR` (4) = 8 colluding accounts suffice to fill
the global pending cap permanently via either stuck-payout route ("L").
Cost estimated at roughly a few hundred thousand SHELL (this round's own
measured `charged_shells` figures for ordinary deploy/`DEPOSIT`/
`WITHDRAW` calls, order-of-magnitude only — actually engineering the
"L" outcome deliberately, as opposed to encountering it, was not
attempted this round, stated as such rather than assumed easy) against
the capital placed at risk, which is locked, not burned (fully
recoverable if the vault is ever abandoned or upgraded). The asymmetry —
low bounded cost against unbounded, permanent, vault-wide denial of
service — is stated as the section's main conclusion, not the specific
SHELL figure.

### 34. Gate 2b round 3 TASK I: test 11 recorded NOT DONE (same treatment as test 2); allocator-budget comments re-measured at the new caps; decode_hex_pubkey's known-vector test now uses a non-repeating 32-byte pattern

**Test 11** (SPEC.md §8): same situation as test 2 (entry 28) — needs a
new real-stand depositor fixture built with the sanctioned
`AVM_E2E_FAILURE_FLOW` pattern, now constructing *two* outbound intents
(a reentrant withdraw and a reentrant deposit) from within its own
`method_id = 0` handler, which this round's time went to TASK F/G's
real-stand A/B investigation instead of building. Recorded explicitly
in SPEC.md as NOT DONE, with the residual risk stated precisely: probe
P4 already directly observed, on the real stand, the exact ordering
this test would assert (for a single reentrant withdraw) — so the risk
gap is "no dedicated, auto-run regression test for the two-intent
shape," not "the ordering claim itself is unconfirmed."

**Allocator-budget comments.** The `counting_allocator` module's doc
comment cited ~60.2–60.9 KB as this contract's own measured worst-case
allocator traffic — stale since TASK H changed the worst-case shape
(220/32 caps instead of 220/12). Re-ran the tests with `--nocapture`
and recorded the actual current figures: ~45.7–46.4 KB. Comment updated
to state both the new figure and *why* it moved (allocator traffic here
tracks `pending_count` more than `ledger_count`, and this round's
pending cap went up, not down, relative to the figure that comment had
cited) rather than just swapping the number silently.

**`decode_hex_pubkey_known_vector_matches_every_byte`** (added at TASK B
this same round) used a 32-byte vector built from a repeating 16-byte
pattern (`"0123456789abcdef"` × 4) — a repeating vector cannot catch a
bug that transposes bytes by a multiple of the repeat period (e.g. an
index computed mod 16 instead of mod 32), since the wrong output would
be byte-identical to the correct one. Replaced with 32 sequential,
all-distinct bytes (`0x00..=0x1f`), with a comment explaining why
non-repeating matters here. `cargo test` 73/73, `cargo fmt -- --check`
clean after both changes.

## Phase 2 status (updated after Gate 2a review round 3)

All 5 round-3 items addressed (entries 2, 3 revised again; entries
14–15 new). 29/29 host-level unit tests pass (up from 23 — this round
added three negative tests (`topup_non_empty_body_is_rejected`,
`callback_with_wrong_magic_tag_is_rejected`,
`callback_from_non_depositor_caller_is_rejected`), three real-allocator
counting tests, and strengthened five existing tests with real
committed-state assertions via the new `test_support::
expect_full_replace_state` helper). `cargo fmt --check` (clean after one
real auto-fix this round — two test blocks' formatting, not a logic
change), `cargo clippy --all-targets -- -D warnings` (host), and `cargo
clippy --target wasm32-unknown-unknown -- -D warnings` all clean. The
stock `avm-contract-build build` continues to succeed with no extra
flags. `lib.rs`'s `#![forbid(unsafe_code)]` is now
`#![cfg_attr(not(test), forbid(unsafe_code))]` +
`#![cfg_attr(test, deny(unsafe_code))]` — production code's guarantee is
unchanged (the wasm32 build never compiles `test_support.rs` at all);
test code gained exactly one scoped, justified `#[allow(unsafe_code)]`
for the counting allocator's `GlobalAlloc` impl, which cannot be
implemented without `unsafe` by that trait's own definition in `std`.

This round corrected two of this project's own prior-round mistakes
(entries 2/9's atomicity rationale, entry 15's demo-key overclaim) and
found one genuinely new, previously-undocumented real platform limit
(entry 14c / `docs/PLATFORM_FINDINGS.md` P14: at most 8 distinct extra
currencies per message, confirmed via isolated fresh-account binary
search) — the kind of finding this engagement's empirical-first approach
exists to surface. This is Gate 2a round 3's items 1–5 — pre-approved to
continue into Phase 3 per instructions; Phase 3 work follows below,
leaving the `MIN_OPERATIONAL_SHELL` re-measurement for last, stopping at
Gate 2b.

(Phase 3, Gate 2b round 2 feedback (entries 35–39 above used the old
pre-round-4 numbering convention and are referenced by task letter
elsewhere in this file; Gate 2b round 3 TASK F–I are entries 31–34
above; Gate 2d assembled the round-3 presentation. Round 4 continues
below as entry 35.)

### 35. Gate 2b round 4 TASK J: measured real callback headroom via a build-time-gated dummy work loop; found and corrected a methodology confound (withdraw's own cap-rejection at pending=32 mimics a fuel-exhaustion result); confirmed 220/32 need not change; bisected the deploy-envelope size limit

**J1/J2 — headroom measurement.** Built a never-committed spike
(`spike/vault-dummywork/`, byte-for-byte copy of production
`constants.rs`/`handlers.rs`/`state.rs` plus a `parse()`-tail dummy work
loop of `AVM_SPIKE_DUMMY_K` black-boxed 64-byte comparisons, inert unless
that env var is set at build time — Hard Rule 5/4) and swept `K` against
the shipping worst case (220 ledger rows, 32 pending). First pass used
`actor.withdraw`'s own response as the pass/fail signal and found what
looked like an unexplained anomaly: 160 rows + 32 pending appeared to
"fail" (no state change) at the same `K` where 160 rows + 0 pending
"succeeded." Investigating *before* writing this up (per this
engagement's standing rule to verify before reporting) found the real
cause: `handle_withdraw`'s own `pending.len() >= MAX_PENDING_WITHDRAWALS`
check (`src/handlers.rs:243`) legitimately rejects any new withdraw once
pending is already at the cap (32) — a correct, by-design no-op that has
nothing to do with available fuel, and was indistinguishable from a
genuine fuel-exhaustion no-op by looking at `actor.withdraw`'s own
reply alone (both shapes carry `events: []`, identical
`charged_shells`, because that reply is just the *actor's* trivial
forwarding response either way — it never reflects the vault's own
outcome). **Fix:** re-ran every data point using `vault.topup` (a
read-only probe with no cap logic at all) as the sole pass/fail signal,
which cleanly reproduces as a hard `VM status 31` trap when `parse()`'s
cost exceeds the chargeable fuel ceiling, with no cap-check confound
possible. Under that corrected signal, the anomaly disappears entirely:
the measured boundary is a clean function of ledger row count alone
(100, 160, 175, 190 rows all clear `K = 5,850`; 220 rows traps at `K =
5,850` regardless of pending count — 0, 12, and 32 all trap identically).
Full measurement, calibration against the removed O(n²) check's 1,024
comparisons (≈5.7×, stated as a conservative lower bound given the old
check's materially higher per-comparison iterator/closure overhead), and
the J3 decision-rule application (5.7× ≥ 2× → **no cap change**,
`MAX_LEDGER_ENTRIES = 220` / `MAX_PENDING_WITHDRAWALS = 32` /
`MIN_OPERATIONAL_SHELL = 1,300,000` all retained) are written into
`SPEC.md` §1 rather than duplicated here.

**J3 — why pending count isn't the lever.** Since row count, not
pending count, drives this headroom for the current lean `parse()`
(inverse of the removed check's own pending-quadratic profile), lowering
`MAX_PENDING_WITHDRAWALS` below 32 would not have bought any additional
headroom; `MAX_LEDGER_ENTRIES` is the only lever that would matter if
more headroom were ever needed in the future. This is now stated
explicitly in `SPEC.md` §1 so a future maintainer tempted to "just lower
the pending cap for extra safety margin" can see directly why that
specific lever is empirically ineffective here.

**J4 — deploy-envelope size limit.** A second never-committed spike
(`spike/sizebisect/`) bisected the exact wasm-size limit behind
`BodyTooLarge`. First construction attempt (a compile-time-sized
`static` padding array, checked for a sentinel value in an "unreachable"
branch to keep it live against dead-code elimination) failed silently
*twice*: a uniform `[0xAB; N]` fill, and then a non-repeating
LCG-generated replacement, both produced byte-identical wasm output
regardless of `N` — because the check itself, and the array's full
contents, were provably compile-time-constant, so LLVM constant-folded
the branch to `false` and deleted the entire static as dead code either
way (the uniform-fill theory from the first attempt was itself wrong;
the real cause was the check's constant-foldability, not the fill
pattern). Fixed by making the padding bytes part of the contract's
actual *returned state* on every call (`StateUpdate::FullReplace(PADDING
.to_vec())`), which is externally observable and so cannot be
eliminated. Confirmed working (33,609-byte baseline → 64,743 bytes at
`AVM_SPIKE_PAD_BYTES=30000`) and bisected by binary search on a real
stand, deploying against a fresh account each time: **65,091 bytes
deploys successfully; 65,095 bytes is rejected** with `"deploy rejected
canonical operation: zero-signature canonical envelope rejected:
BodyTooLarge"`, on the very first (side-effect-free) deploy attempt,
before any funding precondition. The true boundary sits in the
untested 65,092–65,094-byte gap (not reachable by this construction's
padding granularity near a WASM section length-prefix boundary); 4
bytes is as tight as this method resolves. `contract.acab` (80 bytes)
and `contract.descriptor` (66 bytes) were confirmed constant across
every size tried, so the limit is a wasm-size limit specifically, not a
combined-envelope one.

**CORRECTION (Gate 3b TASK O): the above "wasm-size limit specifically,
not a combined envelope" conclusion was wrong, and is kept here
struck-through-in-spirit rather than silently deleted, so the mistake
and its correction are both on the record.** The spike used a minimal
80-byte ACAB — far smaller than the real contract's — which stayed
constant across every trial and therefore could never have revealed a
combined-envelope effect even if one existed, which it does. TASK O
re-bisected using a byte-for-byte copy of the real production source
(confirmed identical ACAB/descriptor hashes to the real build: ACAB
3,182 bytes, descriptor 66 bytes) and found a *different* wasm
boundary — 61,989 deploys / 61,993 rejected — offset from the original
65,091/65,095 by exactly 3,102 bytes, which is exactly the ACAB size
difference (3,182 − 80 = 3,102). Both bisections land on the identical
**combined** total: `65,237` bytes (wasm + acab + descriptor) succeeds,
`65,241` fails, in both cases. The limit is on the combined envelope.
See entry 40 below for the full re-bisection. Recorded as `docs/
MAINTAINER_QUESTIONS.md` #11 (renumbered from #10 when Phase 4's
finding 4 inserted a new #8); current production envelope (`56,304 +
3,182 + 66 = 59,552` bytes combined) sits roughly **8.7%** below the
measured combined limit — not the ≈15.5% this entry originally
reported, which compared wasm-only against a limit that, as it turns
out, was never wasm-only.

**J5 — same-run honesty.** Restated plainly in `SPEC.md` §1: this
synchronous local stand cannot model nonzero elapsed time between a
`WITHDRAW` and its callback, so any interaction between callback delay
and rent accrual remains genuinely untested here (not merely
unreported) — already flagged in round 3 (`docs/PLATFORM_FINDINGS.md`
P21) and repeated here because TASK J asked for it explicitly, same-run.

**Spike cleanup:** `spike/vault-dummywork/`, `spike/sizebisect/`, and
`spike/actor/` all deleted at the end of this task per Hard Rule 5 (never
committed; `git status` shows nothing tracked or untracked under
`spike/` once `spike/vault-prodcaps/`'s long-standing Windows file-lock
issue, flagged at Gate 2d and unrelated to this task, is set aside —
see TASK K below for the explicit confirmation).

No source, test, or constant changes resulted from TASK J — every cap
and constant stays exactly as Gate 2b round 3 (TASK H) left it. This is
the first round-4 task to complete entirely as a measurement exercise
with no code delta, which is itself a valid, honestly-reportable outcome
per Hard Rule 6.

### 36. Gate 2b round 4 TASK K: documentation sync — THREAT_MODEL.md's stale `1,600,000`/PROVISIONAL and byte-arithmetic fixed, SPEC §8 test 3 updated to the real caps, repo-wide grep sweep, `git status` confirmed clean under `spike/`

**THREAT_MODEL.md §5.8:** every live reference to the historical
`1,600,000` placeholder now explicitly says so and points to the final
`1,300,000` value (at the time; see entry 37 below for where that figure
itself was superseded a few hours later), including the summary table's
row 8, which previously said `MIN_OPERATIONAL_SHELL` "is PROVISIONAL
pending Phase 3 re-measurement" — stale since Gate 2b round 3 already
completed that re-measurement.

**THREAT_MODEL.md §5.7:** fixed a real, previously-uncaught claim that
`SPEC.md` §8 test 11 "is the direct Phase 3 regression test for this
exact scenario, built against the real vault" — `SPEC.md`'s own test 11
entry has said **NOT DONE** since Gate 2b round 3 (TASK I), and this
section never caught up. Corrected to state plainly what actually
exists instead: probe P4 (`docs/PLATFORM_FINDINGS.md`) and a host-level
unit test (`handlers.rs::reentrant_double_withdraw_second_attempt_sees_
debited_balance`) — confirmed to exist under that exact name before
citing it — neither of which substitutes for the missing dedicated
end-to-end regression test.

**THREAT_MODEL.md §5.9:** fixed incorrect byte arithmetic that had
conflated two different worst-case candidates. The section previously
computed `21,639` bytes as `91 + 220×85 + 32×89`, correctly, but then
described the figure as covering "252 possible ledger rows" in its own
prose — a holdover from an earlier draft that never got cleaned up.
Rewrote to state both figures precisely and separately: `21,639` bytes
(220 rows + 32 pending) and `21,511` bytes (252 rows + 0 pending, the
*smaller* of the two since a pending record costs more per-row than a
ledger row), with `21,639` identified as the larger, documented worst
case at the time. (Entry 37 below records how this specific
"220+32 vs. 252+0" framing was itself found to be an incomplete
enumeration of the real worst case, discovered independently at Phase 4
less than a day later — not a contradiction of this entry's own fix,
which correctly resolved the specific arithmetic conflation it was
asked to fix, using the two candidates `SPEC.md` named at the time.)

**SPEC.md §8 test 3:** the prose still said `MAX_LEDGER_ENTRIES` was
300 (`300`/`299`/`301` row counts) — stale since Gate 2b round 3's
220/32 cap change. Updated to 220/219/221, and cross-referenced against
`src/handlers.rs::restore_bypasses_ledger_cap_at_exact_boundary`, which
had already been using the correct 220/219/221 values in actual Rust —
only the SPEC prose had drifted, not the test.

**Grep sweep.** Searched every doc for `1,600,000` / `1,870,000` /
`1,200,000` / `300` / `332` / `31,159` / `19,859` / `"12 pending"`.
Every remaining mention in `REVIEW.md` and `docs/PLATFORM_FINDINGS.md`
sits under an explicitly dated round/task heading (e.g. "Gate 2b round
3 TASK G: current caps (220/12)") — satisfying "explicitly historical"
by document structure, append-only logs that this project has
consistently relied on throughout. `SPEC.md`'s own blanket disclaimer
sentence (introduced round 3, "every number in this document derived
from the oldest caps... is now historical") still correctly covers the
remaining round-2/round-1 figures. One additional stale block was found
and fixed during the sweep, beyond what TASK K's instructions named
explicitly: `SPEC.md` §1's round-6 `MIN_OPERATIONAL_SHELL` derivation
paragraph read as if `1,600,000` were still current, with no
"superseded" framing at all, directly contradicting the very next
paragraph's "final value" statement — now marked historical.

**`git status`:** confirmed nothing tracked or untracked under
`examples/multi-currency-vault/spike/` specifically — the entire
`examples/multi-currency-vault/` directory is untracked in this
checkout (`?? examples/multi-currency-vault/`), so `spike/`'s one
leftover locked empty directory (`vault-prodcaps/`, flagged at Gate 2d,
unrelated to this round) has no special status of its own; it is simply
part of the same untracked tree as everything else in this example,
consistent with Hard Rule 1 (no commit has occurred this engagement) and
Hard Rule 5 (`spike/*` never committed). Also noted, not acted on: the
broader `avm-sdk-bounty/repo` shows ~60 files outside this example
directory as modified (`M`) — spot-checked two (`LICENSE.md`,
`README.md`) and confirmed the diffs are a pure line-ending
normalization artifact (every line shows as changed with byte-identical
content), unrelated to anything this engagement has touched; out of
scope for TASK K, which only asked about `spike/`.

### 37. Phase 4: adversarial review by a fresh-context subagent — 4 findings, 3 confirmed and fixed (worst-case state size under-measured, admin check-order bug, error-code mix-up), 1 recorded as an open residual risk pending an unverifiable platform fact

Per the governing instructions: the reviewer was given only `SPEC.md`,
`docs/THREAT_MODEL.md`, `src/*.rs`, `avm-contract.toml`, and
`avm-contract-sdk`'s own source — explicitly withholding `REVIEW.md` and
`docs/PLATFORM_FINDINGS.md` so it could not inherit this engagement's
own rationalizations. It was instructed to try six angles: (a) steal or
lock funds, (b) break invariants I1–I17, (c) crash/trap the guest, (d)
spec-vs-code mismatches, (e) arithmetic/overflow/ordering bugs, (f)
error-code mix-ups — every claimed finding required a concrete failing
test or exact input sequence. Each of the four findings below was
triaged by writing (or, for finding 3, adapting an existing) a failing
host test *first*, confirming it actually failed against the
pre-Phase-4 code, before any fix was applied.

| # | Finding | Category | Verdict | Outcome |
|---|---|---|---|---|
| 1 | The documented worst-case state size (`21,639` bytes, 220 rows + 32 pending) is not the true maximum; `252` rows + `32` pending (`24,359` bytes) is simultaneously reachable, since a *partial* withdrawal opens a pending record without removing its row (I9 only removes a row debited to exactly zero), and `handle_withdraw` never checks `MAX_LEDGER_ENTRIES`. `MIN_OPERATIONAL_SHELL` was consequently under-derived by ~11%. | (d) spec-vs-code, with an (e)/(a) knock-on effect on a safety constant | **CONFIRMED** | **FIXED.** New host test `handlers.rs::true_worst_case_state_is_252_rows_plus_32_pending` constructs the exact drain → refill → restore → partial-withdraw sequence via real handler calls and confirms `24,359` bytes is reached; failed against `MIN_OPERATIONAL_SHELL = 1,300,000` before the fix. `MIN_OPERATIONAL_SHELL` raised to `1,500,000` (`24,359 × 60 = 1,461,540`, rounded up); `SPEC.md` §1/§2.4, `docs/THREAT_MODEL.md` §5.9, `constants.rs`'s own derivation comment, and the `arena_headroom`/`counting_allocator` worst-case fixtures in `lib.rs` (previously 220+32, now 252+32) all updated to match. |
| 2 | `handle_set_pause` checked the reserved-pause-flag-bits (1516) ahead of the stale-`admin_seq` check (1524) — the opposite order from `handle_propose_owner`/`handle_accept_owner`, and the opposite of Invariant I13's own stated requirement ("checked before that call has any other effect whatsoever") and `SPEC.md` §4's documented order. | (f) error-code/check-order, (d) spec-vs-code | **CONFIRMED** | **FIXED.** New test `handlers.rs::set_pause_stale_sequence_is_reported_before_reserved_flag_bits` sends a body that is both stale and carries a reserved bit; failed (`1516` instead of `1524`) before the fix. `handle_set_pause` now checks `admin_seq` first, matching the other two admin methods. |
| 3 | A restore-time `u128` overflow on `credit_ledger` (practically unreachable — Invariant I2's deposit-time precheck is specifically designed to prevent it) returned `ERR_INVALID_CALLBACK` (1513), whose documented meaning is exclusively "caller does not match the pending record's depositor" — a different condition, checked separately a few lines above in the same function. `SPEC.md` §5 additionally claimed the caller-mismatch check was "the only way this branch ever fails," which the code already contradicted. | (f) error-code mix-up, (d) spec-vs-code | **CONFIRMED** (low severity — practically unreachable, a documentation/code-hygiene defect, not a live exploit path) | **FIXED.** Existing test `lib.rs::failing_callback_handler_permanently_strands_the_withdrawal` already exercised this exact path (seeded a near-`u128::MAX` balance, delivered a restore) but asserted the *wrong* expected code (`ERR_INVALID_CALLBACK`); updated the assertion to the correct `ERR_LEDGER_AMOUNT_OVERFLOW` first, confirmed it failed against the unfixed code, then changed `handle_callback` to return `ERR_LEDGER_AMOUNT_OVERFLOW` (1529, already used for the structurally identical `DEPOSIT`-time overflow) instead of `1513`. `SPEC.md` §5/§6 and `avm-contract.toml`'s error messages for both 1513 and 1529 updated to match — 1513 is now genuinely exclusive to the caller-mismatch case. |
| 4 | `GuestArena<N>` (`avm-contract-sdk`) holds the raw inbound message and this contract's own output simultaneously in one fixed-size, bump-allocated region; the input side is allocated and written by the host *before* this contract's code runs, so a large message body counts against the `arena_bytes = 262_144` budget regardless of whether the handler ever reads it. A depositor receiving the vault's payout can set an arbitrarily large `callback_result_body` in their own response, which becomes the delivered callback's body — roughly `208,700`+ bytes would exhaust the arena before `handle_callback` runs, permanently stranding that withdrawal (same consequence as the two already-documented "L" routes). | (c) guest-execution failure, with an (a) funds-stranding consequence | **PLAUSIBLE, mechanism CONFIRMED by independently reading `runtime.rs`/`output.rs`; reachability on the real platform NOT confirmed** | **NOT FIXED — recorded as an open residual risk, not silently dismissed or speculatively patched.** Cannot be verified by a host test: `GuestArena::base_pointer` uses the real WASM linear-memory pointer under `cfg(not(test))`, so `dispatch_message` itself cannot be driven from a 64-bit host test — only `handle_callback` directly, bypassing the arena entirely. Reachability depends on one fact absent from every doc in this repository: whether the platform caps message body size independently of the deploy-envelope (`SPEC.md` §1) and `FullReplace` state-size (§2.4) limits already measured, neither of which is documented as applying to an arbitrary message body. Deliberately did **not** raise `arena_bytes` back toward the SDK's `8 MiB + 16` default: that would reverse a previous, deliberate, measured sizing decision (`arena_bytes = 262_144` was chosen to comfortably cover this contract's own legitimate worst-case traffic with margin) on the strength of an unconfirmed threat, trading a 32×-larger committed memory footprint — with its own undocumented resource/fee implications — for protection against a risk this repository cannot confirm is real. Added as `docs/THREAT_MODEL.md` §5.12 (a third "L" route, §6) and `docs/MAINTAINER_QUESTIONS.md` #8. |

**Reviewer's own coverage disclosure** (included here verbatim in
substance, for completeness): covered (b) and (d) most thoroughly —
every one of I1–I17 checked line-by-line, including re-deriving the
`ledger_count ≤ 252` bound (which is what led to finding 1) and
verifying `serialize(parse(x)) == x`, the `LedgerKey` sort order, and
I9's "no zero-amount row" invariant on both the deposit and restore
paths. Covered (e) and (f) thoroughly — traced every arithmetic site
for overflow potential (all `checked_add`/`checked_sub`, with length
arithmetic in `parse()` confirmed to run before any allocation) and
walked every failure return in all six handlers plus the callback
branch against SPEC §5/§6. Covered (a) and (c) least thoroughly by the
reviewer's own account: no fund-theft/cross-depositor path was found
(no destination field anywhere, ledger keys built only from the
authenticated caller, callback gated on both the wire flag and an exact
depositor match), and SHELL was confirmed structurally unable to
acquire a ledger row through any of the four writable paths (I10); for
(c), the reviewer verified the guest's own Rust is panic-free on
untrusted bytes but explicitly flagged finding 4 as the one place a
real-stand run would most likely find something a host test cannot.

**No findings beyond these four were raised.** All four were covered
by this triage; none were rejected outright — three confirmed and
fixed, one confirmed-as-a-mechanism-but-unverified-as-reachable and
recorded openly rather than either dismissed or speculatively patched.
73 pre-existing tests plus 2 new ones (75 total) pass; `cargo fmt
--check`, `cargo clippy --all-targets -- -D warnings` (host), and
`cargo clippy --target wasm32-unknown-unknown -- -D warnings` are all
clean; `avm-contract-build build` still succeeds (wasm now `56,304`
bytes, from `56,300` — comment/constant changes only, no new logic).

### 38. Phase 5: automated checks — `cargo audit` clean, `cargo deny` installed and run (license-policy result is a missing-config artifact, not a finding), pedantic clippy recorded as a report, a new fuzz-style `VaultState::parse` test added (40,000 iterations, zero panics), `cargo test --release` clean, `cargo mutants` re-run

**`cargo audit`** (already installed). Scanned `Cargo.lock`'s 2 crate
dependencies against 1,350 loaded RustSec advisories. **Clean — no
vulnerabilities found.** Raw output: `<scratch>/phase5_audit.txt`.

**`cargo deny`** (not previously installed — a standard, widely-used
Rust supply-chain tool, installed per the instruction to install
standard tools). No `deny.toml` exists in this project, so `cargo deny
check` ran against its own built-in default config, whose `licenses`
policy has an **empty allow-list by default** — meaning it rejects
every license, including this project's own MIT, not because MIT is
disallowed by any actual project policy but because no policy was ever
written down. `advisories ok, bans ok, licenses FAILED, sources ok` is
the honest summary of running the tool with zero configuration;
**writing a `deny.toml` that encodes an actual license policy was not
done**, since choosing which licenses this project accepts is a project
decision, not something to invent unilaterally (Hard Rule 3's spirit
applied by analogy). Recorded as-is, not silently cleaned up or
misrepresented as a real license problem. Raw output:
`<scratch>/phase5_deny.txt`.

**Pedantic clippy** (`cargo clippy --all-targets -- -W clippy::pedantic`,
a report per the instructions, not a gate). 50 warnings total: 8 in
production (non-test) code — 2 wildcard imports (`use
constants::*`/`use crate::constants::*`, a deliberate, already-justified
pattern elsewhere in this codebase), 1 `explicit_iter_loop`, 1
`too_many_lines` (`state.rs::parse`, 157/100 — a single large function
by design, already heavily commented), 2 `ignored_unit_patterns`, and 2
`cast_possible_truncation` (`usize as u32` for `ledger.len()`/
`pending.len()`, both provably bounded by `252`/`32` respectively, far
under `u32::MAX`) — and 44 in test-only code (the property-test module's
`u64`-returning PRNG paired with array-indexing casts, `#[allow]`-able
but not acted on since this is a report, not a gate). None are
correctness bugs; none acted on. Raw output:
`<scratch>/phase5_clippy_pedantic.txt` (also
`phase5_clippy_pedantic_libonly.txt` for the production-only subset).

**Fuzz-style `VaultState::parse` run (new this round).** Added
`src/lib.rs`'s `tests::fuzz_parse` module: `parse_never_panics_on_
random_bytes` (20,000 iterations, a seeded `Lcg` generating pure random
bytes at random lengths from 0 to ~30,000) and `parse_never_panics_on_
mutated_valid_state` (20,000 iterations, 1–8 bit-flip/byte-overwrite/
truncate-or-extend mutations per iteration applied fresh each time to a
genuinely `serialize()`-produced valid template, not a cumulative
chain). Both wrap the call in `std::panic::catch_unwind` and assert it
never unwinds — an `Err` return was always an acceptable outcome, only
a panic is not. **Caught one real bug during construction — in this
test's own fixture, not in the contract**: the first draft's template
used `currency_id: i` for `i in 0..40`, which collides with
`SHELL_CURRENCY_ID` (`2`) at `i = 2`, correctly rejected by
`validate_currency_shape` — fixed by shifting the range to `i + 10`.
Both tests pass cleanly against the real contract code: **zero panics
across 40,000 total iterations.**

**`cargo test --release`.** 77/77 pass (75 pre-existing/Phase-4 tests
plus the 2 new fuzz tests). Raw output:
`<scratch>/phase5_test_release.txt`.

**`cargo mutants`** (re-run; already installed). **203 mutants tested
in 9 minutes: 181 caught, 17 unviable (didn't compile), 4 missed, 1
timeout.** The 1 timeout (`src/constants.rs:251:15`, `index += 1` →
`index *= 1` in `decode_hex_pubkey`'s digit-decoding loop) is a genuine
infinite loop under this classic "accumulator starts at zero, `+=`
replaced" mutation pattern (`index` starts at `0`, so `*= 1` holds it at
`0` forever) — correctly caught via cargo-mutants' own 20-second
hang-detection rather than a test assertion; not a gap. **All 4 missed
mutants were individually inspected and confirmed to be genuine
equivalent mutants** — cases where the mutated code is byte-for-byte
behaviorally identical to the original for every possible input, which
no test could ever distinguish no matter how thorough, because the two
operands of each `|` provably never share a set bit:

| Site | Expression | Why `\|` and `^` are provably identical |
|---|---|---|
| `constants.rs:27` | `(NORMAL_METHOD_TAG << 32) \| 1` | the `<< 32` shift occupies only bits 32–63; the literal `1` occupies only bit 0 — disjoint by construction |
| `constants.rs:28` | `(NORMAL_METHOD_TAG << 32) \| 2` | same reasoning, bit 1 instead of bit 0 |
| `constants.rs:46` | `(CALLBACK_MAGIC_TAG << 32) \| withdrawal_id as u64` | the shift occupies bits 32–63; `withdrawal_id` is a `u32` cast up, so it can only ever occupy bits 0–31 — disjoint by type, not merely by value |
| `constants.rs:250` | `(hex_digit(...) << 4) \| hex_digit(...)` | `hex_digit` returns a 4-bit nibble (`0`–`15`); the left operand's `<< 4` shift occupies bits 4–7, the right operand only ever occupies bits 0–3 — disjoint by construction |

No code or test changes resulted from this re-run: the 1 timeout is a
correctly-caught mutant via a different (hang-based) detection
mechanism than a failing assertion, and all 4 misses are equivalent
mutants, not coverage gaps. Raw output: `<scratch>/phase5_mutants.txt`;
full per-mutant detail in `mutants.out/` (`missed.txt`, `caught.txt`,
`unviable.txt`, `timeout.txt`, `outcomes.json`).

**Phase 5 summary: no code changes resulted from any automated check.**
`cargo audit` clean, pedantic clippy recorded as a report with nothing
acted on, the new fuzz tests pass cleanly (after fixing one bug in the
fuzz test's own fixture, not the contract), `cargo test --release`
clean, and `cargo mutants`' only two non-"caught" outcomes (1 timeout, 4
misses) were both individually verified to be expected artifacts of
mutation testing, not real gaps. `cargo deny`'s license-policy result
is an artifact of never having written a `deny.toml`, recorded honestly
rather than hidden or misrepresented.

**One real fix found during Gate-3 final re-verification, test-only:**
the gating `-D warnings` host clippy run (re-run clean, not just the
pedantic report) caught `clippy::unusual_byte_groupings` on the new
`fuzz_parse` module's own seed constant
(`0x5EED_F2_5201_F022`, uneven nibble groups) — regrouped to
`0x5EED_F250_1F02_2001`. Test-only, no effect on the compiled wasm
(confirmed: identical hash before and after). `cargo test`,
`cargo clippy` (host + wasm32, both `-D warnings`), `avm-contract-build
build` all re-verified clean after this fix — see Gate 3 below for the
final, unabridged output of every one of these.

## Final summary (Gate 3)

**What this engagement did.** Built a multi-currency deposit/withdraw
vault for the Acki Nacki AVM SDK from a written spec (`SPEC.md`) and
threat model (`docs/THREAT_MODEL.md`), both revised repeatedly against
real, empirical findings from the SDK/platform rather than assumptions
— every platform behavior this contract's own logic depends on was
verified by constructing it directly on `avm-local-stand` or, where the
stand couldn't reach it, stated as explicitly unverified
(`docs/MAINTAINER_QUESTIONS.md`, 11 open questions for the SDK
maintainers, priority-ordered). 77 host-level tests (unit, negative,
property, simulator, and two fuzz-style tests added this round) plus a
real-stand end-to-end run with replay-root comparison. A fresh-context
adversarial review (Phase 4) found three real, now-fixed bugs and one
open, honestly-unresolved risk. Every automated check this engagement's
instructions named (`fmt`, `clippy` at two different strictness levels,
`audit`, `deny`, `mutants`, a fuzz run, release-mode tests) ran, with
raw output recorded rather than summarized away.

**What is explicitly NOT claimed.** Not "secure." Not "audited." This
is: tested as specified above, against this specific SDK/platform
release, with every residual risk this engagement could not close
stated in writing rather than implied solved —
`docs/THREAT_MODEL.md` §6 lists nine of them by name; the single highest-
priority one (whether `TOPUP_METHOD_ID` can even work on a real
network, given `ExternalMessage` has no source account to debit SHELL
from) is serious enough that `README.md` puts it in "Known limitations"
rather than burying it. A reader relying on this contract for anything
beyond local testing and demonstration must read `SPEC.md`,
`docs/THREAT_MODEL.md`, and this file before doing so — not just
`README.md`'s summary.

**Everything delivered, by category:**
- **Source:** `src/{constants,handlers,lib,state,test_support}.rs`,
  `avm-contract.toml`. 77/77 tests pass; `cargo fmt --check` clean;
  `cargo clippy --all-targets -- -D warnings` clean on both the host
  target and `wasm32-unknown-unknown`; `avm-contract-build build`
  succeeds (`contract.wasm` 56,304 bytes).
- **Docs:** `SPEC.md` (behavioral spec, invariants I1–I17, §8 test
  list, §10 Phase-2 README deliverables), `docs/THREAT_MODEL.md` (§5.1–
  §5.12 named attacks/hazards, §6 ten residual risks), `docs/
  PLATFORM_FINDINGS.md` (the full round-by-round empirical log behind
  every platform claim in the other two docs), `docs/
  MAINTAINER_QUESTIONS.md` (11 open questions for the SDK team),
  `README.md` (the loud demo-key warning, the SHELL/rent operator note,
  known limitations, build artifacts + hashes), and this file.
- **Findings this round:** TASK J (headroom measurement — no cap
  change, but found and fixed the true worst-case state size via Phase
  4's finding 1), TASK K (doc sync), Phase 4 (3 bugs fixed: worst-case
  state-size under-measurement → `MIN_OPERATIONAL_SHELL` raised to
  `1,500,000`; an admin check-ordering bug in `SET_PAUSE`; an
  error-code mix-up on a practically-unreachable restore-overflow path
  — plus one open, unverified-reachability risk recorded rather than
  guessed at), Phase 5 (automated checks, no further code changes,
  one real clippy fixup in test-only code).

**Judgment calls and deviations:** logged throughout this file as they
happened, and in the running log at the top (`## Judgment calls and
deviations`). The two that most shaped this round: treating a reviewer
finding's own severity claim as a starting hypothesis to verify, not a
conclusion to accept (Phase 4 finding 1's "21,639 bytes" correction
required independently reconstructing the reachability argument before
trusting it enough to change a security constant); and declining to
"fix" Phase 4 finding 4 by raising `arena_bytes` without evidence the
underlying threat is real, because the cost of being wrong in either
direction (a false sense of security from an unconfirmed fix, versus an
undocumented resource/footprint increase) was not something this
engagement had grounds to decide unilaterally.

**Not committed, not pushed.** Per the hard rules governing this entire
engagement: no `git commit` has occurred at any point this session; `spike/*`
was never committed and the throwaway directories used this round were
deleted when their work was done. This review package — everything
above, plus the unabridged raw output presented at Gate 3 — is ready
for review. Awaiting the explicit instruction to commit (local only,
never pushed) before any git state changes.

---

**Gate 3 was reviewed and NOT approved for commit — TASK L through S below
address specific gaps found during that review.**

### 39. TASK L: the "hash contradiction" was never a build-reproducibility bug — `avm-contract-build`'s own printed value is a BLAKE3 canonical content hash, not a SHA-256 of the file; `contract.wasm` is confirmed path-reproducible (not cross-path-reproducible), `contract.acab`/`contract.descriptor` are fully reproducible

**Root cause, confirmed by inspecting the tool's own binary.** `strings`
on `avm-contract-build` (no source available in this checkout — it
ships as a prebuilt release binary) shows it statically links the
`blake3` crate (v1.8.7, visible via embedded cargo-registry source
paths) and carries symbols named `avm.hash.blake3.bytes` and
`avm-canonical.hash_domain`. Its printed `<filename> <hex>` output is an
**AVM-internal, BLAKE3-based canonical content/package-identity hash**
— not a raw SHA-256 of the file, and not intended as a file-integrity
checksum at all. Gate 3's chat summary pasted this tool-printed value
next to a separately-computed `sha256sum` without making that
distinction explicit, which is exactly what read as a contradiction:
both values *were* hashes of the identical bytes from the identical
build, just computed by two different functions for two different
purposes.

**Direct verification, this round:** rebuilt clean (`rm -rf target`,
`AVM_EXAMPLE_VAULT_OWNER_PUBKEY` unset) at this checkout's own path and
captured both values for the same files in the same build:

```
avm-contract-build's own printed hash:
contract.wasm 9864670ee99019c568439b7f2636a5833308e0bb43d4c0e11474935ca27825be
contract.acab 196e44ac93c3c87a5312fd22c6250ccd9d6a6310921fbc8a4a1706ba52aa3f4c
contract.descriptor 34d33b9b84624e00347780c7f89f1481e02577b7841ea26188b4af7788c62f9e

wc -c:
56304 target/avm/contract.wasm
 3182 target/avm/contract.acab
   66 target/avm/contract.descriptor

sha256sum (the real SHA-256 of these exact file bytes):
20d8f35f15e3096a12d71314b692ad9e1e206ec3c3aef5aa5b23eef5492ed2df  target/avm/contract.wasm
63ade31e1188c223974338e6f1ae919ee75a8ae541c5df26e1faf973b39ee352  target/avm/contract.acab
66f1916ea73f6f70c4ed0325e6e29b3cdb47e4efb66344addd04e750b88ce6b3  target/avm/contract.descriptor
```

Confirmed the two columns genuinely differ for the same file (ruled out
a transcription error) and tried SHA3-256/BLAKE2b/BLAKE2s against the
raw wasm bytes directly — none matched the tool's printed value either,
consistent with it being a *canonical* (domain-separated/structured)
hash rather than a hash of the literal file bytes.

**Reproducibility, re-verified by building three times** (this
checkout's path, plus two independent `/tmp` copies built from two
different absolute paths, each given its own copy of the repo root's
`Cargo.toml`/`Cargo.lock`/`rust-toolchain.toml` and `crates/` so the
workspace resolves):

| Build location | `contract.wasm` SHA-256 | `contract.acab` SHA-256 | `contract.descriptor` SHA-256 |
|---|---|---|---|
| This checkout | `20d8f35f...` | `63ade31e...` | `66f1916e...` |
| `/tmp/repro_a_root` (1st build) | `74b3684e...` | `63ade31e...` | `66f1916e...` |
| `/tmp/repro_a_root` (2nd build, same path) | `74b3684e...` | `63ade31e...` | `66f1916e...` |
| `/tmp/repro_b_root` (different path) | `7d2a82e4...` | `63ade31e...` | `66f1916e...` |

**`contract.acab` and `contract.descriptor` are fully path-independent
and byte-for-byte reproducible** — identical across all four builds.
**`contract.wasm` is reproducible only when the absolute build path is
held constant**: the two `/tmp/repro_a_root` builds (same path, rebuilt
from scratch) produced byte-identical wasm; every one of the three
*distinct* paths produced its own distinct wasm. This reconfirms
`REVIEW.md` entry 24's original finding ("wasm is NOT path-reproducible,
ACAB/descriptor are") exactly — TASK L did not find new
irreproducibility, it re-verified the known kind and additionally
explained the hash-table discrepancy that looked like a second,
separate problem.

**Fix applied:** `README.md`'s "Build artifacts" table now shows both
hash columns side by side, clearly labeled, with the BLAKE3 explanation
inline, plus an explicit reproducibility note (acab/descriptor:
compare freely; wasm: only comparable when rebuilt at this exact path).
Throwaway `/tmp/repro_a_root`/`/tmp/repro_b_root` directories deleted
after use (not under this project's `spike/`, but cleaned up as
throwaway scratch regardless).

### 40. TASK O: the deploy-envelope limit is on `wasm + acab + descriptor` COMBINED, not wasm alone — entry 35's original conclusion, drawn from a spike whose ACAB never varied, was wrong

**Methodology.** Built `spike/sizebisect-real/` (never committed, Hard
Rule 5): a byte-for-byte copy of the real production `constants.rs`/
`handlers.rs`/`state.rs`/`test_support.rs`/`avm-contract.toml`, with
only `lib.rs` modified to append a compile-time-sized, build-time-gated
padding array (`AVM_SPIKE_PAD_BYTES`, same `build_padding()`
LCG-pattern technique as TASK J4, embedded in `init()`'s own returned
state so it can't be dead-code-eliminated) to the genesis state. Built
with `AVM_SPIKE_PAD_BYTES=0` first and confirmed `contract.acab`
(`196e44ac...`) and `contract.descriptor` (`34d33b9b...`) — the tool's
own printed hashes — are **byte-identical to the real production
build's own values**, i.e. this spike's ABI genuinely matches the real
contract's, not an approximation.

**Bisection result.** Binary search on `AVM_SPIKE_PAD_BYTES`, deploying
against a fresh account each time (same technique as TASK J4):

| wasm bytes | Result |
|---|---|
| 59,073 – 61,989 | deploys successfully (multiple points tested while bisecting) |
| 61,989 | **largest succeeding size** |
| 61,993 | **smallest failing size** — `BodyTooLarge` |
| 62,001 – 65,170 | rejected (multiple points tested while bisecting) |

`contract.acab` stayed exactly 3,182 bytes and `contract.descriptor`
exactly 66 bytes across every trial (confirmed via the build script's
own printed sizes each run) — this time matching the real contract,
not a minimal stand-in.

**The combined-envelope proof.** Comparing against TASK J4's original
bisection (which used an 80-byte spike ACAB):

| Bisection | ACAB used | wasm succeeds / fails | `wasm + acab + descriptor` succeeds / fails |
|---|---|---|---|
| TASK J4 (round 4) | 80 bytes (minimal spike) | 65,091 / 65,095 | **65,237 / 65,241** |
| TASK O (this round) | 3,182 bytes (real contract) | 61,989 / 61,993 | **65,237 / 65,241** |

Both bisections — built from different source, at different wasm
sizes, with ACAB sizes differing by `3,102` bytes — land on the
**identical combined total** in both the succeeding and failing case.
This is conclusive: the `BodyTooLarge` check sums `wasm + acab +
descriptor` (and, presumably, a small fixed amount of surrounding
canonical-message framing already accounted for identically in both
cases) against a single limit, **not** a wasm-only limit as entry 35
concluded. Entry 35's own conclusion was wrong because its spike's
ACAB (80 bytes, minimal) never varied across any trial, so there was
no way for that experiment to have revealed a combined-envelope effect
even though one was present the whole time.

**Consequence for this contract.** Real production envelope: `56,304
(wasm) + 3,182 (acab) + 66 (descriptor) = 59,552` bytes combined.
Headroom against the `65,237`-byte combined limit: `5,685` bytes
(**≈8.7%**) — not the ≈15.5% entry 35 originally reported (which
measured wasm-only headroom against a wasm-only limit that doesn't
exist as such). **Practical implication, not previously visible under
the wrong framing:** adding ABI surface — a new method, a new ABI
type, even a longer method/field name in `avm-contract.toml` — shrinks
this same budget exactly as surely as adding compiled code does, since
both wasm and acab draw from the same combined total.

**Fixes applied:** `docs/MAINTAINER_QUESTIONS.md` #11 rewritten with
the full side-by-side table and corrected headroom figure; `SPEC.md`
§1's deploy-envelope bullet rewritten to state the combined-envelope
finding directly; `README.md`'s build-command note updated to cite the
combined `65,237`/`65,241` figures instead of the wasm-only ones; this
file's entry 35 annotated with an explicit correction rather than
silently edited, so the mistake stays visible alongside the fix.
`spike/sizebisect-real/` deleted after use, per Hard Rule 5.

### 41. TASK N: Phase 4 finding 4 (oversized-callback-body arena exhaustion) tested directly on the real stand — NOT reproduced at any size up to 10,000,000 bytes; the source-reading theory behind it may be wrong in a way not yet identified

**Exact limits quoted first, as instructed.** Searched
`crates/avm-contract-sdk/src/output.rs` for every reference to
`callback_result_body` and `validate_message_context`. The only checks
found anywhere in the SDK's own source:

- `output.rs:93` — the field itself: `pub callback_result_body: Vec<u8>` (no length bound in the type).
- `output.rs:356–363` (`validate_message_shape`) — `callback_result_body` may only be non-empty when `output.status_code == 0`; a non-empty body alongside any non-zero status code is rejected (`EncodeError::EffectsOnFailure`).
- `output.rs:404–424` (`validate_message_context`) — line 411: on `init`, `callback_result_body` must be empty, full stop. Line 422: `if !input.message.has_callback_request() && !output.callback_result_body.is_empty() { return Err(EncodeError::InvalidCallback); }` — the *only* other check: a non-empty body requires the inbound message to have carried a callback request.
- `crates/avm-contract-sdk/src/input.rs:598–603` — on the *receiving* side, the inverse constraint: if a callback's `status_code != 0` (a Restore), its `body` must be empty, or decoding fails (`DecodeError::InvalidValue`). No such constraint exists for `status_code == 0` (Settle) — a non-empty body is permitted there, which is exactly the path this task tests.

**No size limit of any kind is checked anywhere in this SDK source for
`callback_result_body`.** The only ceiling Phase 4's finding 4
theorized was the destination's own `GuestArena`/`arena_bytes`,
enforced at the runtime/memory level (`runtime.rs`), not in this
validation code.

**Methodology.** Built `spike/actor-bigbody/` (never committed, Hard
Rule 5): a minimal depositor-contract spike, dispatched by body length
(not method_id, which is host-hashed and unknown until build+inspect —
same technique every "actor" spike this engagement has used).
`actor.deposit` (65-byte body) forwards a real `DEPOSIT` to a vault,
carrying real extra-currency so the actor acquires a genuine, backed
ledger row. `actor.withdraw` (64-byte body) sends a real `WITHDRAW` for
a trivial amount (1 unit), repeatable many times against the same
backed row. On receiving the vault's own payout (`method_id = 0`,
empty body, `Internal` caller — the exact shape `handle_withdraw`
sends, confirmed against `src/handlers.rs`), the actor returns
`status_code = 0` with a `callback_result_body` of
`AVM_SPIKE_BODY_SIZE` bytes, **generated in a runtime loop, not as a
compile-time `static` array** — the first construction attempt used a
static array and immediately hit a confound: a 50,000-byte static
padding array inflated the *actor's own* wasm to 86,568 bytes, already
over TASK O's newly-corrected combined deploy-envelope limit
(`65,237`/`65,241`), long before reaching a large enough body to test
the vault's arena at all. Switching to runtime generation kept the
actor's own wasm constant (~36.9 KB) regardless of `BODY_SIZE`,
decoupling "how large a callback body gets sent" from "how large the
sending contract's own deploy is."

A **fresh, real, unmodified production vault** (not a spike) was
deployed and funded for this test — only the malicious depositor needed
to be a spike. Each trial: fresh actor deployment (new salt), fund,
`actor.deposit` (backing the row), byte-decoded `vault.topup` probe
(before), `actor.withdraw`, byte-decoded probe (after). Resolution is
confirmed structurally, the same way every other round of this
engagement has: `next_withdrawal_id` advancing proves a pending record
was created; `pending_count` returning to its prior value (0 in every
trial here) proves that same record was then removed — i.e., Settled,
not stuck.

**Stand experiment table (TASK N):**

| `callback_result_body` size | vs. vault's `arena_bytes` (262,144) | `next_withdrawal_id` | `pending_count` after | Result |
|---|---|---|---|---|
| 50,000 bytes | 0.19× | advanced | 0 | **Settled cleanly** |
| 200,000 bytes | 0.76× | advanced | 0 | **Settled cleanly** |
| 262,144 bytes (= the vault's own full arena) | 1.00× | advanced | 0 | **Settled cleanly** |
| 1,000,000 bytes | 3.8× | advanced | 0 | **Settled cleanly** |
| 5,000,000 bytes | 19× | advanced | 0 | **Settled cleanly** |
| 10,000,000 bytes | 38× | advanced | 0 | **Settled cleanly** |

**No failure at any size tested.** The largest size tried (10 MB) is
38× the vault's own declared 262,144-byte arena and far beyond the
≈208,700-byte threshold Phase 4's finding 4 estimated from reading
`GuestArena`'s source. This directly contradicts that source-reading.

**Honest accounting of what this does and doesn't prove.** The
source-reading in Phase 4's finding 4 was careful and, applied
elsewhere this same round (TASK O), correctly predicted a real,
confirmed platform behavior (the combined deploy-envelope limit).
Here, it did not hold up empirically, and this triage does **not**
claim to know why — candidate explanations (none confirmed): the real
callback-delivery path may not route the body through the destination's
declared `arena_bytes` the way an ordinary inbound message does; the
`arena_bytes` parameter may constrain something narrower than assumed;
or some other mechanism entirely. Per the task's own instruction, no
mitigation is proposed, since none is needed for a risk that was tested
for and not found. **Updated:** `docs/THREAT_MODEL.md` §5.12 (new
paragraph documenting the test and the open "why" question) and §6
(the third "L" route demoted to a tested-and-not-reproduced note, the
other two routes' "of 3" labels reverted to "of 2"); `docs/
MAINTAINER_QUESTIONS.md` #8 rewritten around the new, narrower question
("what does `arena_bytes` actually constrain," not "is this
reachable"); `README.md`'s known-limitations bullet updated to match.
`spike/actor-bigbody/` deleted after use, per Hard Rule 5.

### 42. TASK M: the dummy-K sweep re-run at the TRUE worst case (252 ledger rows + 32 pending, not TASK J's 220+32) — superseded by TASK M2's control, entry 43 below (the "boundary essentially unchanged" claim in this entry's title did not survive a controlled re-test)

**CORRECTION (Gate 3b TASK M3):** the paragraph immediately below gives
the WRONG mechanism for why 252+32, not 220+32, is the true worst case.
`handle_withdraw` DOES debit the row it withdraws from
(`debit_ledger`/Invariant I9), and a full debit removes the row — "220
settled rows and 32 simultaneously pending ones coexist" describes a
state that is not reachable at all. The real mechanism: restores
bypass `MAX_LEDGER_ENTRIES` entirely and unconditionally recreate the
row they restore (SPEC.md §1/§2.4); starting from a full 220-row
ledger, draining 32 rows fully (32 pending), refilling to 220 with 32
unrelated new deposits, then restoring (not settling) all 32 original
pending withdrawals recreates those rows on top of the already-full
220 (252 rows, 0 pending); 32 subsequent PARTIAL withdrawals on the
restored rows then refill all 32 pending slots without zeroing any row
(a partial debit leaves the row non-zero). Reachability is proven by
the host test
`handlers.rs::true_worst_case_state_is_252_rows_plus_32_pending`. The
corrected explanation now lives in `SPEC.md` directly (not here, to
avoid a third copy of the same reasoning needing to stay in sync).

**Why 220+32 was not actually the worst case (ORIGINAL, WRONG
explanation — kept for the record, not relied on).** A pending
withdrawal does not remove its row from the ledger (`handle_withdraw`
only ever marks a row's amount as provisionally encumbered via the
`pending` vector; `MAX_LEDGER_ENTRIES` and `MAX_PENDING_WITHDRAWALS`
are separate caps on separate vectors). So 220 settled rows and 32
simultaneously pending ones can coexist: **252 ledger rows + 32
pending is the true shipping worst case**, not the 220+32 TASK J
measured. `SPEC.md`'s line "At 220 ledger rows + 32 pending (the
shipping worst case)" has been corrected in place with an explicit
note rather than silently rewritten, crediting this task.

**Reachability, separately proven, satisfied before the sweep even
started.** A never-committed spike (`spike/vault-m252/`, Hard Rule 5,
deleted after use), a copy of the production contract with
`MAX_LEDGER_ENTRIES` raised to `300` (spike-only, purely so the spike
itself could legally hold 252 rows — never a proposal to raise the
production cap) plus the same build-time-gated dummy-work loop TASK J
used, was driven to **287 ledger rows + 32 pending** (strictly harder
than 252+32) via genuine real `DEPOSIT`/`WITHDRAW` calls through 8
depositor-relay actor contracts (`spike/actor-m252/`, each supplying
the `Caller::Internal` a real `DEPOSIT`/`WITHDRAW`/payout-acceptance
needs). All 32 of those withdrawals' resulting callbacks were
confirmed, via byte-decoded before/after `vault.topup` probes, to
resolve as **Settle** (`next_withdrawal_id` 0→32, `pending_count` back
to 0, `ledger_count` steady at 287 throughout); one further,
separately deployed `AVM_ACTOR_REJECT_PAYOUT=1` actor variant was used
to force and confirm a clean **Restore** the same way
(`next_withdrawal_id` 32→33, `ledger_count` 288). This satisfies the
task's "run DEPOSIT and WITHDRAW paths, run both Settle and Restore,
byte-decoded" requirement independently of the K-sweep below.

**The K-sweep itself needed a different construction technique.**
Repeating the real-call marathon above at every K value in a sweep
(each K needs its own fresh wasm build, hence a fresh deploy, hence a
fresh empty state to rebuild from scratch) ran into a genuine,
previously-undocumented platform limit: `avm-local-stand` has a
**session-wide cap on distinct (account, currency) pairs**, separate
from the per-message "8 extra-currencies" limit already documented.
Reusing already-registered currency ids against new accounts worked
early in the day's testing, but after enough cumulative test traffic
in this one stand session, both fresh ids *and* reused ids started
failing to add new rows (fresh ids: `"ABI input failed: InvalidLength"`
directly; reused ids: deposits kept returning contract-level success
with no corresponding new row, confirmed via a dedicated diagnostic
re-run with per-batch, uniquely-named output files, ruling out an
earlier suspected harness bug where a `$$`-based temp filename
collision had been silently overwriting earlier batches' results).
This is written up as a new finding in `docs/PLATFORM_FINDINGS.md`
rather than chased further, since root-causing exactly what the cap
tracks is not needed for TASK M's own purpose.

**Working solution: `vault.seed_full`, a spike-only byte-splice
method.** Rather than construct the 252+32 state via real currency
registrations, a new `AVM_SPIKE_SEED`-gated, External-caller method
was added that takes the call body (after skipping a 4-byte
little-endian length prefix a `kind = "bytes"` ABI value's body()
carries ahead of the actual payload — confirmed empirically via a
diagnostic build reporting the exact byte count received, and the same
mechanism that was the unconfirmed suspicion behind the earlier
abandoned SEED-relay debugging pre-dating this task) and adopts it
directly as the new state via `FullReplace`, with no parsing,
struct-building, or currency registration at all. (An initial version
routed the body through `VaultState::parse` → `Vec<PendingWithdrawal>`
→ `sort_by_key` → `serialize()`; that alone pushed the spike's wasm
~7KB past the TASK-O-corrected combined deploy-envelope limit, since
it was dead-code-eliminated in every earlier build that never set
`AVM_SPIKE_SEED`. Switched to pure byte-slice splicing, which fixed
both the size regression and, incidentally, an off-by-one header bug
the first splice attempt had — `pending_count` was written into the
`ledger_count` field's byte offset, caught by cross-checking against a
from-scratch Python re-implementation of `parse()`'s exact validation
order once the real contract started rejecting the seeded state with
`ERR_INVALID_STATE`.) A sibling method, `vault.seed_pending`, which
only appends new pending records onto an already-real-deposited ledger
rather than replacing the whole state, was built first and used to
validate the byte-splicing technique at small scale before extending
it to the full-state case; both methods' real, build-tool-computed
`method_id`s were extracted directly from the built `contract.acab`'s
method table (reverse-engineered this round: `method_id:u64 LE +
reserved:u16 + input_type_id:u16 LE + reserved:u16 + name_len:u16 LE +
name bytes`, confirmed against the four already-known production
method ids before trusting it for the two new ones) — not guessed or
invented, per Hard Rule 3.

**The sweep.** Per K value: one fresh deploy (fresh `AVM_SPIKE_VARIANT`
each time, since re-deploying byte-identical wasm fails even at a new
salt), one `vault.seed_full` call carrying the exact 252-row + 32-
pending byte layout (`build_state_252_32.py`, a pre-existing, now
finally end-to-end-validated helper), one `vault.topup` probe as the
fuel-exhaustion signal — the same probe-as-signal technique TASK J
used, for the same reason (a real `WITHDRAW` at `pending_count == 32`
is legitimately rejected by the cap check regardless of fuel, which
would be mistaken for a fuel-exhaustion result).

| K | Result |
|---|---|
| 5,800 | PASS |
| 5,850 | PASS |
| 5,900 | PASS |
| 5,925 | FAIL (`VM status 31`) |
| 5,950 | FAIL |
| 6,000 | FAIL |
| 6,200 | FAIL |
| 6,600 | FAIL |
| 7,500 | FAIL |
| 10,000 | FAIL |
| 20,000 | FAIL |
| 50,000 | FAIL |

**Boundary: K = 5,900 safe / K = 5,925 fail** (bisected to this
resolution, not narrowed further). **Comparison to TASK J's 220+32
boundary (K = 5,800 safe / 5,850 fail): essentially unchanged — if
anything slightly higher, not lower**, despite 252+32 carrying 32 more
ledger rows than 220+32 and TASK J's own finding that ledger row count,
not pending count, dominates this cost. The row-count increase from
220 to 252 (+14.5%) did not measurably shrink the fuel headroom at this
resolution. Calibrated against the removed O(n²) check's 1,024-
comparison cost (unchanged from TASK J: `n=32` pending records, same
check, same shape): **5,900 / 1,024 ≈ 5.8×** headroom, indistinguishable
at this resolution from TASK J's 5.7× at 220+32. Applying the same ≥2×
decision rule: comfortably met, by the same margin as before. **No cap
change follows from TASK M** — `MAX_LEDGER_ENTRIES` stays `220` in
production; the spike's `300` was never a proposal to raise it.

**Updated:** `SPEC.md` (the 220-row sentence corrected in place with an
explicit note, not silently rewritten; a new bullet added documenting
the 252+32 re-measurement, comparison table, and conclusion);
`docs/PLATFORM_FINDINGS.md` (new entry for the session-wide
(account,currency)-pair registry cap). `spike/vault-m252/` and
`spike/actor-m252/` deleted after use, per Hard Rule 5.

### 43. TASK M2: making the headroom measurement internally consistent — a same-method control finds a real but modest row-dependency, no confound from aging within 300s, a fuel-budget-confounded callback-leg cross-check, and a corrected, lower headroom figure

**Why this task existed.** TASK M's 252+32 result (K=5,900 safe)
contradicted TASK J's own 220+32 finding (K=5,850 already traps),
with no explanation. The user declined to let that stand as "noise"
and required a proper control before trusting either number.

**Method.** A new spike (`spike/vault-m2/`, Hard Rule 5, deleted after
use) combined `vault.seed_full` (byte-splice full-state injection,
reused from TASK M) with a new append-only `vault.seed_pending`
(reused from TASK M's earlier, abandoned-then-fixed byte-splice
design) and the real-deposit-relay actor pattern
(`spike/actor-m2/`), all on ONE fresh `avm-local-stand` session (new
`--state` file) specifically to rule out interaction with the
session-wide currency-registry exhaustion documented in P22.

**Same-method control (fresh `seed_full` states, one session): a
real but modest row-dependency, not TASK J's cliff and not TASK M's
flat result.** 190+32 and 220+32 both resolve cleanly through
K=5,900 and trap at K=5,950 (bisected to 50). 252+32 resolves
cleanly through K=5,850 and traps at K=5,875 (bisected to 25). 252+32's
boundary sits 50–100 K below 190/220+32's — real, in the direction
TASK J predicted, but only a ~1–2% effect. TASK J's original claim
(190 clears K=5,850 cleanly, 220 traps AT exactly K=5,850) does not
reproduce: 220+32 clears K=5,850 cleanly in this control.

**Aging/rent confound: tested directly, not found at the shapes
reachable.** Real `DEPOSIT` calls through 4 relay actors built
genuinely real ledger rows; `vault.seed_pending` added 32 pending
on top without disturbing them; the stand's block-time counter was
advanced by 300 before probing. The real-deposit construction hit
the SAME session-wide registry exhaustion as TASK M (P22) even in
this brand-new session — proving that cap is not tied to the
`--state` file, a refinement to P22, below — so the achieved shapes
were 148+32 and 124+32, not the 220/252 targeted. At those shapes,
aged real-built states and fresh `seed_full` states at the same row
count and K agreed exactly (both PASS at 148+32/K=5,900 and
124+32/K=5,950). No divergence found. This does not prove no
confound exists at 220/252 (unreachable this round); it found none
at the smaller shapes tested.

**Callback-leg cross-check: diverged, but the divergence is a fuel-
budget artifact, not a finding about the callback leg's fragility.**
A real `WITHDRAW` (creating and resolving the 32nd pending record
in one call, through a relay actor, `--fuel-fee-shells 2,000,000`)
was compared against `vault.topup` (TASK J's original probe,
`--fuel-fee-shells 2,000`) on matching states. At K=5,900/5,950 both
agreed (PASS). Pushed to K=20,000 and K=50,000, they diverged: the
`WITHDRAW`/Settle leg still resolved cleanly at both; `vault.topup`
failed at both. The task's own rule ("if the callback leg fails at
*lower* K, use the callback number") does not trigger — the callback
leg never failed. The divergence is explained by the 1000×
difference in supplied fuel, not by anything about the callback path
being more expensive. **This surfaces a real, unresolved caveat,
logged in `docs/MAINTAINER_QUESTIONS.md`: the measured "headroom" is
a function of the triggering call's own `--fuel-fee-shells`, a
local-stand testing parameter with no identified production
analogue. More fuel buys more headroom; this engagement cannot say
what fuel a real caller would supply.**

**Conclusion.** Per the task's own rule, the LOWEST boundary found
for any realistic state is used: K=5,850 (252+32, this session's
control) — lower than TASK M's uncontrolled 5,900, closer to (not
"essentially unchanged from," stated plainly) TASK J's original
K=5,800. 5,850/1,024 ≈ 5.7× headroom. The ≥2× decision rule is still
comfortably met. **No cap change follows.** `SPEC.md`'s TASK M bullet
is amended in place (not silently rewritten) to mark its "essentially
unchanged" claim and 5,900/5,925 figures as superseded by this
control. `spike/vault-m2/` and `spike/actor-m2/` deleted after use
(TASK N2 and P22b, below, reused the same spike pair before deletion).

### 44. TASK M3: the Task-M/REVIEW-42 explanation of why 252+32 is the true worst case was wrong — corrected to the real mechanism (restores bypass `MAX_LEDGER_ENTRIES`, then partial withdrawals refill pending without removing rows)

**The error.** Entry 42 and `SPEC.md` both said a pending withdrawal
"does not remove its row from the ledger," letting 220 settled rows
and 32 simultaneously pending ones coexist. This is false:
`handle_withdraw` debits the ledger row it withdraws from
(`debit_ledger`, Invariant I9), and Invariant I9 removes a row once
its amount reaches zero. 220 rows with 32 of them "pending" is not a
state the contract's own logic can produce via that mechanism at all.

**The real mechanism (Phase 4 finding 1, previously correctly
identified there and never wired into this correction until now):**
restores bypass `MAX_LEDGER_ENTRIES` entirely and unconditionally
recreate the row they restore. Reachability is proven, not asserted,
by the existing host test
`handlers.rs::true_worst_case_state_is_252_rows_plus_32_pending`:
start from a full 220-row ledger; fully drain 32 of those rows (32
pending, ledger now 188); refill to 220 with 32 unrelated new
deposits; RESTORE (not settle) all 32 original pending withdrawals,
which recreates those 32 rows on top of the already-full 220 (252
rows, 0 pending); then each of the 8 depositors who hold the
restored rows makes 4 PARTIAL withdrawals, debiting without zeroing —
refilling all 32 pending slots without removing a single row. That
is how 252 rows + 32 simultaneously-pending is reached.

**Updated:** `SPEC.md` (the wrong explanation replaced with the
correct one, citing the host test by name); this entry's annotation
on REVIEW.md entry 42 (the original wrong paragraph kept, visibly
marked wrong, not deleted, per this project's correction convention).
Grepped every doc in `docs/` and `README.md` for "does not remove its
row" / "220 settled rows and 32" — no other occurrences found outside
`SPEC.md` and REVIEW.md entry 42, both now corrected.

### 45. TASK N2: CRITICAL — a Settle callback's body is silently dropped above exactly 65,536 bytes; TASK N's "resolves cleanly up to 10,000,000 bytes" conclusion was accurate but answered the wrong question

**The gap in TASK N.** TASK N confirmed the vault's Settle resolution
never crashes or fails at any callback-body size up to 10,000,000
bytes. It never checked whether the body the depositor actor attached
actually arrived at the vault's `handle_callback` — only that the
overall call reported success.

**Method.** A one-line, `AVM_SPIKE_SEED`-gated addition to
`handle_callback` (`spike/vault-m2/`, Hard Rule 5, deleted after use)
recorded the delivered body's exact length into `admin_seq` (a plain
`u32` header field, no cross-validation in `parse()`) instead of its
normal value. A paired depositor actor (`spike/actor-m2/`) attached a
runtime-generated `callback_result_body` of a build-time-configured
size to its payout-accept response. A real `DEPOSIT` then `WITHDRAW`
exercised a genuine Settle each time; `vault.topup` probed `admin_seq`
afterward.

**Result: a razor-sharp threshold at exactly 65,536 bytes (2^16),
bisected to single-byte precision and 100% reproducible.**

| sent | delivered |
|---|---|
| 10 | 10 |
| 10,000 | 10,000 |
| 50,000 | 50,000 |
| 65,536 | 65,536 |
| 65,537 | **0** |
| 70,000 / 80,000 / 90,000 / 130,000 / 262,144 / 1,000,000 / 10,000,000 | 0 |

One byte over 65,536, the delivered length is not 65,536 (truncated)
— it is exactly 0. The call's own status is `"ok"` identically above
and below the threshold; nothing distinguishes a successful delivery
from a silently dropped one from the outside. A secondary observation
(not confirmed as the same mechanism): the responding actor's own
`FullReplace` state update is also lost above the threshold — a
marker written in the same branch persists below 65,536 bytes and
reads back empty above it, suggesting the entire nested response, not
just the body field, is discarded once oversized.

**Why this matters more than the arena theory it was investigating.**
Phase 4 finding 4 and TASK N's own framing were about crashes/stalls
(none found). This is a silent data-integrity ceiling: an actor can
attach an arbitrarily large body to its payout response and the
withdrawal Settles exactly as if nothing were attached — the bytes
are gone, not malformed, not rejected, with no signal of any kind.
This specific vault never reads that body's content (only
`status_code`), so no fund-theft/mis-crediting path follows here —
but the finding reframes what TASK N actually established: not "large
bodies are safe," but "large bodies don't crash anything, and also
don't arrive."

**Updated:** `docs/PLATFORM_FINDINGS.md` new entry P23 (full method
and table); `docs/THREAT_MODEL.md` §5.12 (new paragraph) and §6 (new
bullet); `docs/MAINTAINER_QUESTIONS.md` #8 (rewritten around this
finding, "why it matters" revised again); `README.md`'s known-
limitations bullet (updated). All four now state explicitly that
neither this finding nor the original arena question has been tested
beyond the local stand. `spike/vault-m2/` and `spike/actor-m2/`
deleted after use, per Hard Rule 5 (both were reused across TASK M2,
N2, and P22b before deletion).

### 46. TASK P22b: checked P22's plateaued deposits for funds loss, using a vault call (not the actor's reply) to read ledger state — no loss found in either failure mode tested, but TASK M2's original shortfall was not reproduced either

**Method.** Two scenarios, each on a fresh vault+actor pair
(`spike/vault-m2/`, `spike/actor-m2/`, reused from M2/N2, deleted
after this task per Hard Rule 5), with the vault's own `vault.topup`
response fully decoded — every ledger row, not just header counts —
before and after each attempt. **The status read throughout is the
vault's own embedded state, not the depositor actor's reply**, per
the task's explicit requirement.

1. **Fresh currency ids, cap already hit:** fails loudly —
   `"local call ABI input failed: InvalidLength"` at the top level,
   before the message reaches contract execution at all. No row
   created (confirmed by full decode); no funds-loss risk, since the
   call never runs.
2. **Reusing already-registered ids against a brand-new account** (the
   scenario TASK M2 saw apparently plateau — `call_status == 0`
   reported, fewer new rows than batches sent): re-tested with full
   ledger decoding rather than count-based inference. **4 new rows
   were created correctly, each with the exact deposited amount** — no
   shortfall this time.

**Conclusion.** No funds-loss path confirmed in either mode directly
tested. TASK M2's original shortfall did not reproduce under this
more careful methodology. Given two genuine test-harness bugs were
already found and fixed earlier in this same investigation (a
`$$`/PID temp-filename collision silently overwriting earlier
batches' results; an `"ok"` vs `"ok 0"` string-match bug misreporting
every batch as failed), a third, undiagnosed harness artifact is the
more likely explanation for TASK M2's shortfall than a genuine
platform bug — but this is not proven; the exact prior conditions
(specific actors, very high cumulative call volume) were not
reproduced byte-for-byte. Stated as an open, unresolved discrepancy,
not asserted either way, per Hard Rule 6.

**Updated:** `docs/PLATFORM_FINDINGS.md` P22 (new sub-finding with
both scenarios' results and the conclusion above).

### 47. TASK P: header-aware `parse()` fuzz generator added; the nightly-only `rustfmt.toml` options are not this example's to fix, and the reasoning is stated rather than silently worked around

**(1) Header-aware fuzz generator, added alongside the existing
random-bytes test, not replacing it.** `parse_never_panics_on_random_bytes`
(Phase 5) draws fully random bytes at random lengths — with 4 fixed
magic bytes against, on average, ~15,000 fully random ones, almost
every iteration rejects at the very first check (`magic != MAGIC`)
and never exercises anything past it; the test still has value (it
covers the "garbage from byte zero" case and the magic check itself),
so it is kept, not removed. A new
`parse_never_panics_on_header_aware_random_bytes` always writes a
valid `magic`/`version`/`flags`/`reserved` header (so every one of
its 20,000 iterations gets past that gate) and randomizes
`owner_pubkey`, `pending_owner_pubkey`, `admin_seq`,
`next_withdrawal_id`, `build_flags`, and the declared
`ledger_count`/`pending_count` — including values large enough to
stress the `checked_mul`/`checked_add` length arithmetic those counts
feed into. About half of each run's bodies are sized to genuinely
match the declared counts (clamped to 300 rows / 50 pending so the
test doesn't try to allocate gigabytes), filled with random bytes —
garbage row *content* but self-consistent *shape*, so `parse()`
actually enters its per-row `validate_currency_shape`/zero-amount/
ordering-check loop instead of rejecting on the length check alone;
the other half keep a mismatched random-length body, to keep
exercising that path too. All 78 host tests pass, including both
fuzz tests (`cargo test --lib`, confirmed in this round's build).

**CORRECTION (Gate 3c TASK 1): the claim above — "about half of each
run's bodies are sized to genuinely match the declared counts" — was
FALSE, not a rounding error.** The actual code clamped the body size
to 300 rows / 50 pending while `declared_ledger_count`/
`declared_pending_count` themselves stayed fully random `u32`s
(unclamped) — so the body's real length matched the header's declared
counts only on the astronomically rare occasion a random `u32` count
happened to already be ≤300/≤50. Measured directly (re-running the
unmodified test with a reason tally added, test-first, before any
fix): of 20,000 iterations, 19,537 landed on `LengthMismatch` and only
463 (2.3%) ever got past the length check at all — not "about half"
by any reasonable reading. Fixed at Gate 3c TASK 1: the matching
branch now draws genuinely small `ledger_count`/`pending_count`
directly (not clamped after the fact) and builds the body from
exactly that many rows via dedicated `gen_ledger_row`/`gen_pending_row`
helpers, so the lengths match by construction. A second, independent
bug was found and fixed in the same pass: the shared `Lcg`'s
`below(2)` is not a 50/50 coin — this LCG's odd multiplier and odd
increment make its state transition `state + 1` modulo 2, so the
lowest bit of every `next()` call deterministically ALTERNATES; a
`below(2)`-gated branch choice came out 2.3%/97.7% instead of
~50%/50% for exactly this reason, confirmed by direct instrumentation
before being fixed. A new `coin()` method (bit 31, far enough from the
degenerate bit 0 that its period vastly exceeds this test's iteration
count) replaces every `below(2)` decision added this round; the two
pre-existing fuzz tests were left untouched; see Gate 3c's own REVIEW
entry for the full before/after tallies.

**(2) The nightly-only `rustfmt.toml` options are not fixed, and this
is a deliberate choice, not an oversight.** `cargo fmt --check` on
this example (stable 1.93.0, per `rust-toolchain.toml`) exits `0`
(formatting itself is fine) but prints 9 warnings —
`wrap_comments`, `format_code_in_doc_comments`, `normalize_comments`,
`format_macro_matchers`, `format_macro_bodies`, `imports_granularity`,
`group_imports`, `reorder_impl_items`, and `unstable_features` itself
— because `rustfmt.toml` sets options rustfmt only honors on a
nightly toolchain. **This file lives at the repository root
(`rustfmt.toml`, not anywhere under `examples/multi-currency-vault/`)
and governs formatting for the entire `avm-sdk` repository — every
crate under `crates/`, every other example, not only this one.**
Editing a shared, repo-root tooling config to silence warnings for
one example's own `cargo fmt --check` output is out of scope for this
engagement: it is not this example's file to own, the change would
apply repository-wide with no way to confirm it doesn't surprise
other code or contributors this engagement has no visibility into,
and the warnings do not affect formatting correctness (exit code `0`)
or any check this project's own Gate 3b package reports as a failure.
Stated here rather than worked around.

**Updated:** `src/lib.rs` (new fuzz test, `tests::fuzz_parse` module).

### 48. TASK R: `counting_allocator` re-measured at the true 252-row (+32-pending) worst case; stale doc comment fixed; a Settle-specific host test added, asserting the resulting state byte-for-byte

**What was stale.** The module's own doc comment quoted figures
("~45.7–46.4 KB", "~3.2× margin") from "Gate 2b round 3 TASK H's
restored 220/32 caps" — but the tests themselves (`deposit_at_252_row_
worst_case`, `withdraw_at_252_row_worst_case`,
`callback_at_max_ledger_and_max_pending_worst_case`) already used the
252-row fixture (an earlier round's work, not this one's), so the
comment was describing the wrong row count even though, by
coincidence, its quoted DEPOSIT/WITHDRAW figures turned out still to
be correct at 252 rows. More importantly, **the comment never
mentioned the callback measurement at all** — the actual highest of
the three figures, and the real basis for how much margin the budget
provides.

**Re-measured, `cargo test --lib counting_allocator -- --nocapture`:**
DEPOSIT at 252 rows: 46,087 bytes. WITHDRAW at 252 rows: 46,384 bytes.
CALLBACK (Restore) at 252 rows + 32 pending: 51,534 bytes. The
150,000-byte budget is **≈2.9×** the true highest figure (51,534),
not the ≈3.2× the old comment claimed (which was computed against the
lower DEPOSIT/WITHDRAW numbers, never checked against the callback
case). Comment rewritten in place with all four figures, the correct
margin, and the 220→252 worst-case correction cited by name
(`handlers.rs::true_worst_case_state_is_252_rows_plus_32_pending`),
rather than silently replacing the old text.

**New test: Settle at the same worst case, asserted byte-for-byte.**
The existing callback test only exercises Restore (`status_code != 0`).
A new `settle_at_max_ledger_and_max_pending_worst_case` builds the
identical 252-row + 32-pending fixture, runs a Settle
(`status_code == 0`) callback, and asserts the resulting state against
an independently constructed expected state — not just that it
succeeds, but that `pending.len()` drops from 32 to exactly 31 (only
`withdrawal_id` 1 removed) and the ledger is byte-for-byte identical
to the input (Settle credits nothing back). Measured allocator cost:
51,534 bytes — identical to Restore at this scale, confirming
`credit_ledger`'s own cost on Restore is not what dominates this
budget. All 79 host tests pass (`cargo test --lib`).

**Updated:** `src/lib.rs` (doc comment rewritten; new test added).

### 49. TASK S: README now states `AVM_EXAMPLE_VAULT_OWNER_PUBKEY`'s exact format requirement and that malformed values fail the build by design

Pure documentation addition, no code change — the enforcement already
existed (`src/constants.rs`'s `decode_hex_pubkey`/`hex_digit`,
confirmed by reading: `assert!(bytes.len() == 64, ...)` rejects empty
or wrong-length values, and `hex_digit`'s `match` only accepts
`b'0'..=b'9'` and `b'a'..=b'f'` — any uppercase hex digit or other
non-hex character falls into the `_ => panic!(...)` arm). The README's
existing top-of-file warning already said "64 lowercase hex digits"
in passing; the Build section now states explicitly that empty,
wrong-length, or uppercase/non-hex values fail the build **by
design** — a compile-time assertion, not a bug — so a reader hitting
that panic during their own build understands it immediately rather
than suspecting a toolchain problem.

**Updated:** `README.md` (Build section).

### 50. Independent fresh-context adversarial re-review (post-Phase-4): no new confirmed findings; one promising hypothesis investigated and refuted; coverage disclosure

A second, independent fresh-context reviewer (no access to this file or
`docs/PLATFORM_FINDINGS.md` while forming hypotheses, per this round's
own instructions) re-audited `SPEC.md`, `docs/THREAT_MODEL.md`,
`avm-contract.toml`, all of `src/*.rs`, and
`crates/avm-contract-sdk/src/{lib,input,output,runtime}.rs` from scratch,
focused on (a) fund theft/mis-crediting and (c) trap/DoS/panic/resource
exhaustion, with the explicit mandate to trace actual dispatch logic
rather than trust `SPEC.md`'s own claims. **No new finding met this
engagement's bar (a concrete failing test, or an exact input sequence
backed by direct source citation) this round.** `cargo test --lib`
(WSL, toolchain 1.93.0 per `rust-toolchain.toml`) passed 79/79 before
this round began and 79/79 after — no code changes were made.

**Investigated in depth and found no issue (ground now covered,
for the next reviewer):**

- Full `handle_deposit`/`handle_withdraw`/`handle_topup`/
  `handle_propose_owner`/`handle_accept_owner`/`handle_set_pause`/
  `handle_callback` check-ordering, re-derived independently against
  `SPEC.md` §4/§5's own stated order line by line (including the
  Phase-4-fixed `handle_set_pause` admin-seq-before-pause-flags order,
  and the Phase-4-fixed 1513→1529 restore-overflow code) — every
  ordering matches exactly; no further reordering bug found.
- Whether the ledger can ever genuinely exceed 252 rows
  (`MAX_LEDGER_ENTRIES + MAX_PENDING_WITHDRAWALS`) through *repeated,
  non-simultaneous* gap-fill-then-restore cycles across many
  independent withdrawals over time, rather than one single 32-wide
  batch — i.e., whether `handle_deposit`'s new-row cap check
  (`ledger.len() + new_row_count > MAX_LEDGER_ENTRIES`, evaluated
  against the *current* `ledger.len()` at each individual deposit, not
  against some separately-tracked "excess" counter) could be exploited
  to ratchet the ledger arbitrarily far past 252 by reusing freed
  pending slots across many separate rounds. Worked through the
  arithmetic directly: once `ledger.len()` is ever pushed above 220 by
  a restore, every *subsequent* genuinely-new-row deposit requires
  `ledger.len() + 1 <= 220` at the moment it is evaluated, which is only
  reachable by simultaneously gapping (via pending, uncompleted
  withdrawals) at least as many rows as the current excess plus one —
  and simultaneous gaps are hard-capped by `MAX_PENDING_WITHDRAWALS`
  (32) globally. This makes 252 a genuine hard ceiling, not merely an
  empirically-observed one; confirms (independently, not merely by
  trusting the existing `true_worst_case_state_is_252_rows_plus_32_pending`
  test) that `state.rs::parse()`'s lack of an explicit upper-bound check
  on `ledger.len()` is not exploitable, since the producing logic cannot
  generate a state violating it.
- Whether an `Internal` caller could dispatch `TOPUP_METHOD_ID`/
  `PROPOSE_OWNER_METHOD_ID`/`ACCEPT_OWNER_METHOD_ID`/`SET_PAUSE_METHOD_ID`
  by setting that literal numeric `method_id` on an `OutboundIntent`
  (nothing stops a contract from choosing an arbitrary `u64` there), and
  the symmetric case of an `External` caller targeting
  `DEPOSIT_METHOD_ID`/`WITHDRAW_METHOD_ID` directly on the wire bypassing
  the ABI-name-to-hash ClI path — both already correctly rejected via the
  caller-type check in every affected handler, and both already covered
  by existing tests (`external_caller_sending_internal_only_ids_is_rejected`,
  `internal_caller_sending_external_only_ids_is_rejected`).
- Whether `CODE_UPGRADE_METHOD_ID` could ever numerically collide with a
  genuine `withdrawal_callback_method_id(id)` (upper-32-bit tags differ:
  `0xc2c6_c51b` vs `CALLBACK_MAGIC_TAG = 0x5744_4c43` — provably
  disjoint for any `id`) or with `DEPOSIT_METHOD_ID`/`WITHDRAW_METHOD_ID`
  (`NORMAL_METHOD_TAG = 0x4d43_5654`, also disjoint) — confirmed disjoint
  by direct constant inspection; the upfront
  `method_id == CODE_UPGRADE_METHOD_ID` check in `lib.rs::process_message`
  can never misfire against this contract's own method ids.
- Reentrancy ordering for the two-intent (new-withdraw-and-new-deposit)
  variant `SPEC.md` §8 test 11 names as not independently re-verified by
  a dedicated real-stand fixture: re-derived from the `message_isolated`
  execution model itself (`crates/avm-contract-sdk/src/runtime.rs`'s own
  "one entrypoint on a fresh instance" framing) that a reentrant message
  the depositor's own payout handler constructs cannot possibly observe
  a pre-debit state, because the withdrawal's debit and pending-insert
  are committed to this account's own state as part of the *same* call
  that emits the outbound payout, strictly before that payout message is
  even sent, let alone before any reply the depositor's own code could
  trigger is itself delivered back to the vault as a wholly separate,
  later `process_message` invocation. This is an analytical
  confirmation of the existing claim, not a new empirical test (no new
  risk found, but also no new proof beyond what P4 and the existing CEI
  host test already establish).
- `state.rs::parse()`'s defensive arithmetic
  (`ledger_count.checked_mul(LEDGER_ENTRY_LEN)` etc.) under the
  32-bit-`usize` `wasm32-unknown-unknown` compile target versus the
  64-bit host `cargo test` target: a maliciously huge `ledger_count`
  overflows `checked_mul` on the real 32-bit target (correctly rejected
  via `ERR_INVALID_STATE`, no panic — `checked_mul` cannot panic by
  definition) but would *not* overflow on the 64-bit host test target at
  the same value: on the host, it instead falls through to the
  `pending_end != bytes.len()` length-mismatch check and is rejected
  there instead. Both platforms reject with no panic, via different
  checks — a behavioral footnote worth recording, not a safety gap (see
  disclosure below: not independently verified by actually compiling to
  `wasm32-unknown-unknown` and running there this round).
- Whether `handle_deposit`'s all-existing-row no-op success path still
  correctly enforces `deposits_paused` (it does — the pause check runs
  before credits are even computed, so a wholly-empty/no-op `DEPOSIT`
  while paused still fails `1506`, never silently succeeds as a no-op).
- Whether `LedgerKey`'s `currency_tag` field genuinely prevents
  `Main(currency_id=0)` and `Extra(currency_id=0)` from colliding as the
  same ledger row — confirmed they are distinct keys (tag is part of the
  key), with a `currency_id` value of `0` being an ordinary, legal
  non-SHELL `Extra` currency, unrelated to `Main`'s own conceptual
  `currency_id=0` marker.
- Whether a restore's `credit_ledger` call could double-count value if a
  depositor re-deposits into the exact same key while an earlier
  withdrawal on that key is still pending (the row having been removed
  by I9): worked through concretely — restoring after an intervening
  re-deposit correctly *adds* onto the re-deposited balance, and this is
  not a double-credit, because the amount being restored was never
  actually removed from the vault's own real balance in the first place
  (per the A8 finding `SPEC.md` §1 already cites) — the intervening
  deposit is a wholly separate, genuinely-real credit.

**One hypothesis investigated, found plausible, and then refuted by
cross-referencing a sibling example — recorded so a future reviewer
does not re-spend time on it.** `MessageInput.required_state_output_mode`
(`crates/avm-contract-sdk/src/input.rs` line 311) is read from the wire
on every call and nothing in this vault's own code (`handlers.rs`'s
`success_with_state` always constructs `StateUpdate::FullReplace`
unconditionally) ever checks or branches on it. Reading
`crates/avm-contract-sdk/src/output.rs`'s `message_output_len_for`/
`validate_message_shape` shows that if the host ever required
`DataPatchList` for a call to this account, this mismatch would cause
`dispatch_message_inner` to return `0` — a total, opaque dispatch
failure with no status code at all, strictly worse than any of this
contract's 25 documented error codes, for *every* call type. This
mechanism is real by direct source reading (the same bar Phase 4's own
finding 4 was raised on), and nothing in `SPEC.md`/`docs/THREAT_MODEL.md`
discusses `required_state_output_mode` at all (confirmed by an exhaustive
grep of both files — zero matches) — looked initially like a genuine,
previously-unconsidered state-space gap. **Refuted before being written
up as a finding**: `examples/contract-batch-patch/src/lib.rs` (a sibling
example in this same repository, declared `execution_mode =
"account_batch"`) shows this field tracks the *entry point actually
invoked*, not something that varies freely within one execution mode —
its own `init`/`process_message` (the `message_isolated`-shaped
entrypoints every contract exports) is checked/used only under
`FullReplace`, while only its dedicated `process_batch` entrypoint ever
sees `DataPatchList`; `examples/README.md` independently confirms this
same split in prose ("Initialization uses `FullReplace`; batch execution
... returns a `DataPatchList`"). Since this vault declares
`execution_mode = "message_isolated"` in `avm-contract.toml` and never
exports a batch entrypoint at all (`lib.rs` uses
`export_message_contract!`, not `export_batch_contract!`), there is no
code path by which the host would ever require anything but `FullReplace`
for a call dispatched to `process_message`/`init` here. Not a real risk
for this contract; recorded in detail specifically so the next reviewer
sees the refutation rather than re-discovering the same dead end.

**Coverage disclosure, stated plainly (this round added no new
automated tests and made no code changes):**

- Did not run `avm-local-stand`/`avm-contract-build` this round, despite
  confirming both are installed and reachable in this environment
  (`<home>/.local/bin/{avm-local-stand,avm-contract-build}` under
  WSL) — no real-platform experiment was attempted; everything above is
  grounded in source reading and host-level `cargo test --lib` only.
  Any hypothesis whose real-platform reachability is genuinely unknown
  (there were none strong enough to write up as findings this round) is
  therefore exactly as unverified as before this round started.
- Did not compile or run anything against the actual
  `wasm32-unknown-unknown` target; the 32-bit-vs-64-bit `usize` footnote
  above is reasoned from the language's own `checked_*` semantics, not
  observed on real wasm32.
- Did not re-run `cargo mutants` (the existing `mutants.out/` in this
  directory is from a prior round); only inspected its `missed.txt`/
  `timeout.txt` (four surviving mutants, all `|`-to-`^` on provably
  disjoint-bit constants — equivalent mutants, not coverage gaps) rather
  than generating a fresh report.
- Did not attempt to independently re-verify any of the empirical
  platform findings already recorded in `docs/PLATFORM_FINDINGS.md`
  (SHELL-exhaustion behavior, rent rates, the `TOPUP` real-network
  viability question, the 65,536-byte callback-body truncation, etc.) —
  these remain exactly as confirmed/unconfirmed as this file already
  states them.
- Did not attempt to extend the existing `property` module's random
  operation-sequence coverage (currently 3 depositors × 2 currencies,
  `Extra` tag only, deposit/withdraw/callback only — no `Main`-tag
  withdrawals, no admin pause/rotation interleaving, no `TOPUP`) to a
  wider state space; reasoned about several specific extensions by hand
  (see above) rather than running a larger generated sweep.
- Did not review `docs/getting-started.md`, `docs/cli-reference.md`,
  `docs/fuel-and-fees.md`, or `docs/MAINTAINER_QUESTIONS.md` in this
  round (only `SPEC.md`, `docs/THREAT_MODEL.md`, `avm-contract.toml`,
  `src/*.rs`, and the SDK's four `crates/avm-contract-sdk/src/*.rs`
  files, per this round's own scope).

### 51. Gate 3c (short round): fuzz-coverage defect fixed test-first, doc-count/wording corrections, an isolating Settle-fuel experiment that overturns TASK M2's "more fuel buys more headroom" conclusion and finds SPEC's own "safe" K = 5,850 figure traps the real Settle leg, nightly-format applied to this example, and the mutants.out provenance stated plainly

**TASK 1 — the fuzz coverage defect, fixed test-first.** REVIEW entry
47's claim that "about half" of `parse_never_panics_on_header_aware_
random_bytes`'s iterations genuinely matched the header's declared
counts was false: the body was clamped to 300/50 rows while the
declared counts stayed fully random, unclamped `u32`s, so real
lengths matched only by astronomical coincidence (measured: 463/20,000
= 2.3%, not "about half"). Fixed by drawing small counts directly in
the matching branch and building the body from exactly that many rows.
A second, independent bug surfaced during the fix: the shared `Lcg`'s
`below(2)` is not a 50/50 coin (odd multiplier, odd increment ⇒ state
transition is `+1 mod 2` ⇒ the lowest bit of `next()` deterministically
alternates), fixed with a new `coin()` method reading bit 31. Final,
re-verified tallies (`cargo test --lib tests::fuzz_parse -- --nocapture`,
this round, see the full raw output below):
`header_aware`: `passed_length_check=10122/20000 (50.6%)`, 8 distinct
`Err` reasons (`LengthMismatch`, `BadLedgerCurrencyShape`,
`UnsortedLedger`, `ZeroLedgerAmount`, `BadPendingCurrencyShape`,
`ZeroPendingAmount`, `NextWithdrawalIdTooLow`) — both the ≥25%-
past-length-check and ≥5-distinct-reasons bars cleared with margin.
A new `parse_never_panics_on_mutated_valid_state_reaches_every_named_
branch` test confirms the existing mutation strategy (reused unchanged
from the pre-existing, untouched `parse_never_panics_on_mutated_valid_
state`) reaches all 6 named branches the task asked for
(`UnsortedLedger`, `ZeroLedgerAmount`, `BadLedgerCurrencyShape`,
`UnsortedPending`, `PendingOverCap`, `NextWithdrawalIdTooLow`), via a
mix of the existing random mutations plus 6 new targeted ones built
directly. REVIEW entry 47 was corrected in place (the false claim kept
visible, marked wrong, not deleted — this project's standing
convention) and now points here for the full tallies. `README.md`'s
Testing section rewritten to describe what the tests actually do
rather than repeat the false claim.

**TASK 2 — doc fixes.** (a) `SPEC.md` §2.4's lone remaining bare
`MIN_OPERATIONAL_SHELL = 1300000` corrected to `1500000`; grepped the
whole repo for `1,300,000`/`21,639` — every other occurrence is inside
a clearly marked historical note, left as-is. (b) `README.md`'s
"11 questions" corrected to "12" (`docs/MAINTAINER_QUESTIONS.md`
genuinely has 12 numbered questions); grepped for other hard-coded
counts, none found needing correction. (c) `README.md` Q8 and
`docs/THREAT_MODEL.md` §5.12/§6's "not understood"/"not fully
understood" language replaced with the real P23 mechanism (callback
bodies above 65,536 bytes are never delivered; the arena is 262,144
bytes, so the arena-exhaustion theory was never exercised at any size
that actually arrives) — "local stand only" kept throughout; grepped,
zero remaining "not understood" anywhere in the three files. (d) A
new `README.md` Known-limitations bullet added for the Q12 fuel
caveat (now superseded in part — see TASK 3 below — but the bullet is
updated to reflect the superseding finding, not left stating the
withdrawn claim). (e) `README.md`'s internal round/task/gate labels
replaced with plain descriptions and `REVIEW.md` pointers throughout;
`SPEC.md` §1's ~186-line TASK J/M/M2/M3 narrative replaced with a
~62-line facts-only version (REVIEW entries 35/42/43/44 already carry
the full narrative independently, confirmed before trimming — no
information lost, moved per the task's own instruction) — now further
revised by TASK 3 below.

**TASK 3 — M4, the callback-fuel reconciliation experiment. The
single highest-value result this round; see `docs/PLATFORM_FINDINGS.md`
P24 and `docs/MAINTAINER_QUESTIONS.md` #12 for the full writeup.**
Summary: a throwaway relay-actor spike (`spike/vault-m4`,
`spike/actor-m4`, Hard Rule 5, deleted after use) decoupled, for the
first time in this repo's experiments, the OutboundIntent's own
`fuel_fee_shells` field from the external call's own
`--fuel-fee-shells`, and separately checked the `WITHDRAW`'s own
synchronous leg (actor status) against the Settle *callback's* own
re-entry dispatch (a vault-side probe reading `pending_count` back).
Findings: the intent's own fee field is irrelevant (0 works identically
to any other value); the lowest working external fee is 1 shell (0 is
rejected outright by an unrelated minimum-fee admission gate,
`RouteRequiresFee`); the Settle leg's own K-trap boundary (bisected:
**K=590 resolves, K=610 traps** — see the Gate 3d TASK M5 correction
below: this exact figure did NOT reproduce under repeated, controlled
re-testing next round; the real boundary for this configuration is
K≈16,150, roughly 26× higher — on a minimal one-row ledger) is identical
at fee=1, fee=10, and fee=2,000,000 — a 2,000,000× range with zero
movement, directly answering the task's question (**does not scale
with the triggering fee** — this conclusion held up even after the
boundary itself was corrected); and SPEC.md's own "safe" K = 5,850 figure
(measured via the synchronous `vault.topup` probe, not a real Settle
round trip) was retested against the real Settle leg and **traps**,
even on a trivial one-row state at a 2,000,000-shell fee. TASK M2's
K = 50,000 result and its "more fuel buys more headroom" conclusion
are explained, not guessed at — most likely TASK M2 never isolated the
Settle re-entry dispatch from the `WITHDRAW`'s own fee-scaled
synchronous leg the way this experiment's relay-actor design does —
but this cannot be confirmed with certainty since `spike/vault-m2`/
`spike/actor-m2` are already deleted. **What this round did not test:
whether the real, un-padded Settle callback at the true 252-row +
32-pending worst case resolves cleanly today** — this experiment used
a minimal one-row ledger specifically to isolate the fee/K
relationship from real per-row parsing cost, and did not re-run the
full worst-case state through a genuine async Settle round trip.
Flagged as a higher-priority open question than before this round, not
resolved. `SPEC.md` §1's "no cap change follows" conclusion is
corrected to state it is validated only for the probe's own
synchronous dispatch, not for the real Settle callback.
`spike/vault-m4/`, `spike/actor-m4/` deleted after use (Hard Rule 5).

**TASK 4 — format.** `.github/workflows` does not exist anywhere in
this repository (checked with `find`, read-only, no edits) — there is
no CI fmt command to read. Nightly installed
(`rustup toolchain install nightly --profile minimal -c rustfmt`,
succeeded). `cargo fmt --check` (stable) on this example found real
diffs predating any formatter run this round (introduced by this
round's own TASK 1 edits to `src/lib.rs`/`src/state.rs` never having
been run through `cargo fmt`) — fixed with `cargo fmt`.
`cargo +nightly fmt --check` then found further diffs, entirely
explained by the root `rustfmt.toml`'s nightly-only options
(`imports_granularity = Item`, `group_imports = StdExternalCrate`,
doc-comment/macro formatting, `reorder_impl_items`) applying repo-wide
import-splitting and minor reordering — not specific to this round's
new code. Applied with `cargo +nightly fmt`, scoped to this example's
own files only; the root `rustfmt.toml` was not touched (confirmed via
`git diff --stat rustfmt.toml`, empty). Both `cargo fmt --check`
(stable) and `cargo +nightly fmt --check` now exit 0. Full test suite
(80/80) and both clippy targets re-run clean after the reformat.

**TASK 5 — mutants provenance.** The existing `mutants.out/` in this
directory is dated 2026-10-24 14:34–14:42 (`caught.txt`/`missed.txt`/
`outcomes.json` mtimes); `src/state.rs`/`src/lib.rs` were last modified
2026-10-29 05:20 by this round's own TASK 1 edits and TASK 4 reformat.
**The pasted `cargo mutants` output is from an earlier round, not this
one.** Production code's *external* behavior is unchanged (`parse()`'s
signature and semantics are identical; the `ParseFailureReason`/
`parse_with_reason` split is a behavior-preserving internal refactor,
confirmed by the unchanged 80/80 test pass and unchanged ACAB/
descriptor hashes below) — but its *internal structure* in
`src/state.rs` did change this round, so the older mutants run no
longer tests the exact current source text, only code with identical
behavior. A fresh `cargo mutants` run was not performed this round
(out of the stated "short round" scope); stated plainly rather than
silently reused.

**Build hashes/sizes, this round (unchanged ABI, new wasm bytes from
the TASK 1 refactor + TASK 4 reformat):** `contract.wasm`
`b7e8a36c3e7c65706cbbd95f8337d5cbfaaa898e4e33ba5c0526fe3a6646d4a3`
(55,703 bytes); `contract.acab`
`196e44ac93c3c87a5312fd22c6250ccd9d6a6310921fbc8a4a1706ba52aa3f4c`
(3,182 bytes, unchanged from every prior round — ABI untouched);
`contract.descriptor`
`34d33b9b84624e00347780c7f89f1481e02577b7841ea26188b4af7788c62f9e`
(66 bytes, unchanged).

**Updated this round:** `src/state.rs`, `src/lib.rs` (TASK 1, then
reformatted by TASK 4); `REVIEW.md` entry 47 (correction) and this
entry; `SPEC.md` §1 and §2.4; `README.md` (Testing, Known limitations,
question count, internal-label removal); `docs/THREAT_MODEL.md` §5.12/
§6; `docs/MAINTAINER_QUESTIONS.md` #8 and #12;
`docs/PLATFORM_FINDINGS.md` new entry P24. No commit, no push.

### 52. Gate 3d: TASK M5 — the decisive headroom measurement (a correction to TASK M4's own boundary, real Settle/Restore K-boundaries at every real state size, the removed check's real cost measured directly, and the ≥2× rule FAILS); TASK E1 — full real-stand E2E journey on the current build; TASK E2 — fresh mutants run; TASK D1 — docs synced to the M5 numbers

**TASK M5 — summary (full detail in `docs/PLATFORM_FINDINGS.md` P25
and `SPEC.md` §1).** A fresh spike family (`spike/vault-m5/`,
`spike/vault-m5strip/`, `spike/actor-m5/`, Hard Rule 5, all deleted
after use) measured, for the first time with a real relay actor and
byte-decoded resolution (never receipts or the actor's own reply),
the Settle and Restore callback's own fuel headroom at every real
state size from 1 row up to the true 252-row + 32-pending worst case,
and the removed per-depositor check's own real K-equivalent cost.

**A correction to TASK M4's own boundary, found before any other
number in this task could be trusted.** Re-testing TASK M4's exact
1-row+1-pending/K=610 configuration on this round's fresh build
resolved cleanly, where M4 reported it traps. Chased down
systematically: (1) a real bug was found and fixed along the way —
this round's first dummy-work-loop draft used 32-byte arrays instead
of the established 64-byte calibration; (2) fixing that did not
reproduce M4's boundary either (resolved past K=16,000); (3) two
further differential tests — a vault stripped of all `seed_full`/
`seed_pending` dispatch code, and an actor matching M4's own 101-byte
withdraw-body shape exactly — both still resolved at K=610. **TASK
M4's own K=590/610 figure does not reproduce under this round's
repeated, controlled re-testing; the real boundary for that exact
configuration is K≈16,150 (16,140 resolves, 16,160 traps), roughly 26×
higher.** This round's own entry above (51) is annotated with this
correction, not silently edited. The cause of M4's own wrong number is
not confirmed — that spike is already deleted — in the same spirit as
TASK M2's earlier correction of TASK M's own uncontrolled sweep: a
single, non-repeated bisection pass in this codebase's history has now
twice produced a figure that did not survive a controlled re-test.

**Settle-leg K-boundary, bisected, at every real state size (fee =
2,000,000 throughout; the OutboundIntent's own `fuel_fee_shells` field
and the external call's own fee were both already shown irrelevant by
TASK M4, re-confirmed in passing):**

| State | K resolves | K traps |
|---|---|---|
| 1 row + 1 pending | 16,140 | 16,160 |
| 100 rows + 32 pending | 12,625 | 13,000 |
| 190 rows + 32 pending | 10,025 | 10,350 |
| 220 rows + 32 pending | 9,300 | 9,650 |
| **252 rows + 32 pending (true worst case)** | **8,350** | **8,440** |

Headroom falls smoothly with ledger row count (≈100 K-units per row,
not a cliff), confirming TASK J's original row-count-dominance finding
across five sizes instead of two.

**Restore-leg K-boundary at 252+32: 8,200 resolves / 8,350 traps** —
below Settle's own boundary at the identical state, as the task's own
hint predicted (Restore writes back into the ledger; Settle does not).

**K-cost of the removed check, measured directly rather than
theoretically calibrated: ≈13,190.** A spike-only, build-time-gated
reinstatement of the removed O(n²) per-depositor check (own real
shape: a full linear scan per pending record, 8 depositor groups × 4,
matching its own architectural cap) traps the Settle leg at the true
252+32 worst case **even at K=0** — its own real cost alone already
exceeds that state's entire ~8,400-unit measured headroom. To get a
finite number, the same check was measured at a 1-row ledger instead
(≈16,150 units of no-check headroom to spend): with the check enabled,
the boundary drops to K≈2,960 (2,800 resolves, 3,125 traps). 16,150 −
2,960 ≈ 13,190 — roughly **13× every earlier round's own theoretical
calibration** ("1,024 comparisons," derived from the check's own
pairwise-key-compare shape and used unverified in every headroom
figure since TASK J). The real check, built from idiomatic Rust
iterator chains (`pending.iter().filter(...).count()`), costs far more
per logical comparison in compiled wasm than the raw-array-compare
dummy loop the calibration was based on.

**Headroom = K-boundary(252+32) ÷ K-cost(removed check): Settle
≈8,400/13,190 ≈ 0.64×; Restore ≈8,275/13,190 ≈ 0.63×.** **The ≥2× rule
this section has applied since TASK J fails outright** — the removed
check's own real cost is larger than the *entire* measured headroom at
the true worst case, independently confirmed by the direct K=0 trap
observation above (not merely a ratio computed from two numbers that
could individually be off). This is the first time this engagement has
measured the removed check's own cost in the same units as the
headroom it's being compared against, rather than a hand-derived
theoretical estimate — and the real figure invalidates every "no cap
change follows" conclusion this section has stated since TASK J.

**K=0 sanity (task e): the current, check-removed production logic
resolves cleanly for both Settle and Restore at 252+32 with zero dummy
padding** — confirmed directly on this round's K=0/no-old-check spike
baseline (functionally identical to real `parse()`/`handle_callback`;
the real worst-case-state construction host test,
`handlers.rs::true_worst_case_state_is_252_rows_plus_32_pending`,
independently confirms this exact state is reachable at all).
**Whether the earlier real-call marathon (entry 42, 287 rows + 32
pending, 32 Settles + 1 Restore, byte-decoded) itself ran with the
dummy-K loop at zero: not confirmed.** The available record states its
purpose (reachability and Settle/Restore correctness, not fuel
exhaustion) and that it predates the K-sweep's own separate,
byte-splice-based construction phase — consistent with K=0 — but does
not say so explicitly, and `spike/vault-m252/` is already deleted and
cannot be re-inspected. Stated as inferred, not established, per Hard
Rule 3.

**No fix chosen, per this round's own instruction. Options, with
their own costs, for the user's decision:**

| Option | What it does | Cost |
|---|---|---|
| Lower `MAX_LEDGER_ENTRIES` | Shrinks the true worst-case row count, directly buying back headroom (≈100 K-units per row removed) | Shrinks the 24,359-byte worst-case state and `MIN_OPERATIONAL_SHELL`'s own derivation; a real capacity cut for legitimate depositors; needs its own re-derivation of the 252-row-worst-case mechanism (restores bypassing the cap) since the margin calculus (how many restores-on-top-of-full-ledger are tolerable) changes with the cap itself |
| Lower `MAX_PENDING_WITHDRAWALS` | Shrinks pending count, which this round confirmed is NOT the dominant cost driver for the lean `parse()` (row count is) | Mostly ineffective on its own per this round's own data — pending count affects the removed check's cost (now moot, it's gone) much more than the current lean `parse()`'s cost; lowers the per-depositor collusion-cost analysis's own denominator too |
| Make `parse()`/`handle_callback` itself cheaper | Directly buys headroom without touching either cap | Requires actually profiling/optimizing the hot path (not attempted this round); risk of introducing a new bug in validated logic; no guarantee of hitting 2× without knowing where the remaining real cost (beyond the removed check, which is already gone) actually goes |
| Accept <2× headroom as the new baseline, document it plainly | No code change | Means this contract ships without the safety margin its own `SPEC.md` has claimed since TASK J; every future cap-adjacent change must be re-measured against a much tighter real margin than previously believed |
| Some combination of the above | — | Compounds the respective costs; not decided here |

**TASK E1 — full real-stand E2E journey, current production build
(wasm `b7e8a36c...`, 55,703 bytes), decoded-state evidence per step:**

| Step | Action | Evidence (decoded state) | Result |
|---|---|---|---|
| 1 | Real `DEPOSIT` (currency 888, 5,000 units) via relay actor | `ledger_count` 0→1 | PASS |
| 2 | Real `WITHDRAW` (1,000 units) → Settle (accepting actor) | `next_withdrawal_id` 0→1, `pending_count` stays 0 (resolved) | PASS |
| 3 | Real `WITHDRAW` (500 units) → Restore (rejecting actor) | `ledger_count` 1→2, `next_withdrawal_id` 1→2, `pending_count` stays 0 (resolved via restore) | PASS |
| 4 | `SET_PAUSE` (owner-authorized, vault rebuilt with `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` set to a real stand identity, per `SPEC.md` §1's documented two-step pattern) | `admin_seq` 0→1, call status `ok` | PASS |
| 5 | Owner rotation: `PROPOSE_OWNER` (current owner) then `ACCEPT_OWNER` (new owner, a different stand identity) | `admin_seq` 1→2→3, both calls status `ok` | PASS |
| 6 | `WITHDRAW` against a vault funded with only 10,000 shells (far below `MIN_OPERATIONAL_SHELL` = 1,500,000) | `next_withdrawal_id`/`pending_count` unchanged (rejected before any state write, I17's own signature) | PASS |

All six steps pass on the exact current production build. **Hash
labels, stated plainly per this round's own instruction:** the hash
`avm-contract-build` itself prints (e.g. `b7e8a36c...` for
`contract.wasm`) is **BLAKE3** (an AVM-internal canonical content/
identity hash, confirmed via the tool's own linked symbols — `README.md`
"Build artifacts"); the separate `sha256sum`-computed hash (e.g.
`8c094d96...` for the same file) is the real **SHA-256**. Using the
SET_PAUSE/owner-rotation steps required rebuilding the contract with
`AVM_EXAMPLE_VAULT_OWNER_PUBKEY` set to one of the stand's own
generated identity pubkeys first (`SPEC.md` §1's own documented
two-step pattern, not a new mechanism) — this build's ACAB/descriptor
are confirmed byte-identical to the default demo-key build's (only
`init()`'s baked-in `owner_pubkey` differs), and the default demo-key
build was restored immediately after for all other measurements in
this round.

**TASK E2 — fresh `cargo mutants` run, this round (not reused from an
earlier one):**

```
Found 204 mutants to test
ok       Unmutated baseline in 1s build + 2s test
204 mutants tested in 10m: 4 missed, 181 caught, 18 unviable, 1 timeouts
```

| Mutant | Category | Argument |
|---|---|---|
| `constants.rs:27:62` — `DEPOSIT_METHOD_ID`, replace `\|` with `^` | MISSED | Provably equivalent: `(NORMAL_METHOD_TAG << 32) \| 1` and `... ^ 1` are bit-identical, since bit 0 of `NORMAL_METHOD_TAG << 32` is always 0 (the operands never share a set bit). No test can distinguish them because no input makes them differ. |
| `constants.rs:28:63` — `WITHDRAW_METHOD_ID`, replace `\|` with `^` | MISSED | Same argument, bit 1 instead of bit 0 — equivalent for the identical reason. |
| `constants.rs:46:32` — `withdrawal_callback_method_id`, replace `\|` with `^` | MISSED | Same argument: `CALLBACK_MAGIC_TAG << 32` occupies only the upper 32 bits, `withdrawal_id as u64` (a zero-extended `u32`) only the lower 32 — no shared bit for ANY `withdrawal_id` value, so `\|`/`^` are equivalent for the full domain, not just the tested cases. |
| `constants.rs:250:57` — `decode_hex_pubkey`, replace `\|` with `^` | MISSED | Same argument: `hex_digit(...) << 4` occupies the high nibble, the second `hex_digit(...)` call the low nibble — no shared bit for any valid hex-digit pair, so equivalent across the whole domain. |
| `constants.rs:251:15` — `decode_hex_pubkey`, replace `index += 1` with `index *= 1` | TIMEOUT | Not a coverage gap: `index` starts at 0, and `0 *= 1` stays 0 forever, producing a genuine infinite loop — `cargo-mutants` reports this as a separate `timeout` category rather than `caught`, but the mutant is effectively detected (the test hangs rather than passing), not silently surviving. |

All 4 missed + 1 timeout mutant are accounted for with a one-line
argument each; none represent a genuine coverage gap. This matches the
same 4 missed-mutant pattern the Gate-3b-round `mutants.out` already
recorded (confirming the `ParseFailureReason` refactor changed nothing
about this class of equivalent mutant) plus one newly-surfaced timeout
this round's fresh run caught that the stale `mutants.out` had not
separately reported.

**TASK D1 — docs updated to the M5 numbers.** `README.md` (Build
artifacts table: new wasm size/hash; Known-limitations fuel bullet
rewritten to the M5 headroom-failure result). `SPEC.md` §1 (the
5.7×/"no cap change follows" figures replaced with the full M5 result
and options list; the deploy-envelope combined-size/margin figures
re-measured for the current, smaller wasm). `docs/MAINTAINER_QUESTIONS.md`
#11 (envelope margin re-measured) and #12 (rewritten: ANSWERED, with
the M4 correction, the full M5 numbers, and the headroom-failure
conclusion). `docs/PLATFORM_FINDINGS.md` new entry P25 (full M5
writeup, closes P24's own open question). Grepped the whole repo for
`5.7×`/`5,850`/"no cap change" — all remaining occurrences outside
this file are either `REVIEW.md`'s own historical entries (left as the
record of what each round found at the time, per this project's
standing convention) or this round's own new citations of the
now-superseded figures for comparison, not live claims.

**Updated this round:** `SPEC.md` §1; `README.md` (Build artifacts,
Known limitations); `docs/MAINTAINER_QUESTIONS.md` #11, #12;
`docs/PLATFORM_FINDINGS.md` new entry P25; this entry and entry 51's
correction annotation. No commit, no push.

### 53. Gate 3e: the ≥2× rule dropped as unsatisfiable (user decision, per measured per-row/per-pending costs); TASK F1 — a callback fast-dispatch spike, gate-measured and passed; TASK F2 — the fast path implemented in production, test-first, with full re-verification; TASK F3 — docs updated throughout

**Governing decision, stated by the user, not derived here.** The
≥2× decision rule this section has applied since TASK J was
calibrated against TASK M4's own K=590/610 figure, which Gate 3d
TASK M5 found does not reproduce (entry 52). Given this round's own
measured per-row (≈28 K) and per-pending (≈24 K) costs, the near-
empty 1-row+1-pending case has only ≈16,140 units of spare K — short
of the ≈26,380 a 2× margin against the removed check's own ≈13,190-
unit real cost would require, for ANY combination of the two caps.
The rule is dropped as unsatisfiable on the real numbers, not relaxed
or reinterpreted — this round proceeds directly to TASK F1.

**TASK F1 — the callback fast-dispatch spike and its gate.**
`spike/vault-f1/` (Hard Rule 5, deleted after use): a byte-for-byte
copy of production, modified only to (a) route a callback to a new
function BEFORE any full `VaultState::parse` (the `CODE_UPGRADE`
check stays first, per the task's own instruction) and (b) implement
that new function (`VaultState::fast_callback`) to validate the
header and the entire pending section exactly as `parse()` does, but
trust only the ledger section's declared byte length — never its
content. Settle copies the ledger bytes through verbatim; Restore
locates the target row by binary search over the raw 85-byte records
using a comparator that decodes each candidate's key fields in the
same field order `LedgerKey`'s own derived `Ord` uses (`currency_id`
as a decoded `u32`, never raw little-endian bytes — verified as a
real trap, not a theoretical one, by a dedicated host test built
during F2 covering ids 1/255/256/257). Every existing error code and
check order is preserved: wrong callback-magic-tag and caller
mismatch both still return `ERR_INVALID_CALLBACK` (1513), unknown
withdrawal id still returns `ERR_WITHDRAWAL_NOT_FOUND` (1510), in the
same order as the pre-existing `handle_callback`. `AVM_SPIKE_SEED`-
gated `vault.seed_full` (reused unchanged from Gate 3d TASK M5) built
fixtures without the real currency-registry limits; `AVM_SPIKE_
FASTPATH` toggled which dispatch path a given build used, so baseline
and fast-path numbers come from the same build family, one stand
session, differing only in that one flag.

**A measurement bug caught and fixed before trusting any F1 number.**
8 quick host-level sanity tests (settle, restore-add, restore-insert,
the four-currency-id ordering trap, overflow, unknown id, wrong
caller, corrupted-ledger-content) were run against `fast_callback`
directly before spending any real-stand cycles — all 8 passed on the
first complete implementation, giving confidence to proceed to the
real-stand K-sweep.

**Re-measuring the Gate 3d TASK M5 baseline, as instructed, found it
does not reproduce exactly either — a second, smaller instance of
session-to-session drift.** At 252+32, this round's own baseline
(no fast path) bisects to K≈7,600/7,625 (Settle) and K≈7,525/7,600
(Restore) — roughly 9% below TASK M5's own 8,350/8,440 and
8,200/8,350, in the identical same-session, same-build-family method.
This is reported as a range, not chased further: it is a materially
smaller discrepancy than TASK M4's own ≈26× error (entry 52), and the
*relative* findings TASK M5 established (headroom shrinks with row
count; Restore costs more than Settle) both held up unchanged.

**F1 K-sweep results (bisected; fee = 2,000,000 throughout, confirmed
irrelevant by TASK M4/M5 and not re-swept here):**

| Config | State | K resolves | K traps |
|---|---|---|---|
| Baseline, Settle | 1+1 | ≈15,775–16,150 | — |
| Baseline, Settle | 100+32 | 12,625 | 13,000 |
| Baseline, Settle | 252+32 | 7,600 | 7,625 |
| Baseline, Restore | 252+32 | 7,525 | 7,600 |
| Fast path, Settle | 1+1 | 15,000 | 17,000 |
| Fast path, Restore | 1+1 | 15,000 | 17,000 |
| Fast path, Settle | 252+32 | 13,000 | 14,000 |
| Fast path, Restore | 252+32 | 12,250 | 13,000 |

**Gate: both legs clear ≥1.5× at the true worst case.** Settle:
13,000/7,625 ≈ **1.70×**. Restore: 12,250/7,600 ≈ **1.61×**. (Fast
path's own advantage is negligible-to-slightly-negative at 1 row,
confirming the speed-up scales with row count, as the design
predicts — it specifically targets per-row ledger decode/validation
cost.) **Decision: proceed to TASK F2.** `spike/vault-f1/` deleted
after use.

**TASK F2 — implemented in production, test-first.** `state.rs`:
`VaultState::fast_callback` (same logic as the spike, ported exactly,
with every slicing site changed to `.get()`/`.get_mut()` to satisfy
this crate's own `deny(clippy::indexing_slicing)`, and the 7-field
lookup tuple replaced with a named `PendingTarget` struct to satisfy
`clippy::type_complexity`) and `fast_compare_ledger_row`. `lib.rs`:
the callback check moved before `VaultState::parse`, dispatching to
`handlers::handle_callback_fast`. `handlers::handle_callback` (the
pre-fast-path logic) is kept, `#[cfg(test)]`-gated, as the
differential tests' own ground truth — not left as dead code in a
production build (confirmed by a clean, warning-free `cargo build`;
`state.rs`'s `pending_index`/`remove_pending`, used only by the slow
path, are `#[cfg(test)]`-gated for the identical reason).

**Test suite, written before any of these were known to pass:**
(a) the existing 200-seed random operation-sequence property test
(`tests::property`) now also runs the slow path on every real
callback step's identical pre-state and asserts byte-identical
`encode_message_output` output against the real (fast-path) dispatch
— this is this project's differential test, not a separate smaller
sample, covering all 200 seeds' worth of real callback steps, not a
hand-picked few. (b) `tests::fast_path::fixtures_match_slow_path`:
Settle on a 1-row ledger, Restore adding to an existing row, Restore
inserting into a fully-drained (removed) row — each diffed against
the slow path. (c)
`restore_ordering_matches_slow_path_for_currency_ids_1_255_256_257`:
the ordering trap, all four ids as the restore target, each diffed.
(d) `restore_at_252_rows_insert_add_and_overflow`: add, insert, and
overflow (1529), all at the true 252-row worst case, each diffed.
(e) `fast_path_rejects_unknown_wrong_caller_wrong_magic_and_resolves_
while_paused`: all four, via the real dispatch specifically (not the
slow path directly), confirming the fast path itself — not just the
pre-existing slow-path tests — enforces these. (f)
`corrupted_ledger_content_settle_succeeds_then_next_op_rejects`:
corrupts a ledger row's `currency_tag` to an invalid value, confirms
Settle still succeeds through the fast path, then confirms the
identical bytes are rejected by `parse()` directly and by the next
real `DEPOSIT`-shaped call — both halves of the accepted trade-off,
asserted, not just described. (g)
`fast_path_never_panics_on_random_bytes`/`..._header_aware_random_
bytes`: 5,000 iterations each, `catch_unwind`-wrapped, mirroring
`tests::fuzz_parse`'s own two strategies against `fast_callback`
instead of `parse_with_reason`.

**Two real bugs found and fixed while writing these tests, not
after.** The first draft of `restore_at_252_rows_insert_add_and_overflow`'s
"add" sub-case used an all-zero `[0;32]` dapp/account as both the
ledger filler key AND the message's own caller address — `avm-
contract-sdk`'s own message decoder rejected this as `InvalidValue`
(apparently an all-zero account is not treated as a valid real
address at the wire level) — fixed by using a real, non-zero
depositor for that sub-case instead, matching every other fixture in
the suite. Second: `test_support::expect_full_replace_state` asserts
its own result parses cleanly — unsuitable for the corrupted-ledger
test's own Settle output, which deliberately does not parse; fixed by
reading `output.state_update` directly instead of through that helper
for that one assertion.

**Full verification, this round, in order:**

```
cargo test --lib -- --test-threads=1     89 passed; 0 failed
cargo fmt --check (stable)                 EXIT=0 (after applying real diffs)
cargo +nightly fmt --check                 EXIT=0 (after applying real diffs)
cargo clippy --all-targets -- -D warnings  EXIT=0, host (after 5 indexing_slicing +
                                            1 type_complexity fix)
cargo clippy --target wasm32-unknown-unknown --lib -- -D warnings   EXIT=0
```

**Fresh `cargo mutants` run (full crate, first pass) surfaced two real
gaps, not just the historical equivalents.**

```
Found 252 mutants to test
252 mutants tested in 13m: 6 missed, 222 caught, 18 unviable, 6 timeouts
```

Two of the six missed were new, both inside `VaultState::fast_callback`
specifically — not the four historical `constants.rs` equivalents:
`state.rs:574:26` (`delete !` in `build_flags & !1 != 0`) and
`state.rs:649:39` (`replace * with /` in
`let start = row_index * PENDING_RECORD_LEN;`). Neither is equivalent:
(1) every existing fixture used `build_flags == 0`, where
`0 & !1 == 0 & 1`, so the mutant was never exercised by a case where
the two diverge (`build_flags == 1`, bit 0 set); (2) every existing
fixture had exactly one pending record (`row_index == 0`, where
`0 * N == 0 / N`), so the mutant was never exercised at
`row_index >= 1`. Both fixed with new tests, not declared equivalent:
`fast_path_accepts_build_flag_one_and_rejects_reserved_bit` (dispatches
a real callback through the fast path with `build_flags == 1`, must
resolve and match the slow path, and `build_flags == 2`, a reserved
bit, must still reject) and
`fast_path_resolves_pending_record_at_nonzero_row_index` (two pending
records, ids 1 and 2; the callback resolves id 2 — `row_index == 1` —
for both Settle and Restore, asserting the id-1 record survives
untouched and the correct key is credited).

**Scoped re-run (`--file src/state.rs`) confirmed both fixes catch
their mutant, with no new regressions:**

```
Found 115 mutants to test
115 mutants tested in 7m: 106 caught, 4 unviable, 5 timeouts
```

Zero missed. The same 5 timeouts (all inside `fast_callback`'s binary
search, detailed below) persist unchanged — expected, since the two
test additions target different lines than the search loop.

**Final full-crate run, authoritative for this round:**

```
Found 252 mutants to test
252 mutants tested in 12m: 4 missed, 224 caught, 18 unviable, 6 timeouts
```

| Mutant | Category | Argument |
|---|---|---|
| `constants.rs:27:62` — `\|`→`^` | MISSED | Provably equivalent (historical, unchanged — entry 52's argument applies verbatim: the two operands never share a set bit). |
| `constants.rs:28:63` — `\|`→`^` | MISSED | Same argument, historical, unchanged. |
| `constants.rs:46:32` — `\|`→`^` in `withdrawal_callback_method_id` | MISSED | Same argument, historical, unchanged. |
| `constants.rs:250:57` — `\|`→`^` in `decode_hex_pubkey` | MISSED | Same argument, historical, unchanged. |
| `constants.rs:251:15` — `+=`→`*=` in `decode_hex_pubkey` | TIMEOUT | Historical, unchanged: `index` starts at 0 and `0 *= 1` loops forever — a genuine infinite loop the test suite detects via hang, not a coverage gap. |
| `state.rs:706:22` — `<`→`<=` in `fast_callback`'s binary search (`while lo < hi`) | TIMEOUT | New this round. When `lo == hi` and the target sorts before every remaining row, `Ordering::Greater` sets `hi = mid = lo`, leaving `lo <= hi` true forever under the mutant — a genuine infinite loop in the probe direction a correctly-sorted, legitimately-constructed ledger can reach, detected via hang against the suite's own well-formed fixtures (`restore_ordering_matches_slow_path_...`, `restore_at_252_rows_...`), not a coverage gap. |
| `state.rs:707:30` — `+`→`*` in `let mid = lo + (hi - lo) / 2;` | TIMEOUT | New this round. For `lo > 0` with `hi - lo == 1`, `lo * 0 == 0 < lo`, so `mid` can fall behind `lo`; combined with repeated `Less` outcomes this re-derives the same `(lo, hi)` pair indefinitely. Detected via hang against the existing well-formed multi-row fixtures, not a coverage gap. |
| `state.rs:707:42` — `/`→`%` in the same line's `/ 2` | TIMEOUT | New this round. `(hi - lo) % 2` discards the halving step entirely for even gaps, which can likewise re-derive the same `(lo, hi)` pair forever. Same detection basis as above. |
| `state.rs:716:58` — `+`→`-` in `Less => lo = mid + 1` | TIMEOUT | New this round. `mid - 1` can set `lo` back to or below its own prior value on the `Less` branch, breaking the search's own progress guarantee. Same detection basis as above. |
| `state.rs:716:58` — `+`→`*` in the same line | TIMEOUT | New this round. `mid * 1 == mid` leaves `lo` unchanged on a branch that must strictly advance it, with the identical consequence. Same detection basis as above. |

All 5 new timeouts sit inside the Restore-leg binary search
(`state.rs:702-719`) and share one root cause: mutating the loop's own
termination arithmetic breaks it even against the suite's existing
*well-formed, correctly-sorted* ledger fixtures — meaning the mutation
is genuinely caught, just by a hang (fuel exhaustion in real
production, not a literal unbounded loop — the AVM is fuel-metered)
rather than a failed assertion, consistent with this project's
standing convention (entry 52's own `decode_hex_pubkey` timeout) of
treating `TIMEOUT` as effectively caught and not requiring a
dedicated test. They are not evidence that an *unsorted/corrupted*
ledger would behave any differently than already documented in
`docs/THREAT_MODEL.md` §5.13's own residual-risk note — that remains
untested this round, unchanged from F3's own statement below.

Raw output for every one of these, `cargo test`, both `fmt` checks,
and both `clippy` targets is in the Gate 3e package sent to the user;
summarized here only.

**TASK E1, re-run on the new fast-path wasm** (`contract.wasm`
`90ccbcd5...`, 58,559 bytes): deposit, withdraw→Settle, withdraw→
Restore, pause, owner rotation, and vault-underfunded rejection all
PASS, with decoded-state evidence for each step, on the real dispatch
path this round changed. Owner-authorized steps required rebuilding
with `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` set to a real stand identity
(`SPEC.md` §1's own documented two-step pattern), on a fresh third
stand session (`env3.sh`/`stand3.state`) after the production
schema's own one-time package-swap allowance was already spent on
the first two stand sessions earlier in this engagement — the default
demo-key build was restored immediately after for every other
measurement this round.

**TASK F3 — docs updated throughout, not just where F1/F2 happened to
touch.** `SPEC.md` §1: the "≥2× rule"/"no cap change" framing replaced
with the measured facts (K=0 resolves at 252+32 for both outcomes;
spare K by state size; per-row/per-pending cost; why TASK M4's figure
is distrusted and TASK M5's own re-measurement, though imperfect, is
trusted — same session, monotone, a 9%-not-26×-sized drift); the F1
gate and F2 implementation documented in full, including the accepted
ledger-corruption trade-off and why `DEPOSIT`/`WITHDRAW` are
unaffected. `README.md`: the known-limitations bullet reworded to
plain, non-alarming language with no internal task labels; test count
updated (at the time of this paragraph, 81 host-level + 6 fuzz-style =
87; the two mutation-coverage tests added later in this same entry
brought the final count to 83 host-level + 6 fuzz-style = 89 — see
README.md's own current Testing section); the differential-test and
fast-path-specific test suite described. `docs/THREAT_MODEL.md`: new
§5.13, "A callback no longer requires the full stored ledger to be
valid," with its own residual-risk paragraph (Restore's own behavior
under ledger-level corruption specifically was not separately
constructed this round, unlike Settle's). `docs/MAINTAINER_QUESTIONS.md`
#12: retitled RESOLVED, rewritten with the dropped-rule reasoning and
the F1/F2 resolution. `docs/PLATFORM_FINDINGS.md`: new entry P26.
Grepped the whole repo for "≥2×"/"2x rule"/"decision rule" — all
remaining occurrences outside this file are either this entry's own
citations of the now-retired rule for context, or `REVIEW.md`'s own
historical entries (left as the record of what each round found,
per this project's standing convention).

**Updated this round:** `src/state.rs`, `src/lib.rs`, `src/handlers.rs`;
`SPEC.md` §1; `README.md` (known limitations, Testing); `docs/
THREAT_MODEL.md` new §5.13; `docs/MAINTAINER_QUESTIONS.md` #12; `docs/
PLATFORM_FINDINGS.md` new entry P26; this entry. No commit, no push.

### 54. Gate 3f: consistency sweep (stale build/margin numbers moved to one canonical place), repo-integration findings, submission shaping (quickstart, a real local-stand E2E script plus its own test-only actor, `docs/OVERVIEW.md`), final sanity

**No contract logic changed this round** (`src/*.rs` behavior is
byte-for-byte what it was at the end of entry 53) — confirmed by the
unchanged `contract.acab`/`contract.descriptor` hashes and an unchanged
89/89 test pass.

**TASK 1 — stale numbers.** Grepped the repo for every old size/hash/
margin/count the user named. Found and fixed: README's own Build
artifacts table (stale 55,703-byte/old-hash row → current 58,559 bytes,
SHA-256 `90ccbcd5...`, BLAKE3 `3bd187ba...`); `SPEC.md` §1 and `docs/
MAINTAINER_QUESTIONS.md` #11 both still restated the pre-fast-path
envelope/margin figures (55,703+3,182+66=58,951, ≈9.6%/≈8.7%) as if
current — both rewritten to point at README's own table instead of
repeating numbers that go stale every time the wasm size changes.
`docs/PLATFORM_FINDINGS.md` P26 already had the correct current numbers
(58,559/61,807/≈5.3%) as its own point-in-time delta narrative; added one
clause pointing to README as the going-forward source rather than
restating. REVIEW.md's own historical entries (dated, e.g. entries 35,
39, 42, 52) were left untouched, per this project's standing convention
that REVIEW.md is the record of what each round's own build measured at
the time, not a live document. **Re-checked BLAKE3 after a fresh build,
as instructed, and found the prior round's own "BLAKE3 varies, SHA-256
doesn't" claim did not hold up under direct re-check**: four consecutive
rebuilds from this exact path, no source changes between them, produced
an *identical* BLAKE3 and SHA-256 both times. Re-examining the prior
claim's own numbers showed it had compared BLAKE3 values from two
*genuinely different* wasm builds (pre- and post-fast-path), which both
hash columns were expected to differ on — not evidence of same-content
instability. Corrected in README's own Build artifacts section, not
silently.

**TASK 2 — doc points.** All four added to `docs/THREAT_MODEL.md` §5.13
(the one place these now live; other docs point to it): (a) error
precedence stated explicitly — a callback with a bad shape returns 1513
(`ERR_INVALID_CALLBACK`) before any state-validity 1502
(`ERR_INVALID_STATE`) or 1510 (`ERR_WITHDRAWAL_NOT_FOUND`), since shape
checks run in `handle_callback_fast` before `fast_callback` is ever
called; (b) the atomicity contrast — `DEPOSIT`/`WITHDRAW` can still fail
on fuel, but that failure is atomic (Invariant I6: nothing written,
nothing stranded), unlike a callback failure, which strands a pending
withdrawal (§5.8); (c) the full measured callback-leg table (spare K by
state size, baseline vs fast path, Settle vs Restore, bracket widths, the
≈9% session drift), with the Settle/Restore-no-longer-revalidates-content
fact and the next-deposit/withdraw-catches-it fact both stated in plain
language; (d) confirmed no live doc still claims the callback path does a
full `parse()` or still cites the ">=2x rule" as in force — the one
"callback"+"parse()" co-occurrence found (`docs/PLATFORM_FINDINGS.md`
P25) is itself a dated, pre-fast-path historical entry describing what
was true at the time, not a live claim.

**TASK 3 — repo integration (read-only, no edits except the one
authorized addition below).**
(a) The root `Cargo.toml` workspace lists exactly one member,
`crates/avm-contract-sdk` — no example, including the four official
ones, is a workspace member. Examples are built/tested independently via
`--manifest-path`.
(b) `cargo test --workspace` from the repo root runs only the SDK
crate's own 13 tests (9 unit + 4 integration); this example's 89 tests
are **not** included and never were. No repo-documented test command
covers examples at all — `examples/README.md`'s own "For direct source
checks" section documents only per-example `fmt`/`clippy` commands
(`cargo +1.93.0 fmt --manifest-path .../Cargo.toml -- --check`; `cargo
+1.93.0 clippy --manifest-path .../Cargo.toml --target
wasm32-unknown-unknown -- -D warnings`), never a test command — each
example's own tests are run from inside that example's own directory,
exactly as this engagement has done every round.
(c) `examples/README.md` lists the four official examples by name and
did not list this one. Added an entry (exact diff shown to the user
before applying, then applied — the one repo-wide edit this round's own
instruction pre-authorized).
(d) The four official examples have strictly *less* than this one:
`Cargo.toml`/`Cargo.lock`/`avm-contract.toml`/`src/` only — no `README.md`,
no `docs/`, no `SPEC.md`/`THREAT_MODEL.md`/`REVIEW.md`, no scripts. No
`CONTRIBUTING.md` or PR-template file exists anywhere in this checkout
(searched, none found).
(e) No workspace-wide `cargo deny`/`cargo audit`/`deny.toml` exists
anywhere in this repository (searched, none found) — the only documented
lint/fmt commands are the per-example ones named in (b), both already run
clean on this example in TASK 5 below.

**TASK 4 — submission shape.**
(a) README's first screen is now a "Quickstart": 5 bullets on what the
contract does, one test command, the build command, the E2E script
command, the headline verified numbers, and the three biggest caveats
(demo owner key, `TOPUP` unverified on a real network, everything
measured on the local stand only). Nothing below it was deleted.
(b) `scripts/e2e_journey.sh` — a new, real, reproducible local-stand E2E
script, plus `e2e-actor/` (a small test-only helper contract the vault's
own `DEPOSIT`/`WITHDRAW` require, since both need an internal caller and
no official example ships an E2E script to model this on; built in the
general style of `examples/contract-a-v1`'s own generic forwarder, scaled
to this vault's own ABI). **Two real stand findings surfaced and fixed
while building it, both recorded in the actor's own `README.md` and its
`src/lib.rs` module doc:** (i) a non-`null` `--init-json` on a
message-isolated contract failed identically on two independently funded
accounts with `"InitOutputInvalid"`, with no working example anywhere in
this repository to compare against — abandoned for a build-time flag
(the same proven mechanism `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` already
uses) rather than invented further; (ii) the forward method's own ABI
input type being declared `kind = "bytes"` (variable-length) rather than
`kind = "fixed_bytes"` (with an explicit `len`) caused every forward call
to fail at `avm-local-stand`'s own materialization step with
`InvalidOutboundIntent`, even with field values copied byte-for-byte from
`examples/contract-a-v1`'s own successful forwarding call against the
identical destination in the identical session — switching only the ABI
declaration fixed it. The script runs end to end on the final build,
reproducibly: deposit, withdraw→Settle, withdraw→Restore, owner-gated
pause, two-step owner rotation, and a withdraw rejected pre-write because
the vault's own SHELL balance is below `MIN_OPERATIONAL_SHELL` — 6/6
checks pass, exit code 0, on a fresh stand session with no cached state
from this round's own diagnostic work. No CLI flag was invented at any
point; the two findings above were worked around with build-time/ABI
changes, not fabricated flags.
(c) `docs/OVERVIEW.md` — new: links `SPEC.md`/`THREAT_MODEL.md`/
`REVIEW.md`/`PLATFORM_FINDINGS.md`/`README.md`, states the real current
numbers (89 tests; mutants 224 caught/4 equivalent-missed/6
timeouts-treated-as-caught; callback spare K by state size; the envelope
margin), and a "What is NOT claimed" section.

**TASK 5 — final sanity, this round's own final build:** `cargo test
--lib` 89/89; `cargo fmt --check` (stable) EXIT=0; `cargo +nightly fmt
--check` EXIT=0; `cargo clippy --all-targets -- -D warnings` (host)
EXIT=0; `cargo clippy --target wasm32-unknown-unknown -- -D warnings`
EXIT=0; `contract.wasm`/`contract.acab`/`contract.descriptor` rebuilt,
SHA-256/size unchanged from README's own table (90ccbcd5.../58,559;
63ade31e.../3,182; 66f1916e.../66).

**`git status --short` found something worth flagging rather than
silently working around: every tracked file in this checkout — not just
this example's own files — shows as modified** (`.gitignore` through
`scripts/verify-public-release.py`, 59 files). Checked directly
(`git diff LICENSE.md`): every line is removed and re-added
*identically* — pure line-ending normalization (CRLF/LF) from this
checkout, pre-existing from before this round and unrelated to any of
this round's own edits, not a real content change. `git diff --stat`:
"59 files changed, 8736 insertions(+), 8713 deletions(-)" confirms the
same pattern repo-wide. **The only files this round's own work would add
to a commit** (`git status --porcelain --untracked-files=all`, filtered
to exclude `target/`, `mutants.out*/`, `spike/`): the 20 real files under
`examples/multi-currency-vault/` (its own `Cargo.toml`/`Cargo.lock`/
`avm-contract.toml`/`README.md`/`SPEC.md`/`REVIEW.md`/`src/*.rs`/`docs/
*.md`/`e2e-actor/*`/`scripts/e2e_journey.sh`) plus the one tracked-file
edit, `examples/README.md`. None of `spike/`, `target/`, `mutants.out*/`,
or any key/secret is among them. **No commit was made** — confirmed by
this same `git status` output still showing `examples/multi-currency-vault/`
as untracked (`??`), not staged or committed.

**What could not be done, stated plainly rather than worked around:**
none of this round's own 5 tasks were blocked outright — the two real
stand findings in TASK 4(b) were worked around with proven mechanisms
(a build-time flag; a corrected ABI declaration), not left undone. The
repo-wide line-ending noise found during TASK 5 was reported, not fixed
— fixing it would touch nearly every file in the repository, far
outside this round's own scope, and was never asked for.

**Updated this round:** `README.md` (Build artifacts, Quickstart,
Testing); `SPEC.md` §1; `docs/MAINTAINER_QUESTIONS.md` #11; `docs/
PLATFORM_FINDINGS.md` (P26, one added clause); `docs/THREAT_MODEL.md`
§5.13 (error precedence, atomicity contrast, full K table); `examples/
README.md` (new entry for this example); new `examples/multi-currency-
vault/scripts/e2e_journey.sh`; new `examples/multi-currency-vault/
e2e-actor/` (`Cargo.toml`, `avm-contract.toml`, `src/lib.rs`,
`README.md`); new `docs/OVERVIEW.md`; this entry. No commit, no push.
