# E2E test actor

A minimal relay/witness contract used **only** by
[`../scripts/e2e_journey.sh`](../scripts/e2e_journey.sh) — it is test
infrastructure, not part of the vault's own production surface, and is
never referenced by `src/`. It exists for one reason: the vault's own
`DEPOSIT`/`WITHDRAW` require an *internal* caller
(`../src/handlers.rs`'s `internal_source`), so an external test driver
cannot invoke them directly. This contract is deployed as the depositor
and relays external test-driver calls into the internal messages the
vault will accept — the same shape `examples/contract-a-v1` already uses
for its own one generic forwarding method, just with two fixed-length
forward variants instead of one (see below).

It also plays the second role a withdrawal payout needs: the vault's own
`WITHDRAW` handler pays out to "the caller's own account"
(`../SPEC.md` §5) via a plain internal message with `method_id = 0`,
`body = []`. Whether the vault's own resulting callback reports Settle or
Restore depends on whether this contract's own `process_message` accepts
or rejects that inbound payout — a **build-time** flag
(`AVM_E2E_ACTOR_REJECT_PAYOUT`, read via `option_env!`, the same
documented mechanism `AVM_EXAMPLE_VAULT_OWNER_PUBKEY` already uses in this
repository), not a deploy-time flag. The E2E script builds this same
source twice — once per flag value — and deploys both instances.

## Two real stand findings baked into this contract's own shape

Both were found by testing directly on the local stand, not inferred from
source reading, and are recorded here (and in `src/lib.rs`'s own module
doc) so a future maintainer does not have to rediscover them:

1. **`--init-json` with real content does not work here.** A first draft
   set the accept/reject flag via a 1-byte `--init-json` body instead of a
   build-time flag. Every attempt (two independently funded, differently
   salted fresh accounts) failed identically with `"existing deploy init
   apply failed: InitOutputInvalid"`. No example anywhere in this
   repository's own docs or `scripts/tests/*.py` uses a non-`null`
   `--init-json` to compare against. Abandoned per this project's own rule
   against inventing undocumented CLI/SDK behavior — not worked around
   further.
2. **The forward method's own ABI input type must be `kind = "fixed_bytes"`
   with an explicit `len`, not `kind = "bytes"` (variable-length).** With
   `"bytes"`, every forward call failed at `avm-local-stand`'s own call
   materialization step with `InvalidOutboundIntent` — even with a
   destination/method id/value/extra copied byte-for-byte from
   `examples/contract-a-v1`'s own forwarding call, which succeeds against
   the identical destination in the identical session. Switching only the
   ABI declaration (no Rust change) fixed it immediately. Most likely
   explanation, not independently confirmed against the host's own source
   (not in this checkout): a `"bytes"`-typed input carries a length prefix
   in its own wire encoding that this contract's fixed-offset body reads
   did not account for.

As a consequence, this contract exposes **two** fixed-length forward
methods instead of one generic variable-length one:

| Method | Body length | Shape |
|---|---|---|
| `actor.forward_deposit` | 80 bytes | `dest_dapp_id(32) + dest_account_id(32) + outbound_method_id(8, LE) + outbound_callback_method_id(8, LE)` |
| `actor.forward_withdraw` | 101 bytes | the same 80-byte header, plus the vault's own 21-byte `WITHDRAW` body (`currency_tag(1) + currency_id(4) + amount(16)`) |

Both forward `value_main`/`extra` straight through from the external
call that invoked them; `fuel_fee_shells` on the resulting outbound
message is a generous fixed `200,000` (not a measured minimum) when no
callback is requested — `1` was tried first and materializes fine but
silently starves the destination's own real work (a real stand test:
forwarding a `DEPOSIT` to the vault with `fuel_fee_shells = 1` returned
`status: "ok"` while the vault's own ledger never grew, since no callback
was requested to report the destination's own failure back).

No keys, secrets, or real identities are used anywhere in this contract
or the script that drives it.
