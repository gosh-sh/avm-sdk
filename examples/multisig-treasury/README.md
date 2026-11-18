# Multisig Treasury

An m-of-n treasury for multi-currency ECC funds, with a **timelock**, per-currency
**spending policies**, a **circuit breaker** and **acknowledged payouts**. One
contract, no dependencies beyond the SDK, no arithmetic beyond additions.

| | |
|---|---|
| `contracts/treasury` | the treasury (`avm-multisig-treasury`) and `rt.rs`, the guest runtime shared with the payee |
| `contracts/payee` | fixture that receives a payout, echoes the receipt, or rejects it (exercises the bounce path) |
| `tests` | 42 behaviour tests, 5 treasury↔payee integration tests, randomized invariant tests |
| `scripts` | `gen_abi.py` (manifests + `ids.rs` from one description), `check.sh`, `e2e.py` (journey on `avm-local-stand`) |

## How it works

Up to **8 owners** (external keys) and a `threshold`. Nothing moves, and no
setting changes, except through a *proposal*:

1. An owner **proposes** (and approves their own proposal).
2. Others **approve**; any owner can **revoke** their approval. Once approvals reach
   the threshold the proposal is **queued** with `eta = now + delay`.
3. After `eta`, and before `eta + grace`, any owner **executes** it.
   Proposals expire (`ttl` while collecting approvals, `grace` once queued).

| kind | effect |
|---|---|
| `TRANSFER` | pay `amount` of a currency to an address through an acknowledged payout |
| `ADD_OWNER` / `REMOVE_OWNER` | change the owner set (slots never move; removal strips the owner's approvals and reopens proposals that depended on them) |
| `SET_THRESHOLD` | demotes queued proposals that no longer have enough approvals, promotes open ones that now do |
| `SET_DELAY` | timelock for later proposals (≤ 30 days) |
| `SET_POLICY` | per-currency **fast limit** (transfers up to it skip the delay) and **window limit** (max outflow per `window_secs`); `window_secs = 0` removes the policy |
| `UNFREEZE` | lifts a freeze; no delay, still needs the threshold |

Governance proposals other than `UNFREEZE` never get the fast lane. Every proposal is re-validated just
before it executes (the owner set, threshold and balances may have changed).

**Freeze.** Any single owner can freeze the treasury. While frozen nothing can be
proposed, approved or executed except an `UNFREEZE`; deposits, revokes, cancels and
payout callbacks remain permitted. Settlement still requires a valid callback and
available accounting capacity; there is no recovery method for missing or invalid callbacks.

**Payouts.** A transfer debits the ledger first and sends the funds with a callback
request. `TRS1 | currency u32 | proposal id u64 | amount u128` is the receipt the
payee must echo. A bounce returns the funds and puts the proposal back in the queue. It restores
spending allowance only if the spending window has not rolled over; one payout per recipient may be in flight (at most 4
overall), so the callback's source address identifies the payout.

**Safety net.** Every call decodes and validates the structural state invariants.
Ordinary calls additionally check physical balances against ledger holdings
(`ERR_INSOLVENT` otherwise); callbacks skip that solvency check. A rejected call
returns no effects. This is not a guarantee of complete insolvency detection.

**Balance capacity.** Eight slots cover the distinct currencies held or pending,
including one permanently reserved for SHELL: at most seven non-SHELL currencies.
An emptied balance does not free its reservation while a payout can return that
currency. New deposits cannot consume refund or SHELL reservations. The wire layout
is unchanged, but previously persisted states exceeding this reservation limit are
rejected; no existing-account migration or recovery is provided.

## Wire layouts (little endian)

| call | body |
|---|---|
| constructor | `threshold u8 · owners u8 · delay u32 · grace u32 · ttl u32 · fuel u128 · owner keys[32 each]`, passed as raw init bytes (the manifest declares no init type, like `bounty-escrow`: the stand length-prefixes a declared `bytes` type) |
| `treasury.deposit` | empty; value in `extra`. Anyone, owners or contracts. SHELL funds the FUEL for payouts |
| `treasury.propose` | `kind u8 · currency u32 · aux u32 · amount u128 · target[64]` (89 B) |
| `treasury.approve/revoke/cancel/execute` | `proposal id u64` |
| `treasury.freeze` | empty |

`target` is a recipient address (`TRANSFER`), an owner key in its first 32 bytes
(owner changes) or the window limit as `u128` in its first 16 bytes (`SET_POLICY`,
with `amount` = fast limit and `aux` = window seconds). Unused fields must be zero.

State is a fixed 3264-byte image, full-replaced on each call. Errors 4000–4017.

## SDK version compatibility

Fresh-deployment validation with SDK 0.4.0 / Core-WASM V2 and Rust 1.99.1
was executed on 2026-11-06: **49 native tests**, both contract builds,
28 rebuilt ABI IDs, and the existing local-stand journey (88 call probes)
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
./scripts/check.sh      # id rule, manifests, fmt, clippy (native and wasm32) -D warnings, tests
```

On Linux with the SDK commands installed, build both contracts and run the journey:

```sh
avm-contract-build build --manifest-path contracts/treasury/Cargo.toml
avm-contract-build build --manifest-path contracts/payee/Cargo.toml
python3 scripts/e2e.py --sdk "$AVM_REPRO_SDK"
```

## Sizes

Built with `avm-contract-build` (Rust 1.93.0):

| | WASM | ACAB | descriptor |
|---|---|---|---|
| treasury | 56,658 B | 2,427 B | 66 B |
| payee | 27,867 B | 355 B | 66 B |

A treasury deploy body is WASM + ACAB + descriptor + init data + the envelope. With
a 3-owner init (126 B) and a 289-byte envelope (the docs state 288) that is
**59,566 B**, 5,970 B under `max_body_size = 65536`.

The same contracts built as ordinary `std` crates were much larger (treasury WASM
66,234 B, a 69,165 B body, too big to deploy; the stand rejects a 66,701 B body with
`BodyTooLarge`). Most of the difference is the allocator and panic machinery that
`std` links in, not the contract logic.

## `no_std` guest runtime

On `wasm32` both contracts are `no_std`. `contracts/treasury/src/rt.rs`, which the
payee includes by path, supplies what `std` would have:

- a bump `#[global_allocator]` over a static 128 KiB heap, recycled whenever no
  allocation is live (so it is safe even if an instance serves several messages);
  an allocation failure traps;
- a `#[panic_handler]` that traps (`unreachable`).

This is the only `unsafe` code in the example. The contracts keep
`forbid(unsafe_code)` on native builds and use `deny(unsafe_code)` on `wasm32`, with
a single `allow` on the `rt` module. The SDK itself is already `no_std` and its
macro does not install an allocator, so there is no conflict.

The heap holds a copy of the whole init input, which contains the deploy body, so
it is sized by `max_body_size`, not by the steady state: the original pre-fix journey passed with a
64 KiB heap (deploy body 59 KB) and failed at 32 KiB. Those heap-size variants
were not rerun for the capacity fix; its journey uses the configured 128 KiB heap. At the largest allowed body,
65,536 B, the peak is estimated below 80 KB, which was not measured.

Tried and not kept (measured on an earlier, reduced build of the treasury): `opt-level`
`z`, 2 and 3 (all larger for the treasury),
`strip = true` and `--gc-sections` (no change, the builder already strips and the
linker already collects), and replacing indexing with `get()` (about 300 B on the
payee, and the `core::fmt` block that remains is retained by panic sites inside the
SDK). Nightly `build-std` and `panic_immediate_abort` were not used.

## Verified on `avm-local-stand`

`scripts/e2e.py` asserts selected decoded fields and expected outcomes at
checkpoints on the stand. It does not compare the complete state after every step
against an independent model.

- deploy a 2-of-3 treasury (the three stand identities) and a payee; bind them;
- deposit C7, C9 and SHELL;
- transfer: propose, approve (queued with `eta = now + delay`), execute refused
  before `eta` (`4007`), execute after it, callback delivered in the same step,
  receipt recorded by the payee;
- revoke an approval (the queued proposal reopens, a second revoke is refused with
  `4005`, a new approval requeues it with a restarted clock), then cancel;
- a rejecting payee: the payout bounces, funds return and the proposal is queued
  again; after the payee accepts, the same proposal pays out;
- `SET_DELAY` changes the eta of later proposals; `SET_THRESHOLD` demotes a queued
  proposal that no longer has enough approvals, and lowering it works under the
  higher threshold;
- `SET_POLICY`: a transfer within the fast limit has `eta = now` and executes at
  once; a larger one waits; a transfer past the window limit is refused with `4008`
  and stays queued; a bounced payout gives its allowance back; after the window
  ends a new one opens and the refused transfer goes through;
- add an owner, then remove it; freeze (propose and execute refused with `4003`),
  unfreeze without delay;
- all 17 event names are found in emitted bytes, and the stand journal replays to
  the same roots. The journey does not decode and validate every event field.

**FUEL.** All 88 calls of the journey fit in the smallest reserve the stand accepts,
1 SHELL (1,000,000 FUEL); a reserve of 0 is refused (`RouteRequiresFee`). The stand
does not report exact FUEL per call, so no finer figure is claimed. Fees and storage
rent are charged to the contract's own SHELL, so the script tops both accounts up.

**`input.extra`.** It is the account balance *after* the message's own value is
credited: with 1000 C7 already held and 777 attached, the contract saw 1777. The
solvency check is written so it holds either way.

## Honest status

Also verified natively: all tests, clippy, and that the id rule
(`u64_le(blake3(name)[0..8])`) reproduces the official ids of `bounty-escrow` and
`fungible-token`. The ids in the real ACAB match `ids.rs` (8 treasury methods, 3
payee methods, 17 events). The tests drive canonical ACMI frames built from the
SDK's documented layout. The native tests run the `std` code path; the bump
allocator and the trapping panic handler are exercised only by the stand journey.

Covered only by the native tests, not by the stand journey: proposal expiry, the
8-owner cap, table limits, callbacks while frozen, non-canonical state rejection.

Unaudited, not for real funds.

Known limits: the solvency check cannot see a shortfall smaller than the value
arriving in the same message; a bounce restores the spending allowance only if the
window has not rolled over meanwhile; a single owner can freeze (that is the
design), so the owner set must trust each member not to grief.

The stand journey now requires every call probe to return the expected status at
1 SHELL and executes the main calls with the same 1 SHELL reserve. Its state,
payout and replay assertions therefore also run at that reserve. A probe requiring
more FUEL fails the journey. Exact FUEL consumption remains unmeasured.

The 1-SHELL reserve bound applies to the listed direct journey calls, including
calls expected to return a contract error. Deployment uses a separate reserve.
Outbound payouts are configured with a separate 10,000-SHELL fuel reserve;
successful settlement does not establish a 1-SHELL bound for each downstream
payout or callback message. Exact FUEL consumption remains unmeasured.
