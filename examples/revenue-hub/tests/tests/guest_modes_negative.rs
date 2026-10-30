#[path = "support/guest_abi.rs"]
mod guest_abi;

fn unsigned(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let byte = (value & 127) as u8;
        value >>= 7;
        bytes.push(byte | if value == 0 { 0 } else { 128 });
        if value == 0 {
            return bytes;
        }
    }
}

fn signed(mut value: i64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let byte = (value & 127) as u8;
        value >>= 7;
        let done = value == 0 && byte & 64 == 0 || value == -1 && byte & 64 != 0;
        bytes.push(byte | if done { 0 } else { 128 });
        if done {
            return bytes;
        }
    }
}

fn section(wasm: &mut Vec<u8>, id: u8, bytes: &[u8]) {
    wasm.push(id);
    wasm.extend(unsigned(bytes.len() as u64));
    wasm.extend(bytes);
}

fn fixture(packed: u64, mutate_input: bool, export_free: bool, trap_free: u8) -> Vec<u8> {
    let mut wasm = b"\0asm\x01\0\0\0".to_vec();
    section(
        &mut wasm,
        1,
        &[3, 0x60, 1, 0x7f, 1, 0x7f, 0x60, 2, 0x7f, 0x7f, 0, 0x60, 2, 0x7f, 0x7f, 1, 0x7e],
    );
    section(&mut wasm, 3, &[3, 0, 1, 2]);
    section(&mut wasm, 5, &[1, 0, 1]);
    let mut exports = vec![if export_free { 5 } else { 4 }];
    for (name, kind, index) in
        [("memory", 2, 0), ("avm_guest_alloc", 0, 0), ("init", 0, 2), ("process_batch", 0, 2)]
    {
        exports.extend(unsigned(name.len() as u64));
        exports.extend(name.as_bytes());
        exports.extend([kind, index]);
    }
    if export_free {
        let name = "avm_guest_free";
        exports.extend(unsigned(name.len() as u64));
        exports.extend(name.as_bytes());
        exports.extend([0, 1]);
    }
    section(&mut wasm, 7, &exports);
    let mut alloc = vec![0, 0x41];
    alloc.extend(signed(1024));
    alloc.push(0x0b);
    let free = match trap_free {
        1 => vec![0, 0, 0x0b],
        2 => {
            let mut code = vec![0, 0x20, 0, 0x41];
            code.extend(signed(1024));
            code.extend([0x46, 0x04, 0x40, 0, 0x0b, 0x0b]);
            code
        }
        _ => vec![0, 0x0b],
    };
    let mut call = vec![0];
    // Reusing one instance would change the next output from42to43.
    call.push(0x41);
    call.extend(signed(2049));
    call.extend([0x2d, 0, 0, 0x04, 0x40, 0x41]);
    call.extend(signed(2048));
    call.extend([0x41, 43, 0x3a, 0, 0, 0x0b, 0x41]);
    call.extend(signed(2049));
    call.extend([0x41, 1, 0x3a, 0, 0]);
    if mutate_input {
        call.extend([0x20, 0, 0x41, 0, 0x3a, 0, 0]);
    }
    call.push(0x42);
    call.extend(signed(packed as i64));
    call.push(0x0b);
    let mut code = vec![3];
    for body in [alloc, free, call] {
        code.extend(unsigned(body.len() as u64));
        code.extend(body);
    }
    section(&mut wasm, 10, &code);
    let mut data = vec![1, 0, 0x41];
    data.extend(signed(2048));
    data.extend([0x0b, 1, 42]);
    section(&mut wasm, 11, &data);
    wasm
}

const OUTPUT: u64 = (2048_u64 << 32) | 1;
const VECTOR: &str = "fixture\tinit\t07\t2a\n";

#[test]
fn accepts_exact_bytes_and_fresh_instances_for_every_row() {
    assert_eq!(
        guest_abi::check_vectors(&fixture(OUTPUT, false, true, 0), &VECTOR.repeat(3)),
        Ok(3)
    );
}

#[test]
fn rejects_wrong_bytes_and_input_mutation() {
    assert!(guest_abi::check_vectors(&fixture(OUTPUT, false, true, 0), "wrong\tinit\t07\t2b\n")
        .unwrap_err()
        .contains("output mismatch"));
    assert!(guest_abi::check_vectors(&fixture(OUTPUT, true, true, 0), VECTOR)
        .unwrap_err()
        .contains("input ownership"));
}

#[test]
fn rejects_zero_and_out_of_bounds_output() {
    assert!(guest_abi::check_vectors(&fixture(0, false, true, 0), VECTOR)
        .unwrap_err()
        .contains("zero packed output"));
    assert!(guest_abi::check_vectors(&fixture((65530_u64 << 32) | 16, false, true, 0), VECTOR)
        .unwrap_err()
        .contains("output range"));
}

#[test]
fn requires_entrypoint_free_export_and_successful_free_calls() {
    assert!(guest_abi::check_vectors(
        &fixture(OUTPUT, false, true, 0),
        "missing\tmissing\t07\t2a\n"
    )
    .unwrap_err()
    .contains("entrypoint export"));
    assert!(guest_abi::check_vectors(&fixture(OUTPUT, false, false, 0), VECTOR)
        .unwrap_err()
        .contains("free export"));
    assert!(guest_abi::check_vectors(&fixture(OUTPUT, false, true, 1), VECTOR)
        .unwrap_err()
        .contains("free output"));
    assert!(guest_abi::check_vectors(&fixture(OUTPUT, false, true, 2), VECTOR)
        .unwrap_err()
        .contains("free input"));
}

#[test]
fn rejects_empty_or_malformed_vectors() {
    let wasm = fixture(OUTPUT, false, true, 0);
    assert!(guest_abi::check_vectors(&wasm, "").unwrap_err().contains("empty vector"));
    assert!(guest_abi::check_vectors(&wasm, "bad\tinit\t0\t2a\n")
        .unwrap_err()
        .contains("hexadecimal"));
    assert!(guest_abi::check_vectors(&wasm, "missing-columns\n")
        .unwrap_err()
        .contains("expected name"));
}
