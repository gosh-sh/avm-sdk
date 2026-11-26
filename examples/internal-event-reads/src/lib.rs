#![forbid(unsafe_code)]

//! Reads internal events of other accounts. One contract plays three roles:
//! `params.set` publishes a parameter as an internal event, `params.ask` sends
//! `params.read` to another account and declares the internal event rings
//! that message reads, and `params.read` checks that it received exactly the
//! declared rings and keeps what it read.

use avm_contract_sdk::{
    decode_event_fields, encode_event_fields, set_owner_key, AccountAddress, Caller, Contract,
    EventField, EventIntent, EventKind, EventReadEntry, EventValue, InternalEventReadRequest,
    MessageInput, MessageOutput, OutboundIntent, StateUpdate,
};

// State layout:
//   [0..4]    reads received, u32
//   [4..13]   read 0: number of events, the newest `value` (u64; 0 without events)
//   [13..22]  read 1: the same
//   [22..54]  owner key (`owner_key_offset`), the deploy's owner_id
const STATE_BYTES: usize = 22;
const DATA_BYTES: usize = STATE_BYTES + 32;
const SLOTS_START: usize = 4;
const SLOT_BYTES: usize = 9;
const MAX_READS: usize = 2;
/// One read: source DApp id, source account id, `event_class` (u64).
const KEY_BYTES: usize = 72;
/// The FUEL reserve of `params.read`; a message within one DApp pays none.
const READ_FUEL_FEE_SHELLS: u128 = 1000;

const SET_METHOD_ID: u64 = 0xb342_885a_33c4_005e;
const ASK_METHOD_ID: u64 = 0xdcee_35ca_566a_be9d;
const READ_METHOD_ID: u64 = 0x951e_256e_78cb_685d;

const INVALID_STATE: u32 = 1600;
const INVALID_ARGS: u32 = 1601;
const EXTERNAL_CALLER_REQUIRED: u32 = 1602;
const INTERNAL_CALLER_REQUIRED: u32 = 1603;
const WRONG_READS: u32 = 1604;
const UNKNOWN_METHOD: u32 = 1605;

struct InternalEventReads;

impl Contract for InternalEventReads {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let mut state = vec![0; DATA_BYTES];
        if let Ok(deploy) = input.deploy() {
            // The key slot is inside the state built above.
            let _ = set_owner_key(&mut state, STATE_BYTES as u32, deploy.owner_id());
        }
        MessageOutput::success(StateUpdate::FullReplace(state))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        if input.state.len() != DATA_BYTES {
            return MessageOutput::failure(INVALID_STATE);
        }
        match input.message.method_id() {
            SET_METHOD_ID => set(input),
            ASK_METHOD_ID => ask(input),
            READ_METHOD_ID => read(input),
            _ => MessageOutput::failure(UNKNOWN_METHOD),
        }
    }
}

/// Publishes `value` as an internal event of `event_class`. Args (ABI
/// `bytes`, after its u32 length prefix): `event_class` u64, `value` u64.
fn set(input: MessageInput<'_>) -> MessageOutput {
    let Caller::External(_) = input.caller else {
        return MessageOutput::failure(EXTERNAL_CALLER_REQUIRED);
    };
    let Some(args) = bytes_args(input.message.body()).filter(|args| args.len() == 16) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let (Some(event_class), Some(value)) = (le_u64(args, 0), le_u64(args, 8)) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let Ok(fields_binary) =
        encode_event_fields(&[EventField { name: "value".into(), value: EventValue::U64(value) }])
    else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let mut output = MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()));
    output.event_intents = vec![EventIntent {
        input_index: 0,
        kind: EventKind::Internal,
        event_class,
        event_name: "params.set".into(),
        topics: Vec::new(),
        fields_binary,
    }];
    output
}

/// Sends `params.read` to `destination` and declares a read of each listed
/// ring. Args (ABI `bytes`, after its u32 length prefix): destination DApp id
/// 32 | account id 32, then one or two rings of 72 bytes each, in strictly
/// ascending order. The rings are also the body of `params.read`, so the
/// receiver can check that it reads exactly them.
fn ask(input: MessageInput<'_>) -> MessageOutput {
    let Caller::External(_) = input.caller else {
        return MessageOutput::failure(EXTERNAL_CALLER_REQUIRED);
    };
    let Some(args) = bytes_args(input.message.body()) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let (Some(destination), Some(keys)) = (args.get(..64).and_then(address), args.get(64..)) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let Some(reads) = rings(keys) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let Ok(keys_len) = u32::try_from(keys.len()) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let mut body = keys_len.to_le_bytes().to_vec();
    body.extend_from_slice(keys);
    let mut output = MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()));
    output.outbound_intents = vec![OutboundIntent {
        input_index: 0,
        destination,
        fuel_fee_shells: READ_FUEL_FEE_SHELLS,
        extra: Vec::new(),
        method_id: READ_METHOD_ID,
        body,
    }];
    // Reads that are not strictly ascending make the output unencodable, and
    // the call ends with status 43 (`VM_ABI_OUTPUT_MISSING`).
    output.outbound_event_reads = vec![EventReadEntry { outbound_index: 0, reads }];
    output
}

/// Keeps the number of events and the newest `value` of each ring that the
/// message declares, after checking that the declared rings are the ones in
/// its body.
fn read(input: MessageInput<'_>) -> MessageOutput {
    let Caller::Internal(_) = input.caller else {
        return MessageOutput::failure(INTERNAL_CALLER_REQUIRED);
    };
    let Some(expected) = bytes_args(input.message.body()).and_then(rings) else {
        return MessageOutput::failure(INVALID_ARGS);
    };
    let Ok(reads) = input.internal_event_reads() else {
        return MessageOutput::failure(WRONG_READS);
    };
    if reads.iter().map(|(read, _)| *read).ne(expected) {
        return MessageOutput::failure(WRONG_READS);
    }
    let mut state = input.state.to_vec();
    for (index, (read, events)) in reads.iter().enumerate() {
        if events.iter().any(|event| event.event_class != read.event_class) {
            return MessageOutput::failure(WRONG_READS);
        }
        // Events come oldest first, so the last one is the newest.
        let newest = match events.last() {
            None => 0,
            Some(event) => match decode_event_fields(event.fields_binary).as_deref() {
                Ok([EventField { name, value: EventValue::U64(value) }]) if name == "value" => {
                    *value
                }
                _ => return MessageOutput::failure(WRONG_READS),
            },
        };
        let Ok(count) = u8::try_from(events.len()) else {
            return MessageOutput::failure(WRONG_READS);
        };
        let slot = SLOTS_START + index * SLOT_BYTES;
        state[slot] = count;
        state[slot + 1..slot + SLOT_BYTES].copy_from_slice(&newest.to_le_bytes());
    }
    let Some(received) = le_u32(&state).checked_add(1) else {
        return MessageOutput::failure(INVALID_STATE);
    };
    state[..4].copy_from_slice(&received.to_le_bytes());
    MessageOutput::success(StateUpdate::FullReplace(state))
}

/// The value of an ABI `bytes` argument: a u32 length, then that many bytes.
fn bytes_args(body: &[u8]) -> Option<&[u8]> {
    let len = usize::try_from(le_u32(body.get(..4)?)).ok()?;
    body.get(4..).filter(|args| args.len() == len)
}

/// One or two rings of 72 bytes each.
fn rings(keys: &[u8]) -> Option<Vec<InternalEventReadRequest>> {
    if keys.is_empty()
        || !keys.len().is_multiple_of(KEY_BYTES)
        || keys.len() / KEY_BYTES > MAX_READS
    {
        return None;
    }
    keys.as_chunks::<KEY_BYTES>()
        .0
        .iter()
        .map(|key| {
            Some(InternalEventReadRequest {
                source: address(&key[..64])?,
                event_class: le_u64(key, 64)?,
            })
        })
        .collect()
}

fn address(bytes: &[u8]) -> Option<AccountAddress> {
    Some(AccountAddress {
        dapp_id: bytes.get(..32)?.try_into().ok()?,
        account_id: bytes.get(32..64)?.try_into().ok()?,
    })
}

fn le_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn le_u32(bytes: &[u8]) -> u32 {
    bytes.get(..4).and_then(|value| value.try_into().ok()).map_or(0, u32::from_le_bytes)
}

avm_contract_sdk::export_message_contract!(InternalEventReads);
