use wasmtime::{Engine, Instance, Module, Store};

pub fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("odd hexadecimal length".to_owned());
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).ok_or("invalid hexadecimal")?;
            let low = (pair[1] as char).to_digit(16).ok_or("invalid hexadecimal")?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}

pub fn check_vectors(wasm: &[u8], vectors: &str) -> Result<usize, String> {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm).map_err(|error| error.to_string())?;
    if module.imports().next().is_some() {
        return Err("guest host imports are not supplied".to_owned());
    }
    let mut count = 0;
    for row in vectors.trim().lines() {
        let fields: Vec<_> = row.split('\t').collect();
        if fields.len() != 4 || fields[0].is_empty() {
            return Err("expected name, entrypoint, input hex and expected hex".to_owned());
        }
        let (name, entrypoint) = (fields[0], fields[1]);
        let input = decode_hex(fields[2]).map_err(|error| format!("{name}: {error}"))?;
        let expected = decode_hex(fields[3]).map_err(|error| format!("{name}: {error}"))?;
        let mut store = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|error| format!("{name}: instantiate: {error}"))?;
        let memory = instance.get_memory(&mut store, "memory").ok_or("missing memory export")?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "avm_guest_alloc")
            .map_err(|error| format!("{name}: allocator export: {error}"))?;
        let length = u32::try_from(input.len()).map_err(|_| "input exceeds u32")?;
        let pointer = alloc
            .call(&mut store, length as i32)
            .map_err(|error| format!("{name}: allocation: {error}"))? as u32;
        if pointer == 0 {
            return Err(format!("{name}: zero input allocation"));
        }
        memory
            .write(&mut store, pointer as usize, &input)
            .map_err(|error| format!("{name}: input range: {error}"))?;
        let call = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, entrypoint)
            .map_err(|error| format!("{name}: entrypoint export: {error}"))?;
        let packed = call
            .call(&mut store, (pointer as i32, length as i32))
            .map_err(|error| format!("{name}: execution: {error}"))? as u64;
        if packed == 0 {
            return Err(format!("{name}: zero packed output"));
        }
        let output_pointer = (packed >> 32) as u32;
        let output_length = packed as u32;
        let start = output_pointer as usize;
        let end = start.checked_add(output_length as usize).ok_or("output range overflow")?;
        let output = memory
            .data(&store)
            .get(start..end)
            .ok_or_else(|| format!("{name}: invalid output range"))?;
        if output != expected {
            return Err(format!("{name}: WASM/native canonical output mismatch"));
        }
        let mut retained = vec![0; input.len()];
        memory
            .read(&store, pointer as usize, &mut retained)
            .map_err(|error| format!("{name}: retained input range: {error}"))?;
        if retained != input {
            return Err(format!("{name}: input ownership changed"));
        }
        let free = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "avm_guest_free")
            .map_err(|error| format!("{name}: free export: {error}"))?;
        free.call(&mut store, (output_pointer as i32, output_length as i32))
            .map_err(|error| format!("{name}: free output: {error}"))?;
        free.call(&mut store, (pointer as i32, length as i32))
            .map_err(|error| format!("{name}: free input: {error}"))?;
        count += 1;
    }
    if count == 0 {
        return Err("empty vector input".to_owned());
    }
    Ok(count)
}
