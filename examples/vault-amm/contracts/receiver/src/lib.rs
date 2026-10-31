#![forbid(unsafe_code)]

//! Companion fixture: a payout destination bound to one Vault AMM. It accepts
//! a withdrawal, records the receipt and echoes it in its callback, or it
//! deliberately rejects the delivery so the AMM's bounce path can run.

use avm_contract_sdk::{
    AccountAddress, Caller, CanonicalMessage, Contract, MessageInput, MessageOutput, StateUpdate,
};
use avm_vault_amm::ids;

pub const STATE_BYTES: usize = 144;
pub const ERR_AUTH: u32 = 2200;
pub const ERR_ARGS: u32 = 2201;
pub const ERR_REJECT: u32 = 2202;

const OFF_BOUND: usize = 32;
const OFF_REJECT: usize = 96;
const OFF_COUNT: usize = 104;
const OFF_LAST: usize = 112;

pub struct Receiver;

impl Contract for Receiver {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let Caller::External(owner) = input.caller else {
            return MessageOutput::failure(ERR_AUTH);
        };
        if !input.state.is_empty()
            || !input.deploy().is_ok_and(|d| d.init_data().is_empty())
            || !input.message.extra().is_empty()
        {
            return MessageOutput::failure(ERR_ARGS);
        }
        let mut state = vec![0; STATE_BYTES];
        state[..32].copy_from_slice(&owner);
        MessageOutput::success(StateUpdate::FullReplace(state))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}

fn arr<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], u32> {
    bytes.get(offset..offset + N).and_then(|s| s.try_into().ok()).ok_or(ERR_ARGS)
}

fn address(bytes: &[u8], offset: usize) -> Result<AccountAddress, u32> {
    Ok(AccountAddress { dapp_id: arr(bytes, offset)?, account_id: arr(bytes, offset + 32)? })
}

fn process(input: MessageInput<'_>) -> Result<MessageOutput, u32> {
    let bytes = input.state;
    if bytes.len() != STATE_BYTES || bytes[OFF_REJECT] > 1 || bytes[97..OFF_COUNT] != [0; 7] {
        return Err(ERR_ARGS);
    }
    let owner: [u8; 32] = arr(bytes, 0)?;
    let mut state = bytes.to_vec();
    let method = input.message.method_id();
    if method == ids::RECEIVER_BIND || method == ids::RECEIVER_REJECT {
        if input.caller != Caller::External(owner) {
            return Err(ERR_AUTH);
        }
        if !input.message.extra().is_empty() {
            return Err(ERR_ARGS);
        }
        let body = input.message.body();
        if method == ids::RECEIVER_BIND {
            let amm = address(body, 0)?;
            if body.len() != 64
                || bytes[OFF_BOUND..OFF_REJECT] != [0; 64]
                || amm.dapp_id == [0; 32]
                || amm.account_id == [0; 32]
                || amm == input.destination
            {
                return Err(ERR_ARGS);
            }
            state[OFF_BOUND..OFF_REJECT].copy_from_slice(body);
        } else {
            if body.len() != 1 || body[0] > 1 {
                return Err(ERR_ARGS);
            }
            state[OFF_REJECT] = body[0];
        }
        return Ok(MessageOutput::success(StateUpdate::FullReplace(state)));
    }
    let (Caller::Internal(source), CanonicalMessage::Internal(message)) =
        (input.caller, input.message)
    else {
        return Err(ERR_AUTH);
    };
    if source != address(bytes, OFF_BOUND)?
        || method != ids::RECEIVER_RECEIVE
        || message.is_callback()
        || !message.has_callback_request()
    {
        return Err(ERR_AUTH);
    }
    if bytes[OFF_REJECT] == 1 {
        return Err(ERR_REJECT);
    }
    let body = message.body;
    let [entry] = message.extra.iter().collect::<Vec<_>>()[..] else {
        return Err(ERR_ARGS);
    };
    if body.len() != 32 || &body[..4] != b"VAM1" {
        return Err(ERR_ARGS);
    }
    let currency = u32::from_le_bytes(arr(body, 4)?);
    let id = u64::from_le_bytes(arr(body, 8)?);
    let amount = u128::from_le_bytes(arr(body, 16)?);
    let count = u64::from_le_bytes(arr(bytes, OFF_COUNT)?);
    let last_id = u64::from_le_bytes(arr(bytes, OFF_LAST + 8)?);
    if amount == 0
        || currency != entry.currency_id
        || amount != entry.amount
        || (count != 0 && id <= last_id)
    {
        return Err(ERR_ARGS);
    }
    state[OFF_COUNT..OFF_COUNT + 8].copy_from_slice(&count.saturating_add(1).to_le_bytes());
    state[OFF_LAST..OFF_LAST + 32].copy_from_slice(body);
    let mut output = MessageOutput::success(StateUpdate::FullReplace(state));
    output.callback_result_body = body.to_vec();
    Ok(output)
}

/// Number of payouts this receiver has accepted.
pub fn received_count(state: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(arr(state, OFF_COUNT).ok()?))
}

/// The most recent accepted payout receipt.
pub fn last_receipt(state: &[u8]) -> Option<[u8; 32]> {
    arr(state, OFF_LAST).ok()
}

#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_message_contract!(Receiver, arena_bytes = 262_144);
