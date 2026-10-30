#!/usr/bin/env python3
"""Deterministic post-build optimization; original compiler bytes are retained."""
from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile
ROOT=Path(__file__).resolve().parents[1]
FLAGS=["-Oz","--enable-bulk-memory","--enable-sign-ext","--enable-nontrapping-float-to-int",
       "--enable-mutable-globals","--enable-multivalue","--enable-reference-types","--strip-debug"]
def run(binary,output):
    version=subprocess.check_output([str(binary),"--version"],text=True).strip()
    assert version.startswith("wasm-opt version 133"),version
    results={"binaryen":version,"flags":FLAGS,"contracts":{}}
    for contract in ["vesting","receiver"]:
        directory=ROOT/"contracts"/contract/"target/avm";wasm=directory/"contract.wasm"
        original=wasm.read_bytes();compiler=directory/"contract.compiler.wasm";compiler.write_bytes(original)
        with tempfile.TemporaryDirectory(prefix="stream-wasm-opt-") as d:
            first,second=Path(d)/"first.wasm",Path(d)/"second.wasm"
            for target in [first,second]:subprocess.run([str(binary),str(compiler),*FLAGS,"-o",str(target)],check=True)
            optimized=first.read_bytes();assert optimized==second.read_bytes(),"Optimizer is not reproducible"
            wasm.write_bytes(optimized)
        results["contracts"][contract]={"compiler_bytes":len(original),"optimized_bytes":len(optimized),
            "compiler_sha256":hashlib.sha256(original).hexdigest(),"optimized_sha256":hashlib.sha256(optimized).hexdigest(),
            "two_independent_optimizer_outputs_match":True}
    output.parent.mkdir(parents=True,exist_ok=True)
    output.write_text(json.dumps(results,indent=2)+"\n")
    print("Pinned Binaryen optimization reproducibility: PASS")
if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("--wasm-opt",type=Path,required=True);p.add_argument("--output",type=Path,required=True);a=p.parse_args();run(a.wasm_opt.resolve(),a.output.resolve())
