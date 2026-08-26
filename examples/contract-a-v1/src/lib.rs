#![forbid(unsafe_code)]

use avm_contract_sdk::AccountAddress;
use avm_contract_sdk::CallbackIntent;
use avm_contract_sdk::Caller;
use avm_contract_sdk::CanonicalMessage;
use avm_contract_sdk::CodeUpgradeIntent;
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
const CODE_UPGRADE_HEADER_BYTES: usize = 72;
const MIGRATION_PAYLOAD: [u8; 24] = *b"avm-a-v2-migrate-state!!";
const MIGRATION_MARKER: [u8; 4] = *b"V1MG";
const PACKAGE_OWNER_PUBKEY: [u8; 32] = [
    0x81, 0x39, 0x77, 0x0e, 0xa8, 0x7d, 0x17, 0x5f, 0x56, 0xa3, 0x54, 0x66, 0xc3, 0x4c, 0x7e, 0xcc,
    0xcb, 0x8d, 0x8a, 0x91, 0xb4, 0xee, 0x37, 0xa2, 0x5d, 0xf6, 0x0f, 0x5b, 0x8f, 0xc9, 0xb3, 0x94,
];
const VERSION_MARKER: u8 = 1;
const VERSION_METHOD_ID: u64 = 0x0dc1_fee6_93a5_219c;

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
        let Some(arguments) = input.message.body().get(..FORWARD_ARGS_BYTES) else {
            return MessageOutput::failure(1000);
        };
        if input.message.body().len() != FORWARD_ARGS_BYTES {
            return MessageOutput::failure(1000);
        }
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
        if input.state[8..].iter().any(|byte| *byte != 0) {
            return MessageOutput::failure(1001);
        }

        let destination =
            AccountAddress { dapp_id: destination_dapp_id, account_id: destination_account_id };
        let mut next_state = vec![0; STATE_BYTES];
        next_state[..8].copy_from_slice(&next_count.to_le_bytes());

        let mut output = MessageOutput::success(StateUpdate::FullReplace(next_state));
        output.outbound_intents = vec![OutboundIntent {
            input_index: 0,
            destination,
            value_main: VALUE_MAIN,
            fuel_fee_shells: u128::from(callback_method_id == 0),
            extra: vec![CurrencyAmount {
                currency_id: EXTRA_CURRENCY_ID,
                amount: EXTRA_CURRENCY_AMOUNT,
            }],
            method_id,
            body: Vec::from(OUTBOUND_BODY),
        }];
        if callback_method_id != 0 {
            output.callback_intents =
                vec![CallbackIntent { outbound_index: 0, handler_method_id: callback_method_id }];
        }
        output
    }
}

fn process_code_upgrade(input: MessageInput<'_>) -> MessageOutput {
    let Caller::External(package_owner) = input.caller else {
        return MessageOutput::failure(1004);
    };
    if package_owner != PACKAGE_OWNER_PUBKEY
        || input.state.len() != STATE_BYTES
        || input.message.body().len() != CODE_UPGRADE_HEADER_BYTES + MIGRATION_PAYLOAD.len()
        || input.message.body().get(..4) != Some(b"ACUG")
        || input.message.body().get(68..72)
            != Some((MIGRATION_PAYLOAD.len() as u32).to_le_bytes().as_slice())
        || input.message.body().get(CODE_UPGRADE_HEADER_BYTES..)
            != Some(MIGRATION_PAYLOAD.as_slice())
    {
        return MessageOutput::failure(1004);
    }
    let Some(target_selected_code_id) =
        input.message.body().get(4..36).and_then(|bytes| bytes.try_into().ok())
    else {
        return MessageOutput::failure(1004);
    };
    let Some(expected_code_hash) =
        input.message.body().get(36..68).and_then(|bytes| bytes.try_into().ok())
    else {
        return MessageOutput::failure(1004);
    };
    if target_selected_code_id == input.selected_code_id
        || target_selected_code_id == [0; 32]
        || expected_code_hash == [0; 32]
    {
        return MessageOutput::failure(1004);
    }

    let mut migrated_state = input.state.to_vec();
    migrated_state[16..20].copy_from_slice(&MIGRATION_MARKER);
    migrated_state[20..24].copy_from_slice(&(MIGRATION_PAYLOAD.len() as u32).to_le_bytes());
    migrated_state[24..48].copy_from_slice(&MIGRATION_PAYLOAD);
    for (state_byte, payload_byte) in migrated_state[48..72].iter_mut().zip(MIGRATION_PAYLOAD) {
        *state_byte ^= payload_byte;
    }

    let mut output = MessageOutput::success(StateUpdate::FullReplace(migrated_state));
    output.code_upgrade_intent =
        Some(CodeUpgradeIntent { target_selected_code_id, expected_code_hash });
    output
}

fn process_version_probe(input: MessageInput<'_>) -> MessageOutput {
    let Caller::External(_) = input.caller else {
        return MessageOutput::failure(1003);
    };
    if !input.message.body().is_empty() || input.state.len() != STATE_BYTES {
        return MessageOutput::failure(1003);
    }
    let mut next_state = input.state.to_vec();
    next_state[16] = VERSION_MARKER;
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
