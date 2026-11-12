//! Canonical vectors for executing the current canonical guest ABI through actual WASM.
#[path = "../../tests/support/mod.rs"]
mod support;
use avm_contract_sdk::{encode_message_output, Contract, StateOutputMode};
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
        let receipt = Spec { mode, ..audit_receipt(0, 1, 7) };
        for (label, state) in [("valid", audit_state()), ("invalid-state", vec![])] {
            let bytes = frame(&state, &receipt);
            let out = encode_message_output(&AuditLedger::process_message(input(&bytes))).unwrap();
            println!("message-{label}-{mode:?}\tprocess_message\t{}\t{}", hex(&bytes), hex(&out));
        }
    }
}
