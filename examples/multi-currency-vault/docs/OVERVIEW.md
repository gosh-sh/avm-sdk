# Overview

One page linking the rest of this contract's documentation, with the
real, current numbers in one place. Nothing here is claimed to be
"secure" or "audited" — see "What is NOT claimed" at the bottom.

## Where to look

- **[`SPEC.md`](../SPEC.md)** — the full behavioral specification: every
  method, every invariant, every cap, and the full history of how each
  one was derived and measured.
- **[`docs/THREAT_MODEL.md`](THREAT_MODEL.md)** — per-attack analysis,
  trust assumptions, and the full list of unresolved residual risks,
  named rather than solved.
- **[`REVIEW.md`](../REVIEW.md)** — the complete engagement log: every
  round's own task, every judgment call, every finding and its triage,
  in full and unabridged.
- **[`docs/PLATFORM_FINDINGS.md`](PLATFORM_FINDINGS.md)** — platform
  behavior discovered empirically on the local stand (deploy-envelope
  limits, fuel costs, method-id derivation, etc.) that isn't documented
  anywhere else in this repository.
- **[`docs/MAINTAINER_QUESTIONS.md`](MAINTAINER_QUESTIONS.md)** — every
  question this engagement could not resolve from the repository alone,
  priority-ordered, with current status.
- **[`README.md`](../README.md)** — quickstart, build instructions, the
  demo-owner-key warning, and the canonical build-artifact hashes/sizes.

## The real numbers, as of this round

- **89 tests** (83 host-level unit/property/simulator + 6 fuzz-style,
  zero panics across 80,000+ fuzz iterations).
- **`cargo mutants`: 224 caught, 4 missed (all provably equivalent, each
  with a one-line argument), 6 timeouts (treated as effectively caught —
  each is a genuine infinite loop under the mutation, detected via hang
  rather than a failed assertion).**
- **Callback (Settle/Restore) spare K by state size** (dummy-work fuel
  padding, bisected): ≈15,775–16,150 at 1 row + 1 pending, down to 7,600
  (Settle) / 7,525 (Restore) at the true 252-row + 32-pending worst
  case, for the baseline dispatch; the current fast-dispatch path raises
  that to 13,000 (Settle) / 12,250 (Restore) at the same worst case —
  ≈1.71×/≈1.63× more margin. Full table: `SPEC.md` §1,
  `docs/THREAT_MODEL.md` §5.13.
- **Deploy envelope**: `contract.wasm` (58,559) + `contract.acab`
  (3,182) + `contract.descriptor` (66) = 61,807 bytes — ≈5.3% under the
  measured 65,237-byte combined limit. Canonical hashes (BLAKE3 and
  SHA-256, both labeled): `README.md`'s own "Build artifacts" section —
  the one place these numbers are kept current.
- **Real local-stand E2E journey** (`scripts/e2e_journey.sh`): deposit,
  withdraw→Settle, withdraw→Restore, owner-gated pause, two-step owner
  rotation, and a withdraw rejected because the vault's own SHELL
  balance is below `MIN_OPERATIONAL_SHELL` — all six assert on the
  vault's own decoded state, pass end to end, reproducibly.

## What is NOT claimed

- **Not "secure" or "audited."** This is a from-scratch spec/implement/
  test/review engagement by one party, not an independent security
  audit by a separate firm.
- **Not verified on any real network.** Every number above, and every
  number in `SPEC.md`/`REVIEW.md`, is a *local-stand* measurement. The
  real mainnet fuel model, fee model, and deploy-envelope limit are
  unconfirmed to match this local stand's own (`docs/MAINTAINER_QUESTIONS.md`
  #11).
- **`TOPUP`'s real-network behavior is unverified** — it has only ever
  been exercised on this local stand.
- **A callback no longer re-validates the stored ledger's own content**
  on the Settle/Restore step specifically (only its declared length) —
  an accepted trade-off for callback fuel margin, not a defect; see
  `docs/THREAT_MODEL.md` §5.13 for exactly what this does and does not
  change for an attacker.
- **Not reproducible across different absolute checkout paths** for
  `contract.wasm` specifically (`contract.acab`/`contract.descriptor`
  are) — see `README.md`'s own "Reproducibility" note.
- **The default build's owner key is a public, checked-in fallback** —
  explicitly for local testing only, never for anything real.
- **Restore's own behavior under direct ledger-content corruption**
  (as opposed to pending-section corruption, which is tested) was not
  separately constructed as a host test this round — logged as an open
  residual risk, not asserted away (`docs/THREAT_MODEL.md` §5.13's own
  closing paragraph).
