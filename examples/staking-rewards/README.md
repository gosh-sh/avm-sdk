# Staking Rewards Distributor example

A single-pool staking contract: stakers deposit currency `S`, accrue a
share of a separately-funded reward currency `R` proportional to their
stake, and withdraw principal and/or accrued reward via `UNSTAKE`/`CLAIM`.
Accumulator-per-share pattern (`acc_reward_per_share`, scale `2^44`).
Local synthetic-funds example only; no production deployment or audit
claim.

## Init parameters

Chosen at deploy time via 40-byte `init_data` (`stake_currency_id:u32 |
reward_currency_id:u32 | min_stake:u128 | fuel_reserve:u128`), not fixed
constants: the two currency ids must differ from each other and from
SHELL (`currency_id = 2`); `min_stake` and `fuel_reserve` are each
`1..=2^60`. `fuel_reserve` is the exact SHELL the caller attaches to
every `UNSTAKE`/`CLAIM`; the pool prepays it onward. Whether any
unspent part returns to the pool is not established — assume it is
fully consumed (`docs/SPEC.md`'s "Fee model").

## Methods and attachments

- `STAKE`: exactly `S` attached, matching the body's own `amount`.
- `UNSTAKE`/`CLAIM`: exactly `fuel_reserve` of SHELL, nothing else; body
  supplies the caller's own payout `AccountAddress` fresh each call.
- `FUND_REWARDS`: exactly `R` attached, matching its own `amount`;
  permissionless.

Any missing/wrong-currency/wrong-amount/extra-currency combo is rejected.

## Payout destinations

Must be a deployed contract — a funded-but-undeployed address is
rejected the moment anything pays into it. For `S`/`R` (never SHELL),
the destination's guest code must also accept an internal, non-callback
`method_id: 0` bare transfer; an ordinary unmodified contract (this
one's own, or self-payment) does not, and cleanly failure-restores.
SHELL needs no guest-side acceptance. `contracts/test-receiver/` is a
minimal example that accepts `S`/`R`, used only by `scripts/e2e.py` —
never tested against a third party.

## Operator note

Rent accrues on this contract's own state bytes
(`docs/runtime-limits.md:70-98`); the third unpaid attempt freezes the
account until due rent is paid in full — funds locked, not lost; paying
it unfreezes. No internal rent-payment path exists. The pool's own
SHELL funds rent and every external call's own declared fee (assume
`fuel_reserve` itself is fully consumed, not refunded). Top-up, local
stand only: `avm-local-stand fund-user --address <addr> --shells <n>`
(a dev tool), or a bare SHELL transfer from another contract — neither
confirmed on a real network. Any keyholder can send calls this
contract rejects and the pool is still charged, up to nearly its whole
balance in one call (measured: funding `10,000`, one rejected call
declaring fee `9,995` left only `3`) — uncontrollable by this
contract; the effect is the same rent-unpaid freeze above, not loss.

## Run checks

Rust 1.99.1 with rustfmt, Clippy and `wasm32-unknown-unknown`, plus Python 3.

```bash
cargo +1.99.1 test --manifest-path examples/staking-rewards/Cargo.toml --workspace --locked --offline
python3 examples/staking-rewards/scripts/check.py \
  --sdk /absolute/path/to/verified/avm-sdk-linux-x86_64 \
  --out /absolute/path/to/staking-rewards-check
```

For explicit tools, replace `--sdk` with `--builder`, `--stand` and
`--profile` paths, and a fresh writable `--out`. `check.py` builds both
contracts, runs fmt/Clippy/all workspace tests, canonical method ID
verification, and a real local-stand journey, every outcome decoded.

## Contract boundaries

See [the specification](docs/SPEC.md) for the byte layout, equations,
invariants (1-17), and capacity/fuel tables, and [the threat
model](docs/THREAT_MODEL.md) for accepted limitations. No admin, pause,
or upgrade; a reward shortfall clamps `CLAIM`/`UNSTAKE`'s reward leg,
but `UNSTAKE`'s principal leg never blocks on an empty pool.
