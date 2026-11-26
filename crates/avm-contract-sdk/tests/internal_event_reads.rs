//! The reader of internal-event read results, the results of the reads that
//! a message declares (`internal_event_results` of the input). The byte
//! vectors are built here by hand from those layouts, independently of the
//! SDK codecs.

#![cfg(feature = "event-rings")]

use avm_contract_sdk::{
    decode_event_fields, decode_internal_event_results, decode_message_input, encode_event_fields,
    AccountAddress, DecodeError, EventField, EventValue, InternalEventReadRequest,
};

const EVENT_CLASS: u64 = 0x127e_58f9_3e9d_40f8;

fn address(dapp: u8, account: u8) -> AccountAddress {
    AccountAddress { dapp_id: [dapp; 32], account_id: [account; 32] }
}

fn push_address(bytes: &mut Vec<u8>, address: AccountAddress) {
    bytes.extend_from_slice(&address.dapp_id);
    bytes.extend_from_slice(&address.account_id);
}

/// A canonical internal message from `source` to
/// `destination` that declares `reads` in its `InternalEventReadRequestSection`.
fn internal_message(
    source: AccountAddress,
    destination: AccountAddress,
    reads: &[(AccountAddress, u64)],
) -> Vec<u8> {
    let body = b"read";
    let mut bytes = vec![0_u8; 176];
    bytes[0] = 0; // kind = internal
    bytes[1] = if reads.is_empty() { 0 } else { 1 << 3 }; // has_internal_event_reads
    bytes[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    bytes[8..16].copy_from_slice(&0x5eed_u64.to_le_bytes()); // method_id
    bytes[32..40].copy_from_slice(&7_u64.to_le_bytes()); // created_lt
    bytes[40..44].copy_from_slice(&1_800_000_000_u32.to_le_bytes()); // created_at
    bytes[48..80].copy_from_slice(&source.dapp_id);
    bytes[80..112].copy_from_slice(&source.account_id);
    bytes[112..144].copy_from_slice(&destination.dapp_id);
    bytes[144..176].copy_from_slice(&destination.account_id);
    if !reads.is_empty() {
        bytes.extend_from_slice(&(reads.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        for (src, event_class) in reads {
            push_address(&mut bytes, *src);
            bytes.extend_from_slice(&event_class.to_le_bytes());
        }
    }
    bytes.extend_from_slice(body);
    bytes
}

/// A canonical signed external message to `destination`.
fn external_message(destination: AccountAddress) -> Vec<u8> {
    let mut bytes = vec![0_u8; 192];
    bytes[0] = 1; // kind = external
    bytes[1] = 1; // signed
    bytes[8..16].copy_from_slice(&0x5eed_u64.to_le_bytes());
    bytes[48..80].copy_from_slice(&destination.dapp_id);
    bytes[80..112].copy_from_slice(&destination.account_id);
    bytes[112..144].copy_from_slice(&[0x77; 32]); // pubkey
    bytes[144..146].copy_from_slice(&64_u16.to_le_bytes()); // sig_len
    bytes.extend_from_slice(&[0x55; 64]);
    bytes
}

/// A `MessageInput` that carries `message` and `results`.
fn message_input(message: &[u8], caller: Option<AccountAddress>, results: &[u8]) -> Vec<u8> {
    let destination = address(0x30, 0x31);
    let mut bytes = b"ACMI".to_vec();
    bytes.extend_from_slice(&9_u64.to_le_bytes()); // block_lt
    bytes.extend_from_slice(&10_u32.to_le_bytes()); // block_now
    bytes.extend_from_slice(&11_u32.to_le_bytes()); // block_seq_no
    bytes.extend_from_slice(&[0x12; 32]); // block_rand
    match caller {
        Some(source) => {
            bytes.push(0);
            bytes.extend_from_slice(&[0; 7]);
            push_address(&mut bytes, source);
            bytes.extend_from_slice(&[0; 32]);
        }
        None => {
            bytes.push(1);
            bytes.extend_from_slice(&[0; 7]);
            bytes.extend_from_slice(&[0; 64]);
            bytes.extend_from_slice(&[0x77; 32]);
        }
    }
    push_address(&mut bytes, destination);
    bytes.extend_from_slice(&[0x44; 32]); // selected_code_id
    bytes.push(0); // FullReplace
    bytes.extend_from_slice(&[0; 7]);
    bytes.extend_from_slice(&0_u16.to_le_bytes()); // extra_count
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // state_len
    bytes.extend_from_slice(&(message.len() as u32).to_le_bytes());
    bytes.extend_from_slice(message);
    bytes.extend_from_slice(&(results.len() as u32).to_le_bytes());
    bytes.extend_from_slice(results);
    bytes
}

/// One readable event of the `InternalEventReadResults` frame.
struct Event<'a> {
    lt: u64,
    seq: u32,
    event_class: u64,
    name: &'a [u8],
    topics: &'a [&'a [u8]],
    fields: &'a [u8],
}

/// The `InternalEventReadResults` frame of `results`: each entry is a request
/// index with its events, oldest first.
fn results_frame(results: &[(u16, &[Event<'_>])]) -> Vec<u8> {
    let mut bytes = (results.len() as u16).to_le_bytes().to_vec();
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    for (request_index, events) in results {
        bytes.extend_from_slice(&request_index.to_le_bytes());
        bytes.push(events.len() as u8);
        bytes.push(0);
        for event in *events {
            bytes.extend_from_slice(&[event.lt as u8; 32]); // tx_hash
            bytes.extend_from_slice(&event.lt.to_le_bytes());
            bytes.extend_from_slice(&(1_800_000_000 + event.lt as u32).to_le_bytes()); // time
            bytes.extend_from_slice(&event.seq.to_le_bytes());
            bytes.extend_from_slice(&event.event_class.to_le_bytes());
            bytes.extend_from_slice(&(event.name.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&(event.topics.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&(event.fields.len() as u32).to_le_bytes());
            bytes.extend_from_slice(event.name);
            for topic in event.topics {
                bytes.extend_from_slice(&(topic.len() as u16).to_le_bytes());
                bytes.extend_from_slice(topic);
            }
            bytes.extend_from_slice(event.fields);
        }
    }
    bytes
}

/// The `AvmEventFieldList` with one `u64` field `value`.
fn value_field(value: u64) -> Vec<u8> {
    let mut bytes = 1_u16.to_le_bytes().to_vec();
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&5_u16.to_le_bytes());
    bytes.push(2); // u64
    bytes.extend_from_slice(&8_u16.to_le_bytes());
    bytes.extend_from_slice(b"value");
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes
}

#[test]
fn message_reads_pair_each_declared_key_with_its_ring_newest_last() {
    let parameters = address(0x10, 0x11);
    let missing = address(0x20, 0x21);
    let older = value_field(5);
    let newer = value_field(9);
    let reads = [(parameters, EVENT_CLASS), (missing, EVENT_CLASS)];
    let message = internal_message(address(0x01, 0x02), address(0x30, 0x31), &reads);
    let events = [
        Event {
            lt: 5,
            seq: 0,
            event_class: EVENT_CLASS,
            name: b"params.set",
            topics: &[b"fee"],
            fields: &older,
        },
        Event {
            lt: 9,
            seq: 1,
            event_class: EVENT_CLASS,
            name: b"params.set",
            topics: &[],
            fields: &newer,
        },
    ];
    let results = results_frame(&[(0, &events), (1, &[])]);
    let input_bytes = message_input(&message, Some(address(0x01, 0x02)), &results);
    let input = decode_message_input(&input_bytes).expect("canonical input");

    let read = input.internal_event_reads().expect("results of the declared reads");
    assert_eq!(read.len(), 2);
    assert_eq!(
        read[0].0,
        InternalEventReadRequest { source: parameters, event_class: EVENT_CLASS }
    );
    assert_eq!(read[1].0, InternalEventReadRequest { source: missing, event_class: EVENT_CLASS });
    assert!(read[1].1.is_empty(), "a key that holds no ring reads no event");
    let ring = &read[0].1;
    assert_eq!(ring.len(), 2);
    let newest = ring.last().expect("newest event");
    assert_eq!(newest.lt, 9);
    assert_eq!(newest.tx_hash, [9; 32]);
    assert_eq!(newest.time, 1_800_000_009);
    assert_eq!(newest.seq, 1);
    assert_eq!(newest.event_class, EVENT_CLASS);
    assert_eq!(newest.event_name, "params.set");
    assert!(newest.topics.is_empty());
    assert_eq!(
        decode_event_fields(newest.fields_binary).expect("field list"),
        vec![EventField { name: "value".into(), value: EventValue::U64(9) }],
    );
    assert_eq!(ring[0].lt, 5);
    assert_eq!(ring[0].topics, vec![&b"fee"[..]]);
    assert_eq!(ring[0].fields_binary, &older[..]);

    // The same frame without the message context: request indexes and events.
    let frame = decode_internal_event_results(&results).expect("results frame");
    assert_eq!(frame.len(), 2);
    assert_eq!((frame[0].request_index, frame[0].events.len()), (0, 2));
    assert_eq!((frame[1].request_index, frame[1].events.len()), (1, 0));
    assert_eq!(frame[0].events, read[0].1);
}

#[test]
fn input_without_declared_reads_reads_nothing() {
    let empty = results_frame(&[]);
    assert_eq!(empty, [0, 0, 0, 0]);
    let internal = internal_message(address(0x01, 0x02), address(0x30, 0x31), &[]);
    for results in [&empty[..], &[][..]] {
        let bytes = message_input(&internal, Some(address(0x01, 0x02)), results);
        let input = decode_message_input(&bytes).expect("internal input");
        assert_eq!(input.internal_event_reads(), Ok(Vec::new()));
        let bytes = message_input(&external_message(address(0x30, 0x31)), None, results);
        let input = decode_message_input(&bytes).expect("external input");
        assert_eq!(input.internal_event_reads(), Ok(Vec::new()));
        assert_eq!(decode_internal_event_results(results), Ok(Vec::new()));
    }
}

#[test]
fn results_that_do_not_answer_the_declared_reads_are_rejected() {
    let first = address(0x10, 0x11);
    let second = address(0x20, 0x21);
    let reads = [(first, EVENT_CLASS), (second, EVENT_CLASS)];
    let message = internal_message(address(0x01, 0x02), address(0x30, 0x31), &reads);
    let decode = |results: &[u8]| {
        let bytes = message_input(&message, Some(address(0x01, 0x02)), results);
        decode_message_input(&bytes).expect("canonical input").internal_event_reads().map(|_| ())
    };
    assert_eq!(decode(&results_frame(&[(0, &[]), (1, &[])])), Ok(()));
    assert_eq!(decode(&results_frame(&[(0, &[])])), Err(DecodeError::InvalidLength));
    assert_eq!(decode(&results_frame(&[])), Err(DecodeError::InvalidLength));
    assert_eq!(decode(&results_frame(&[(1, &[]), (0, &[])])), Err(DecodeError::InvalidValue));
    assert_eq!(decode(&results_frame(&[(0, &[]), (0, &[])])), Err(DecodeError::InvalidValue));

    // Results for a message that declares no read.
    let external =
        message_input(&external_message(address(0x30, 0x31)), None, &results_frame(&[(0, &[])]));
    let input = decode_message_input(&external).expect("external input");
    assert_eq!(input.internal_event_reads(), Err(DecodeError::InvalidLength));
}

#[test]
fn malformed_results_frames_are_rejected() {
    let fields = value_field(1);
    let event = [Event {
        lt: 1,
        seq: 0,
        event_class: 3,
        name: b"params.set",
        topics: &[b"t"],
        fields: &fields,
    }];
    let valid = results_frame(&[(0, &event)]);
    assert_eq!(decode_internal_event_results(&valid).map(|frame| frame.len()), Ok(1));

    let mut reserved = valid.clone();
    reserved[2] = 1;
    assert_eq!(decode_internal_event_results(&reserved), Err(DecodeError::NonZeroReserved));
    let mut result_reserved = valid.clone();
    result_reserved[7] = 1;
    assert_eq!(decode_internal_event_results(&result_reserved), Err(DecodeError::NonZeroReserved));
    for cut in [1, 3, 5, 7, 40, valid.len() - 1] {
        assert_eq!(
            decode_internal_event_results(&valid[..cut]),
            Err(DecodeError::UnexpectedEof),
            "a frame cut at {cut} bytes",
        );
    }
    let mut trailing = valid.clone();
    trailing.push(0);
    assert_eq!(decode_internal_event_results(&trailing), Err(DecodeError::TrailingBytes));
    // The event name starts after the 8-byte frame and result headers and the
    // 64-byte fixed event header.
    let mut name = valid.clone();
    name[8 + 64] = 0xff;
    assert_eq!(decode_internal_event_results(&name), Err(DecodeError::InvalidValue));
}

#[test]
fn event_fields_decode_every_value_kind_and_reject_malformed_lists() {
    let fields = vec![
        EventField { name: "a_null".into(), value: EventValue::Null },
        EventField { name: "b_bool".into(), value: EventValue::Bool(true) },
        EventField { name: "c_u64".into(), value: EventValue::U64(u64::MAX) },
        EventField { name: "d_i64".into(), value: EventValue::I64(-3) },
        EventField { name: "e_u128".into(), value: EventValue::U128(u128::MAX - 1) },
        EventField { name: "f_i128".into(), value: EventValue::I128(-5) },
        EventField { name: "g_text".into(), value: EventValue::Text("pay".into()) },
        EventField { name: "h_data".into(), value: EventValue::Data(vec![1, 2, 3]) },
        EventField { name: "i_address".into(), value: EventValue::Address(address(0x61, 0x62)) },
    ];
    let bytes = encode_event_fields(&fields).expect("canonical field list");
    assert_eq!(decode_event_fields(&bytes), Ok(fields));
    assert_eq!(decode_event_fields(&[0, 0, 0, 0]), Ok(Vec::new()));

    let field = |name: &[u8], kind: u8, value: &[u8]| {
        let mut bytes = (name.len() as u16).to_le_bytes().to_vec();
        bytes.push(kind);
        bytes.extend_from_slice(&(value.len() as u16).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(value);
        bytes
    };
    let list = |count: u16, entries: &[Vec<u8>]| {
        let mut bytes = count.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    };
    let one = field(b"a", 2, &7_u64.to_le_bytes());
    assert_eq!(
        decode_event_fields(&list(1, std::slice::from_ref(&one))),
        Ok(vec![EventField { name: "a".into(), value: EventValue::U64(7) }]),
    );
    let mut reserved = list(1, std::slice::from_ref(&one));
    reserved[3] = 1;
    assert_eq!(decode_event_fields(&reserved), Err(DecodeError::NonZeroReserved));
    assert_eq!(
        decode_event_fields(&list(2, &[one.clone(), one.clone()])),
        Err(DecodeError::Unsorted)
    );
    assert_eq!(
        decode_event_fields(&list(2, &[field(b"b", 0, &[]), field(b"a", 0, &[])])),
        Err(DecodeError::Unsorted),
    );
    assert_eq!(decode_event_fields(&list(1, &[field(b"a", 9, &[])])), Err(DecodeError::UnknownTag));
    for (kind, value) in
        [(0, &[1][..]), (1, &[2][..]), (2, &[0; 7][..]), (4, &[0; 15][..]), (8, &[0; 63][..])]
    {
        assert_eq!(
            decode_event_fields(&list(1, &[field(b"a", kind, value)])),
            Err(DecodeError::InvalidValue),
            "kind {kind} with {} value bytes",
            value.len(),
        );
    }
    assert_eq!(
        decode_event_fields(&list(1, &[field(b"a", 6, &[0xff])])),
        Err(DecodeError::InvalidValue)
    );
    assert_eq!(
        decode_event_fields(&list(1, &[field(&[0xff], 0, &[])])),
        Err(DecodeError::InvalidValue)
    );
    assert_eq!(
        decode_event_fields(&list(2, std::slice::from_ref(&one))),
        Err(DecodeError::UnexpectedEof)
    );
    let mut trailing = list(1, &[one]);
    trailing.push(0);
    assert_eq!(decode_event_fields(&trailing), Err(DecodeError::TrailingBytes));
}
