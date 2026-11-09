# Multisig Treasury

An m-of-n treasury for multi-currency ECC funds, with a **timelock**, per-currency
**fast-lane policies**, a **circuit breaker** and **acknowledged payouts**. One
contract, no dependencies beyond the SDK, no arithmetic beyond additions.

| | |
|---|---|
| `contracts/treasury` | the treasury (`avm-multisig-treasury`) |
| `contracts/payee` | fixture that receives a payout, echoes the receipt, or rejects it (exercises the bounce path) |
| `tests` | 31 behaviour tests, 5 treasury↔payee integration tests, randomized invariant tests |
| `scripts` | `gen_abi.py` (manifests + `ids.rs` from one description), `check.sh`, `e2e.py` (journey on `avm-local-stand`) |

## How it works

Up to **8 owners** (external keys) and a `threshold`. Nothing moves, and nothing
about the governance changes, except through a *proposal*:

1. An owner **proposes** (and approves their own proposal).
2. Others **approve**. Once approvals reach the threshold the proposal is
   **queued** with `eta = now + delay`.
3. After `eta`, and before `eta + grace`, any owner **executes** it.
   Proposals expire (`ttl` while collecting approvals, `grace` once queued).

| kind | effect |
|---|---|
| `TRANSFER` | pay `amount` of a currency to an address through an acknowledged payout |
| `ADD_OWNER` / `REMOVE_OWNER` | change the owner set (slots never move; removal strips the owner's approvals and reopens proposals that depended on them) |
| `SET_POLICY` | per-currency **fast limit**: transfers up to it skip the delay. `aux = 1` sets it, `aux = 0` removes it |
| `UNFREEZE` | lifts a freeze; no delay, still needs the threshold |

The threshold and the delay are fixed at deploy time. Governance proposals never
get the fast lane. Every proposal is re-validated just before it executes (the
owner set and balances may have changed).

**Freeze.** Any single owner can freeze the treasury. While frozen nothing can be
proposed, approved or executed except an `UNFREEZE`; deposits, cancels and payout
callbacks keep working, so a freeze can never trap value in flight.

**Payouts.** A transfer debits the ledger first and sends the funds with a callback
request. `TRS1 | currency u32 | proposal id u64 | amount u128` is the receipt the
payee must echo. A bounce returns the funds and puts the proposal back in the
queue; one payout per recipient may be in flight (at most 4 overall), so the
callback's source address identifies the payout.

**Safety net.** Every call re-validates the whole canonical state and checks that
the contract physically holds at least what it believes it owns
(`ERR_INSOLVENT` otherwise). A rejected call returns no effects.

## Wire layouts (little endian)

| call | body |
|---|---|
| constructor | `threshold u8 · owners u8 · delay u32 · grace u32 · ttl u32 · fuel u128 · owner keys[32 each]`, passed as raw init bytes (the manifest declares no init type, like `bounty-escrow`) |
| `treasury.deposit` | empty; value in `extra`. Anyone, owners or contracts. SHELL funds the FUEL for payouts |
| `treasury.propose` | `kind u8 · currency u32 · aux u32 · amount u128 · target[64]` (89 B) |
| `treasury.approve/cancel/execute` | `proposal id u64` |
| `treasury.freeze` | empty |

`target` is a recipient address (`TRANSFER`) or an owner key in its first 32 bytes
(owner changes); for every other kind it is zero. `SET_POLICY` uses `amount` as the
fast limit. Unused fields must be zero.

State is a fixed 3136-byte image, full-replaced on each call. Errors 4000–4017;
4008 is retired and not in the ABI.

## Verify

```sh
pip install blake3
./scripts/check.sh      # id rule, manifests, fmt, clippy -D warnings, tests
```

On Linux with the SDK commands installed, build both contracts and run the journey:

```sh
avm-contract-build build --manifest-path contracts/treasury/Cargo.toml
avm-contract-build build --manifest-path contracts/payee/Cargo.toml
python3 scripts/e2e.py --sdk ~/.local/share/avm-sdk/current
```

## Sizes

Built with `avm-contract-build` (Rust 1.93.0):

| | WASM | ACAB | descriptor |
|---|---|---|---|
| treasury | 63,211 B | 1,756 B | 66 B |
| payee | 39,457 B | 355 B | 66 B |

A treasury deploy body is WASM + ACAB + descriptor + init data + the envelope. With
a 3-owner init (126 B) and a 289-byte envelope that is **65,448 B**, 88 B under
`max_body_size = 65536`. The stand rejects a larger body with `BodyTooLarge`; an
intermediate build with a 66,701 B body was rejected, and this one deploys. The
margin is thin, so any new code in the treasury needs a size check.

## Verified on `avm-local-stand`

`scripts/e2e.py` passes against the stand. After every step it decodes the
treasury state and compares it with what the script computed on its own.

- deploy a 2-of-3 treasury (the three stand identities) and a payee; bind them;
- deposit C7, C9 and SHELL;
- transfer: propose, approve (queued with `eta = now + delay`), execute refused
  before `eta` (`4007`), execute after it, callback delivered in the same step,
  receipt recorded by the payee;
- a rejecting payee: the payout bounces, funds return and the proposal is queued
  again; after the payee accepts, the same proposal pays out;
- `SET_POLICY` fast lane: a transfer within the limit has `eta = now` and executes
  at once; one unit above it still waits for the delay and is then cancelled;
- add an owner, then remove it; freeze (propose and execute refused with `4003`),
  unfreeze without delay;
- all 14 events appear on the stand, and the stand journal replays to the same roots.

**FUEL.** Every call fits in the smallest reserve the stand accepts, 1 SHELL
(1,000,000 FUEL); a reserve of 0 is refused (`RouteRequiresFee`). The stand does not
report exact FUEL per call, so no finer figure is claimed.

**`input.extra`.** It is the account balance *after* the message's own value is
credited: with 1000 C7 already held and 777 attached, the contract saw 1777. The
solvency check is written so it holds either way.

## Honest status

Also verified natively: all tests, clippy, and that the id rule
(`u64_le(blake3(name)[0..8])`) reproduces the official ids of `bounty-escrow` and
`fungible-token`. The ids in the real ACAB match `ids.rs` (7 treasury methods, 3
payee methods, 14 events). The tests drive canonical ACMI frames built from the
SDK's documented layout.

Covered only by the native tests, not by the stand journey: proposal expiry, the
8-owner cap, table limits, callbacks while frozen, non-canonical state rejection.

Unaudited, not for real funds.

**Cut to fit the deploy-body limit.** An earlier design also had `revoke`, threshold
and delay proposals, and a per-window spending cap. They were removed because the
treasury did not fit `max_body_size`. Compiler profile changes (`opt-level` z, 1, 2,
3) and several refactors did not help; the `core::fmt` and panic code that remains
comes from the SDK and `std` and is shared with the payee.

Known limits: the solvency check cannot see a shortfall smaller than the value
arriving in the same message; there is no spending cap, so a threshold of owners can
move any amount after the delay; a single owner can freeze (that is the design), so
the owner set must trust each member not to grief.
