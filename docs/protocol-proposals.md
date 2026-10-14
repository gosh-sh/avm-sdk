# Funding, deploy failures and sender code: research proposals

These are proposals for [#8](https://github.com/gosh-sh/avm-sdk/issues/8) and
the remaining parts of [#13](https://github.com/gosh-sh/avm-sdk/issues/13).
They are not adopted protocol rules or released capabilities. Canonical changes
need a separate documentation PR, independent review and owner approval before
implementation can merge or a binary can ship.

## Deploy failure accounting

The existing message rules require a failed internal deploy to consume the
failed lead message, preserve the sender's already accepted transition, and
return transferred value. A requested callback carries the failure status and
unused prepaid FUEL; otherwise a method-zero refund credits value without
running user callback code. Availability/preparation errors must abort the
unpublished candidate, not become a VM status.

The candidate implements that path for deploy prechecks before precharge/init:
malformed body, active destination, absent package and deploy-owner validation
failures. It reuses the ordinary refund builder and queue path. Status 63 is
`VM_DEPLOY_FAILED`; the target account/placeholder, registry, events and init
counter remain unchanged. External CLI rejection behavior remains unchanged.
The narrow host handoffs `settle_rejected_deploy` and
`materialize_rejected_deploy` describe an unexecuted rejection explicitly; they
do not fabricate a `WasmExecutionResult` or charge an init/callback fee again.
Their public host names still need canonical registration and review.

Failures after init starts expose a concrete owner-contract contradiction:

- `modules/avm-block-pipeline.md` says failed deploys leave the committed
  placeholder unchanged and refund the lead's transferred value.
- `gas-and-fees.md`, Account init, charges 10 SHELL before init from the
  placeholder plus transferred SHELL. `message-semantics.md` says already
  charged fees do not roll back.

For a 10-SHELL placeholder and a 5-SHELL transfer, preserving both while burning
10 creates 10 SHELL. With an empty placeholder, a deploy funded entirely by its
message makes the conflict unavoidable. Ordinary callback tests cannot resolve
this accounting.

Proposed resolution, requiring explicit adoption: reserve the init fee before
WASM but burn it only on successful account creation. On failed init, discard
that reservation, retain the placeholder and refund the complete transferred
value. Burn actual FUEL used from the original prepaid reserve and return its
unused portion only through a requested callback. For external deploys, the
destination's committed funding pays the ordinary external FUEL/delivery
charges even on failure; the blanket “placeholder unchanged” sentence must
state that exception. Publication fees remain tied to a committed new selection.

Exact edits proposed: replace “debited once before `init` runs” with “reserved
once before `init`; committed and burned only when creation succeeds”; qualify
the pipeline's unchanged-placeholder rule with external FUEL/delivery charges;
specify failed-init settlement separately from successful `DeployCommitPlan`.
No such fee change is enabled in this candidate.

Required acceptance cases: internal/external, empty/prefunded placeholder,
transfer-only funding, callback/no callback, contract status, trap, OOG,
malformed output, init/publication fee conservation, exact status precedence,
only-lead consumption, unaffected sibling work, persistence rollback and replay.
Missing code bytes/engine preparation failure must remain availability errors.

## DApp-confined funding without a new currency

Current SHELL is transferable account value. `dapp_id` is routing identity;
`fuel_fee_shells` is a message reserve. Neither confines funds to a DApp, and
an upgradeable application's withdrawal restriction is not protocol enforcement.

Proposed minimal protocol design:

1. A committed funding-policy record binds one funding account permanently to
   one DApp. Anyone can top it up with ordinary SHELL. The account cannot run
   WASM, emit transfers, upgrade, delete, change DApp, or withdraw its balance.
2. A payer authorization names the funding account, destination DApp/account,
   permitted methods, maximum SHELL debit, nonce and validity interval. An
   immutable policy or policy-authorized signature grants it. Depositing funds
   alone does not authorize arbitrary callers to drain them. Mutable spending
   grants cannot remove the permanent withdrawal/DApp restrictions.
3. The economics owner reserves against the authenticated funding account only
   for covered protocol fees. Transferred value stays the sender's obligation.
   Failed authorization or exhaustion rejects before WASM with no debit; there
   is no silent fallback to an unrelated payer. Local-free calls reserve no
   compute fee. Rent/init/publication coverage must be separately explicit.
4. Unused sponsored reserves return only to that funding account. A callback
   must not turn a sponsored reserve into withdrawable recipient value. Batch
   Library collector payments are excluded from this first proposal: allowing
   an unrestricted collector would defeat confinement.
5. A DApp split retains the DApp binding but requires the ordinary paid-route
   rules. Concurrent debits serialize at the funding account's existing owner
   boundary; reservations, fees and refunds commit atomically with the trace.

This requires a canonical policy/state representation, authenticated payer
binding in messages, replay and apply rules, budget limits, and a measured
contention design. Those are genuine missing contracts, not SDK accessor fixes.
No funding/state implementation is included here. Adoption must specify exact
encodings, activation and errors before production code; no vmSHELL is proposed.

The research encoding would use a versioned policy record keyed by the existing
64-byte funding address: version `u16=1`, flags `u16=0`, bound DApp `32 bytes`,
grant-authority key `32 bytes`, grant nonce `u64`, remaining period budget `u128`,
and period end `u32`, all integers little-endian. The funding account's ordinary
currency collection holds SHELL; the policy record is committed alongside its
account identity, not in mutable contract data. Signed grant preimages must
bind the network, policy key/version, nonce, destination/method, maximum fee and
expiry. A new authenticated payer section, not a guest payload convention,
carries that grant. This is a proposed encoding, not authority to modify the
active State V2/storage implementation.

Required tests include upgrade/withdrawal/delete bypasses, forged/replayed
authorizations, cross-DApp targets, split routing, exhaustion, concurrent
reservations, cancellation, refunds and callback/batch leakage, full per-currency
conservation, literal apply/replay equivalence and hot funding-account load.

## Authenticated sender code identity

`Caller::Internal` authenticates an address. `selected_code_id` in guest input
belongs to the receiver. Deriving a known deployment address can bind its
package/owner/salt/init data, but does not pin its current selected version.
Guest-written payloads or events claiming a code hash are not VM proofs.

Proposed emission provenance: an activated internal-message version commits the
sender's executing `selected_code_id` in its canonical bytes and hash. The
materializer supplies it from the verified executing owner, never a guest field.
The receiver's immutable Code Library snapshot resolves that selection's package
and code hash. A send during a successful upgrade records the old code that
executed the send; later upgrades do not rewrite queued provenance. VM-generated
callbacks/refunds have an explicit VM-origin discriminator and cannot pretend
to be contract emissions, including failed deploys with no created child.

This proves code at emission, not the sender's current code at delivery. A
separate delivery-time query would require a defined snapshot and additional
state dependencies; it must not silently replace the claim. Legacy messages
expose provenance as unavailable and fail a policy requiring it.

The proposed wire section is 36 bytes: kind `u8` (`1=contract`, `2=VM`), three
zero reserved bytes, then `selected_code_id[32]` (all zero only for VM origin).
An activated internal-message flag `1 << 4` locates it after optional read
requests and before the body. Old parsers reject that previously reserved bit.
Queued contract provenance must retain its selection metadata until delivery;
it does not require retaining executable code blobs. This retention dependency
must be part of Code Library GC rules, not assumed from a cache hit.

Adoption must ratify this discriminator/selection layout, compatibility
activation and guest-input format, charge the real added bytes under existing
caps, and register the host/guest names. Required tests cover spoofing, legacy
messages, send-plus-upgrade, intervening upgrade/delete/redeploy, VM callbacks,
missing selection availability, replay, caps and no extra sender-state lookup.
This provenance API is not implemented or promised for a release.
