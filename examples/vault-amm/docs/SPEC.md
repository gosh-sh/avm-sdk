# Vault AMM – specification notes

## Accounting identities
For each side `s`: `vault_balance(s) == reserve[s] + fees[s] + Σ holder.balance[s]`.
Value in flight (a withdrawal waiting for its callback) has left the vault and is
tracked in `pending`; `held(s)` is the first sum, `in_flight(s)` the pending
amounts. Conservation, checked by the fuzz test after every step:
`deposited == held + in_flight + delivered`.

## Pricing
`in_f = floor(in·(10000−fee)/10000)`; `out = floor(r_out·in_f/(r_in+in_f))`.
Exact-out: `need = ceil(r_in·out/(r_out−out))`, `in = ceil(need·10000/(10000−fee))`.
All products use a 256-bit intermediate (`math.rs`), validated against Python
big integers (`scripts/gen_vectors.py`, 2450 vectors incl. overflow/liquidity edges).

## Payout protocol
1. `withdraw` debits the ledger, allocates a pending slot (one per recipient),
   sends `receive` with the receipt and prepays `fuel_reserve` SHELL, requests callback `vault.amm.callback`.
2. Callback source must equal the pending recipient. Success: body == receipt,
   no principal returned. Failure: exactly the pending amount returned, empty
   body; the holder is re-credited. Otherwise `ERR_CALLBACK` and the slot stays.
3. Settled receipts' cause hashes go into a 16-entry ring to reject replays.

## Canonical state rules
Contiguous account slots; no empty unreferenced accounts; unique keys;
`Σ lp == lp_total`; pending holders must have accounts; receipt ring consistent.
Any non-canonical image is rejected on decode.

## Receiver fixture
`bind` (owner, once) → `receive` (only from the bound AMM, with callback request,
strictly increasing id, currency/amount equal to the attached value) → echo.
`reject` makes it bounce deliveries, to test refunds.
