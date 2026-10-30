use avm_contract_sdk::{
    decode_batch_input, decode_message_input, encode_message_output, AccountAddress,
    CanonicalMessage, CurrencyAmount, DecodeError, MessageOutput, OutboundIntent, StateUpdate,
};

const CURRENCIES: [(u32, u128); 3] = [(0, u128::MAX), (2, 19), (u32::MAX, 0)];

fn currencies(bytes: &mut Vec<u8>) {
    for (id, amount) in CURRENCIES {
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
    }
}

// These fixtures spell out the adopted wire layout independently of SDK encoders.
fn wire(external: bool) -> Vec<u8> {
    let mut bytes = vec![0; if external { 192 } else { 176 }];
    bytes[0] = u8::from(external);
    bytes[1] = u8::from(external);
    bytes[2..4].copy_from_slice(&3u16.to_le_bytes());
    bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
    bytes[8..16].copy_from_slice(&91u64.to_le_bytes());
    bytes[16..32].copy_from_slice(&17u128.to_le_bytes());
    bytes[32..40].copy_from_slice(&23u64.to_le_bytes());
    bytes[40..44].copy_from_slice(&29u32.to_le_bytes());
    if external {
        bytes[44..48].copy_from_slice(&31u32.to_le_bytes());
        bytes[48..80].fill(3);
        bytes[80..112].fill(4);
        bytes[112..144].fill(5);
        bytes[144..146].copy_from_slice(&64u16.to_le_bytes());
    } else {
        bytes[48..80].fill(1);
        bytes[80..112].fill(2);
        bytes[112..144].fill(3);
        bytes[144..176].fill(4);
    }
    currencies(&mut bytes);
    bytes.extend_from_slice(&[7, 8, 9]);
    if external {
        bytes.extend_from_slice(&[6; 64]);
    }
    bytes
}

fn context(bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&37u64.to_le_bytes());
    bytes.extend_from_slice(&41u32.to_le_bytes());
    bytes.extend_from_slice(&43u32.to_le_bytes());
    bytes.extend_from_slice(&[47; 32]);
}

fn account(bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&[3; 32]);
    bytes.extend_from_slice(&[4; 32]);
    bytes.extend_from_slice(&[53; 32]);
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&3u16.to_le_bytes());
    currencies(bytes);
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&[59, 61, 67]);
}

fn message_frame(external: bool) -> Vec<u8> {
    let mut bytes = b"ACMI".to_vec();
    context(&mut bytes);
    bytes.push(u8::from(external));
    bytes.extend_from_slice(&[0; 7]);
    bytes.extend_from_slice(&[if external { 0 } else { 1 }; 32]);
    bytes.extend_from_slice(&[if external { 0 } else { 2 }; 32]);
    bytes.extend_from_slice(&[if external { 5 } else { 0 }; 32]);
    account(&mut bytes);
    let message = wire(external);
    bytes.extend_from_slice(&(message.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&message);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes
}

#[test]
fn ecc_only_message_frames_preserve_every_currency_and_wire_field() {
    for external in [false, true] {
        let bytes = message_frame(external);
        let input = decode_message_input(&bytes).expect("adopted ECC-only ACMI");
        let actual: Vec<_> = input.extra.into_iter().map(|v| (v.currency_id, v.amount)).collect();
        assert_eq!(actual, CURRENCIES);
        assert_eq!(input.state, &[59, 61, 67]);
        assert_eq!(input.message.fuel_fee_shells(), 17);
        assert_eq!(input.message.method_id(), 91);
        assert_eq!(input.message.body(), &[7, 8, 9]);
        let actual: Vec<_> =
            input.message.extra().into_iter().map(|v| (v.currency_id, v.amount)).collect();
        assert_eq!(actual, CURRENCIES);
        match input.message {
            CanonicalMessage::Internal(message) => {
                assert_eq!(message.created_lt, 23);
                assert_eq!(message.created_at, 29);
                assert_eq!(
                    message.source,
                    AccountAddress { dapp_id: [1; 32], account_id: [2; 32] }
                );
            }
            CanonicalMessage::External(message) => {
                assert_eq!((message.nonce, message.created_at, message.expire_at), (23, 29, 31));
                assert_eq!(message.pubkey, [5; 32]);
                assert_eq!(message.signature, &[6; 64]);
            }
        }
    }
}

#[test]
fn ecc_only_batch_frame_has_no_unnamed_balance_slot() {
    let message = wire(false);
    let mut bytes = b"ACBI".to_vec();
    context(&mut bytes);
    account(&mut bytes);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(message.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&message);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let input = decode_batch_input(&bytes).expect("adopted ECC-only ACBI");
    assert_eq!(
        input.extra.into_iter().map(|v| (v.currency_id, v.amount)).collect::<Vec<_>>(),
        CURRENCIES
    );
    assert_eq!(input.message_count(), 1);
    assert_eq!(input.messages().next().unwrap().unwrap().canonical_bytes(), message);
}

#[test]
fn ecc_only_frames_reject_truncation_duplicate_currency_and_reserved_bytes() {
    for external in [false, true] {
        let bytes = message_frame(external);
        for end in 0..bytes.len() {
            assert!(decode_message_input(&bytes[..end]).is_err(), "prefix {end}");
        }
        let mut duplicate = bytes.clone();
        // ACMI account currency entries start at byte 262; the second id is at 282.
        duplicate[282..286].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode_message_input(&duplicate), Err(DecodeError::Unsorted));
        let mut reserved = bytes;
        // The account state ends at 329; wire bytes follow its four-byte length.
        reserved[333 + if external { 146 } else { 44 }] = 1;
        assert_eq!(decode_message_input(&reserved), Err(DecodeError::NonZeroReserved));
    }
}

#[test]
fn ecc_only_outbound_bytes_keep_fee_and_each_transferred_ecc_distinct() {
    let mut output = MessageOutput::success(StateUpdate::FullReplace(Vec::new()));
    output.outbound_intents.push(OutboundIntent {
        input_index: 0,
        destination: AccountAddress { dapp_id: [3; 32], account_id: [4; 32] },
        fuel_fee_shells: 17,
        extra: CURRENCIES
            .into_iter()
            .map(|(currency_id, amount)| CurrencyAmount { currency_id, amount })
            .collect(),
        method_id: 91,
        body: vec![7, 8, 9],
    });
    let mut expected = b"ACMO".to_vec();
    expected.extend_from_slice(&[0; 4]);
    expected.extend_from_slice(&[1, 0, 0, 0, 0]);
    expected.extend_from_slice(&[0; 8]);
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend_from_slice(&0u32.to_le_bytes());
    expected.extend_from_slice(&[3; 32]);
    expected.extend_from_slice(&[4; 32]);
    expected.extend_from_slice(&17u128.to_le_bytes());
    expected.extend_from_slice(&3u16.to_le_bytes());
    currencies(&mut expected);
    expected.extend_from_slice(&91u64.to_le_bytes());
    expected.extend_from_slice(&3u32.to_le_bytes());
    expected.extend_from_slice(&[7, 8, 9]);
    expected.extend_from_slice(&[0; 8]);
    assert_eq!(expected.len(), 25 + 98 + 60 + 3 + 8);
    assert_eq!(encode_message_output(&output).unwrap(), expected);
}
