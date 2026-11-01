# Vault AMM – specification notes

## Accounting identities
For each side `s`: `vault_balance(s) >= reserve[s] + fees[s] + Σ holder.balance[s]`.
Surplus ECC is not credited to any holder and has no sweep method.
Value in flight (a withdrawal waiting for its callback) has left the vault and is
tracked in `pending`; `held(s)` is the first sum, `in_flight(s)` the pending
amounts. Conservation, checked by the fuzz test after every step:
`deposited == held + in_flight + delivered`.

## Pricing
`in_f = floor(in·(10000−fee)/10000)`; `out = floor(r_out·in_f/(r_in+in_f))`.
`math::swap_in` (exact-out: `need = ceil(r_in·out/(r_out−out))`, `in = ceil(need·10000/(10000−fee))`)
is exposed by `vault.amm.swap_exact_out`.
All products use a 256-bit intermediate (`math.rs`), validated against Python
big integers (`scripts/gen_vectors.py`, 2450 vectors incl. overflow/liquidity edges).

## Payout protocol
1. `withdraw` debits the ledger, allocates a pending slot (one per recipient),
   sends `receive` with the receipt and prepays `fuel_reserve` SHELL, requests callback `vault.amm.callback`.
2. Callback source must equal the pending recipient, with the native callback
   flag, handler, cause address and nonzero unseen hash checked. Native success:
   no principal returned; result body is diagnostic and cannot retain the slot.
   Failure: exactly the pending amount returned, empty body; the holder is
   re-credited. Invalid provenance/value is refused and the slot stays.
   The SDK output API lets receivers choose result bytes, not native callback
   status, source or returned currencies. These are runtime trust boundaries.
3. Settled receipts' cause hashes go into a 16-entry ring to reject replays.

## Canonical state rules
Contiguous account slots; no empty unreferenced accounts; unique keys;
`Σ holder.lp + locked_liquidity == lp_total`; pending holders must have accounts; receipt ring consistent.
Any non-canonical image is rejected on decode.

## Receiver fixture
`bind` (owner, once) → `receive` (only from the bound AMM, with callback request,
strictly increasing id, currency/amount equal to the attached value) → echo.
`reject` accepts fixture mode 0 (normal echo), 1 (bounce delivery), or 2
(accept principal and return an empty receipt). Mode 2 checks payout liveness.

## Balance semantics and TWAP
External calls use post-credit `input.extra`. Deposit checks the old and new
ledger against that balance. Native callbacks check returned principal rather
than global solvency. TWAP updates are lazy at reserve-changing operations:
`cumulative += floor(reserve_out * 2^64 / reserve_in) * elapsed (mod 2^128)`.
Consumers must bound observation intervals/ratios to avoid ambiguous full wraps;
these accumulators are not an independently secured price oracle.
