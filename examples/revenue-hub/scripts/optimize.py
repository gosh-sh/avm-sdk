#!/usr/bin/env python3
"""Deterministic post-build optimization; original compiler bytes are retained."""
from pathlib import Path
import argparse,hashlib,json,subprocess,tempfile
ROOT=Path(__file__).resolve().parents[1]
FLAGS=["-Oz","--enable-bulk-memory","--enable-sign-ext","--enable-nontrapping-float-to-int",
       "--enable-mutable-globals","--enable-multivalue","--enable-reference-types","--strip-debug"]
def run(binary, output, compact=False):
    version=subprocess.check_output([str(binary),"--version"],text=True).strip()
    assert version.startswith("wasm-opt version 133"),version
    results={"binaryen":version,"flags":FLAGS,"contracts":{}}
    paths = {"compact-audit": ROOT / "experiments/compact-audit/target/avm"} if compact else {
        name: ROOT / "contracts" / name / "target/avm" for name in ["vault", "receiver", "audit"]}
    for contract, directory in paths.items():
        wasm=directory/"contract.wasm"
        original=wasm.read_bytes();compiler=directory/"contract.compiler.wasm";compiler.write_bytes(original)
        with tempfile.TemporaryDirectory(prefix="hub-wasm-opt-") as d:
            first,second=Path(d)/"first.wasm",Path(d)/"second.wasm"
            for target in [first,second]:subprocess.run([str(binary),str(compiler),*FLAGS,"-o",str(target)],check=True)
            optimized=first.read_bytes();assert optimized==second.read_bytes(),"Optimizer is not reproducible"
            wasm.write_bytes(optimized)
        results["contracts"][contract]={"compiler_bytes":len(original),"optimized_bytes":len(optimized),
            "compiler_sha256":hashlib.sha256(original).hexdigest(),"optimized_sha256":hashlib.sha256(optimized).hexdigest(),
            "two_independent_optimizer_outputs_match":True}
    output.write_text(json.dumps(results,indent=2)+"\n")
    print("Pinned Binaryen optimization reproducibility: PASS")
if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("--wasm-opt",type=Path,required=True);p.add_argument("--out",type=Path,required=True);p.add_argument("--compact",action="store_true");a=p.parse_args();run(a.wasm_opt.resolve(),a.out.resolve(),a.compact)
