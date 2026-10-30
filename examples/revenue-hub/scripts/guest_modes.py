#!/usr/bin/env python3
"""Check guest ABI vectors with the pinned host-only Rust Wasmtime harness."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wasm", type=Path)
    parser.add_argument("vectors", type=Path)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    wasm, vectors, evidence = (path.resolve() for path in (args.wasm, args.vectors, args.evidence))
    environment = os.environ.copy()
    if environment.get("AVM_GUEST_CHECKER_TARGET_DIR"):
        environment["CARGO_TARGET_DIR"] = environment["AVM_GUEST_CHECKER_TARGET_DIR"]
    subprocess.run([
        "cargo", "+1.93.0", "test", "--locked", "--offline",
        "--manifest-path", str(ROOT / "tests/Cargo.toml"),
        "--features", "guest-abi-check", "--test", "guest_modes", "--", str(wasm), str(vectors),
    ], cwd=ROOT, env=environment, check=True)
    cases = []
    for row in vectors.read_text().strip().splitlines():
        name, entrypoint, input_hex, expected_hex = row.split("\t")
        cases.append({"name": name, "entrypoint": entrypoint,
                      "input_bytes": len(bytes.fromhex(input_hex)),
                      "output_bytes": len(bytes.fromhex(expected_hex))})
    evidence.write_text(json.dumps({"status": "PASS",
        "scope": "Guest ABI only; in-tree guest SDK, Rust Wasmtime 36.0.12",
        "wasm_sha256": hashlib.sha256(wasm.read_bytes()).hexdigest(), "cases": cases}, indent=2) + "\n")
    print(f"Audit WASM/native output agreement and rejected-mode serialization: {len(cases)} PASS")


if __name__ == "__main__":
    main()
