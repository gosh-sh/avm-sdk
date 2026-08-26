#![forbid(unsafe_code)]

use avm_contract_sdk::BatchContract;
use avm_contract_sdk::BatchInput;
use avm_contract_sdk::BatchOutput;
use avm_contract_sdk::CanonicalMessage;
use avm_contract_sdk::Contract;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::StateOutputMode;
use avm_contract_sdk::StatePatch;
use avm_contract_sdk::StateUpdate;

const STATE_PAYLOAD_BYTES: usize = 65_536;
const STATE_BYTES: usize = 4 + STATE_PAYLOAD_BYTES;
const COUNTER_OFFSET: usize = 4;
const COUNTER_BYTES: usize = 8;
const INTERNAL_PATCH_BODY: [u8; 16] = [0x42; 16];
const VALUE_MAIN: u128 = 37;
const EXTRA_CURRENCY_ID: u32 = 7;
const EXTRA_CURRENCY_AMOUNT: u128 = 11;

struct BatchPatchContract;

impl Contract for BatchPatchContract {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        if input.required_state_output_mode != StateOutputMode::FullReplace {
            return MessageOutput::failure(1400);
        }
        let mut state = vec![0; STATE_BYTES];
        state[..4].copy_from_slice(&(STATE_PAYLOAD_BYTES as u32).to_le_bytes());
        MessageOutput::success(StateUpdate::FullReplace(state))
    }

    fn process_message(_input: MessageInput<'_>) -> MessageOutput {
        MessageOutput::failure(1401)
    }
}

impl BatchContract for BatchPatchContract {
    fn process_batch(input: BatchInput<'_>) -> BatchOutput {
        let failure = || BatchOutput {
            state_update: StateUpdate::DataPatchList(Vec::new()),
            status_codes: vec![1402; input.message_count() as usize],
            outbound_intents: Vec::new(),
            event_intents: Vec::new(),
        };
        let Some(previous_counter) = valid_counter(input.state) else { return failure() };
        if input.required_state_output_mode != StateOutputMode::DataPatchList
            || input.message_count() != 1
            || input.internal_event_results != 0_u32.to_le_bytes()
        {
            return failure();
        }
        let Some(Ok(CanonicalMessage::Internal(message))) = input.messages().next() else {
            return failure();
        };
        let mut extra = message.extra.iter();
        let Some(currency) = extra.next() else { return failure() };
        if message.source == input.destination
            || message.source.dapp_id != input.destination.dapp_id
            || message.destination != input.destination
            || message.value_main != VALUE_MAIN
            || message.fuel_fee_shells != 1
            || currency.currency_id != EXTRA_CURRENCY_ID
            || currency.amount != EXTRA_CURRENCY_AMOUNT
            || extra.next().is_some()
            || message.is_callback()
            || message.is_deploy()
            || message.has_callback_request()
            || message.event_reads.is_some()
            || message.body != INTERNAL_PATCH_BODY
        {
            return failure();
        }
        let Some(next_counter) = previous_counter.checked_add(1) else { return failure() };

        BatchOutput {
            state_update: StateUpdate::DataPatchList(vec![StatePatch {
                offset: COUNTER_OFFSET as u32,
                old_len: COUNTER_BYTES as u32,
                new_bytes: next_counter.to_le_bytes().to_vec(),
            }]),
            status_codes: vec![0],
            outbound_intents: Vec::new(),
            event_intents: Vec::new(),
        }
    }
}

fn valid_counter(state: &[u8]) -> Option<u64> {
    if state.len() != STATE_BYTES
        || state[..4] != (STATE_PAYLOAD_BYTES as u32).to_le_bytes()
        || state[COUNTER_OFFSET + COUNTER_BYTES..].iter().any(|byte| *byte != 0)
    {
        return None;
    }
    state[COUNTER_OFFSET..COUNTER_OFFSET + COUNTER_BYTES].try_into().ok().map(u64::from_le_bytes)
}

avm_contract_sdk::export_batch_contract!(BatchPatchContract, arena_bytes = 524_288);
