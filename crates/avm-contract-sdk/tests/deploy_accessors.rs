use avm_contract_sdk::{decode_message_input, DecodeError};

// Independent vectors from avm-contracts.md's 288-byte ACDP header. These
// assertions exercise the guest convenience boundary, not host admission.
fn envelope(inline: bool) -> Vec<u8> {
    let mut bytes = vec![0; 288];
    bytes[..4].copy_from_slice(b"ACDP");
    bytes[8..40].fill(0x44);
    bytes[40..72].fill(0x21);
    bytes[136..168].fill(0x31);
    bytes[266..268].copy_from_slice(&1u16.to_le_bytes());
    bytes[268..272].copy_from_slice(&8u32.to_le_bytes());
    if inline {
        bytes[264..266].copy_from_slice(&4u16.to_le_bytes());
        bytes[272..276].copy_from_slice(&5u32.to_le_bytes());
        bytes[276..280].copy_from_slice(&2u32.to_le_bytes());
        bytes[280..284].copy_from_slice(&3u32.to_le_bytes());
        bytes[284..286].copy_from_slice(&2u16.to_le_bytes());
        bytes[232..264].fill(0x51);
        bytes[4] = 1 << 5;
        bytes.extend_from_slice(b"name");
    }
    bytes.push(0x61);
    bytes.extend_from_slice(&0x0123_4567_89ab_cdefu64.to_le_bytes());
    if inline {
        bytes.extend_from_slice(b"WASM!DDABI\0\0");
        bytes.extend_from_slice(&[0x71; 64]);
    }
    bytes
}

fn frame(body: &[u8], deploy: bool, external: bool) -> Vec<u8> {
    let mut message = vec![0; if external { 208 } else { 192 }];
    message[0] = u8::from(external);
    message[1] = if deploy { 2 } else { 0 };
    if external {
        message[1] |= 1; // Signed external envelope; ingress owns signature verification.
        message[64..96].fill(0x21);
        message[96..128].fill(0x22);
        message[128..160].fill(0x33);
        message[160..162].copy_from_slice(&64u16.to_le_bytes());
    } else {
        message[64..96].fill(0x11);
        message[96..128].fill(0x12);
        message[128..160].fill(0x21);
        message[160..192].fill(0x22);
    }
    message[4..8].copy_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    message[8..16].copy_from_slice(&1u64.to_le_bytes());
    message.extend_from_slice(body);
    if external {
        message.extend_from_slice(&[0x91; 64]);
    }
    let mut bytes = vec![0; 282];
    bytes[..4].copy_from_slice(b"ACMI");
    bytes[52] = u8::from(external);
    if external {
        bytes[124..156].fill(0x33);
    } else {
        bytes[60..92].fill(0x11);
        bytes[92..124].fill(0x12);
    }
    bytes[156..188].fill(0x21);
    bytes[188..220].fill(0x22);
    bytes[220..252].fill(0x44);
    bytes.extend_from_slice(&u32::try_from(message.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&message);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes
}

#[test]
fn constructor_accessors_borrow_only_the_requested_payload_for_both_callers() {
    for inline in [false, true] {
        let body = envelope(inline);
        if !inline {
            assert_eq!(body.len(), 297);
        }
        for external in [false, true] {
            let bytes = frame(&body, true, external);
            let input = decode_message_input(&bytes).unwrap();
            let deploy = input.deploy().unwrap();
            assert_eq!(deploy.package_id(), [0x21; 32]);
            assert_eq!(deploy.owner_id(), [0x31; 32]);
            assert_eq!(deploy.ui_name(), if inline { b"name".as_slice() } else { &[] });
            assert_eq!(deploy.salt(), &[0x61]);
            assert_eq!(deploy.init_data(), &0x0123_4567_89ab_cdefu64.to_le_bytes());
            let offset = if inline { 293 } else { 289 };
            assert_eq!(deploy.init_data().as_ptr(), input.message.body()[offset..].as_ptr());
            assert_eq!(input.message.body(), body);
        }
    }
}

#[test]
fn deploy_accessor_requires_the_deploy_flag() {
    let bytes = frame(&envelope(false), false, false);
    assert_eq!(decode_message_input(&bytes).unwrap().deploy(), Err(DecodeError::InvalidValue));
}

#[test]
fn every_truncated_envelope_and_a_trailing_byte_are_rejected() {
    let body = envelope(true);
    for len in 0..body.len() {
        let bytes = frame(&body[..len], true, false);
        assert!(decode_message_input(&bytes).unwrap().deploy().is_err(), "length {len}");
    }
    let mut body = body;
    body.push(0);
    let bytes = frame(&body, true, false);
    assert_eq!(decode_message_input(&bytes).unwrap().deploy(), Err(DecodeError::TrailingBytes));
}

#[test]
fn malformed_header_flags_keys_and_lengths_fail_closed() {
    for (offset, value) in [
        (0, b'X'),
        (4, 0x40),
        (5, 1),
        (6, 1),
        (7, 1),
        (286, 1),
        (287, 1),
        (200, 1),
        (232, 1),
        (4, 0x10),
        (4, 0x18),
    ] {
        let mut body = envelope(false);
        body[offset] = value;
        let bytes = frame(&body, true, false);
        assert!(
            decode_message_input(&bytes).unwrap().deploy().is_err(),
            "offset {offset}, value {value}"
        );
    }
    for offset in [268, 272, 276, 280] {
        let mut body = envelope(false);
        body[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let bytes = frame(&body, true, false);
        assert!(decode_message_input(&bytes).unwrap().deploy().is_err());
    }
}
