use avm_contract_sdk::{
    decode_batch_input, decode_message_input, encode_batch_output, encode_event_fields,
    encode_message_output, AccountAddress, BatchOutput, CallbackIntent, Caller, CurrencyAmount,
    EventField, EventIntent, EventKind, EventValue, MessageOutput, OutboundIntent, StatePatch,
    StateUpdate,
};

#[test]
fn acmi_decode_and_acmo_encode_cover_all_effect_sections() {
    let destination = address(0x30, 0x31);
    let source = address(0x20, 0x21);
    let message = internal_message(source, destination, 0x04, 77, b"call", true);
    let input_bytes = message_input(&message, source, destination, 1);
    let input = decode_message_input(&input_bytes).expect("canonical ACMI must decode");

    assert_eq!(input.context.block_lt, 9);
    assert_eq!(input.context.block_time, 10);
    assert_eq!(input.context.block_seq_no, 11);
    assert_eq!(input.context.block_rand, [0x12; 32]);
    assert_eq!(input.caller, Caller::Internal(source));
    assert_eq!(input.destination, destination);
    assert_eq!(input.selected_code_id, [0x44; 32]);
    assert_eq!(input.balance_main, 500);
    assert_eq!(input.state, b"old-state");
    assert_eq!(input.message.method_id(), 77);
    assert_eq!(input.message.body(), b"call");
    assert!(input.message.has_callback_request());
    let event_reads = match input.message {
        avm_contract_sdk::CanonicalMessage::Internal(message) => {
            message.event_reads.expect("declared event read")
        }
        avm_contract_sdk::CanonicalMessage::External(_) => panic!("expected internal message"),
    };
    assert_eq!(event_reads.len(), 1);
    assert_eq!(event_reads.into_iter().next().unwrap().event_class, 92);
    assert_eq!(
        input.extra.into_iter().collect::<Vec<_>>(),
        vec![
            CurrencyAmount { currency_id: 2, amount: 3 },
            CurrencyAmount { currency_id: 9, amount: 10 },
        ]
    );

    let fields = encode_event_fields(&[
        EventField { name: "count".into(), value: EventValue::U64(4) },
        EventField { name: "owner".into(), value: EventValue::Address(source) },
    ])
    .expect("canonical fields must encode");
    let output = MessageOutput {
        status_code: 0,
        state_update: Some(StateUpdate::DataPatchList(vec![StatePatch {
            offset: 3,
            old_len: 2,
            new_bytes: b"XY".to_vec(),
        }])),
        callback_result_body: b"result".to_vec(),
        outbound_intents: vec![OutboundIntent {
            input_index: 0,
            destination: address(0x50, 0x51),
            value_main: 7,
            fuel_fee_shells: 8,
            extra: vec![CurrencyAmount { currency_id: 2, amount: 9 }],
            method_id: 88,
            body: b"out".to_vec(),
        }],
        callback_intents: vec![CallbackIntent { outbound_index: 0, handler_method_id: 99 }],
        event_intents: vec![EventIntent {
            input_index: 0,
            kind: EventKind::Internal,
            event_class: 123,
            event_name: "counter.changed".into(),
            topics: vec![b"topic".to_vec()],
            fields_binary: fields,
        }],
        code_upgrade_intent: None,
    };
    let encoded = encode_message_output(&output).expect("canonical ACMO must encode");
    assert_eq!(&encoded[..4], b"ACMO");
    assert_eq!(u32::from_le_bytes(encoded[4..8].try_into().unwrap()), 0);
    assert_eq!(encoded[8], 1);
    assert_eq!(encoded[9], 1);
    assert_eq!(&encoded[10..13], &[0, 0, 0]);
    assert!(encoded.windows(b"counter.changed".len()).any(|bytes| bytes == b"counter.changed"));
    assert!(encoded.ends_with(&output.event_intents[0].fields_binary));
}

#[test]
fn acbi_decode_and_acbo_encode_preserve_indexes_and_statuses() {
    let destination = address(0x60, 0x61);
    let first = internal_message(address(0x70, 0x71), destination, 0, 1, b"one", false);
    let second = internal_message(address(0x72, 0x73), destination, 0, 2, b"two", false);
    let input_bytes = batch_input(destination, &[first, second]);
    let input = decode_batch_input(&input_bytes).expect("canonical ACBI must decode");
    assert_eq!(input.message_count(), 2);
    assert_eq!(
        input
            .messages()
            .map(|message| message.expect("validated message").method_id())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );

    let output = BatchOutput {
        state_update: StateUpdate::FullReplace(b"batch-state".to_vec()),
        status_codes: vec![0, 1000],
        outbound_intents: vec![OutboundIntent {
            input_index: 0,
            destination: address(0x80, 0x81),
            value_main: 1,
            fuel_fee_shells: 0,
            extra: Vec::new(),
            method_id: 3,
            body: Vec::new(),
        }],
        event_intents: vec![EventIntent {
            input_index: 0,
            kind: EventKind::External,
            event_class: 4,
            event_name: "batch.done".into(),
            topics: Vec::new(),
            fields_binary: encode_event_fields(&[]).unwrap(),
        }],
    };
    let encoded = encode_batch_output(&output).expect("canonical ACBO must encode");
    assert_eq!(&encoded[..4], b"ACBO");
    assert_eq!(encoded[4], 0);
    assert_eq!(&encoded[5..8], &[0, 0, 0]);
    assert_eq!(u32::from_le_bytes(encoded[8..12].try_into().unwrap()), 11);
    assert_eq!(&encoded[12..23], b"batch-state");
    assert_eq!(u32::from_le_bytes(encoded[23..27].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(encoded[27..31].try_into().unwrap()), 0);
    assert_eq!(u32::from_le_bytes(encoded[31..35].try_into().unwrap()), 1000);
}

#[test]
fn malformed_or_noncanonical_frames_fail_closed() {
    assert!(decode_message_input(b"ACMI").is_err());
    assert!(decode_batch_input(b"ACBO").is_err());
    assert!(encode_message_output(&MessageOutput::failure(7)).is_err());
    assert!(encode_event_fields(&[
        EventField { name: "z".into(), value: EventValue::Null },
        EventField { name: "a".into(), value: EventValue::Null },
    ])
    .is_err());
}

fn address(dapp: u8, account: u8) -> AccountAddress {
    AccountAddress { dapp_id: [dapp; 32], account_id: [account; 32] }
}

fn internal_message(
    source: AccountAddress,
    destination: AccountAddress,
    flags: u8,
    method_id: u64,
    body: &[u8],
    with_event_read: bool,
) -> Vec<u8> {
    let mut bytes = vec![0; 192];
    bytes[0] = 0;
    bytes[1] = flags | if with_event_read { 0x08 } else { 0 };
    bytes[2..4].copy_from_slice(&1u16.to_le_bytes());
    bytes[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    bytes[8..16].copy_from_slice(&method_id.to_le_bytes());
    bytes[16..32].copy_from_slice(&5u128.to_le_bytes());
    bytes[32..48].copy_from_slice(&6u128.to_le_bytes());
    bytes[48..56].copy_from_slice(&7u64.to_le_bytes());
    bytes[56..60].copy_from_slice(&8u32.to_le_bytes());
    bytes[64..96].copy_from_slice(&source.dapp_id);
    bytes[96..128].copy_from_slice(&source.account_id);
    bytes[128..160].copy_from_slice(&destination.dapp_id);
    bytes[160..192].copy_from_slice(&destination.account_id);
    if flags & 0x04 != 0 {
        bytes.extend_from_slice(&99u64.to_le_bytes());
    }
    if with_event_read {
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&[0x90; 32]);
        bytes.extend_from_slice(&[0x91; 32]);
        bytes.extend_from_slice(&92u64.to_le_bytes());
    }
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&3u128.to_le_bytes());
    bytes.extend_from_slice(body);
    bytes
}

fn message_input(
    message: &[u8],
    source: AccountAddress,
    destination: AccountAddress,
    mode: u8,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"ACMI");
    bytes.extend_from_slice(&9u64.to_le_bytes());
    bytes.extend_from_slice(&10u32.to_le_bytes());
    bytes.extend_from_slice(&11u32.to_le_bytes());
    bytes.extend_from_slice(&[0x12; 32]);
    bytes.push(0);
    bytes.extend_from_slice(&[0; 7]);
    bytes.extend_from_slice(&source.dapp_id);
    bytes.extend_from_slice(&source.account_id);
    bytes.extend_from_slice(&[0; 32]);
    bytes.extend_from_slice(&destination.dapp_id);
    bytes.extend_from_slice(&destination.account_id);
    bytes.extend_from_slice(&[0x44; 32]);
    bytes.push(mode);
    bytes.extend_from_slice(&[0; 7]);
    bytes.extend_from_slice(&500u128.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&3u128.to_le_bytes());
    bytes.extend_from_slice(&9u32.to_le_bytes());
    bytes.extend_from_slice(&10u128.to_le_bytes());
    bytes.extend_from_slice(&9u32.to_le_bytes());
    bytes.extend_from_slice(b"old-state");
    bytes.extend_from_slice(&(message.len() as u32).to_le_bytes());
    bytes.extend_from_slice(message);
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    bytes
}

fn batch_input(destination: AccountAddress, messages: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"ACBI");
    bytes.extend_from_slice(&9u64.to_le_bytes());
    bytes.extend_from_slice(&10u32.to_le_bytes());
    bytes.extend_from_slice(&11u32.to_le_bytes());
    bytes.extend_from_slice(&[0x12; 32]);
    bytes.extend_from_slice(&destination.dapp_id);
    bytes.extend_from_slice(&destination.account_id);
    bytes.extend_from_slice(&[0x44; 32]);
    bytes.push(0);
    bytes.extend_from_slice(&[0; 7]);
    bytes.extend_from_slice(&500u128.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(b"old");
    bytes.extend_from_slice(&(messages.len() as u32).to_le_bytes());
    let mut offset = 0u32;
    bytes.extend_from_slice(&offset.to_le_bytes());
    for message in messages {
        offset += message.len() as u32;
        bytes.extend_from_slice(&offset.to_le_bytes());
    }
    for message in messages {
        bytes.extend_from_slice(message);
    }
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes
}
