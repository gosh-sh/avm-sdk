# Vault AMM

A constant-product (x·y = k) market maker for two ECC currencies, built as a
**vault with an internal ledger** instead of a token-minting DEX. Everything is
one AVM contract plus a small companion contract used as a payout destination.

| | |
|---|---|
| `contracts/amm` | the AMM (`avm-vault-amm`), `no_std`-style message contract |
| `contracts/receiver` | fixture that receives payouts, echoes the receipt, or rejects (exercises the bounce path) |
| `tests` | 42 AMM tests, a 2450-vector arithmetic oracle, deterministic fuzzing, AMM↔Receiver integration |
| `scripts` | manifest→`ids.rs` generator/checker, Python big-int oracle, `check.sh` |

## What it does

* **Deposit** either side of the pair (ECC `extra` entries) into a per-holder ledger.
* **Add / remove liquidity** against the ledger. Uniswap-v2 shares: first mint is
  `sqrt(a0·a1) − 1000` (the 1000 are locked forever), later mints are
  proportional with the excess left in the ledger. Slippage minimums and a deadline on every call.
* **Swap** exact-in or exact-out with an LP fee (≤ 10%) and an optional protocol
  cut (≤ 50% of the fee). Exact-in rounds output down; exact-out rounds the
  required input up twice, always in the pool's favour, so `k` never decreases.
* **Withdraw** ledger balances to any address. The payout is a message with a
  callback: the pool debits first, remembers one pending payout per recipient and
  re-credits the holder if delivery bounces. A success callback must echo the
  32-byte receipt `"VAM1" | currency u32 | id u64 | amount u128` exactly.
* **TWAP** accumulators (UQ64.64 price × seconds, wrapping) for both directions.
* **Admin**: `set_params` (fees, pause), `collect_fees`. Pausing blocks new
  activity but never exits (withdrawals, remove-liquidity, callbacks keep working).
* **Self-check on every call**: the canonical state is re-validated and the vault's
  real ECC balance must cover `reserve + fees + holder balances`
  (`ERR_INSOLVENT` otherwise). A failed call returns no effects.

Errors are 3000–3015 (receiver 2200–2202), all declared in the manifests.

## Wire layouts (little endian)

| method | body |
|---|---|
| `vault.amm.deposit` | empty; value in `extra` (either/both pool currencies) |
| `vault.amm.add_liquidity` | desired0 u128 · desired1 u128 · min0 u128 · min1 u128 · deadline u32 (68 B) |
| `vault.amm.remove_liquidity` | shares u128 · min0 u128 · min1 u128 · deadline u32 (52 B) |
| `vault.amm.swap_exact_in` / `_out` | direction u8 · exact u128 · bound u128 · deadline u32 (37 B); bound = min out / max in |
| `vault.amm.withdraw` | currency u32 · amount u128 · recipient dapp_id[32]+account_id[32] (84 B) |
| `vault.amm.set_params` | swap_fee_bps u16 · protocol_fee_bps u16 · paused u8 (5 B) |
| `vault.amm.topup` | SHELL only, funds the FUEL reserve for payouts |

State is a fixed 4376-byte image (32 holder slots, 8 pending payouts, ring of 16
receipts), full-replaced each call and kept under the 65536-byte FullReplace limit.
Worked example in the tests: pool 1_000_000 / 1_000_000, fee 30 bps, 10_000 in → 9_871 out.

## Verify

```sh
pip install blake3
./scripts/check.sh      # ids, oracle vectors, fmt, clippy -D warnings, tests
```

## Honest status

Verified here: all native tests, clippy, and that the id rule
(`u64_le(blake3(name)[0..8])`) reproduces the 22 official ids in `bounty-escrow`
and `fungible-token`.

**Not verified** (no wasm32 target, no release download in the build sandbox):
the real `avm-contract-build` output / ACAB check, WASM size and fuel cost, and an
end-to-end run on a local stand. The harness builds ACMI frames from the SDK's
documented layout; whether `input.extra` holds the pre- or post-credit balance on
the live runtime is handled both ways (tests cover both), but a deposit-sized
shortfall in the solvency check is the accepted limitation. Not audited; do not
use with real funds.
