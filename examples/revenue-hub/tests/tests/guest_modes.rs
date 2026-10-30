#[path = "support/guest_abi.rs"]
mod guest_abi;

fn main() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err("expected WASM and native-vector TSV paths".to_owned());
    }
    let wasm = std::fs::read(&arguments[0]).map_err(|error| error.to_string())?;
    let vectors = std::fs::read_to_string(&arguments[1]).map_err(|error| error.to_string())?;
    let count = guest_abi::check_vectors(&wasm, &vectors)?;
    println!("Guest ABI native/WASM output and ownership checks: {count} PASS");
    Ok(())
}
