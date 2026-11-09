use avm_contract_sdk::{owner_key, set_owner_key, EncodeError, OWNER_KEY_BYTES};

// Independent expectations from the external call admission rule for the
// destination owner key: the key is data[owner_key_offset ..
// owner_key_offset + 32]; it is absent when those bytes do not fit the data,
// the end overflows, or they are all zero.

#[test]
fn reads_the_key_at_the_declared_offset_including_at_the_end_of_data() {
    let mut data = vec![0x11; 72];
    data.extend_from_slice(&[0x5a; 32]);
    assert_eq!(OWNER_KEY_BYTES, 32);
    assert_eq!(owner_key(&data, 72), Some([0x5a; 32]));
    assert_eq!(owner_key(&data, 0), Some(data[..32].try_into().unwrap()));
}

#[test]
fn an_out_of_range_overflowing_or_zero_slot_is_no_key() {
    let data = [0x22; 40];
    assert_eq!(owner_key(&data, 9), None);
    assert_eq!(owner_key(&data, 40), None);
    assert_eq!(owner_key(&data, u32::MAX), None);
    assert_eq!(owner_key(&[], 0), None);
    assert_eq!(owner_key(&[0; 64], 16), None);
}

#[test]
fn set_writes_only_the_slot_and_rotates_the_key() {
    let mut data = vec![0x33; 100];
    set_owner_key(&mut data, 8, [0x44; 32]).unwrap();
    assert_eq!(&data[..8], &[0x33; 8]);
    assert_eq!(&data[8..40], &[0x44; 32]);
    assert_eq!(&data[40..], &[0x33; 60]);
    assert_eq!(owner_key(&data, 8), Some([0x44; 32]));

    set_owner_key(&mut data, 8, [0x55; 32]).unwrap();
    assert_eq!(owner_key(&data, 8), Some([0x55; 32]));

    // A zero key leaves the account with no owner key.
    set_owner_key(&mut data, 8, [0; 32]).unwrap();
    assert_eq!(owner_key(&data, 8), None);
}

#[test]
fn set_rejects_a_slot_outside_the_data_and_changes_nothing() {
    let mut data = vec![0x66; 40];
    for offset in [9, 40, u32::MAX] {
        assert_eq!(set_owner_key(&mut data, offset, [0x77; 32]), Err(EncodeError::InvalidIndex));
        assert_eq!(data, vec![0x66; 40]);
    }
}
