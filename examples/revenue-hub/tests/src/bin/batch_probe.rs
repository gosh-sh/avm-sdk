#[path = "../../tests/support/mod.rs"]
mod support;
use avm_contract_sdk::{decode_batch_input, encode_batch_output, BatchContract, StateUpdate};
use avm_revenue_audit::AuditLedger;
use support::*;

fn main() {
    let state = audit_state();
    println!("batch_messages,patch_count,patch_payload_bytes,encoded_output_bytes,full_state_bytes,full_replace_output_bytes");
    for count in [1, 8, 64, 256] {
        let messages = (1..=count).map(|n| audit_message(n % 3, n as u64, 10)).collect::<Vec<_>>();
        let frame = batch_frame(&state, &messages);
        let out = AuditLedger::process_batch(decode_batch_input(&frame).unwrap());
        assert_eq!(out.status_codes, vec![0; count]);
        let StateUpdate::DataPatchList(patches) = &out.state_update else { panic!() };
        let payload: usize = patches.iter().map(|p| p.new_bytes.len()).sum();
        let wire = encode_batch_output(&out).unwrap().len();
        let mut full = out.clone();
        full.state_update = StateUpdate::FullReplace(apply(&state, &out.state_update));
        let full_wire = encode_batch_output(&full).unwrap().len();
        println!("{count},{},{payload},{wire},{},{full_wire}", patches.len(), state.len());
    }
}
