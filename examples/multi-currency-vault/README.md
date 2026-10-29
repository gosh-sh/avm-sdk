# Multi-Currency Vault

A deposit/withdraw vault for the Acki Nacki AVM SDK. Depositors `DEPOSIT`
arbitrary non-SHELL currencies into a per-depositor ledger and later
`WITHDRAW` them; an owner key can pause new deposits and rotate itself in
two steps. Full behavioral spec: [`SPEC.md`](SPEC.md). Full threat model:
[`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md). Full engagement log,
including every judgment call and every finding's triage: [`REVIEW.md`](REVIEW.md).

## Quickstart

**What it does:**
- `DEPOSIT` any non-SHELL currency into the caller's own per-currency ledger row.
- `WITHDRAW` it back out — debits immediately, pays out asynchronously, and
  resolves via a callback (Settle on success, Restore on failure) that always
  returns funds to the caller's own account, never anywhere else.
- `TOPUP` lets anyone send the vault plain SHELL to cover its own operating costs.
- An owner key can pause new deposits (never withdrawals) and rotate itself in two steps.
- No upgrade path exists — `CODE_UPGRADE_METHOD_ID` is explicitly rejected.

**Run the tests** (from this directory):
```bash
cargo test --lib
```

**Build it:**
```bash
avm-contract-build build --manifest-path Cargo.toml
```

**Run the local-stand E2E journey** (deposit, withdraw→Settle, withdraw→Restore,
pause, owner rotation, underfunded-vault rejection — see `scripts/e2e_journey.sh`
and `e2e-actor/README.md`):
```bash
bash scripts/e2e_journey.sh
```

**Headline verified facts** (full detail in `docs/OVERVIEW.md`):
89 host-level/fuzz tests pass; `cargo mutants` reports 224 caught, 4
provably-equivalent missed (justified individually), 6 timeouts treated as
caught; the real-stand E2E journey above passes end to end; the deploy
envelope has ≈5.3% margin under the platform's own measured limit.

**The three biggest caveats, stated plainly:**
1. The default build's owner key is a public, checked-in fallback — fine for
   local testing, **not for anything real** (see the warning immediately below).
2. `TOPUP`'s own real-network behavior is unverified — it has only ever been
   exercised on this local stand, never against a real mainnet fee/rent model.
3. **Every number in this README, `SPEC.md`, and `REVIEW.md` is a local-stand
   measurement.** Nothing here has been verified against a real network.

Full detail on everything above — the full method list, every known
limitation, every measured number and how it was measured, every residual
risk — is in the sections below and in `SPEC.md`/`docs/THREAT_MODEL.md`/
`REVIEW.md`/`docs/PLATFORM_FINDINGS.md`/`docs/OVERVIEW.md`. Nothing is
deleted from this README to make room for this section.

## ⚠️ The default build uses a publicly-known demo owner key — read this first

**This is not a footnote.** If `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` is unset at
compile time, the built contract's `owner_pubkey` is a **fixed, hardcoded
fallback key** (`FIXTURE_VAULT_OWNER_PUBKEY`, `src/constants.rs`), identical
in spirit to `contract-a-v1`'s own fallback pattern elsewhere in this repo.

- **Its private-key secrecy is explicitly unverified, not confirmed safe.**
  A search of this public repository found no private key material paired
  with the fallback public key anywhere in this checkout — that proves only
  that *this* public repo doesn't contain it, not that nobody else does, or
  ever will.
- A build using this fallback key is for **local testing and
  demonstration only.**
- **Before deploying this contract for anything real, set
  `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` to a freshly generated key you control**
  (64 lowercase hex digits), e.g.:

  ```bash
  AVM_EXAMPLE_VAULT_OWNER_PUBKEY=<your 64 hex digits> \
    avm-contract-build build --manifest-path Cargo.toml
  ```

- A contract built with the fallback key has `build_flags` bit 0 set in its
  own stored state (`USING_DEMO_OWNER_KEY`, readable via `inspect`) —
  **check this bit before trusting any already-deployed instance of this
  contract.** Owner-key compromise is a griefing/control risk (the owner
  can pause deposits, and a compromised owner can rotate itself to an
  attacker-controlled key) — it is never a funds-theft risk: Invariant I5
  guarantees the owner can never redirect, seize, or withdraw any
  depositor's own ledger balance under any circumstance.

## What this contract does

- `DEPOSIT` — any non-SHELL currency, credited to the caller's own
  per-currency ledger row.
- `WITHDRAW` — debits the ledger immediately, creates a pending record,
  and sends the payout; the later callback (Settle on success, Restore on
  failure) resolves it. Always pays out to the caller's own account —
  there is no destination argument anywhere in this contract.
- `TOPUP` — anyone can send the vault plain SHELL to cover its own
  operating costs (see the operator note below — this is the *only* way
  SHELL reaches the vault after genesis).
- `PROPOSE_OWNER` / `ACCEPT_OWNER` — two-step owner rotation.
- `SET_PAUSE` — owner-only, pauses/unpauses new deposits (never
  withdrawals).

No upgrade path exists in v1 (`CODE_UPGRADE_METHOD_ID` is explicitly
rejected). See `SPEC.md` §1 for the full design rationale behind every
decision summarized here.

## Build

```bash
cd examples/multi-currency-vault
avm-contract-build build --manifest-path Cargo.toml
```

Produces `target/avm/contract.wasm` / `contract.acab` /
`contract.descriptor`. See "Build artifacts" below for this build's exact
sizes and hashes, and `SPEC.md` §1/`docs/MAINTAINER_QUESTIONS.md` #11 for
the measured deploy-envelope size limit — **on the combined `wasm +
acab + descriptor` total, not wasm alone** (`65,237` bytes combined
succeeds, `65,241` bytes combined is rejected) — this contract's build
must stay under. Adding ABI surface (new methods/types) shrinks the
same budget just as surely as adding code does.

`AVM_EXAMPLE_VAULT_OWNER_PUBKEY` (see the warning at the top of this
file), if set, **must be exactly 64 lowercase hex digits**
(`src/constants.rs`'s `decode_hex_pubkey`). An empty value, a value of
the wrong length, or a value containing any uppercase hex digit or
other non-hex character fails the build **by design** — a compile-time
`assert!`/`panic!` in a `const fn`, not a bug to work around — rather
than silently falling back to the demo key or accepting a malformed
owner key.

## Operator note: SHELL and storage rent

**This vault has no built-in mechanism to notice or react to its own
SHELL level.** Running it for real requires an off-contract **keeper**
that periodically checks the vault's own SHELL balance and sends a
`TOPUP` large enough to clear any accrued rent debt — this is an
operational requirement, not an optional nicety.

**Burn-rate formula** (local-stand measurement, **not a confirmed
mainnet rate** — see the open maintainer question below): storage rent
accrues at roughly **1 SHELL per byte of the vault's current state, per
second**, since it was last touched. So, approximately:

> daily burn (SHELL) ≈ current state size (bytes) × 86,400

At this contract's own absolute worst-case state (252 ledger rows + 32
pending withdrawals, `24,359` bytes — `SPEC.md` §2.4), that is on the
order of **2.1 billion SHELL/day** if the vault is never touched. A
near-empty vault burns proportionally almost nothing. Use your vault's
*actual* current state size and expected touch frequency against this
formula, not the abstract worst case.

**`MIN_OPERATIONAL_SHELL` (currently `1,500,000`) is a withdrawal-
admission *gate*, not an operating balance — never read it as "how much
SHELL to keep in the vault."** It exists solely to answer one narrow
question at the moment a `WITHDRAW` begins: can the vault plausibly
afford to see *this one* withdrawal's payout chain through? At this
contract's own worst-case state and the measured local rate, that value
represents only about **61.6 seconds** of rent — and real-stand testing
found that framing *optimistic*: a vault funded with exactly that amount
(or even 3× it) failed its very first withdrawal attempt within seconds
of being touched, once ordinary setup overhead was accounted for. A real
deployment's keeper must size the vault's actual holdings against the
burn formula above and its own real touch frequency — watched far more
often than once every 60 seconds — never against `MIN_OPERATIONAL_SHELL`.

**Confirm genuine execution on `TOPUP`, not just a successful-looking
call.** A call can report `"status": "ok"` while this contract's own
code never actually ran — SHELL was charged (partially paying down
accrued rent) but no real execution happened. This is distinguishable
only by checking whether the response's `raw_executor_output`/`events`
came back non-empty. When topping up a vault suspected of being
significantly rent-delinquent: **send enough SHELL to comfortably clear
the entire accrued debt, not a token amount, and verify the response
shows genuine execution.** An undersized *external* topup is at least
rejected outright and atomically (no partial credit) — but it
accomplishes nothing, and the vault remains exactly as delinquent as
before.

**Depositor-side consequence of vault delinquency.** A `DEPOSIT` sent to
a severely rent-delinquent vault can come back with an internal failure
callback carrying status code `56`. Treat this as an ordinary, recoverable
delivery failure — the depositor's value was never taken, confirmed by
direct balance readback — not as evidence of a bug or a loss; simply
retry later.

**Watch the two caps, well before they're hit.** `MAX_LEDGER_ENTRIES`
(`220`) and `MAX_PENDING_WITHDRAWALS` (`32`, global across every
depositor) both resolve cleanly right up to their own limits, but hitting
either one means new `DEPOSIT`/`WITHDRAW` requests are correctly rejected
outright until usage drops back down — itself worth attention well
before it happens. Encourage depositors to withdraw fully (which removes
their row) rather than letting many small balances accumulate, and never
treat "the cap hasn't rejected anything yet" as proof the vault is at a
safe size.

## Known limitations

- **No upgrade path.** This is a v1, non-upgradeable design. Any fix or
  change requires a fresh deployment.
- **Global stuck-pending-slot risk — a standing operational risk, not a
  theoretical edge case.** This vault has no reclaim mechanism for a
  withdrawal whose payout produces no callback at all, via either of
  two open routes documented in `docs/THREAT_MODEL.md` §6 "L": (1) a
  same-DApp-but-split delivery route whose liveness is unresolved, and
  (2) a SHELL-starved dropped callback (directly observed on the real
  stand). A third theorized route — a malicious depositor's oversized
  callback-response body exhausting the guest's own fixed-size memory
  arena before this contract's code runs — was tested directly on the
  real stand and **did not reproduce** at any size up to
  10,000,000 bytes (38× the vault's own declared arena; see `REVIEW.md`
  for the full investigation). A follow-up check explains why: on the
  local stand, a `callback_result_body`
  above exactly 65,536 bytes is never delivered at all — silently
  dropped (not truncated, not an error) while the call still reports
  success (`docs/PLATFORM_FINDINGS.md` P23) — and 65,536 is itself well
  under the vault's 262,144-byte arena, so no body sent during the
  oversized-callback check ever
  actually arrived at a size capable of stressing the arena in the
  first place; the arena-exhaustion theory could not be exercised at
  any size that arrives. What `arena_bytes` constrains on mainnet, or
  for message shapes other than this one, remains open
  (`docs/MAINTAINER_QUESTIONS.md` #8). This vault never reads the
  callback body's content, so none of this creates a fund risk here,
  but it means nothing in this repository about callback-body size has
  been tested beyond the local stand — mainnet behavior for any of it,
  including whether the 65,536-byte threshold even applies, is **not
  established**. Because `MAX_PENDING_WITHDRAWALS`
  is a **single global cap shared across every depositor**, as few as
  32 stuck pending records — from any mix of depositors, via either of
  the two remaining routes — permanently disable `WITHDRAW` for
  everyone in the vault, not just the ones whose own withdrawals got
  stuck. There is no in-contract recovery once this happens.
- **Whether this vault can even be funded on a real network is
  unverified.** Every `TOPUP` this engagement ever tested was funded
  through a local-stand-only test-harness minting flag
  (`avm-local-stand call --extra-currencies "2:N"`). `ExternalMessage`
  carries no source account at all, only a bare public key — there is no
  structural mechanism visible anywhere in this SDK for a real network
  to debit real SHELL value from an external message's sender. If no
  such mechanism exists on a real network, `TOPUP_METHOD_ID` — the
  vault's *only* replenishment path after genesis — would be unusable
  there, and the vault would have no way to recover its own SHELL once
  genesis funding runs out. The consequence is a **permanent withdrawal
  outage, not a loss of funds**: every ledger balance stays exactly
  intact, but no depositor could ever withdraw again. This is the
  highest-priority open question for the SDK maintainers — see `docs/
  MAINTAINER_QUESTIONS.md` #1.
- **A depositor must be able to accept a plain, method-less value
  transfer.** A withdrawal's payout is a fixed-shape message
  (`method_id = 0`, empty body). If the depositor is itself a smart
  contract that cannot accept this shape, their withdrawal fails safely
  at the point of delivery — their ledger balance is unconditionally
  restored — but they must fix their own contract (or otherwise arrange
  to accept the transfer) before successfully re-requesting the
  withdrawal.
- **External method ids are pinned against one specific SDK release.**
  The externally-invoked method ids (`TOPUP`/`PROPOSE_OWNER`/
  `ACCEPT_OWNER`/`SET_PAUSE`) are host-computed hashes with no documented
  formula; this build pins the values discovered against
  `avm-dev-06e96ef71006-sdk-0748f4f0767a` (see `src/constants.rs`'s own
  header comment). These must be re-verified if the installed SDK release
  ever changes.
- **Mainnet storage-rent formula and long-arrears behavior are
  unverified.** Every rent figure in this README and in `SPEC.md` is a
  local-stand measurement. `docs/MAINTAINER_QUESTIONS.md` lists every
  open question this engagement could not resolve from this repository
  alone (12 questions, priority-ordered).
- **A withdrawal's Settle/Restore step reads less of the stored state
  than it used to, to keep comfortable margin against the platform's
  own fuel limits at this contract's largest possible size.** At the
  largest state this contract's own rules can ever produce (252 ledger
  rows, 32 simultaneously pending withdrawals), the step that finishes
  a withdrawal now trusts the ledger's declared length but no longer
  re-checks every ledger row's own content on that step specifically —
  deposits and withdrawals still check everything, every time, as
  before. The practical effect: if the stored ledger were ever
  corrupted by something outside this contract's own code, a
  withdrawal already in flight would still finish normally, and the
  corruption would be caught on the very next deposit or withdrawal
  instead of on that step itself — a one-step delay in catching a kind
  of corruption this contract's own logic cannot itself produce.
  Measured, real numbers for this change (bisected directly, not
  estimated): see `docs/MAINTAINER_QUESTIONS.md` #12 and `REVIEW.md`.

## Testing

83 host-level unit/property/simulator tests plus 6 fuzz-style tests (zero
panics across 80,000 random/mutated-byte/header-aware-random
`VaultState::parse` iterations); the header-aware generator always
clears the magic-byte check the plain random-bytes generator almost
never does, and is confirmed — not just claimed — to get past the
length check on at least 25% of iterations and to produce at least 5
distinct rejection reasons (`cargo test --lib ... -- --nocapture`,
tallies printed and asserted on, not estimated; an earlier version's
coverage claim turned out false on inspection and was corrected —
see `REVIEW.md`); a fourth test separately confirms the
mutated-valid-state generator reaches all six specifically named
`parse()` rejection branches, via the same kind of tally plus small
deterministic cases for the two branches random mutation alone
structurally cannot reach; two more fuzz tests separately confirm the
callback fast path (below) never panics across 10,000 further random
and header-aware random iterations. The 200-seed random operation
sequence also differentially checks every real callback step's
encoded output against the pre-fast-path logic directly, and five
further host tests cover the fast path's own specific edge cases
(restore ordering across currency ids whose byte and numeric order
disagree, restore at the true 252-row worst case for both the
add-to-existing-row and insert-a-new-row cases, and a corrupted-ledger
scenario proving a Settle still succeeds while the next deposit or
withdrawal still catches the corruption) — all byte-for-byte
differentially checked against the pre-fast-path logic, not merely
status-code-checked.
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (host and
`wasm32-unknown-unknown`), `cargo audit`, and `cargo mutants` all run and
recorded. An independent, fresh-context adversarial review found and
fixed three real bugs (a worst-case state-size under-measurement, an
admin check-ordering bug, and an error-code mix-up) and recorded one
open, unverified-reachability risk. **A second, independent
fresh-context review found no new confirmed issues** — it
investigated and refuted one additional hypothesis (a host-side
`required_state_output_mode` mismatch risk) and disclosed its own
coverage gaps explicitly (`REVIEW.md` entry 50). Full raw output, the
complete test list, and the fix/triage log for every finding, round by
round:
`REVIEW.md`. **Not claimed:**
"secure" or "audited." What was tested, what was not, and every residual
risk are stated explicitly throughout `SPEC.md`, `docs/THREAT_MODEL.md`,
and `REVIEW.md` — read those before relying on this contract for
anything beyond local testing and demonstration.

**Beyond the host-level suite above, `scripts/e2e_journey.sh` runs a real
local-stand journey** (deposit, withdraw→Settle, withdraw→Restore, pause,
owner rotation, underfunded-vault rejection), asserting on the vault's own
decoded state at every step — see "Quickstart" above and
`e2e-actor/README.md` for the one test-only helper contract it needs.

## Build artifacts

**This is the single current source for this contract's build
numbers — every other doc in this repository (`SPEC.md`,
`docs/MAINTAINER_QUESTIONS.md`, `docs/PLATFORM_FINDINGS.md`,
`REVIEW.md`'s live/non-historical text) points here rather than
restating these figures.**

Built with `rustc 1.93.0` / `cargo 1.93.0` (`rust-toolchain.toml` pins
`channel = "1.93.0"`), fallback demo owner key (`AVM_EXAMPLE_VAULT_OWNER_PUBKEY`
unset — see the warning above), from this checkout's own path
(`.../repo/examples/multi-currency-vault`). **Two different hashes are
shown per file, because `avm-contract-build` itself prints a value that
is *not* a SHA-256 of the file** — confirmed by inspecting the tool's
own binary, which links the `blake3` crate and carries symbols named
`avm.hash.blake3.bytes`/`avm-canonical.hash_domain`: it is an
AVM-internal, BLAKE3-based canonical content/identity hash, serving a
different purpose (package/selection identity) than a file-integrity
checksum. Use the **SHA-256** column to verify these exact files.

| File | Bytes | SHA-256 (`sha256sum`, file integrity) | `avm-contract-build`'s own printed hash (BLAKE3, canonical identity) |
|---|---|---|---|
| `contract.wasm` | 58,559 | `90ccbcd54843c86184968ccb35919e7901bddd52731606910f0900e01df07eb3` | `3bd187bacb708b3fa277a87e1a52fd6533c52e01caca45b0b82f8bc393fb5193` |
| `contract.acab` | 3,182 | `63ade31e1188c223974338e6f1ae919ee75a8ae541c5df26e1faf973b39ee352` | `196e44ac93c3c87a5312fd22c6250ccd9d6a6310921fbc8a4a1706ba52aa3f4c` |
| `contract.descriptor` | 66 | `66f1916ea73f6f70c4ed0325e6e29b3cdb47e4efb66344addd04e750b88ce6b3` | `34d33b9b84624e00347780c7f89f1481e02577b7841ea26188b4af7788c62f9e` |

**Combined deploy envelope: 58,559 + 3,182 + 66 = 61,807 bytes — 3,430
bytes (≈5.3%) under the measured 65,237-byte combined limit** (see
"Build" above and `SPEC.md` §1 for how that limit itself was bisected).
`contract.acab`/`contract.descriptor` are unchanged from every prior
round (the contract's ABI is untouched by this round's work);
`contract.wasm` grew from 55,703 to 58,559 bytes (+2,856) when the
callback fast-dispatch path (`SPEC.md` §1, `docs/THREAT_MODEL.md`
§5.13) was added — shrinking the margin from the prior round's ≈9.6%
to today's ≈5.3%, worth tracking if future growth continues. Earlier
rounds' own smaller/larger figures for this same build (e.g. 55,703,
56,304, or the ≈8.7%/≈9.6% margins) are superseded by the numbers
above; they remain correct only as the historical record of what each
round's own build measured at the time, in `REVIEW.md`'s dated entries.

**BLAKE3 reproducibility, re-checked this round:** four consecutive
rebuilds from this exact path, with no source changes between them,
produced an *identical* BLAKE3 and SHA-256 both times (`3bd187ba...` /
`90ccbcd5...`, every time) — fully reproducible within a session, at a
fixed path, for fixed source. An earlier round's report that "the
printed BLAKE3 varied between builds while the SHA-256 did not" does
not hold up under this direct re-check: re-examining that report's own
numbers shows it was actually comparing BLAKE3 values of two
*genuinely different* wasm builds (pre- and post-fast-path, 55,703 vs
58,559 bytes) — both hash columns were expected to differ there, and
did; it was not evidence of same-content instability. Corrected here,
not silently.

**Reproducibility** (found originally, then re-verified in a later
round by building three times — this checkout's path, plus two
independent `/tmp` copies at two different absolute paths — which also
identified the BLAKE3 cause of the two-hash-table confusion above; see
`REVIEW.md` entry 24 and the round that re-verified it):
- **`contract.acab` and `contract.descriptor` are fully
  path-independent and byte-for-byte reproducible** — identical SHA-256
  across all three build locations.
- **`contract.wasm` is reproducible only when built from the same
  absolute path** — rebuilding twice from the identical path produces
  byte-identical output, but the two independent `/tmp` copies (built
  from two different absolute paths) each produced a *different* wasm
  (three builds, three different wasm hashes, one per distinct path).
  The compiled wasm evidently embeds something path-derived (most
  likely debug-info source paths from the standard Rust build, not
  investigated further than confirming the pattern). **Do not compare
  `contract.wasm` hashes across checkouts at different paths — compare
  `contract.acab`/`contract.descriptor` for that, and rebuild at this
  exact path to verify `contract.wasm`.**
