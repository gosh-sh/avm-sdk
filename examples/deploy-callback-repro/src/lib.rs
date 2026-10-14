#![forbid(unsafe_code)]
// Regression derived from alandocarmo/avm-sdk commit
// 3e44f176c690639c6cccbcd3074f364cf38feac1 (SDK issue #13).
// The matching runtime candidate must deliver the requested deploy callback.

//! One contract plays both roles of the reproducer. As a parent it forwards a
//! message, optionally with a callback request, and records any callback it
//! receives. As a child its `init` records how it was deployed.

use avm_contract_sdk::AccountAddress;
use avm_contract_sdk::CallbackIntent;
use avm_contract_sdk::Caller;
use avm_contract_sdk::CanonicalMessage;
use avm_contract_sdk::Contract;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::OutboundIntent;
use avm_contract_sdk::StateUpdate;

// State layout:
//   [0]       phase: 0 idle, 1 forwarded, 2 callback received
//   [1]       init caller: 1 external, 2 internal
//   [2]       init message carried a callback request: 0 or 1
//   [3]       callbacks received, checked for overflow
//   [4..8]    callback status code u32
//   [8..40]   callback cause message hash
//   [40..48]  init method id u64
//   [48..52]  init body length u32
//   [52]      init body captured: 0 or 1
//   [64..]    init body, when it fits
const STATE_BYTES: usize = 1024;
const BODY_OFFSET: usize = 64;
const FORWARD_HEADER_BYTES: usize = 73;

const FORWARD_METHOD_ID: u64 = 0xe5fc_1714_1fa5_6ba2;
const PING_METHOD_ID: u64 = 0x96ec_e1cd_e222_841c;
const CALLBACK_METHOD_ID: u64 = 0x72ba_0382_87dc_1d84;

const INVALID_STATE: u32 = 1500;
const INIT_BODY_TOO_LARGE: u32 = 1501;
const INVALID_FORWARD_ARGS: u32 = 1502;
const EXTERNAL_CALLER_REQUIRED: u32 = 1503;
const INVALID_CALLBACK: u32 = 1504;
const UNKNOWN_METHOD: u32 = 1505;

struct DeployCallbackRepro;

impl Contract for DeployCallbackRepro {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let body = input.message.body();
        let Ok(body_len) = u32::try_from(body.len()) else {
            return MessageOutput::failure(INIT_BODY_TOO_LARGE);
        };
        let mut state = vec![0; STATE_BYTES];
        state[1] = match input.caller {
            Caller::External(_) => 1,
            Caller::Internal(_) => 2,
        };
        state[2] = u8::from(input.message.has_callback_request());
        state[40..48].copy_from_slice(&input.message.method_id().to_le_bytes());
        state[48..52].copy_from_slice(&body_len.to_le_bytes());
        // A deploy that carries WASM does not fit; only its length is kept.
        if let Some(capture) = state.get_mut(BODY_OFFSET..BODY_OFFSET + body.len()) {
            capture.copy_from_slice(body);
            state[52] = 1;
        }
        MessageOutput::success(StateUpdate::FullReplace(state))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        if input.state.len() != STATE_BYTES {
            return MessageOutput::failure(INVALID_STATE);
        }
        if input.message.is_callback() {
            return record_callback(input);
        }
        match input.message.method_id() {
            PING_METHOD_ID => {
                MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()))
            }
            FORWARD_METHOD_ID => forward(input),
            _ => MessageOutput::failure(UNKNOWN_METHOD),
        }
    }
}

// Forward args, after the u32 length prefix of the ABI `bytes` type:
// destination dapp id 32 | account id 32 | method id u64 | request callback u8 | body.
fn forward(input: MessageInput<'_>) -> MessageOutput {
    let Caller::External(_) = input.caller else {
        return MessageOutput::failure(EXTERNAL_CALLER_REQUIRED);
    };
    let encoded = input.message.body();
    let Some(args) = encoded
        .get(..4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .and_then(|len| usize::try_from(len).ok())
        .zip(encoded.get(4..))
        .filter(|(len, args)| *len == args.len())
        .map(|(_, args)| args)
    else {
        return MessageOutput::failure(INVALID_FORWARD_ARGS);
    };
    let (Some(dapp_id), Some(account_id), Some(method_id), Some(request_callback)) = (
        args.get(..32).and_then(|bytes| bytes.try_into().ok()),
        args.get(32..64).and_then(|bytes| bytes.try_into().ok()),
        args.get(64..72).and_then(|bytes| bytes.try_into().ok()).map(u64::from_le_bytes),
        args.get(72).copied().filter(|flag| *flag <= 1),
    ) else {
        return MessageOutput::failure(INVALID_FORWARD_ARGS);
    };
    let mut state = input.state.to_vec();
    state[0] = 1;
    state[4..40].fill(0);
    let mut output = MessageOutput::success(StateUpdate::FullReplace(state));
    output.outbound_intents = vec![OutboundIntent {
        input_index: 0,
        destination: AccountAddress { dapp_id, account_id },
        value_main: 0,
        fuel_fee_shells: 1000,
        extra: Vec::new(),
        method_id,
        body: args[FORWARD_HEADER_BYTES..].to_vec(),
    }];
    if request_callback == 1 {
        output.callback_intents =
            vec![CallbackIntent { outbound_index: 0, handler_method_id: CALLBACK_METHOD_ID }];
    }
    output
}

fn record_callback(input: MessageInput<'_>) -> MessageOutput {
    let CanonicalMessage::Internal(message) = input.message else {
        return MessageOutput::failure(INVALID_CALLBACK);
    };
    let Some(callback) = message.callback else {
        return MessageOutput::failure(INVALID_CALLBACK);
    };
    let mut state = input.state.to_vec();
    state[0] = 2;
    let Some(count) = state[3].checked_add(1) else {
        return MessageOutput::failure(INVALID_CALLBACK);
    };
    state[3] = count;
    state[4..8].copy_from_slice(&callback.status_code.to_le_bytes());
    state[8..40].copy_from_slice(&callback.cause_msg_hash);
    MessageOutput::success(StateUpdate::FullReplace(state))
}

avm_contract_sdk::export_message_contract!(DeployCallbackRepro, arena_bytes = 262_144);
