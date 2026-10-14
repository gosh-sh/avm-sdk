#![forbid(unsafe_code)]
use avm_contract_sdk::{
    Caller, CanonicalMessage, Contract, MessageInput, MessageOutput, StateUpdate,
};
use avm_stream_core::*;
pub const STATE_BYTES: usize = 368;
pub const E_RECEIVER_AUTH: u32 = 4100;
pub const E_RECEIVER_ARGS: u32 = 4101;
pub const E_REJECT: u32 = 4102;
pub struct Receiver;
impl Contract for Receiver {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}
fn validate(b: &[u8]) -> ResultCode<()> {
    if b.len() != STATE_BYTES
        || b[96] > 1
        || b[97] > 1
        || b[98] >= 8
        || b[99..104] != [0; 5]
        || b[360..368] != [0; 8]
    {
        return Err(E_RECEIVER_ARGS);
    }
    Ok(())
}
fn initialize(input: MessageInput<'_>) -> ResultCode<MessageOutput> {
    let Caller::External(owner) = input.caller else {
        return Err(E_RECEIVER_AUTH);
    };
    let body = deploy_data(input.message.body())?;
    if body.len() != 2
        || body[0] > 1
        || body[1] >= 8
        || !input.state.is_empty()
        || input.message.value_main() != 0
        || !input.message.extra().is_empty()
    {
        return Err(E_RECEIVER_ARGS);
    }
    let mut b = vec![0; STATE_BYTES];
    b[..32].copy_from_slice(&owner);
    b[97] = body[0];
    b[98] = body[1];
    Ok(MessageOutput::success(StateUpdate::FullReplace(b)))
}
fn process(input: MessageInput<'_>) -> ResultCode<MessageOutput> {
    let b = input.state;
    validate(b)?;
    let method = input.message.method_id();
    let body = input.message.body();
    if method == ids::RECEIVER_STATUS || method == ids::BIND || method == ids::REJECT {
        let Caller::External(actor) = input.caller else {
            return Err(E_RECEIVER_AUTH);
        };
        if input.message.value_main() != 0 || !input.message.extra().is_empty() {
            return Err(E_RECEIVER_ARGS);
        }
        let mut next = b.to_vec();
        if method == ids::RECEIVER_STATUS {
            no_body(body)?;
        } else {
            if actor != array(b, 0)? {
                return Err(E_RECEIVER_AUTH);
            }
            if method == ids::BIND {
                if body.len() != 64 || b[32..96] != [0; 64] {
                    return Err(E_RECEIVER_ARGS);
                }
                let a = address_at(body, 0)?;
                if !nonzero_address(a)
                    || a == input.destination
                    || a.dapp_id != input.destination.dapp_id
                {
                    return Err(E_RECEIVER_ARGS);
                }
                put_address(&mut next, 32, a);
            } else {
                if body.len() != 1 || body[0] > 1 {
                    return Err(E_RECEIVER_ARGS);
                }
                next[96] = body[0];
            }
        }
        return Ok(MessageOutput::success(StateUpdate::FullReplace(next)));
    }
    let (Caller::Internal(source), CanonicalMessage::Internal(msg)) = (input.caller, input.message)
    else {
        return Err(E_RECEIVER_AUTH);
    };
    if method != ids::RECEIVE
        || source != address_at(b, 32)?
        || !nonzero_address(source)
        || !msg.has_callback_request()
        || msg.is_callback()
    {
        return Err(E_RECEIVER_AUTH);
    }
    if b[96] == 1 {
        return Err(E_REJECT);
    }
    let (index, kind, nonce, amount) = read_payment(body)?;
    if index != usize::from(b[98])
        || kind != usize::from(b[97])
        || amount != msg.value_main
        || !msg.extra.is_empty()
    {
        return Err(E_RECEIVER_ARGS);
    }
    let o = 104 + index * 32;
    if nonce <= u64_at(b, o)? {
        return Err(E_RECEIVER_ARGS);
    }
    let mut next = b.to_vec();
    next[o..o + 8].copy_from_slice(&nonce.to_le_bytes());
    next[o + 8..o + 24].copy_from_slice(
        &u128_at(b, o + 8)?.checked_add(amount).ok_or(E_RECEIVER_ARGS)?.to_le_bytes(),
    );
    next[o + 24..o + 32]
        .copy_from_slice(&u64_at(b, o + 24)?.checked_add(1).ok_or(E_RECEIVER_ARGS)?.to_le_bytes());
    let mut out = MessageOutput::success(StateUpdate::FullReplace(next));
    out.callback_result_body = body.to_vec();
    Ok(out)
}
#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_message_contract!(Receiver, arena_bytes = 262_144);
