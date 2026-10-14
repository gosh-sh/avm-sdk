use avm_contract_sdk::{
    encode_batch_output, encode_message_output, AccountAddress, BatchOutput, EncodeError,
    InternalEventReadRequest, MessageOutput, OutboundIntent, StateUpdate,
};

fn intent(reads: Vec<InternalEventReadRequest>) -> OutboundIntent {
    OutboundIntent {
        input_index: 0,
        destination: AccountAddress { dapp_id: [1; 32], account_id: [2; 32] },
        value_main: 0,
        fuel_fee_shells: 3,
        extra: Vec::new(),
        internal_event_reads: reads,
        method_id: 4,
        body: vec![5],
    }
}

#[test]
fn no_read_output_retains_the_original_wire_and_reads_use_explicit_version() {
    let mut output = MessageOutput::success(StateUpdate::FullReplace(Vec::new()));
    output.outbound_intents.push(intent(Vec::new()));
    let original = encode_message_output(&output).unwrap();
    assert_eq!(&original[..4], b"ACMO");
    assert_eq!(original.len(), 148);
    output.outbound_intents[0].internal_event_reads.push(InternalEventReadRequest {
        source: AccountAddress { dapp_id: [7; 32], account_id: [8; 32] },
        event_class: 0x0102030405060708,
    });
    let extended = encode_message_output(&output).unwrap();
    assert_eq!(&extended[..4], b"ACM2");
    assert_eq!(extended.len(), original.len() + 76);
    let start = 25 + 102; // output prefix, then intent fields through extra_count
    assert_eq!(&extended[start..start + 4], &[1, 0, 0, 0]);
    assert_eq!(&extended[start + 4..start + 36], &[7; 32]);
    assert_eq!(&extended[start + 36..start + 68], &[8; 32]);
    assert_eq!(&extended[start + 68..start + 76], &0x0102030405060708u64.to_le_bytes());
    let mut downgraded = extended.clone();
    downgraded[..4].copy_from_slice(b"ACMO");
    downgraded.drain(start..start + 76);
    assert_eq!(downgraded, original);
    let batch = BatchOutput {
        state_update: StateUpdate::FullReplace(Vec::new()),
        status_codes: vec![0],
        outbound_intents: output.outbound_intents,
        event_intents: Vec::new(),
    };
    assert_eq!(&encode_batch_output(&batch).unwrap()[..4], b"ACB2");
}

#[test]
fn duplicate_and_unsorted_reads_reject_without_reordering_guest_intent() {
    let mut output = MessageOutput::success(StateUpdate::FullReplace(Vec::new()));
    let read = InternalEventReadRequest {
        source: AccountAddress { dapp_id: [7; 32], account_id: [8; 32] },
        event_class: 9,
    };
    output.outbound_intents.push(intent(vec![read, read]));
    assert_eq!(encode_message_output(&output), Err(EncodeError::InvalidEventReads));
    output.outbound_intents[0].internal_event_reads[1].event_class = 8;
    assert_eq!(encode_message_output(&output), Err(EncodeError::InvalidEventReads));
    output.outbound_intents[0].internal_event_reads.reverse();
    assert!(encode_message_output(&output).is_ok());
}
