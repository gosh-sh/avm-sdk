# Repair validation — 2026-11-01

Baseline: `82d3b23ad051e2d970bb7ff413e3f186a1637977`.
Source digests and artifact hashes: [evidence/repair-validation.json](evidence/repair-validation.json).
Evidence applies to the listed source digests; documentation-only changes do not change those digests.

## Executed locally

Toolchain: Rust 1.93.0, Linux x86_64, SDK release
`avm-dev-2662a32e7c91-sdk-93e34e16e449`.

- Baseline original native workspace: PASS.
- Three added regressions against original code: FAIL as expected (malformed
  successful receipt, deposit masking a shortfall, overflowing TWAP price).
- `scripts/check.sh`: PASS, including identifier checks, 2450 Python oracle
  vectors, fmt, native/wasm32 clippy `-D warnings`, and 58 native tests:
  4 math, 45 AMM, 2 fuzz, 5 integration, 2 arithmetic-oracle tests.
- `avm-contract-build build --manifest-path contracts/amm/Cargo.toml`: PASS.
- `avm-contract-build build --manifest-path contracts/receiver/Cargo.toml`: PASS.
- `python3 scripts/e2e.py --sdk <release-directory>`: PASS, 32 calls, each
  explicitly required to complete with a 1 SHELL reserve. Exact FUEL remains
  unknown. Includes success/refund/retry, exact-in/out bounds, TWAP at ordinary
  ratios, status and identical replay roots.
- Stand adversarial case: nine consecutive accepted payouts with empty callback
  result bodies, then an accepted payout by another holder, all with no retained
  pending slot. No manual callback injection is used by this stand journey.
- Negative stand control: original AMM WASM with the repaired adversarial
  receiver fixture leaves `pending = 1` after the first accepted empty-receipt
  payout; the new journey fails its `pending == 0` assertion, as expected.

Native success-body cases additionally cover 31/33-byte results, altered magic,
currency/id/amount and unrelated bytes, including a malformed result combined
with a legitimate unused-SHELL refund. Forged source/method, unexpected
principal/currencies and inexact refunds remain rejected. TWAP native tests
cover both directions across the representability boundary, fractional large
ratios and repeated modular accumulation. Deposit shortfall tests cover both
currencies and amounts below/equal/above the deficit. Healthy deposits and
normal payouts continue to pass.

## Coverage limits

The new TWAP extremes and injected deficit cases are native tests, not stand
journey cases. The stand exercises the guest allocator and actual native
callbacks, but does not expose an independent physical ECC balance oracle.
Receiver-controlled callback bodies are diagnostics; status/source/value are
runtime-authenticated, as exercised in `scripts/tests/fixture_oracle.py` and
`packaged_lifecycle.py`. Arbitrary user-crafted internal callback headers are
not an authority to settle withdrawals.

A missing runtime callback can still retain a slot. No timeout-based refund is
introduced because a payout may already be delivered. The bounded receipt ring
is not a substitute for native provenance and one-time runtime delivery.

No upstream CI pass, maintainer approval, external audit, real-fund suitability,
mainnet execution, measured maximum heap, account migration or current-head
second-version publication is claimed.
