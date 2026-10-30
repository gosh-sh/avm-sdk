#![forbid(unsafe_code)]

use avm_contract_sdk::{
    AccountAddress, Caller, CanonicalMessage, Contract, MessageInput, MessageOutput,
    OutboundIntent, StateUpdate,
};
use avm_revenue_core::*;
pub const STATE_BYTES: usize = 224;
pub const E_RECEIVER_AUTH: u32 = 3100;
pub const E_RECEIVER_ARGS: u32 = 3101;
pub const E_REJECT: u32 = 3102;
pub struct PayoutReceiver;
impl Contract for PayoutReceiver {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}
fn initialize(input: MessageInput<'_>) -> HubResult<MessageOutput> {
    let Caller::External(owner) = input.caller else {
        return Err(E_RECEIVER_AUTH);
    };
    let body = input.deploy().map_err(|_| E_ARGS)?.init_data();
    if body.len() != 88
        || body[65..72] != [0; 7]
        || body[64] >= MAX_MEMBERS as u8
        || !input.state.is_empty()
        || !input.message.extra().is_empty()
    {
        return Err(E_RECEIVER_ARGS);
    }
    let audit = address_at(body, 0)?;
    let fuel = u128_at(body, 72)?;
    if !nonzero_address(audit)
        || audit == input.destination
        || audit.dapp_id != input.destination.dapp_id
        || fuel == 0
    {
        return Err(E_RECEIVER_ARGS);
    }
    let mut b = vec![0; STATE_BYTES];
    b[..32].copy_from_slice(&owner);
    put_address(&mut b, 32, audit);
    b[160] = body[64];
    b[200..216].copy_from_slice(&fuel.to_le_bytes());
    Ok(MessageOutput::success(StateUpdate::FullReplace(b)))
}
fn process(input: MessageInput<'_>) -> HubResult<MessageOutput> {
    let b = input.state;
    if b.len() != STATE_BYTES
        || b[162..168] != [0; 6]
        || b[217..224] != [0; 7]
        || b[216] > 1
        || b[161] > 1
        || b[160] >= 16
    {
        return Err(E_RECEIVER_ARGS);
    }
    let owner = array(b, 0)?;
    let method = input.message.method_id();
    let body = input.message.body();
    if method == ids::RECEIVER_STATUS {
        let Caller::External(_) = input.caller else {
            return Err(E_RECEIVER_AUTH);
        };
        no_body(body)?;
        if !input.message.extra().is_empty() {
            return Err(E_RECEIVER_ARGS);
        }
        return Ok(MessageOutput::success(StateUpdate::FullReplace(b.to_vec())));
    }
    if method == ids::BIND || method == ids::REJECT || method == ids::DUPLICATE_AUDIT {
        if input.caller != Caller::External(owner) {
            return Err(E_RECEIVER_AUTH);
        }
        if !input.message.extra().is_empty() {
            return Err(E_RECEIVER_ARGS);
        }
        let mut next = b.to_vec();
        if method == ids::BIND {
            if body.len() != 64 || b[96..160] != [0; 64] {
                return Err(E_RECEIVER_ARGS);
            }
            let vault = address_at(body, 0)?;
            if !nonzero_address(vault)
                || vault == input.destination
                || vault.dapp_id != input.destination.dapp_id
            {
                return Err(E_RECEIVER_ARGS);
            }
            put_address(&mut next, 96, vault);
        } else {
            if body.len() != 1 || body[0] > 1 {
                return Err(E_RECEIVER_ARGS);
            }
            next[if method == ids::REJECT { 161 } else { 216 }] = body[0];
        }
        return Ok(MessageOutput::success(StateUpdate::FullReplace(next)));
    }
    let (Caller::Internal(source), CanonicalMessage::Internal(message)) =
        (input.caller, input.message)
    else {
        return Err(E_RECEIVER_AUTH);
    };
    if source != address_at(b, 96)?
        || method != ids::RECEIVE
        || !message.has_callback_request()
        || message.is_callback()
    {
        return Err(E_RECEIVER_AUTH);
    }
    if b[161] == 1 {
        return Err(E_REJECT);
    }
    let (index, nonce, amount) = read_payment(body)?;
    if index != usize::from(b[160])
        || nonce <= u64_at(b, 168)?
        || amount != principal(message.extra)
        || message.extra.len() != 1
    {
        return Err(E_RECEIVER_ARGS);
    }
    let mut next = b.to_vec();
    next[168..176].copy_from_slice(&nonce.to_le_bytes());
    next[176..192].copy_from_slice(
        &u128_at(b, 176)?.checked_add(amount).ok_or(E_RECEIVER_ARGS)?.to_le_bytes(),
    );
    next[192..200]
        .copy_from_slice(&u64_at(b, 192)?.checked_add(1).ok_or(E_RECEIVER_ARGS)?.to_le_bytes());
    let mut out = MessageOutput::success(StateUpdate::FullReplace(next));
    out.callback_result_body = body.to_vec();
    out.outbound_intents.push(OutboundIntent {
        input_index: 0,
        destination: address_at(b, 32)?,
        fuel_fee_shells: u128_at(b, 200)?,
        extra: vec![],
        method_id: ids::AUDIT_RECORD,
        body: body.to_vec(),
    });
    if b[216] == 1 {
        out.outbound_intents.push(out.outbound_intents[0].clone());
    }
    Ok(out)
}
pub fn received(b: &[u8]) -> HubResult<u128> {
    u128_at(b, 176)
}
pub fn vault_address(b: &[u8]) -> HubResult<AccountAddress> {
    address_at(b, 96)
}

#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_message_contract!(PayoutReceiver, arena_bytes = 262_144);
