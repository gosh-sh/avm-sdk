# Lending Market

An over-collateralised lending market for one collateral asset and one borrowed
asset (two ECC currencies), with a **utilisation-based interest curve**, **share-based
supply accounting**, **liquidations**, **bad-debt write-off** and **acknowledged
payouts**. One contract, no dependencies beyond the SDK.

| | |
|---|---|
| `contracts/lending` | the market (`avm-lending-market`): `state.rs` (canonical state), `math.rs` (256-bit `mul_div`, rate curve), `rt.rs` (`no_std` guest runtime) |
| `contracts/payee` | fixture that receives a payout, echoes the receipt, or rejects it (exercises the bounce path) |
| `tests` | 33 behaviour tests, 5 math unit tests, 8 market↔payee integration tests, 1 randomized invariant test (40 seeds × 1,500 steps + crash scenarios) |
| `scripts` | `gen_abi.py` (manifests + `ids.rs` from one description), `check.sh`, `e2e.py` (a loan's life on `avm-local-stand`) |
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
  hashes. The echo is a payee fixture assertion.

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

## Verify

For the recorded SDK 0.4.0 results, use the checksum-verified immutable bundle
`avm-dev-d103b6ccc759-sdk-21364083b24a` and Rust/Cargo 1.99.1.
Install that exact tag using the repository's `scripts/release.py install`
with its release archive and checksum file before continuing. The commands below
assume the default SDK installation directory; adjust it for a custom installation.
They select the immutable release directly rather than the mutable `current` link:

```sh
export AVM_REPRO_SDK="$HOME/.local/share/avm-sdk/releases/avm-dev-d103b6ccc759-sdk-21364083b24a/avm-sdk-linux-x86_64"
test -x "$AVM_REPRO_SDK/bin/avm-contract-build" &&
test -x "$AVM_REPRO_SDK/bin/avm-local-stand" || exit 1
export PATH="$AVM_REPRO_SDK/bin:$PATH"
export RUSTUP_TOOLCHAIN=1.99.1
rustc --version
cargo --version
```

Run the following commands from this example's directory in the same shell.
Using another SDK bundle or toolchain requires a new validation; these instructions
do not qualify SDK 0.5.x.

```sh
pip install blake3
./scripts/check.sh      # id rule, manifests, fmt, clippy (native and wasm32) -D warnings, tests
```

On Linux with the SDK commands installed:

```sh
avm-contract-build build --manifest-path contracts/lending/Cargo.toml
avm-contract-build build --manifest-path contracts/payee/Cargo.toml
python3 scripts/e2e.py --sdk "$AVM_REPRO_SDK"
```

`e2e.py` deploys the market and a payee and walks three stand identities through
supply, collateral, borrowing, thirty days of interest, a price falling in steps of
at most 20%, liquidation in rounds, a write-off, redemption and withdrawals
(delivered, bounced by a payee that rejects, delivered again). It carries its own
model of the accounting and compares decoded accounting fields and accounts at
checkpoints. FUEL probes compare the call status on copies of the stand state.
The main journey checks settlement separately. The peak heap is unmeasured.

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
one fixed-size state. With SDK 0.4.0 and Rust 1.99.1, the lending build produces
58,576 B of WASM, 1,861 B of ACAB and a 66 B descriptor. With 80 B of init data
and a conservatively counted 289 B deploy envelope, the calculated deploy body is
**60,872 B**, leaving **4,664 B** under the 65,536 B limit. Using the upstream
`docs/runtime-limits.md` envelope of 288 B gives **60,871 B** and 4,665 B headroom.
The one-byte envelope difference is not independently measured here.
The previously documented 60,231 B used 57,935 B of WASM and was not reproduced
at the current source revision; it is not the authoritative current-build size.
See [VALIDATION-0.4.0.md](VALIDATION-0.4.0.md) and the accompanying artifact hashes
and executed logs. Rebuild and remeasure when changing SDK, toolchain or source.

## Scope and limits

Unaudited example code, not for real funds. Interest grows the index linearly
between calls and compounds at each call, the price is a trusted admin feed, and
there is a single collateral/debt pair. Liquidations are partial by design (50% close
factor); a position can be liquidated again in the next call while it stays unhealthy.

The stand journey now requires every call probe to return the expected status at
1 SHELL and executes the main calls with the same 1 SHELL reserve. Its state,
payout and replay assertions therefore also run at that reserve. A probe requiring
more FUEL fails the journey. Exact FUEL consumption remains unmeasured.

The local-stand journey does not exercise `repay`, `remove_collateral`,
`set_params` (including pausing), `collect_reserves`, or the `lend.params` event.
These are covered by native tests only. Native tests use the std code path;
the stand journey exercises the wasm32 guest runtime only for the paths it executes.

The 1-SHELL reserve bound applies to the listed direct journey calls, including
calls expected to return a contract error. Deployment uses a separate reserve.
Outbound payouts are configured with a separate 10,000-SHELL fuel reserve;
successful settlement does not establish a 1-SHELL bound for each downstream
payout or callback message. Exact FUEL consumption remains unmeasured.
