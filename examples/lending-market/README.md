# Lending Market

An over-collateralised lending market for one collateral asset and one borrowed
asset (two ECC currencies), with a **utilisation-based interest curve**, **share-based
supply accounting**, **liquidations**, **bad-debt write-off** and **acknowledged
payouts**. The market depends on the SDK.

| | |
|---|---|
| `contracts/lending` | the market (`avm-lending-market`): `state.rs` (canonical state), `math.rs` (256-bit `mul_div`, rate curve) |
| `contracts/payee` | fixture that receives a payout, echoes the receipt, or rejects it (exercises the bounce path) |
| `tests` | 33 behaviour tests, 5 math unit tests, 10 market↔payee integration tests, 1 randomized invariant test (40 seeds × 1,500 steps + crash scenarios) |
| `scripts` | `gen_abi.py` (manifests + `ids.rs` from one description), `check-sdk065.sh` (builds, tests and local-stand scenarios), `e2e.py` (historical SDK 0.4.0 journey) |
| `docs/SPEC.md` | the accounting rules and rounding directions |

## How it works

Funds enter with `deposit` (value in `extra`) and live on an internal ledger: each
account has idle balances in both currencies plus its market position. Nothing is
ever pushed to a user; money leaves only through `withdraw`.

- **Supply.** `supply` turns idle borrowed asset into *shares*. A share's value is
  `(cash + debt − reserves) / total_shares`, so it rises as borrowers pay interest.
  `redeem` burns shares for their value if it fits within the available cash,
  `max(cash − reserves, 0)`.
- **Borrow.** `add_collateral` locks collateral. `borrow` lends out up to
  `ltv` of the collateral's value. Debt is stored as `scaled` units; the real debt is
  `ceil(scaled × index / 1e18)`, and one global `index` grows with interest, so
  the accrual calculation is O(1). Ordinary calls calculate accrual
  before applying the requested operation. Only accepted calls persist it.
  Payout callbacks skip accrual.
- **Rate.** Annual rate in basis points from utilisation `u = debt / (cash + debt)`:
  `base + slope1 × min(u, kink)/kink + slope2 × max(u − kink, 0)/(1 − kink)`.
  `reserve_factor` of the interest goes to protocol reserves.
- **Liquidation.** A position whose debt exceeds `liq` of its collateral value can be
  liquidated by anyone: the liquidator repays at most **50%** of the debt (rounded up)
  and takes collateral worth the repayment plus `bonus`. If the collateral cannot
  cover the bonus the liquidator takes all of it and pays only what it is worth.
- **Bad debt.** Anyone can call `write_off` when debt remains and collateral is gone.
  An unhealthy position also qualifies at a fresh price if its entire collateral
  would yield zero liquidation repayment. In that case the collateral returns to
  the borrower’s idle balance. Reserves absorb the decrease in aggregate debt
  first, then suppliers share the remaining loss.
- **Price.** The admin feeds the price (borrowed units per collateral unit, scaled
  by 1e18). Each update may move it by at most `max_move` relative to the last one,
  and `borrow`, `remove_collateral` (with debt) and `liquidate` refuse a price older
  than `max_age`. The admin controls the feed and can update it repeatedly. The move cap applies
  to each update separately.
- **Admin.** `set_price`, `set_params` (rate curve, LTV, bonus, reserve factor, price
  guards and the pause switch, within configured bounds),
  `collect_reserves`. Pausing stops new supply, collateral and borrowing only;
  repayment, redemption, collateral exit, liquidation and withdrawal stay open.
- **Withdrawals.** `withdraw` debits the ledger first and sends the funds with a
  callback request. The receipt `LND1 | currency u32 | id u64 | amount u128` is echoed
  by an accepting receiver. A bounce returns the exact principal into the ledger. One
  payout per recipient may be in flight (at most 8), so the callback source identifies it.
  Settlement trusts the native callback status and checks a bounded history of cause
  hashes. The echo is a payee fixture assertion. A rejected callback leaves the
  payout pending, occupying a slot and preventing another payout to the same
  recipient. The example relies on the runtime to deliver a valid settlement
  callback and exposes no timeout, cancellation, or recovery method.

**Safety net.** Every call re-validates the whole canonical state (totals equal the
sums over accounts, index ≥ 1, no empty or duplicate accounts, pending payouts
consistent) and ordinary calls check that the contract physically holds at least what the ledger
says (`ERR_INSOLVENT` otherwise). Deposits use the post-credit balance for the
second check. A rejected call returns no effects.

## Wire layouts (little endian)

| call | body |
|---|---|
| constructor | 80 bytes: `admin[32] · collateral u32 · debt u32 · ltv u16 · liq u16 · bonus u16 · reserve_factor u16 · kink u16 · base u16 · slope1 u16 · slope2 u16 · max_move u16 · pad[2] · max_age u32 · fuel u128` |
| `lend.deposit` | empty; value in `extra` (the two market currencies only) |
| `lend.supply / redeem / add_collateral / remove_collateral / borrow / repay` | `amount u128` (`redeem` takes shares; `repay` is capped at the debt) |
| `lend.liquidate` | `borrower[32] · repay u128` |
| `lend.write_off` | `borrower[32]` |
| `lend.withdraw` | `currency u32 · amount u128 · recipient dapp_id[32] · account_id[32]` |
| `lend.set_price` | `price u128` (admin) |
| `lend.set_params` | `ltv … max_move` as 9 × `u16` · `max_age u32` · `paused u8` (admin) |
| `lend.collect_reserves` | empty (admin) |
| `lend.topup` | empty; SHELL in `extra` (FUEL for payouts) |

State is a fixed 4528-byte image (24 accounts, 8 pending payouts, 16 callback
receipts), full-replaced on each call. Errors 5000–5016. Events: `lend.created`,
`lend.action` (one event for every ledger movement, with an action code, amount,
an action-specific number, holder and counterparty), `lend.price`, `lend.params`.

## Verify on AVM SDK 0.6.5

Use Rust and Cargo 1.99.1 with the `wasm32-unknown-unknown` target. Use the
[SDK 0.6.5 Linux x86-64 bundle](https://github.com/gosh-sh/avm-sdk/releases/tag/avm-dev-d72c855b2eae-sdk-d89a8e7c92c2)
with archive SHA-256 `37e290db6b827ad9ec05f48ef1d35b3d07ab553167b43e9c2e1d32faa6bdf8a3`.

```sh
export RUSTUP_TOOLCHAIN=1.99.1
bash scripts/check-sdk065.sh /path/to/avm-sdk-linux-x86_64 /path/to/new-output
```

The script stages the example against the published SDK bundle, builds deployment
artifacts, runs native tests and records the local-stand scenarios as JSON.
[VALIDATION-0.6.5.md](VALIDATION-0.6.5.md) gives the measured results and scope.
[VALIDATION-0.4.0.md](VALIDATION-0.4.0.md) records the previous build and journey.

## Scope and limits

This unaudited example uses a trusted admin price feed and a single collateral
and debt pair. Use fresh stand state and synthetic balances. Guest calls from
wallets use SHA-256 of the full internal wallet address as the holder key.
The administrator's external key remains the owner. Existing-account migration
and Shellnet deployment remain unvalidated.
