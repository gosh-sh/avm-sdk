#![forbid(unsafe_code)]

use avm_bounty_escrow::{array, ids, principal, read_address, read_u128, write_address};
use avm_contract_sdk::{
    AccountAddress, Caller, CanonicalMessage, Contract, MessageInput, MessageOutput, StateUpdate,
};

pub const STATE_BYTES: usize = 136;
pub const ERR_AUTH: u32 = 2100;
pub const ERR_ARGS: u32 = 2101;
pub const ERR_REJECT: u32 = 2102;

/// Companion fixture: a bound destination that acknowledges a receipt or
/// deliberately rejects it, allowing the actual AVM failure/refund path to run.
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

fn process(input: MessageInput<'_>) -> Result<MessageOutput, u32> {
    let b = input.state;
    if b.len() != STATE_BYTES || b[129..] != [0; 7] || b[128] > 1 {
        return Err(ERR_ARGS);
    }
    let owner = array::<32>(b, 0).map_err(|_| ERR_ARGS)?;
    let mut state = b.to_vec();
    if input.message.method_id() == ids::BIND || input.message.method_id() == ids::REJECT {
        if input.caller != Caller::External(owner) {
            return Err(ERR_AUTH);
        }
        if !input.message.extra().is_empty() {
            return Err(ERR_ARGS);
        }
        if input.message.method_id() == ids::BIND {
            if input.message.body().len() != 64 || b[32..96] != [0; 64] {
                return Err(ERR_ARGS);
            }
            let source = read_address(input.message.body(), 0).map_err(|_| ERR_ARGS)?;
            if source.account_id == [0; 32]
                || source.dapp_id == [0; 32]
                || source == input.destination
            {
                return Err(ERR_ARGS);
            }
            write_address(&mut state, 32, source);
        } else {
            if input.message.body().len() != 1 || input.message.body()[0] > 1 {
                return Err(ERR_ARGS);
            }
            state[128] = input.message.body()[0];
        }
        return Ok(MessageOutput::success(StateUpdate::FullReplace(state)));
    }
    let (Caller::Internal(source), CanonicalMessage::Internal(message)) =
        (input.caller, input.message)
    else {
        return Err(ERR_AUTH);
    };
    let bound = read_address(b, 32).map_err(|_| ERR_ARGS)?;
    if source != bound
        || message.method_id != ids::RECEIVE
        || message.is_callback()
        || !message.has_callback_request()
    {
        return Err(ERR_AUTH);
    }
    if b[128] == 1 {
        return Err(ERR_REJECT);
    }
    let body = message.body;
    if body.len() != 32
        || &body[..4] != b"BEP1"
        || body[6..8] != [0; 2]
        || !matches!(body[4], 1 | 2)
        || body[4] == 1 && body[5] >= 8
        || body[4] == 2 && body[5] != 255
        || message.extra.len() != 1
    {
        return Err(ERR_ARGS);
    }
    let attempt = u64::from_le_bytes(array(body, 8).map_err(|_| ERR_ARGS)?);
    let amount = read_u128(body, 16).map_err(|_| ERR_ARGS)?;
    let last_attempt = u64::from_le_bytes(array(b, 96).map_err(|_| ERR_ARGS)?);
    if attempt <= last_attempt || amount == 0 || principal(message.extra) != amount {
        return Err(ERR_ARGS);
    }
    let received = read_u128(b, 104).map_err(|_| ERR_ARGS)?.checked_add(amount).ok_or(ERR_ARGS)?;
    let count =
        u64::from_le_bytes(array(b, 120).map_err(|_| ERR_ARGS)?).checked_add(1).ok_or(ERR_ARGS)?;
    state[96..104].copy_from_slice(&attempt.to_le_bytes());
    state[104..120].copy_from_slice(&received.to_le_bytes());
    state[120..128].copy_from_slice(&count.to_le_bytes());
    let mut output = MessageOutput::success(StateUpdate::FullReplace(state));
    output.callback_result_body = body.to_vec();
    Ok(output)
}

pub fn received(state: &[u8]) -> Option<u128> {
    read_u128(state, 104).ok()
}
pub fn bound_source(state: &[u8]) -> Option<AccountAddress> {
    read_address(state, 32).ok()
}

#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_message_contract!(Receiver, arena_bytes = 262_144);
