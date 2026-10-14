#![forbid(unsafe_code)]

use avm_contract_sdk::AccountAddress;
use avm_contract_sdk::CallbackIntent;
use avm_contract_sdk::Caller;
use avm_contract_sdk::CanonicalMessage;
use avm_contract_sdk::Contract;
use avm_contract_sdk::CurrencyAmount;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::OutboundIntent;
use avm_contract_sdk::StateUpdate;
use avm_contract_sdk::CODE_UPGRADE_METHOD_ID;

const STATE_BYTES: usize = 72;
const FORWARD_ARGS_BYTES: usize = 80;
const VALUE_MAIN: u128 = 37;
const EXTRA_CURRENCY_ID: u32 = 7;
const EXTRA_CURRENCY_AMOUNT: u128 = 11;
const OUTBOUND_BODY: [u8; 16] = [0x42; 16];
const CALLBACK_BODY: [u8; 16] = [0x99; 16];
const VERSION_MARKER: u8 = 2;
const VERSION_METHOD_ID: u64 = 0x0dc1_fee6_93a5_219c;
const DEPLOY_METHOD_ID: u64 = 0x1608_e0e0_319b_4d0f;

struct ContractA;

impl Contract for ContractA {
    fn init(_input: MessageInput<'_>) -> MessageOutput {
        MessageOutput::success(StateUpdate::FullReplace(vec![0; STATE_BYTES]))
    }

    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        if input.message.is_callback() {
            return process_callback(input);
        }
        if input.message.method_id() == CODE_UPGRADE_METHOD_ID {
            return process_code_upgrade(input);
        }
        if input.message.method_id() == VERSION_METHOD_ID {
            return process_version_probe(input);
        }
        let Caller::External(_) = input.caller else {
            return MessageOutput::failure(1000);
        };
        let encoded_body = input.message.body();
        let message_body = if input.message.method_id() == DEPLOY_METHOD_ID {
            let Some(encoded_len) = encoded_body
                .get(..4)
                .and_then(|bytes| bytes.try_into().ok())
                .map(u32::from_le_bytes)
                .and_then(|len| usize::try_from(len).ok())
            else {
                return MessageOutput::failure(1000);
            };
            let Some(body) = encoded_body.get(4..) else {
                return MessageOutput::failure(1000);
            };
            if body.len() != encoded_len {
                return MessageOutput::failure(1000);
            }
            body
        } else {
            encoded_body
        };
        let Some(arguments) = message_body.get(..FORWARD_ARGS_BYTES) else {
            return MessageOutput::failure(1000);
        };
        let Some(destination_dapp_id) = arguments.get(..32).and_then(|bytes| bytes.try_into().ok())
        else {
            return MessageOutput::failure(1000);
        };
        let Some(destination_account_id) =
            arguments.get(32..64).and_then(|bytes| bytes.try_into().ok())
        else {
            return MessageOutput::failure(1000);
        };
        let Some(method_id) =
            arguments.get(64..72).and_then(|bytes| bytes.try_into().ok()).map(u64::from_le_bytes)
        else {
            return MessageOutput::failure(1000);
        };
        let Some(callback_method_id) =
            arguments.get(72..80).and_then(|bytes| bytes.try_into().ok()).map(u64::from_le_bytes)
        else {
            return MessageOutput::failure(1000);
        };
        let deploy_body = &message_body[FORWARD_ARGS_BYTES..];
        let is_deploy = deploy_body.starts_with(b"ACDP");
        if (deploy_body.is_empty() && callback_method_id == 0)
            || (!deploy_body.is_empty() && (!is_deploy || callback_method_id != 0))
        {
            return MessageOutput::failure(1000);
        }
        let Some(previous_count) = input
            .state
            .get(..8)
            .filter(|_| input.state.len() == STATE_BYTES)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u64::from_le_bytes)
        else {
            return MessageOutput::failure(1001);
        };
        let Some(next_count) = previous_count.checked_add(1) else {
            return MessageOutput::failure(1001);
        };
        if !is_deploy && input.state[8..].iter().any(|byte| *byte != 0) {
            return MessageOutput::failure(1001);
        }

        let destination =
            AccountAddress { dapp_id: destination_dapp_id, account_id: destination_account_id };
        let mut next_state = if is_deploy { input.state.to_vec() } else { vec![0; STATE_BYTES] };
        next_state[..8].copy_from_slice(&next_count.to_le_bytes());

        let mut output = MessageOutput::success(StateUpdate::FullReplace(next_state));
        output.outbound_intents = vec![OutboundIntent {
            input_index: 0,
            destination,
            value_main: VALUE_MAIN,
            fuel_fee_shells: u128::from(destination.dapp_id != input.destination.dapp_id),
            extra: vec![CurrencyAmount {
                currency_id: EXTRA_CURRENCY_ID,
                amount: EXTRA_CURRENCY_AMOUNT,
            }],
            method_id,
            body: if is_deploy { deploy_body.to_vec() } else { Vec::from(OUTBOUND_BODY) },
        }];
        if !is_deploy {
            output.callback_intents =
                vec![CallbackIntent { outbound_index: 0, handler_method_id: callback_method_id }];
        }
        output
    }
}

fn process_code_upgrade(_input: MessageInput<'_>) -> MessageOutput {
    MessageOutput::failure(1004)
}

fn process_version_probe(input: MessageInput<'_>) -> MessageOutput {
    if !input.message.body().is_empty() || input.state.len() != STATE_BYTES {
        return MessageOutput::failure(1003);
    }
    let mut next_state = input.state.to_vec();
    match input.caller {
        Caller::External(_) => next_state[16] = VERSION_MARKER,
        Caller::Internal(_)
            if input.message.value_main() != 0
                || input.message.extra().iter().any(|currency| currency.amount != 0) => {}
        Caller::Internal(_) => return MessageOutput::failure(1003),
    }
    MessageOutput::success(StateUpdate::FullReplace(next_state))
}

fn process_callback(input: MessageInput<'_>) -> MessageOutput {
    let (Caller::Internal(_), CanonicalMessage::Internal(message)) = (input.caller, input.message)
    else {
        return MessageOutput::failure(1002);
    };
    let Some(callback) = message.callback else {
        return MessageOutput::failure(1002);
    };
    let Some(previous_forward_count) = input
        .state
        .get(..8)
        .filter(|_| input.state.len() == STATE_BYTES)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
    else {
        return MessageOutput::failure(1001);
    };
    let Some(previous_callback_count) =
        input.state.get(8..16).and_then(|bytes| bytes.try_into().ok()).map(u64::from_le_bytes)
    else {
        return MessageOutput::failure(1001);
    };
    if previous_forward_count != 1
        || previous_callback_count != 0
        || input.state[16..].iter().any(|byte| *byte != 0)
        || message.has_callback_request()
        || callback.status_code != 0
        || callback.cause_address != input.destination
        || message.body != CALLBACK_BODY
        || input.internal_event_results != 0_u32.to_le_bytes()
    {
        return MessageOutput::failure(1002);
    }

    let mut next_state = vec![0; STATE_BYTES];
    next_state[..8].copy_from_slice(&previous_forward_count.to_le_bytes());
    next_state[8..16].copy_from_slice(&1_u64.to_le_bytes());
    next_state[16..20].copy_from_slice(&callback.status_code.to_le_bytes());
    next_state[20..24].copy_from_slice(&(CALLBACK_BODY.len() as u32).to_le_bytes());
    next_state[24..40].copy_from_slice(&CALLBACK_BODY);
    next_state[40..72].copy_from_slice(&callback.cause_msg_hash);
    MessageOutput::success(StateUpdate::FullReplace(next_state))
}

avm_contract_sdk::export_message_contract!(ContractA, arena_bytes = 262_144);
