#![forbid(unsafe_code)]

//! A minimal relay/witness contract used only by this example's local-stand
//! E2E script (`../scripts/e2e_journey.sh`) — never part of the vault's own
//! production surface. It exists because the vault's own `DEPOSIT`/
//! `WITHDRAW` require an *internal* caller (`src/handlers.rs`'s
//! `internal_source`), so an external test driver cannot invoke them
//! directly; this contract is deployed as the depositor and relays external
//! test-driver calls into internal messages the vault will accept, exactly
//! in the spirit of `examples/contract-a-v1`'s own generic forwarding
//! method (same "any non-reserved method id from an external caller is a
//! forward request" dispatch, same 1000-prefixed style of own-defined error
//! codes).
//!
//! It also plays the second role a withdrawal payout needs: the vault's own
//! `WITHDRAW` handler pays out to "the caller's own account" (`SPEC.md`
//! §5) via a plain internal message with `method_id = 0`, `body = []`
//! (`docs/THREAT_MODEL.md` §7) and requests a callback on its own delivery.
//! Whether that callback reports Settle (`status_code == 0`) or Restore
//! (`status_code != 0`) depends on whether THIS contract's own
//! `process_message` accepts or rejects that inbound payout message.
//!
//! That accept/reject choice is a **build-time** flag
//! (`AVM_E2E_ACTOR_REJECT_PAYOUT`, read via `option_env!`), the same
//! documented mechanism `AVM_EXAMPLE_VAULT_OWNER_PUBKEY`/
//! `AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY` already use in this repository — not
//! a deploy-time `--init-json` flag. A non-`null` `--init-json` on a
//! message-isolated contract was tried first and tested real on the local
//! stand: both a fresh account funded for the first time and a second,
//! differently-salted fresh account the same way failed identically with
//! `"existing deploy init apply failed: InitOutputInvalid"`, with no
//! documented example of a non-`null` `--init-json` anywhere in this
//! repository's own docs or `scripts/tests/*.py` to compare against. Per
//! this engagement's own rule against inventing undocumented CLI/SDK
//! behavior, that path was abandoned rather than worked around further —
//! this contract is instead built twice, once per instance, exactly like
//! the vault's own owner-pubkey rebuild already does for its E1 journey.
//!
//! **A second real stand finding, load-bearing for this contract's own ABI
//! shape.** The forward method's input type was first declared
//! `kind = "bytes"` (variable-length) in `avm-contract.toml`, matching
//! this contract's own `actor_state` state type. Every forward call then
//! failed at `avm-local-stand`'s own call-materialization step with
//! `InvalidOutboundIntent`, even with a destination/method id/value/extra
//! copied byte-for-byte from `examples/contract-a-v1`'s own OWN forwarding
//! call, which succeeds against the identical destination in the identical
//! session. Re-declaring the SAME input type `kind = "fixed_bytes"` (with
//! an explicit `len`) — matching `examples/contract-a-v1`'s own
//! `causal_a_forward_args` declaration exactly — fixed it immediately, no
//! other change. The most likely explanation (not independently confirmed
//! against the host's own source, which isn't in this checkout): a
//! `"bytes"`-typed input carries a length prefix in its own wire encoding
//! that a `"fixed_bytes"`-typed input does not, so this contract's own
//! fixed-offset body reads (`body.get(0..32)`, etc.) were silently reading
//! shifted, corrupted bytes under the `"bytes"` declaration — plausible
//! enough to explain every symptom observed, but stated as the most likely
//! explanation, not a confirmed one. Two fixed-length forward methods
//! (`actor.forward_deposit`, 80 bytes; `actor.forward_withdraw`, 101
//! bytes) replace the original single variable-length one as a result.

use avm_contract_sdk::AccountAddress;
use avm_contract_sdk::CallbackIntent;
use avm_contract_sdk::Caller;
use avm_contract_sdk::Contract;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::OutboundIntent;
use avm_contract_sdk::StateUpdate;
use avm_contract_sdk::CODE_UPGRADE_METHOD_ID;

const ERR_INVALID_CALLER: u32 = 1700;
const ERR_INVALID_FORWARD_ARGS: u32 = 1701;
const ERR_PAYOUT_REJECTED: u32 = 1702;
const ERR_UNRECOGNIZED_METHOD: u32 = 1703;

/// Set at compile time via `AVM_E2E_ACTOR_REJECT_PAYOUT=1` (any non-empty
/// value): this instance deliberately fails every inbound payout, forcing
/// the vault's own callback into Restore instead of Settle.
const REJECT_PAYOUT: bool = option_env!("AVM_E2E_ACTOR_REJECT_PAYOUT").is_some();

/// `dest_dapp_id(32) + dest_account_id(32) + outbound_method_id(8, LE) +
/// outbound_callback_method_id(8, LE)` — the common header both fixed-length
/// forward bodies share; anything after this is forwarded verbatim as the
/// outbound message's own body.
const FORWARD_HEADER_LEN: usize = 80;
/// `actor.forward_deposit`'s own fixed body length: just the header, no
/// trailing bytes (the vault's own `DEPOSIT` reads `value_main`/`extra`,
/// never the body).
const FORWARD_DEPOSIT_LEN: usize = FORWARD_HEADER_LEN;
/// `actor.forward_withdraw`'s own fixed body length: the header plus the
/// vault's own 21-byte `WITHDRAW` body (`currency_tag(1) + currency_id(4) +
/// amount(16)`, `src/handlers.rs`'s `WITHDRAW_BODY_LEN`).
const FORWARD_WITHDRAW_LEN: usize = FORWARD_HEADER_LEN + 21;
/// Fuel budget handed to the destination for a forwarded, no-callback
/// message. `1` (this contract's first draft) let the outbound intent
/// materialize but left the destination unable to finish real work (a
/// real stand test: forwarding a `DEPOSIT` to the vault with
/// `fuel_fee_shells = 1` returned `status: "ok"` yet the vault's own
/// ledger never grew — the credit silently ran out of fuel, with no
/// callback requested to report it). This value is a generous fixed
/// budget, not a measured minimum.
const FORWARD_FUEL_FEE_SHELLS: u128 = 200_000;

/// One byte: a payout counter, incremented (wrapping) on every ACCEPTED
/// inbound payout, for a decoded-state assertion distinct from the vault's
/// own state. Never incremented on a rejected payout — a failed call
/// writes no state, exactly like every other failure in this repository
/// (Invariant I6).
const STATE_LEN: usize = 1;

struct E2eActor;

impl Contract for E2eActor {
    fn init(_input: MessageInput<'_>) -> MessageOutput {
        MessageOutput::success(StateUpdate::FullReplace(vec![0; STATE_LEN]))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        if input.message.is_callback() {
            // This contract never requests a callback for itself (every
            // forward it issues uses `callback_method_id == 0` in this
            // example's own E2E script), so a genuine callback delivery
            // should never reach it.
            return MessageOutput::failure(ERR_UNRECOGNIZED_METHOD);
        }
        if input.message.method_id() == CODE_UPGRADE_METHOD_ID {
            return MessageOutput::failure(ERR_UNRECOGNIZED_METHOD);
        }

        if input.message.method_id() == 0 {
            return process_payout(&input);
        }
        process_forward(&input)
    }
}

/// The vault's own withdrawal-payout message: `method_id == 0`, sent as a
/// plain (non-callback) internal message to the depositor's own account
/// (`docs/THREAT_MODEL.md` §7 confirms `method_id == 0` carries no special
/// platform-level meaning for a non-callback message, so this contract must
/// recognize it itself, by caller type).
fn process_payout(input: &MessageInput<'_>) -> MessageOutput {
    let Caller::Internal(_) = input.caller else {
        return MessageOutput::failure(ERR_INVALID_CALLER);
    };
    if REJECT_PAYOUT {
        return MessageOutput::failure(ERR_PAYOUT_REJECTED);
    }
    let Some(&counter) = input.state.first().filter(|_| input.state.len() == STATE_LEN) else {
        return MessageOutput::failure(ERR_INVALID_CALLER);
    };
    MessageOutput::success(StateUpdate::FullReplace(vec![counter.wrapping_add(1)]))
}

/// Any method id other than `0` and `CODE_UPGRADE_METHOD_ID`, from an
/// external caller, is a forward request — the same "no dedicated id check,
/// anything else from an external caller is a forward" dispatch
/// `examples/contract-a-v1` already uses for its own one forwarding method.
/// Dispatches on the body's own exact length (`FORWARD_DEPOSIT_LEN` or
/// `FORWARD_WITHDRAW_LEN` — see `avm-contract.toml`'s two fixed-length ABI
/// methods, `actor.forward_deposit`/`actor.forward_withdraw`) rather than on
/// the method id, since both are reached through this same catch-all.
fn process_forward(input: &MessageInput<'_>) -> MessageOutput {
    let Caller::External(_) = input.caller else {
        return MessageOutput::failure(ERR_INVALID_CALLER);
    };
    let body = input.message.body();
    if body.len() != FORWARD_DEPOSIT_LEN && body.len() != FORWARD_WITHDRAW_LEN {
        return MessageOutput::failure(ERR_INVALID_FORWARD_ARGS);
    }
    let Some(destination_dapp_id) = body.get(0..32).and_then(|bytes| bytes.try_into().ok()) else {
        return MessageOutput::failure(ERR_INVALID_FORWARD_ARGS);
    };
    let Some(destination_account_id) = body.get(32..64).and_then(|bytes| bytes.try_into().ok())
    else {
        return MessageOutput::failure(ERR_INVALID_FORWARD_ARGS);
    };
    let Some(outbound_method_id) =
        body.get(64..72).and_then(|bytes| bytes.try_into().ok()).map(u64::from_le_bytes)
    else {
        return MessageOutput::failure(ERR_INVALID_FORWARD_ARGS);
    };
    let Some(callback_method_id) =
        body.get(72..80).and_then(|bytes| bytes.try_into().ok()).map(u64::from_le_bytes)
    else {
        return MessageOutput::failure(ERR_INVALID_FORWARD_ARGS);
    };
    let Some(forward_body) = body.get(FORWARD_HEADER_LEN..) else {
        return MessageOutput::failure(ERR_INVALID_FORWARD_ARGS);
    };

    let mut output = MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()));
    output.outbound_intents = vec![OutboundIntent {
        input_index: 0,
        destination: AccountAddress {
            dapp_id: destination_dapp_id,
            account_id: destination_account_id,
        },
        value_main: input.message.value_main(),
        // Real stand testing (this file's own module doc) found `0` here
        // rejected outright with `InvalidOutboundIntent` for a no-callback
        // forward — matches `examples/contract-a-v1`'s own exact pattern.
        fuel_fee_shells: if callback_method_id == 0 { FORWARD_FUEL_FEE_SHELLS } else { 0 },
        extra: input.message.extra().into_iter().collect(),
        method_id: outbound_method_id,
        body: forward_body.to_vec(),
    }];
    if callback_method_id != 0 {
        output.callback_intents =
            vec![CallbackIntent { outbound_index: 0, handler_method_id: callback_method_id }];
    }
    output
}

avm_contract_sdk::export_message_contract!(E2eActor, arena_bytes = 262_144);
