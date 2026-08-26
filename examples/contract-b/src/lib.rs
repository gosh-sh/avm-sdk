/* AVM_E2E_FAILURE_FLOW_FILE_BEGIN */#![cfg_attr(rustfmt, rustfmt::skip)]/* AVM_E2E_FAILURE_FLOW_FILE_END */#![forbid(unsafe_code)]

use avm_contract_sdk::encode_event_fields;
use avm_contract_sdk::Caller;
use avm_contract_sdk::CanonicalMessage;
use avm_contract_sdk::Contract;
use avm_contract_sdk::EventField;
use avm_contract_sdk::EventIntent;
use avm_contract_sdk::EventKind;
use avm_contract_sdk::EventValue;
use avm_contract_sdk::MessageInput;
use avm_contract_sdk::MessageOutput;
use avm_contract_sdk::InternalEventReadRequests;
use avm_contract_sdk::OutboundIntent;
use avm_contract_sdk::StateUpdate;

const STATE_BYTES: usize = 128;
const CAUSAL_A_STATE_BYTES: usize = 72;
const VALUE_MAIN: u128 = 37;
const EXTRA_CURRENCY_ID: u32 = 7;
const EXTRA_CURRENCY_AMOUNT: u128 = 11;
const OUTBOUND_BODY: [u8; 16] = [0x42; 16];
const CALLBACK_BODY: [u8; 16] = [0x99; 16];
const DRAIN_METHOD_ID: u64 = 0x4c2d_0105_ca92_4509;
const PAYOUT_METHOD_ID: u64 = 0x0dc1_fee6_93a5_219c;
const EXTERNAL_EVENT_CLASS: u64 = 0x7122_8a3d_c179_9631;
const INTERNAL_EVENT_CLASS: u64 = 0x127e_58f9_3e9d_40f8;
const EVENT_READ_STATE_MARKER: [u8; 8] = *b"EVREAD1!";

struct ContractB;

impl Contract for ContractB {
    fn init(input: MessageInput<'_>) -> MessageOutput {
        let Caller::Internal(source) = input.caller else {
            return MessageOutput::success(StateUpdate::FullReplace(vec![0; STATE_BYTES]));
        };
        let mut transferred_extra = input.message.extra().iter();
        let Some(currency) = transferred_extra.next() else {
            return MessageOutput::failure(1102);
        };
        if !input.message.is_deploy()
            || input.message.value_main() != VALUE_MAIN
            || currency.currency_id != EXTRA_CURRENCY_ID
            || currency.amount != EXTRA_CURRENCY_AMOUNT
            || transferred_extra.next().is_some()
        {
            return MessageOutput::failure(1102);
        }

        let mut state = vec![0; STATE_BYTES];
        state[..8].copy_from_slice(&1_u64.to_le_bytes());
        let external_fields = match encode_event_fields(&[
            EventField { name: "count".into(), value: EventValue::U64(1) },
            EventField {
                name: "destination".into(),
                value: EventValue::Address(input.destination),
            },
            EventField { name: "value_main".into(), value: EventValue::U128(VALUE_MAIN) },
        ]) {
            Ok(fields) => fields,
            Err(_) => return MessageOutput::failure(1105),
        };
        let mut causal_a_state = vec![0; CAUSAL_A_STATE_BYTES];
        causal_a_state[..8].copy_from_slice(&1_u64.to_le_bytes());
        let internal_fields = match encode_event_fields(&[
            EventField { name: "count".into(), value: EventValue::U64(1) },
            EventField { name: "state".into(), value: EventValue::Data(causal_a_state) },
        ]) {
            Ok(fields) => fields,
            Err(_) => return MessageOutput::failure(1105),
        };

        let mut output = MessageOutput::success(StateUpdate::FullReplace(state));
        output.outbound_intents = (0..2)
            .map(|_| OutboundIntent {
                input_index: 0,
                destination: source,
                value_main: 1,
                fuel_fee_shells: u128::from(source.dapp_id != input.destination.dapp_id),
                extra: Vec::new(),
                method_id: PAYOUT_METHOD_ID,
                body: Vec::new(),
            })
            .collect();
        output.event_intents = vec![
            EventIntent {
                input_index: 0,
                kind: EventKind::External,
                event_class: EXTERNAL_EVENT_CLASS,
                event_name: "causal.a.forwarded".into(),
                topics: vec![input.destination.account_id.to_vec()],
                fields_binary: external_fields,
            },
            EventIntent {
                input_index: 0,
                kind: EventKind::Internal,
                event_class: INTERNAL_EVENT_CLASS,
                event_name: "causal.a.state".into(),
                topics: vec![1_u64.to_le_bytes().to_vec()],
                fields_binary: internal_fields,
            },
        ];
        output
    }
/* AVM_E2E_FAILURE_FLOW_RUSTFMT_BEGIN */    #[rustfmt::skip]/* AVM_E2E_FAILURE_FLOW_RUSTFMT_END */
    fn process_message(input: MessageInput<'_>) -> MessageOutput {
        let Caller::Internal(source) = input.caller else {
            return MessageOutput::failure(1100);
        };
        let Some(previous_count) = input
            .state
            .get(..8)
            .filter(|_| input.state.len() == STATE_BYTES)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u64::from_le_bytes)
        else {
            return MessageOutput::failure(1101);
        };
        if let CanonicalMessage::Internal(message) = input.message {
            if let Some(reads) = message.event_reads {
                return process_event_read(&input, previous_count, reads);
            }
        }
        let Some(next_count) = previous_count.checked_add(1) else {
            return MessageOutput::failure(1101);
        };
        let mut extra = input.message.extra().iter();
        let Some(currency) = extra.next() else {
            return MessageOutput::failure(1102);
        };
        if input.message.value_main() != VALUE_MAIN
            || currency.currency_id != EXTRA_CURRENCY_ID
            || currency.amount != EXTRA_CURRENCY_AMOUNT
            || extra.next().is_some()
        {
            return MessageOutput::failure(1102);
        }
        if input.message.body() != OUTBOUND_BODY {
            return MessageOutput::failure(1103);
        }
        if !input.message.has_callback_request() {
            return MessageOutput::failure(1104);
        }
        /* AVM_E2E_FAILURE_FLOW_HOOK_BEGIN */if let Some(output) = failure_flow(&input) { return output; } /* AVM_E2E_FAILURE_FLOW_HOOK_END */if input.message.method_id() == DRAIN_METHOD_ID {
            if previous_count != 1 {
                return MessageOutput::failure(1101);
            }
            let mut output = MessageOutput::success(StateUpdate::FullReplace(input.state.to_vec()));
            output.outbound_intents = vec![OutboundIntent {
                input_index: 0,
                destination: source,
                value_main: input.balance_main,
                fuel_fee_shells: 0,
                extra: input.extra.into_iter().collect(),
                method_id: PAYOUT_METHOD_ID,
                body: Vec::new(),
            }];
            output.callback_result_body = CALLBACK_BODY.to_vec();
            return output;
        }

        let mut state = Vec::with_capacity(STATE_BYTES);
        state.extend_from_slice(&next_count.to_le_bytes());
        state.extend_from_slice(&source.dapp_id);
        state.extend_from_slice(&source.account_id);
        state.extend_from_slice(&input.message.value_main().to_le_bytes());
        state.extend_from_slice(&1_u32.to_le_bytes());
        state.extend_from_slice(&currency.currency_id.to_le_bytes());
        state.extend_from_slice(&currency.amount.to_le_bytes());
        state.extend_from_slice(&input.message.method_id().to_le_bytes());
        state.extend_from_slice(
            &u32::try_from(input.message.body().len()).unwrap_or(0).to_le_bytes(),
        );
        state.extend_from_slice(&input.message.body()[..4]);
        if state.len() != STATE_BYTES {
            return MessageOutput::failure(1101);
        }
        let mut causal_a_state = vec![0; CAUSAL_A_STATE_BYTES];
        causal_a_state[..8].copy_from_slice(&next_count.to_le_bytes());
        let external_fields = match encode_event_fields(&[
            EventField { name: "count".into(), value: EventValue::U64(next_count) },
            EventField {
                name: "destination".into(),
                value: EventValue::Address(input.destination),
            },
            EventField { name: "value_main".into(), value: EventValue::U128(VALUE_MAIN) },
        ]) {
            Ok(fields) => fields,
            Err(_) => return MessageOutput::failure(1105),
        };
        let internal_fields = match encode_event_fields(&[
            EventField { name: "count".into(), value: EventValue::U64(next_count) },
            EventField { name: "state".into(), value: EventValue::Data(causal_a_state) },
        ]) {
            Ok(fields) => fields,
            Err(_) => return MessageOutput::failure(1105),
        };
        let mut output = MessageOutput::success(StateUpdate::FullReplace(state));
        output.callback_result_body = CALLBACK_BODY.to_vec();
        output.event_intents = vec![
            EventIntent {
                input_index: 0,
                kind: EventKind::External,
                event_class: EXTERNAL_EVENT_CLASS,
                event_name: "causal.a.forwarded".into(),
                topics: vec![input.destination.account_id.to_vec()],
                fields_binary: external_fields,
            },
            EventIntent {
                input_index: 0,
                kind: EventKind::Internal,
                event_class: INTERNAL_EVENT_CLASS,
                event_name: "causal.a.state".into(),
                topics: vec![next_count.to_le_bytes().to_vec()],
                fields_binary: internal_fields,
            },
        ];
        output
    }
}

fn process_event_read(
    input: &MessageInput<'_>,
    previous_count: u64,
    reads: InternalEventReadRequests<'_>,
) -> MessageOutput {
    let mut reads = reads.iter();
    let Some(read) = reads.next() else {
        return MessageOutput::failure(1106);
    };
    let expected_prefix = [1, 0, 0, 0, 0, 0, 1, 0];
    if previous_count != 0
        || input.state[8..].iter().any(|byte| *byte != 0)
        || reads.next().is_some()
        || read.event_class != INTERNAL_EVENT_CLASS
        || read.source == input.destination
        || input.message.is_callback()
        || input.message.has_callback_request()
        || input.message.value_main() != 0
        || input.message.extra().iter().next().is_some()
        || input.internal_event_results.get(..expected_prefix.len()) != Some(&expected_prefix)
        || input.message.body() != input.internal_event_results
    {
        return MessageOutput::failure(1106);
    }
    let Ok(result_len) = u32::try_from(input.internal_event_results.len()) else {
        return MessageOutput::failure(1106);
    };

    let mut state = vec![0; STATE_BYTES];
    state[..8].copy_from_slice(&1_u64.to_le_bytes());
    state[8..16].copy_from_slice(&EVENT_READ_STATE_MARKER);
    state[16..20].copy_from_slice(&result_len.to_le_bytes());
    state[20..52].copy_from_slice(&read.source.dapp_id);
    state[52..84].copy_from_slice(&read.source.account_id);
    state[84..92].copy_from_slice(&read.event_class.to_le_bytes());
    state[92..100].copy_from_slice(&event_read_checksum(input.internal_event_results).to_le_bytes());
    MessageOutput::success(StateUpdate::FullReplace(state))
}

fn event_read_checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |checksum, byte| {
        (checksum ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

avm_contract_sdk::export_message_contract!(ContractB, arena_bytes = 262_144);/* AVM_E2E_FAILURE_FLOW_HELPER_BEGIN */

#[inline(always)]
fn failure_flow(input: &MessageInput<'_>) -> Option<MessageOutput> {
    if option_env!("AVM_E2E_FAILURE_FLOW") != Some("1") {
        return None;
    }
    let avm_contract_sdk::CanonicalMessage::Internal(message) = input.message else {
        return Some(MessageOutput::failure(1104));
    };
    if input.message.method_id() == DRAIN_METHOD_ID {
        if message.callback_handler_method_id == Some(avm_contract_sdk::CODE_UPGRADE_METHOD_ID) {
            panic!("deterministic receiver trap fixture");
        }
        if message.callback_handler_method_id == Some(PAYOUT_METHOD_ID) {
            loop {
                core::hint::spin_loop();
            }
        }
    }
    if message.callback_handler_method_id == Some(avm_contract_sdk::CODE_UPGRADE_METHOD_ID) {
        let malformed_fields = match encode_event_fields(&[]) {
            Ok(fields) => fields,
            Err(_) => return Some(MessageOutput::failure(1105)),
        };
        let mut replacement_state = input.state.to_vec();
        let Some(first_byte) = replacement_state.first_mut() else {
            return Some(MessageOutput::failure(1101));
        };
        *first_byte ^= 0xff;
        let mut output = MessageOutput::success(StateUpdate::FullReplace(replacement_state));
        output.event_intents = vec![EventIntent {
            input_index: 0,
            kind: EventKind::External,
            event_class: EXTERNAL_EVENT_CLASS,
            event_name: String::new(),
            topics: Vec::new(),
            fields_binary: malformed_fields,
        }];
        return Some(output);
    }
    None
}/* AVM_E2E_FAILURE_FLOW_HELPER_END */
