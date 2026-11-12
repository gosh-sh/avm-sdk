mod support;
use avm_contract_sdk::Contract;
use avm_stream_core::{ids, E_EMPTY};
use avm_stream_vesting::Vesting;
use support::*;

fn deployed(time: u32) -> Book {
    let spec = Spec { deploy: true, body: envelope(&config().encode()), time, ..Spec::default() };
    Book { state: initialize::<Vesting>(&spec), time, lt: 10 }
}

#[test]
fn relative_deploy_accepts_every_execution_time() {
    for time in [0, 100, 200, 1_800_000_000, u32::MAX] {
        deployed(time);
    }
}

#[test]
fn relative_funding_accepts_after_every_schedule_point() {
    for time in [100, 200, 1_800_000_000, u32::MAX] {
        let mut book = deployed(10);
        assert_eq!(book.call(OWNER, ids::FUND, vec![], 2000, time).status_code, 0);
        assert_outbound_principal(book.claim(0, time.max(210)), 1000);
    }
}

#[test]
fn relative_first_claim_after_terminal_cliff_releases_all() {
    for origin in [0, 100, 1_800_000_000, u32::MAX - 300] {
        let mut cfg = config();
        cfg.streams[0].cliff = cfg.streams[0].end;
        let spec =
            Spec { deploy: true, body: envelope(&cfg.encode()), time: origin, ..Spec::default() };
        let mut book = Book { state: initialize::<Vesting>(&spec), time: origin, lt: 10 };
        assert_eq!(book.call(OWNER, ids::FUND, vec![], 2000, origin).status_code, 0);
        fails(book.claim(0, origin + 199), E_EMPTY);
        assert_outbound_principal(book.claim(0, origin + 201), 1000);
        assert_eq!(book.settle(0, 0, true).status_code, 0);
        fails(book.claim(0, origin + 202), E_EMPTY);
    }
}

#[test]
fn relative_cancellation_after_end_preserves_all_earned_funds() {
    let mut book = deployed(0);
    assert_eq!(book.call(OWNER, ids::FUND, vec![], 2000, 300).status_code, 0);
    assert_eq!(book.cancel(0, 300).status_code, 0);
    assert_outbound_principal(book.claim(0, 300), 1000);
    assert_eq!(book.settle(0, 0, true).status_code, 0);
    fails(book.refund(0, 300), E_EMPTY);
}

#[test]
fn relative_init_uses_execution_context_not_message_creation_time() {
    let spec =
        Spec { deploy: true, body: envelope(&config().encode()), time: 1000, ..Spec::default() };
    let mut bytes = frame(&[], &spec);
    bytes[12..16].copy_from_slice(&5u32.to_le_bytes());
    let output = Vesting::init(input(&bytes));
    assert_eq!(output.status_code, 0);
    let mut book =
        Book { state: apply(&[], output.state_update.as_ref().unwrap()), time: 5, lt: 10 };
    assert_eq!(book.call(OWNER, ids::FUND, vec![], 2000, 6).status_code, 0);
    fails(book.claim(0, 134), E_EMPTY);
    assert_outbound_principal(book.claim(0, 135), 300);
}

#[test]
fn relative_address_preimage_is_the_actual_contract_init_config() {
    let cfg = config();
    let expected = cfg.encode();
    for origin in [0, 1_800_000_000, u32::MAX] {
        let spec = Spec { deploy: true, body: envelope(&expected), time: origin, ..Spec::default() };
        let bytes = frame(&[], &spec);
        let message = input(&bytes);
        assert_eq!(message.deploy().unwrap().init_data(), expected);
        let output = Vesting::init(message);
        assert_eq!(output.status_code, 0);
        let state = apply(&[], output.state_update.as_ref().unwrap());
        assert_eq!(&state[..avm_stream_core::state::CONFIG_BYTES], expected);
        let actual = avm_stream_core::state::Config::decode(&state[..expected.len()]).unwrap();
        assert_eq!(actual, cfg);
    }
}
#[test]
fn relative_zero_offset_starts_accrual_on_the_first_execution() {
    let mut cfg = config(); cfg.streams[0].start = 0; cfg.streams[0].cliff = 0; cfg.streams[0].end = 100;
    for origin in [0, 1_800_000_000] {
        let spec = Spec { deploy: true, body: envelope(&cfg.encode()), time: origin, ..Spec::default() };
        let mut book = Book { state: initialize::<Vesting>(&spec), time: origin, lt: 10 };
        assert_eq!(book.call(OWNER, ids::FUND, vec![], 2000, origin).status_code, 0);
        fails(book.claim(0, origin), E_EMPTY);
        assert_outbound_principal(book.claim(0, origin + 50), 500);
    }
}
