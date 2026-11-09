#![forbid(unsafe_code)]

//! An owner wallet. Its account data is the owner key alone, declared by
//! `owner_key_offset = 0`, so the node admits an external call to it only when
//! the owner key signs it. An external message carries no value; the wallet
//! is how its owner calls other contracts with value: `wallet.send` turns one
//! signed external call into one internal message paid from the wallet
//! balance, and the destination sees the wallet address as its caller.

use avm_contract_sdk::encode_event_fields;
use avm_contract_sdk::set_owner_key;
use avm_contract_sdk::AccountAddress;
use avm_contract_sdk::CallbackIntent;
use avm_contract_sdk::Caller;
use avm_contract_sdk::CanonicalMessage;
use avm_contract_sdk::Contract;
use avm_contract_sdk::CurrencyAmount;
use avm_contract_sdk::EventField;
use avm_contract_sdk::EventIntent;
use avm_contract_sdk::EventKind;
use avm_contract_sdk::EventValue;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::OutboundIntent;
use avm_contract_sdk::StateUpdate;
use avm_contract_sdk::OWNER_KEY_BYTES;

/// `owner_key_offset` in `avm-contract.toml`.
const OWNER_KEY_OFFSET: u32 = 0;
// Send args after the u32 length prefix of the ABI `bytes` type:
// destination 64 | method id u64 | fuel_fee_shells u128 | request callback u8 |
// extra count u8 | count x (currency id u32 | amount u128) | payload.
const SEND_HEADER_BYTES: usize = 90;
const EXTRA_ENTRY_BYTES: usize = 20;

const SEND_METHOD_ID: u64 = 0x4378_f997_1132_d918;
const SET_OWNER_KEY_METHOD_ID: u64 = 0xaa89_1178_5e57_cf0d;
const RECEIVE_METHOD_ID: u64 = 0x545c_8dab_459b_5349;
const CALLBACK_METHOD_ID: u64 = 0x9bb7_baa3_6154_de27;
/// `hash_u64_le("wallet.callback")`, the same name as the handler method.
const CALLBACK_EVENT_CLASS: u64 = CALLBACK_METHOD_ID;

const INVALID_STATE: u32 = 1700;
const EXTERNAL_CALLER_REQUIRED: u32 = 1701;
const INVALID_SEND_ARGS: u32 = 1702;
const INVALID_OWNER_KEY: u32 = 1703;
const UNKNOWN_METHOD: u32 = 1704;
const INVALID_CALLBACK: u32 = 1705;

struct Wallet;

impl Contract for Wallet {
    /// Writes the deploy's `owner_id` as the owner key. A deploy without a
    /// readable envelope or with a zero `owner_id` leaves a keyless wallet.
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let mut data = vec![0; OWNER_KEY_BYTES];
        if let Ok(deploy) = input.deploy() {
            // The 32 bytes always fit the 32-byte data.
            let _ = set_owner_key(&mut data, OWNER_KEY_OFFSET, deploy.owner_id());
        }
        MessageOutput::success(StateUpdate::FullReplace(data))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        if input.state.len() != OWNER_KEY_BYTES {
            return MessageOutput::failure(INVALID_STATE);
        }
        if input.message.is_callback() {
            return callback(input);
        }
        match input.message.method_id() {
            SEND_METHOD_ID => send(input),
            SET_OWNER_KEY_METHOD_ID => rotate(input),
            RECEIVE_METHOD_ID => unchanged(input),
            _ => MessageOutput::failure(UNKNOWN_METHOD),
        }
    }
}

fn unchanged(input: MessageInput<'_>) -> MessageOutput {
    MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()))
}

fn send(input: MessageInput<'_>) -> MessageOutput {
    let Caller::External(_) = input.caller else {
        return MessageOutput::failure(EXTERNAL_CALLER_REQUIRED);
    };
    let Some(args) = length_prefixed(input.message.body()) else {
        return MessageOutput::failure(INVALID_SEND_ARGS);
    };
    let Some((intent, request_callback)) = send_intent(args) else {
        return MessageOutput::failure(INVALID_SEND_ARGS);
    };
    let mut output = unchanged(input);
    output.outbound_intents = vec![intent];
    if request_callback {
        output.callback_intents =
            vec![CallbackIntent { outbound_index: 0, handler_method_id: CALLBACK_METHOD_ID }];
    }
    output
}

fn send_intent(args: &[u8]) -> Option<(OutboundIntent, bool)> {
    let header = args.get(..SEND_HEADER_BYTES)?;
    let request_callback = match header[88] {
        0 => false,
        1 => true,
        _ => return None,
    };
    let count = usize::from(header[89]);
    let extra_end = SEND_HEADER_BYTES.checked_add(count.checked_mul(EXTRA_ENTRY_BYTES)?)?;
    let mut extra: Vec<CurrencyAmount> = Vec::with_capacity(count);
    for entry in args.get(SEND_HEADER_BYTES..extra_end)?.as_chunks::<EXTRA_ENTRY_BYTES>().0 {
        let currency_id = u32::from_le_bytes(entry[..4].try_into().ok()?);
        let amount = u128::from_le_bytes(entry[4..].try_into().ok()?);
        if extra.last().is_some_and(|last| last.currency_id >= currency_id) {
            return None;
        }
        extra.push(CurrencyAmount { currency_id, amount });
    }
    let intent = OutboundIntent {
        input_index: 0,
        destination: AccountAddress {
            dapp_id: header[..32].try_into().ok()?,
            account_id: header[32..64].try_into().ok()?,
        },
        fuel_fee_shells: u128::from_le_bytes(header[72..88].try_into().ok()?),
        extra,
        method_id: u64::from_le_bytes(header[64..72].try_into().ok()?),
        body: args[extra_end..].to_vec(),
    };
    Some((intent, request_callback))
}

/// Rotates the owner key; the new key authorizes external calls from the next
/// block, and a zero key leaves the wallet keyless.
fn rotate(input: MessageInput<'_>) -> MessageOutput {
    let Caller::External(_) = input.caller else {
        return MessageOutput::failure(EXTERNAL_CALLER_REQUIRED);
    };
    let Ok(key) = <[u8; OWNER_KEY_BYTES]>::try_from(input.message.body()) else {
        return MessageOutput::failure(INVALID_OWNER_KEY);
    };
    let mut data = input.state.to_vec();
    if set_owner_key(&mut data, OWNER_KEY_OFFSET, key).is_err() {
        return MessageOutput::failure(INVALID_STATE);
    }
    MessageOutput::success(StateUpdate::FullReplace(data))
}

/// Keeps the value a callback returns and reports the destination's exact
/// status code and the returned currencies in one `wallet.callback` event.
fn callback(input: MessageInput<'_>) -> MessageOutput {
    let CanonicalMessage::Internal(message) = input.message else {
        return MessageOutput::failure(INVALID_CALLBACK);
    };
    let Some(header) = message.callback else {
        return MessageOutput::failure(INVALID_CALLBACK);
    };
    // Field names in ascending order, as the event field encoding requires.
    let Ok(fields_binary) = encode_event_fields(&[
        EventField {
            name: "extra".into(),
            value: EventValue::Data(message.extra.canonical_bytes().to_vec()),
        },
        EventField {
            name: "status_code".into(),
            value: EventValue::U64(header.status_code.into()),
        },
    ]) else {
        return MessageOutput::failure(INVALID_CALLBACK);
    };
    let mut output = unchanged(input);
    output.event_intents = vec![EventIntent {
        input_index: 0,
        kind: EventKind::External,
        event_class: CALLBACK_EVENT_CLASS,
        event_name: "wallet.callback".into(),
        topics: vec![header.cause_msg_hash.to_vec()],
        fields_binary,
    }];
    output
}

fn length_prefixed(body: &[u8]) -> Option<&[u8]> {
    let len = usize::try_from(u32::from_le_bytes(body.get(..4)?.try_into().ok()?)).ok()?;
    body.get(4..).filter(|args| args.len() == len)
}

avm_contract_sdk::export_message_contract!(Wallet);
