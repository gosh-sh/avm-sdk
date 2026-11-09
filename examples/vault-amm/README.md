# Vault AMM

A constant-product (x·y = k) market maker for two ECC currencies, built as a
**vault with an internal ledger** instead of a token-minting DEX. Everything is
one AVM contract plus a small companion contract used as a payout destination.

| | |
|---|---|
| `contracts/amm` | the AMM (`avm-vault-amm`), a message-isolated contract |
| `contracts/receiver` | fixture that receives payouts, echoes the receipt, or rejects (exercises the bounce path) |
| `tests` | 40 AMM tests, a 2450-vector arithmetic oracle, deterministic fuzzing, AMM↔Receiver integration |
| `scripts` | manifest→`ids.rs` generator/checker, Python big-int oracle, `check.sh`, `e2e.py` (real stand journey) |

## What it does

* **Deposit** either side of the pair (ECC `extra` entries) into a per-holder ledger.
* **Add / remove liquidity** against the ledger. Uniswap-v2 shares: first mint is
  `sqrt(a0·a1) − 1000` (the 1000 are locked forever), later mints are
  proportional with the excess left in the ledger. Slippage minimums and a deadline on every call.
* **Swap** exact-in with an LP fee (≤ 10%) and an optional protocol cut (≤ 50% of
  the fee). Output rounds down in the pool's favour, so `k` never decreases.
* **Withdraw** ledger balances to any address. The payout is a message with a
  callback: the pool debits first, remembers one pending payout per recipient and
  re-credits the holder if delivery bounces. A success callback must echo the
  32-byte receipt `"VAM1" | currency u32 | id u64 | amount u128` exactly.
* **Admin**: `set_params` (fees, pause), `collect_fees`. Pausing blocks new
  activity but never exits (withdrawals, remove-liquidity, callbacks keep working).
* **Self-check on every call**: the canonical state is re-validated and the vault's
  ECC balance must cover `reserve + fees + holder balances`
  (`ERR_INSOLVENT` otherwise). A failed call returns no effects.

Errors are 3000–3015 (receiver 2200–2202), all declared in the manifests.

### What was left out to fit the deploy limit

The AMM's first draft was 72,560 B of WASM, which `avm-local-stand` refuses to
deploy: the whole deploy body (WASM + ACAB + descriptor + init data + 289 B of
envelope) must fit `max_body_size = 65,536 B`. After size work that does not
change behaviour (see the commit history), three features were removed:
exact-output swaps (`swap_exact_out`; `math::swap_in` remains as a tested library
function), the TWAP price accumulators (their 40 state bytes are now reserved and
must be zero), and the read-only `status` method and event.

## Wire layouts (little endian)

| method | body |
|---|---|
| `vault.amm.deposit` | empty; value in `extra` (either/both pool currencies) |
| `vault.amm.add_liquidity` | desired0 u128 · desired1 u128 · min0 u128 · min1 u128 · deadline u32 (68 B) |
| `vault.amm.remove_liquidity` | shares u128 · min0 u128 · min1 u128 · deadline u32 (52 B) |
| `vault.amm.swap_exact_in` | direction u8 · amount_in u128 · min_out u128 · deadline u32 (37 B) |
| `vault.amm.withdraw` | currency u32 · amount u128 · recipient dapp_id[32]+account_id[32] (84 B) |
| `vault.amm.set_params` | swap_fee_bps u16 · protocol_fee_bps u16 · paused u8 (5 B) |
| `vault.amm.collect_fees` | empty; admin only |
| `vault.amm.topup` | SHELL only, funds the FUEL reserve for payouts |

State is a fixed 4376-byte image (32 holder slots, 8 pending payouts, ring of 16
receipts), full-replaced each call and kept under the 65536-byte FullReplace limit.
Worked example in the tests: pool 1_000_000 / 1_000_000, fee 30 bps, 10_000 in → 9_871 out.

## Verify

```sh
pip install blake3
./scripts/check.sh      # ids, oracle vectors, fmt, clippy -D warnings, tests
```

On a machine with the SDK installed (`scripts/install.sh` in the repository root),
build both contracts and run the journey on the local stand:

```sh
avm-contract-build build --manifest-path contracts/amm/Cargo.toml
avm-contract-build build --manifest-path contracts/receiver/Cargo.toml
python3 scripts/e2e.py --sdk "$HOME/.local/share/avm-sdk/current"
```

## Sizes

Built with SDK release `avm-dev-2662a32e7c91-sdk-93e34e16e449`:

| artifact | AMM | receiver |
|---|---|---|
| `contract.wasm` | 63,130 B | 39,396 B |
| `contract.acab` | 1,922 B | 361 B |
| `contract.descriptor` | 66 B | 66 B |

AMM deploy body: 63,130 + 1,922 + 66 + 64 (init) + 289 (envelope) = 65,471 B of 65,536 B.
There is little headroom: adding methods or events to the AMM will need further
size work.

## Verified on the builder and the local stand

* `avm-contract-build build` produces both artifact triples; every method and
  event id in the built ACAB equals `ids.rs`, and `ids.rs` equals the output of
  `scripts/gen_ids.py`.
* `scripts/check.sh` passes (fmt, clippy `-D warnings`, all tests).
* `scripts/e2e.py` passes on `avm-local-stand`: deploy of both contracts, bind,
  SHELL top-up, deposit, add liquidity, swap (and a rejected slippage bound),
  withdraw to the receiver with callback settlement, a rejecting receiver that
  bounces the payout and re-credits the holder, a retried payout, remove
  liquidity, and journal replay with identical roots. After each step the decoded
  ledger and reserves are compared with figures the script computes itself.
* Each call of that journey completes within a 1 SHELL FUEL reserve
  (1,000,000 FUEL; the per-message cap is 10,000,000). This is an upper bound:
  the stand does not report the exact `fuel_used` for successful calls.
* `publish-version` of a second AMM build into the same package succeeds. The
  version's constructor runs, so the publisher must be the pool admin named in the
  init data (a `package-owner` publisher with another admin is rejected with 3001).
  This was run by hand and is not part of `e2e.py`.
* `input.extra` carries the **post-credit** balance: a scratch contract that
  recorded `input.extra` on every call saw the balance already including the
  message's own transfer (calls with `0:1000,7:2000` then `0:500` showed 1000/2000
  and then 1500/2000), and it also lists the SHELL balance (currency 2). The
  probe is not included here. The solvency check works under either reading, but
  with the post-credit balance a shortfall of up to the size of the current
  call's own deposit goes unnoticed until the next call; the tests exercise both
  framings.

## Not verified

* The stand does not expose per-account ECC balances, so the vault's real balances
  are only checked indirectly through the contract's own solvency check.
* Exact FUEL per call (only the 1 SHELL bound above).
* Behaviour on a network other than the local stand. The contract is not audited;
  do not use it with real funds.
