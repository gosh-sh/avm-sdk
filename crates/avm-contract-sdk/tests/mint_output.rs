//! The SDK encodes the mint intent of a system minter in `MessageOutput` as
//! the execution ABI reads it. Run with
//! the SDK feature `system-mint`.
#![cfg(feature = "system-mint")]

use avm_contract_sdk::{
    encode_message_output, encode_mint_output, CurrencyAmount, EncodeError, MessageOutput,
    StateUpdate,
};
#[cfg(feature = "event-rings")]
use avm_contract_sdk::{AccountAddress, EventReadEntry, InternalEventReadRequest, OutboundIntent};

fn output() -> MessageOutput {
    MessageOutput::success(StateUpdate::FullReplace(vec![7]))
}

fn mint(currency_id: u32, amount: u128) -> CurrencyAmount {
    CurrencyAmount { currency_id, amount }
}

#[test]
fn mint_output_is_the_ordinary_output_with_the_flag_and_trailing_mints() {
    let plain = encode_message_output(&output()).unwrap();
    assert_eq!(encode_mint_output(&output(), &[]).unwrap(), plain);
    let mut expected = plain;
    expected[8] |= 1 << 2;
    expected.extend_from_slice(&2_u16.to_le_bytes());
    expected.extend_from_slice(&2_u32.to_le_bytes());
    expected.extend_from_slice(&5_u128.to_le_bytes());
    expected.extend_from_slice(&9_u32.to_le_bytes());
    expected.extend_from_slice(&u128::MAX.to_le_bytes());
    assert_eq!(encode_mint_output(&output(), &[mint(2, 5), mint(9, u128::MAX)]).unwrap(), expected);
}

#[test]
fn mint_output_rejects_a_malformed_intent_and_a_failure() {
    for mints in [vec![mint(2, 0)], vec![mint(9, 1), mint(2, 1)], vec![mint(2, 1), mint(2, 1)]] {
        assert_eq!(encode_mint_output(&output(), &mints), Err(EncodeError::InvalidExtraCurrencies));
    }
    assert_eq!(
        encode_mint_output(&MessageOutput::failure(1000), &[mint(2, 1)]),
        Err(EncodeError::EffectsOnFailure)
    );
}

/// `MessageOutput` puts the mint intent before the trailing
/// `OutboundEventReads`.
#[cfg(feature = "event-rings")]
#[test]
fn mint_intent_precedes_the_outbound_event_reads() {
    let address = AccountAddress { dapp_id: [3; 32], account_id: [4; 32] };
    let mut reading = output();
    reading.outbound_intents.push(OutboundIntent {
        input_index: 0,
        destination: address,
        fuel_fee_shells: 1,
        extra: Vec::new(),
        method_id: 5,
        body: Vec::new(),
    });
    reading.outbound_event_reads = vec![EventReadEntry {
        outbound_index: 0,
        reads: vec![InternalEventReadRequest { source: address, event_class: 6 }],
    }];
    let plain = encode_message_output(&reading).unwrap();
    // event_read_entry_count, outbound_index, read_count, _reserved, one read.
    let (ordinary, reads) = plain.split_at(plain.len() - (4 + 4 + 4 + 72));
    let mut expected = ordinary.to_vec();
    expected[8] |= 1 << 2;
    expected.extend_from_slice(&1_u16.to_le_bytes());
    expected.extend_from_slice(&2_u32.to_le_bytes());
    expected.extend_from_slice(&5_u128.to_le_bytes());
    expected.extend_from_slice(reads);
    assert_eq!(encode_mint_output(&reading, &[mint(2, 5)]).unwrap(), expected);
}
