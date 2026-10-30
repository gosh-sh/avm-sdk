//! Canonical vectors for executing the current canonical guest ABI through actual WASM.
#[path = "../../tests/support/mod.rs"]
mod support;
use avm_contract_sdk::{
    decode_batch_input, encode_batch_output, encode_message_output, BatchContract, Contract,
    StateOutputMode,
};
use avm_revenue_audit::AuditLedger;
use support::*;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn main() {
    for mode in [StateOutputMode::FullReplace, StateOutputMode::DataPatchList] {
        let spec = Spec {
            destination: address(9),
            deploy: true,
            body: envelope(&[3]),
            mode,
            ..Spec::default()
        };
        let bytes = frame(&[], &spec);
        let out = encode_message_output(&AuditLedger::init(input(&bytes))).unwrap();
        println!("init-{mode:?}\tinit\t{}\t{}", hex(&bytes), hex(&out));
        let state = audit_state();
        for (label, state, messages) in [
            ("valid", state.clone(), vec![audit_message(0, 1, 7)]),
            ("empty", state.clone(), vec![]),
            ("oversized", state, vec![audit_message(0, 1, 7); 257]),
            ("invalid-state", vec![], vec![audit_message(0, 1, 7)]),
        ] {
            let mut bytes = batch_frame(&state, &messages);
            bytes[148] = mode as u8;
            let out = encode_batch_output(&AuditLedger::process_batch(
                decode_batch_input(&bytes).unwrap(),
            ))
            .unwrap();
            println!("batch-{label}-{mode:?}\tprocess_batch\t{}\t{}", hex(&bytes), hex(&out));
        }
    }
}
