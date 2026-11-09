# Lending Market

An over-collateralised lending market for one collateral asset and one borrowed
asset (two ECC currencies), with a **utilisation-based interest curve**, **share-based
supply accounting**, **liquidations**, **bad-debt write-off** and **acknowledged
payouts**. One contract, no dependencies beyond the SDK.

| | |
|---|---|
| `contracts/lending` | the market (`avm-lending-market`): `state.rs` (canonical state), `math.rs` (256-bit `mul_div`, rate curve), `rt.rs` (`no_std` guest runtime) |
| `contracts/payee` | fixture that receives a payout, echoes the receipt, or rejects it (exercises the bounce path) |
| `tests` | 31 behaviour tests, 8 market↔payee integration tests, 1 randomized invariant test (40 seeds × 1,500 steps + crash scenarios) |
| `scripts` | `gen_abi.py` (manifests + `ids.rs` from one description), `check.sh` |
| `docs/SPEC.md` | the accounting rules and rounding directions |

## How it works

Funds enter with `deposit` (value in `extra`) and live on an internal ledger: each
account has idle balances in both currencies plus its market position. Nothing is
ever pushed to a user; money leaves only through `withdraw`.

- **Supply.** `supply` turns idle borrowed asset into *shares*. A share's value is
  `(cash + debt − reserves) / total_shares`, so it rises as borrowers pay interest.
  `redeem` burns shares for their value as far as the market holds cash.
- **Borrow.** `add_collateral` locks collateral. `borrow` lends out up to
  `ltv` of the collateral's value. Debt is stored as `scaled` units; the real debt is
  `ceil(scaled × index / 1e18)`, and one global `index` grows with interest, so
  accrual is O(1) and happens at the start of every call.
- **Rate.** Annual rate in basis points from utilisation `u = debt / (cash + debt)`:
  `base + slope1 × min(u, kink)/kink + slope2 × max(u − kink, 0)/(1 − kink)`.
  `reserve_factor` of the interest goes to protocol reserves.
- **Liquidation.** A position whose debt exceeds `liq` of its collateral value can be
  liquidated by anyone: the liquidator repays at most **50%** of the debt (rounded up)
  and takes collateral worth the repayment plus `bonus`. If the collateral cannot
  cover the bonus the liquidator takes all of it and pays only what it is worth.
- **Bad debt.** When all collateral is gone but debt remains, anyone can call
  `write_off`: the protocol reserves absorb the loss first, then suppliers share
  the remainder through the share price.
- **Price.** The admin feeds the price (borrowed units per collateral unit, scaled
  by 1e18). Each update may move it by at most `max_move` relative to the last one,
  and `borrow`, `remove_collateral` (with debt) and `liquidate` refuse a price older
  than `max_age`. The oracle is the one trusted input of this example.
- **Admin.** `set_price`, `set_params` (rate curve, LTV, bonus, reserve factor, price
  guards and the pause switch, all within hard bounds that keep liquidation sound),
  `collect_reserves`. Pausing stops new supply, collateral and borrowing only;
  repayment, redemption, collateral exit, liquidation and withdrawal stay open.
- **Withdrawals.** `withdraw` debits the ledger first and sends the funds with a
  callback request. The receipt `LND1 | currency u32 | id u64 | amount u128` is echoed
  by an accepting receiver. A bounce returns the exact principal into the ledger. One
  payout per recipient may be in flight (at most 8), so the callback source identifies it.

**Safety net.** Every call re-validates the whole canonical state (totals equal the
sums over accounts, index ≥ 1, no empty or duplicate accounts, pending payouts
consistent) and checks that the contract physically holds at least what the ledger
says (`ERR_INSOLVENT` otherwise). A rejected call returns no effects.

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

## Verify

```sh
pip install blake3
./scripts/check.sh      # id rule, manifests, fmt, clippy (native and wasm32) -D warnings, tests
```

On Linux with the SDK commands installed:

```sh
avm-contract-build build --manifest-path contracts/lending/Cargo.toml
avm-contract-build build --manifest-path contracts/payee/Cargo.toml
```

The randomized test checks after every accepted call that the ledger equals the
physical balance **exactly** for both currencies, that everything ever deposited is
held, in flight or delivered, that the share price never falls (except through a
write-off), that whoever borrows or pulls collateral ends within `ltv`, that
liquidation only touches unhealthy positions and never closes more than 50%, and that
a rejected call changes nothing.

## Size

The contract was designed for the 65,536-byte deploy-body limit from the start: it
is `no_std` on `wasm32` (the allocator and panic handler in `rt.rs`, the same as in
the other examples), has no sort or formatting code, a handful of short events, and
one fixed-size state. The numbers for a build with `avm-contract-build` are recorded
in the pull request description.

## Scope and limits

Unaudited example code, not for real funds. Interest grows the index linearly
between calls and compounds at each call, the price is a trusted admin feed, and
there is a single collateral/debt pair. Liquidations are partial by design (50% close
factor); a position can be liquidated again in the next call while it stays unhealthy.
