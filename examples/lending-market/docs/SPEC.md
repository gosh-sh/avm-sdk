# Accounting rules

## Units

- `index`: borrow index, 1e18 = 1.0, never below 1e18.
- `scaled`: a borrower's debt in index-scaled units; `debt = ceil(scaled × index / 1e18)`.
- `price`: borrowed units per collateral unit, scaled by 1e18.
- `value = cash + debt − reserves` is what suppliers collectively own;
  `shares × value / total_shares` is what a supplier can redeem.
- `available = cash − reserves`: what can be lent or redeemed (reserves stay backed by cash).

## Rounding (always against the caller, in favour of the market)

| operation | rounding |
|---|---|
| debt from scaled | up |
| `borrow`: scaled added | up |
| `repay`: scaled removed | down (the full debt, when fully repaid, is exact) |
| `supply`: shares minted | down |
| `redeem`: assets paid | down |
| collateral value, limits | down |
| liquidation: collateral seized | down |
| reserves from interest | down |

These directions make `value` (cash + debt − reserves) non-decreasing under every
operation except `write_off`, and make the share price non-decreasing between write-offs.
The randomized test asserts both.

## Interest

At the start of every call, `dt = now − last_accrual`. With `u = debt × 1e4 / (cash + debt)`
the annual rate in bps is `base + slope1·min(u,kink)/kink + slope2·(u−kink)/(1e4−kink)` (the
last term only above the kink). `index += index × rate × dt / (31_536_000 × 1e4)`. The
interest added to the debt is split: `reserve_factor` of it to `reserves`, the rest to suppliers
through `value`. A parameter change is applied after accrual, so it never applies retroactively.

## Risk checks

- `borrow` / `remove_collateral`: `debt ≤ collateral_value × ltv / 1e4`, with a fresh price.
- `liquidate`: requires `debt > collateral_value × liq / 1e4`, fresh price, repay ≤ ⌈debt/2⌉.
  Parameter bounds (`ltv < liq ≤ 9500`, `liq × (1e4 + bonus) ≤ 0.99e8`) guarantee a liquidation
  at the threshold does not make the position worse.
- `write_off`: only when `collateral == 0 && debt > 0`.

## Invariants (checked by `State::validate` on every decode and before every output)

- `Σ account.shares = total_shares`, `Σ scaled = total_scaled`, `Σ collateral = total_collateral`.
- `reserves ≤ cash + debt`.
- Per currency, ledger (`total_collateral` or `cash`, plus idle balances) equals the physical balance.
- No empty, zero-key or duplicate accounts; pending payouts reference existing accounts, distinct
  recipients and ids; the receipt ring holds distinct nonzero hashes in exactly the written slots.
