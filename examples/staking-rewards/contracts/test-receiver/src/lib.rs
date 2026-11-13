#![forbid(unsafe_code)]

//! Test-only companion fixture: a toggleable receiver that accepts or
//! rejects an incoming payout by owner-controlled method call, mirroring
//! `examples/bounty-escrow/contracts/receiver/src/lib.rs`'s own
//! BIND/REJECT/RECEIVE pattern. Adapted for `staking-rewards`'s own payout
//! protocol: `combined_payout` (`contracts/staking-rewards/src/lib.rs`)
//! sends a bare currency transfer (`method_id: 0, body: []`), not a named
//! method call -- so unlike escrow's receiver, this one must accept any
//! internal, non-callback `method_id` from its bound source, not only one
//! specific selector. Never used by the production `staking-rewards`
//! contract itself; shipped only so `scripts/e2e.py` can force deterministic
//! success/failure outcomes without relying on fragile incidental
//! conditions (an undeployed address, etc).

pub mod ids;

use avm_contract_sdk::{
    AccountAddress, Caller, CanonicalMessage, Contract, CurrencyAmount, MessageInput,
    MessageOutput, OutboundIntent, StateUpdate,
};

pub const MAX_ENTRIES: usize = 4;
const OFFSET_OWNER: usize = 0;
const OFFSET_BOUND: usize = 32;
const OFFSET_REJECT: usize = 96;
const OFFSET_COUNT: usize = 97;
const OFFSET_ENTRIES: usize = 98;
const ENTRY_LEN: usize = 20; // currency_id:u32 + amount:u128
pub const STATE_BYTES: usize = OFFSET_ENTRIES + MAX_ENTRIES * ENTRY_LEN;

pub const ERR_ARGS: u32 = 9400;
pub const ERR_AUTH: u32 = 9401;
pub const ERR_REJECT: u32 = 9402;

pub struct TestReceiver;

impl Contract for TestReceiver {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        initialize(input).unwrap_or_else(MessageOutput::failure)
    }
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        process(input).unwrap_or_else(MessageOutput::failure)
    }
}

fn read_address(body: &[u8], offset: usize) -> AccountAddress {
    AccountAddress {
        dapp_id: body[offset..offset + 32].try_into().unwrap(),
        account_id: body[offset + 32..offset + 64].try_into().unwrap(),
    }
}

fn write_address(state: &mut [u8], offset: usize, address: AccountAddress) {
    state[offset..offset + 32].copy_from_slice(&address.dapp_id);
    state[offset + 32..offset + 64].copy_from_slice(&address.account_id);
}

fn initialize(input: MessageInput<'_>) -> Result<MessageOutput, u32> {
    let Caller::External(owner) = input.caller else { return Err(ERR_AUTH) };
    if !input.state.is_empty()
        || !input.deploy().is_ok_and(|d| d.init_data().is_empty())
        || !input.message.extra().is_empty()
    {
        return Err(ERR_ARGS);
    }
    let mut state = vec![0u8; STATE_BYTES];
    state[..32].copy_from_slice(&owner);
    Ok(MessageOutput::success(StateUpdate::FullReplace(state)))
}

fn process(input: MessageInput<'_>) -> Result<MessageOutput, u32> {
    let b = input.state;
    if b.len() != STATE_BYTES {
        return Err(ERR_ARGS);
    }
    let owner: [u8; 32] = b[OFFSET_OWNER..OFFSET_OWNER + 32].try_into().unwrap();

    if let Caller::External(_) = input.caller {
        if input.caller != Caller::External(owner) {
            return Err(ERR_AUTH);
        }
        if !input.message.extra().is_empty() {
            return Err(ERR_ARGS);
        }
        let method = input.message.method_id();
        let body = input.message.body();
        let mut state = b.to_vec();
        if method == ids::BIND {
            if body.len() != 64 {
                return Err(ERR_ARGS);
            }
            write_address(&mut state, OFFSET_BOUND, read_address(body, 0));
            return Ok(MessageOutput::success(StateUpdate::FullReplace(state)));
        }
        if method == ids::TOGGLE {
            if body.len() != 1 || body[0] > 1 {
                return Err(ERR_ARGS);
            }
            state[OFFSET_REJECT] = body[0];
            return Ok(MessageOutput::success(StateUpdate::FullReplace(state)));
        }
        if method == ids::SEND {
            // Fires an arbitrary bare transfer (`method_id: 0, body: []`)
            // into any destination, with no callback requested -- used to
            // test whether an unsolicited currency transfer is accepted or
            // rejected by the destination's own guest code.
            if body.len() != 64 + 4 + 16 {
                return Err(ERR_ARGS);
            }
            let destination = read_address(body, 0);
            let currency_id = u32::from_le_bytes(body[64..68].try_into().unwrap());
            let amount = u128::from_le_bytes(body[68..84].try_into().unwrap());
            let mut output = MessageOutput::success(StateUpdate::FullReplace(state));
            output.outbound_intents.push(OutboundIntent {
                input_index: 0,
                destination,
                fuel_fee_shells: 10,
                extra: vec![CurrencyAmount { currency_id, amount }],
                method_id: 0,
                body: Vec::new(),
            });
            return Ok(output);
        }
        return Err(ERR_ARGS);
    }

    // Internal, non-callback delivery: `combined_payout` always sends a
    // bare transfer (`method_id: 0, body: []`), so -- unlike escrow's
    // receiver -- no specific `method_id`/body shape is checked here,
    // only that it genuinely came from the bound source.
    let (Caller::Internal(source), CanonicalMessage::Internal(_)) = (input.caller, input.message)
    else {
        return Err(ERR_AUTH);
    };
    let bound = read_address(b, OFFSET_BOUND);
    if source != bound {
        return Err(ERR_AUTH);
    }
    let entries: Vec<_> = input.message.extra().iter().collect();
    if entries.len() > MAX_ENTRIES {
        return Err(ERR_ARGS);
    }
    let mut state = b.to_vec();
    state[OFFSET_COUNT] = entries.len() as u8;
    for slot in state[OFFSET_ENTRIES..OFFSET_ENTRIES + MAX_ENTRIES * ENTRY_LEN].iter_mut() {
        *slot = 0;
    }
    for (i, c) in entries.iter().enumerate() {
        let off = OFFSET_ENTRIES + i * ENTRY_LEN;
        state[off..off + 4].copy_from_slice(&c.currency_id.to_le_bytes());
        state[off + 4..off + 20].copy_from_slice(&c.amount.to_le_bytes());
    }
    if state[OFFSET_REJECT] == 1 {
        return Err(ERR_REJECT);
    }
    Ok(MessageOutput::success(StateUpdate::FullReplace(state)))
}

/// Decodes the `(currency_id, amount)` entries recorded on the most recent
/// accepted delivery -- empty if the delivery was rejected (state rolls
/// back to before the attempt, so a rejected attempt leaves whatever was
/// recorded by the previous accepted one, if any).
pub fn received_entries(state: &[u8]) -> Vec<(u32, u128)> {
    let count = state[OFFSET_COUNT] as usize;
    (0..count.min(MAX_ENTRIES))
        .map(|i| {
            let off = OFFSET_ENTRIES + i * ENTRY_LEN;
            let currency_id = u32::from_le_bytes(state[off..off + 4].try_into().unwrap());
            let amount = u128::from_le_bytes(state[off + 4..off + 20].try_into().unwrap());
            (currency_id, amount)
        })
        .collect()
}

#[cfg(target_arch = "wasm32")]
avm_contract_sdk::export_message_contract!(TestReceiver, arena_bytes = 262_144);
