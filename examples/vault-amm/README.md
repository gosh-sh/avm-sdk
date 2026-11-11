# Vault AMM

A constant-product (x·y = k) market maker for two ECC currencies, built as a
**vault with an internal ledger** instead of a token-minting DEX. Everything is
one AVM contract plus a small companion contract used as a payout destination.

| | |
|---|---|
| `contracts/amm` | the AMM (`avm-vault-amm`), a message-isolated contract |
| `contracts/receiver` | fixture that receives payouts, echoes the receipt, or rejects (exercises the bounce path) |
| `tests` | 45 AMM tests, a 2450-vector arithmetic oracle, deterministic fuzzing, AMM↔Receiver integration |
| `scripts` | manifest→`ids.rs` generator/checker, Python big-int oracle, `check.sh`, `e2e.py` (real stand journey) |

## What it does

* **Deposit** either side of the pair (ECC `extra` entries) into a per-holder ledger.
* **Add / remove liquidity** against the ledger. Uniswap-v2 shares: first mint is
  `sqrt(a0·a1) − 1000` (the 1000 are locked forever), later mints are
  proportional with the excess left in the ledger. Slippage minimums and a deadline on every call.
* **Swap** exact-in or exact-out with an LP fee (≤ 10%) and an optional protocol
  cut (≤ 50% of the fee). Exact-in rounds output down; exact-out rounds the
  required input up twice, always in the pool's favour, so `k` never decreases.
* **Withdraw** ledger balances to an address that can accept the payout method. The payout is a message with a
  callback: the pool debits first, remembers one pending payout per recipient and
  re-credits the holder if delivery bounces. The 32-byte receipt
  `"VAM1" | currency u32 | id u64 | amount u128` is diagnostic: a native success
  callback settles even if the receiver echoes incorrect or empty bytes. Delivery
  is authenticated by the runtime callback, not by receiver-controlled result bytes.
  Source/header/replay and returned-value checks remain mandatory.
* **TWAP** accumulators (UQ64.64 price × seconds, wrapping) for both directions,
  and a read-only **`status`** method that emits the pool's reserves, fees, share
  supply, op counter and pause flag as an event.
* **Admin**: `set_params` (fees, pause), `collect_fees`. Pausing blocks new
  activity but does not itself block exits. Withdrawals still require a free pending
  slot, sufficient SHELL for FUEL and an accepting receiver.
* **Self-checks**: every call validates canonical state. External calls check that
  post-credit ECC balances cover `reserve + fees + holder balances`; deposits
  additionally check the newly credited ledger before committing. Native callbacks
  validate principal refunds separately and do not run the global balance guard.
  A failed call returns no contract effects.

Errors are 3000–3015 (receiver 2200–2202), all declared in the manifests.

### `no_std` guest runtime

The first draft was 72,560 B of WASM, which `avm-local-stand` refuses to deploy:
the whole deploy body (WASM + ACAB + descriptor + init data + 289 B of envelope)
must fit `max_body_size = 65,536 B`. An earlier revision of this example removed
`swap_exact_out`, the TWAP accumulators and the `status` method to fit. They are
back, because most of the size was not the contract logic but the allocator and
panic machinery that `std` links in.

On `wasm32` both contracts are now `no_std`. `contracts/amm/src/rt.rs`, which the
receiver includes by path, supplies a bump `#[global_allocator]` over a static
128 KiB heap, recycled whenever no allocation is live (an allocation failure
traps), and a `#[panic_handler]` that traps. The SDK is already `no_std` and its
export macro installs no allocator. The receiver no longer depends on the amm
crate; it includes `ids.rs` by path.

This is the only `unsafe` code in the example: native builds keep
`forbid(unsafe_code)`, `wasm32` uses `deny(unsafe_code)` with one `allow` on the
`rt` module. The heap holds a copy of the whole init input, which contains the
deploy body, so it is sized by `max_body_size`, not by the steady state: the
journey passes with a 64 KiB heap (deploy body 57 KB) and fails at 32 KiB. At the
largest allowed body the peak is estimated below 80 KB, which was not measured.
The native tests run the `std` code path; the allocator and panic handler are
exercised only by the stand journey.

## Wire layouts (little endian)

| method | body |
|---|---|
| `vault.amm.deposit` | empty; value in `extra` (either/both pool currencies) |
| `vault.amm.add_liquidity` | desired0 u128 · desired1 u128 · min0 u128 · min1 u128 · deadline u32 (68 B) |
| `vault.amm.remove_liquidity` | shares u128 · min0 u128 · min1 u128 · deadline u32 (52 B) |
| `vault.amm.swap_exact_in` / `_out` | direction u8 · exact u128 · bound u128 · deadline u32 (37 B); bound = min out / max in |
| `vault.amm.withdraw` | currency u32 · amount u128 · recipient dapp_id[32]+account_id[32] (84 B) |
| `vault.amm.set_params` | swap_fee_bps u16 · protocol_fee_bps u16 · paused u8 (5 B) |
| `vault.amm.collect_fees` | empty; admin only |
| `vault.amm.topup` | SHELL only, funds the FUEL reserve for payouts |
| `vault.amm.status` | empty; emits an `amm.status` event, changes no ledger or reserve |

State is a fixed 4376-byte image (32 holder slots, 8 pending payouts, ring of 16
receipts), full-replaced each call and kept under the 65536-byte FullReplace limit.
A missing native callback can still retain a pending slot. There is no timeout
refund: principal may already have been delivered. The 16-entry receipt ring is
bounded duplicate detection; native callback provenance and one-time delivery
remain runtime assumptions.

TWAP stores `floor(reserve_out * 2^64 / reserve_in) * elapsed` modulo `2^128`,
including ratios whose scaled price exceeds u128. It does not saturate. Values
are lazy: liquidity changes and swaps accrue at the old price; read cumulative
values and `last_ts` from state and extrapolate off-chain between updates.
`status` emits reserves and counters, not cumulative values. An average is only
unambiguous over intervals whose true cumulative delta does not wrap `2^128`;
extreme ratios may exceed that domain even within one second.
Worked example in the tests: pool 1_000_000 / 1_000_000, fee 30 bps, 10_000 in → 9_871 out.

## SDK version compatibility

Fresh-deployment validation with SDK 0.4.0 / Core-WASM V2 and Rust 1.99.1
was executed on 2026-11-06: **58 native tests**, both contract builds,
27 rebuilt ABI IDs, and the existing local-stand journey (32 call probes)
passed. The example-local compiler and wasm feature configuration are pinned to
the matching 0.4.0 settings. See [VALIDATION-0.4.0.md](VALIDATION-0.4.0.md)
for artifact sizes/hashes, logs and scope. Earlier figures below are historical
0.3.0 evidence. No account migration, Mainnet or real-fund qualification is claimed.

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
./scripts/check.sh      # ids, oracle vectors, fmt, clippy -D warnings, tests
```

On a machine with the SDK installed (`scripts/install.sh` in the repository root),
build both contracts and run the journey on the local stand:

```sh
avm-contract-build build --manifest-path contracts/amm/Cargo.toml
avm-contract-build build --manifest-path contracts/receiver/Cargo.toml
python3 scripts/e2e.py --sdk "$AVM_REPRO_SDK"
```

## Sizes

Built with SDK release `avm-dev-2662a32e7c91-sdk-93e34e16e449`:

| artifact | AMM | receiver |
|---|---|---|
| `contract.wasm` | 54,647 B | 27,872 B |
| `contract.acab` | 2,103 B | 361 B |
| `contract.descriptor` | 66 B | 66 B |

AMM deploy body: 54,647 + 2,103 + 66 + 64 (init) + 289 (envelope) = 57,169 B of
65,536 B, so 8,367 B of headroom. As ordinary `std` crates the same code was
63,130 B of WASM with the three features removed.

## Verified on the builder and the local stand

* `avm-contract-build build` produces both artifact triples; every method and
  event id in the built ACAB equals `ids.rs`, and `ids.rs` equals the output of
  `scripts/gen_ids.py`.
* `scripts/check.sh` passes (fmt, clippy `-D warnings`, all tests).
* `scripts/e2e.py` passes on `avm-local-stand`: deploy of both contracts, bind,
  SHELL top-up, deposit, add liquidity, swap exact-in (and a rejected slippage
  bound) with the TWAP accumulators compared against an independent computation,
  withdraw to the receiver with callback settlement, a rejecting receiver that
  bounces the payout and re-credits the holder, a retried payout, remove
  liquidity, an exact-out swap (the double round-up input, `k` not decreasing, a
  bound one unit too tight refused without effects), the `status` view, and journal
  replay with identical roots. The repaired journey also makes nine consecutive
  payouts to a receiver that accepts value but deliberately returns an empty
  receipt, followed by a successful withdrawal by another holder. Each pending
  slot is released. Receiver `reject` fixture modes: 0 = echo, 1 = reject,
  2 = accept without receipt. Selected decoded accounting fields, reserves and
  payout outcomes are checked at checkpoints against expected values. The complete
  ledger is not independently compared after every call; the repeated empty-receipt
  payout loop checks pending-slot release and receiver delivery counts.
* Each of the 32 calls of that journey completes within a 1 SHELL FUEL reserve
  (1,000,000 FUEL; the per-message cap is 10,000,000). This is an upper bound:
  the stand does not report the exact `fuel_used` for successful calls.
* `publish-version` of a second AMM build into the same package succeeds. The
  version's constructor runs, so the publisher must be the pool admin named in the
  init data (a `package-owner` publisher with another admin is rejected with 3001).
  This was run by hand on an earlier pre-`no_std` revision, is not part of
  `e2e.py`, and has not been repeated on the repaired revision. It does not
  establish migration or upgrade safety for an existing funded account.
* `input.extra` carries the **post-credit** balance: a scratch contract that
  recorded `input.extra` on every call saw the balance already including the
  message's own transfer (calls with `0:1000,7:2000` then `0:500` showed 1000/2000
  and then 1500/2000), and it also lists the SHELL balance (currency 2). The
  probe is not included here. The example now relies explicitly on post-credit
  balances and rechecks after deposit ledger credit, so a deposit cannot mask a
  pre-existing shortfall. Native tests exercise deficits smaller than, equal to
  and larger than the incoming deposit in both currencies.

## Not verified

* The stand does not expose per-account ECC balances, so the vault's real balances
  are only checked indirectly through the contract's own solvency check.
* Exact FUEL per call (only the 1 SHELL bound above).
* Behaviour on a network other than the local stand. The contract is not audited;
  do not use it with real funds.

## Repair validation

See [VALIDATION.md](VALIDATION.md) for the baseline failures, repaired checks,
artifact hashes and the division between native and local-stand coverage.
These results are local evidence; upstream CI and maintainer approval are separate.

The stand journey now requires every call probe to return the expected status at
1 SHELL and executes the main calls with the same 1 SHELL reserve. Its state,
payout and replay assertions therefore also run at that reserve. A probe requiring
more FUEL fails the journey. Exact FUEL consumption remains unmeasured.

The stand journey does not exercise `vault.amm.set_params`,
`vault.amm.collect_fees`, or a nonzero protocol-fee configuration. Extreme TWAP
ratios and injected deposit-shortfall cases remain native-test coverage only.

The 1-SHELL reserve bound applies to the listed direct journey calls, including
calls expected to return a contract error. Deployment uses a separate reserve.
Outbound payouts are configured with a separate 10,000-SHELL fuel reserve;
successful settlement does not establish a 1-SHELL bound for each downstream
payout or callback message. Exact FUEL consumption remains unmeasured.
