//! A contract declares the exact internal-event reads of an internal message
//! that it sends in `OutboundEventReads`, the last section of its
//! `MessageOutput` (flag bit 3) or `BatchOutput` (`flags` bit 0)
//! of the execution ABI. The expected bytes are written field by field from
//! that layout.

#![cfg(feature = "event-rings")]

use avm_contract_sdk::{
    encode_batch_output, encode_message_output, AccountAddress, BatchOutput, EncodeError,
    EventReadEntry, InternalEventReadRequest, MessageOutput, OutboundIntent, StateUpdate,
};

fn read(dapp: u8, account: u8, event_class: u64) -> InternalEventReadRequest {
    InternalEventReadRequest {
        source: AccountAddress { dapp_id: [dapp; 32], account_id: [account; 32] },
        event_class,
    }
}

fn outbound(method_id: u64) -> OutboundIntent {
    OutboundIntent {
        input_index: 0,
        destination: AccountAddress { dapp_id: [0x51; 32], account_id: [0x52; 32] },
        fuel_fee_shells: 3,
        extra: Vec::new(),
        method_id,
        body: Vec::new(),
    }
}

fn entry(outbound_index: u32, reads: Vec<InternalEventReadRequest>) -> EventReadEntry {
    EventReadEntry { outbound_index, reads }
}

/// `outbound_index`, then the `InternalEventReadRequestSection` of
/// the message format: `read_count`, `_reserved` and 72 bytes per read.
fn entry_bytes(outbound_index: u32, reads: &[InternalEventReadRequest]) -> Vec<u8> {
    let mut bytes = outbound_index.to_le_bytes().to_vec();
    bytes.extend_from_slice(&u16::try_from(reads.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    for read in reads {
        bytes.extend_from_slice(&read.source.dapp_id);
        bytes.extend_from_slice(&read.source.account_id);
        bytes.extend_from_slice(&read.event_class.to_le_bytes());
    }
    bytes
}

#[test]
fn message_output_declares_the_reads_of_its_outbound_messages_last() {
    let mut output = MessageOutput::success(StateUpdate::FullReplace(vec![1]));
    output.outbound_intents = vec![outbound(1), outbound(2), outbound(3)];
    let plain = encode_message_output(&output).unwrap();
    assert_eq!(plain[8], 1, "without reads flag bit 3 is clear and the section is absent");
    let first = vec![read(1, 1, 7)];
    // Ascending by DApp id, account id, then `event_class` as a number: 1 comes
    // before 256 although the little-endian bytes of 256 sort first.
    let third = vec![read(1, 2, 1), read(1, 2, 256), read(2, 0, 0)];
    output.outbound_event_reads = vec![entry(0, first.clone()), entry(2, third.clone())];
    let mut expected = plain;
    expected[8] |= 1 << 3;
    expected.extend_from_slice(&2_u32.to_le_bytes());
    expected.extend_from_slice(&entry_bytes(0, &first));
    expected.extend_from_slice(&entry_bytes(2, &third));
    assert_eq!(encode_message_output(&output).unwrap(), expected);
}

#[test]
fn batch_output_declares_the_reads_after_its_event_intents() {
    let mut output = BatchOutput {
        state_update: StateUpdate::FullReplace(vec![1]),
        status_codes: vec![0],
        outbound_intents: vec![outbound(1)],
        event_intents: Vec::new(),
        outbound_event_reads: Vec::new(),
    };
    let plain = encode_batch_output(&output).unwrap();
    assert_eq!(&plain[4..8], &[0, 0, 0, 0], "kind, flags 0 and reserved without reads");
    let reads = vec![read(3, 4, 5)];
    output.outbound_event_reads = vec![entry(0, reads.clone())];
    let mut expected = plain;
    expected[5] = 1;
    expected.extend_from_slice(&1_u32.to_le_bytes());
    expected.extend_from_slice(&entry_bytes(0, &reads));
    assert_eq!(encode_batch_output(&output).unwrap(), expected);
}

#[test]
fn reads_that_break_the_section_rules_are_not_encoded() {
    let mut output = MessageOutput::success(StateUpdate::FullReplace(Vec::new()));
    output.outbound_intents = vec![outbound(1), outbound(2)];
    for entries in [
        // No such outbound intent.
        vec![entry(2, vec![read(1, 1, 1)])],
        // An outbound intent with two entries.
        vec![entry(1, vec![read(1, 1, 1)]), entry(1, vec![read(1, 1, 2)])],
        // Decreasing outbound indexes.
        vec![entry(1, vec![read(1, 1, 1)]), entry(0, vec![read(1, 1, 1)])],
        // An entry without reads.
        vec![entry(0, Vec::new())],
        // A repeated read.
        vec![entry(0, vec![read(1, 1, 1), read(1, 1, 1)])],
        // Classes, accounts or DApps not ascending.
        vec![entry(0, vec![read(1, 1, 256), read(1, 1, 1)])],
        vec![entry(0, vec![read(1, 2, 0), read(1, 1, 9)])],
        vec![entry(0, vec![read(2, 0, 0), read(1, 9, 9)])],
    ] {
        output.outbound_event_reads = entries;
        assert_eq!(encode_message_output(&output), Err(EncodeError::InvalidEventReads));
    }
    let batch = BatchOutput {
        state_update: StateUpdate::FullReplace(Vec::new()),
        status_codes: vec![0],
        outbound_intents: vec![outbound(1)],
        event_intents: Vec::new(),
        outbound_event_reads: vec![entry(1, vec![read(1, 1, 1)])],
    };
    assert_eq!(encode_batch_output(&batch), Err(EncodeError::InvalidEventReads));
    // A failed call has no effects, so it declares no reads either.
    let mut failure = MessageOutput::failure(1000);
    failure.outbound_event_reads = vec![entry(0, vec![read(1, 1, 1)])];
    assert_eq!(encode_message_output(&failure), Err(EncodeError::EffectsOnFailure));
}
