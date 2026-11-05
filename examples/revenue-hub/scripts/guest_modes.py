#!/usr/bin/env python3
"""Check guest ABI vectors with the pinned host-only Rust Wasmtime harness."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import re
import fcntl
import sys

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
    process = subprocess.run([
        "cargo", "+1.99.1", "test", "--locked", "--offline",
        "--manifest-path", str(ROOT / "tests/Cargo.toml"),
        "--features", "guest-abi-check", "--test", "guest_modes", "--", str(wasm), str(vectors),
    ], cwd=ROOT, env=environment, capture_output=True, text=True)
    print(process.stdout, end="")
    print(process.stderr, end="", file=sys.stderr)
    if environment.get("AVM_MEMORY_REPORT"):
        import blake3
        rows = re.findall(r"^AVM_MEMORY_USAGE (\S+) (\d+) (\d+) (\d+) (\d+)$", process.stdout, re.MULTILINE)
        expected_calls = len(vectors.read_text().strip().splitlines())
        value = {"complete": process.returncode == 0 and len(rows) == expected_calls,
                 "runs": [{"code_hash": blake3.blake3(wasm.read_bytes()).hexdigest(), "entrypoint": row[0],
                           "initial_bytes": int(row[1]), "peak_bytes": int(row[2]),
                           "rejected_growths": int(row[3]), "declared_max_bytes": int(row[4]) or None}
                          for row in rows]}
        with open(environment["AVM_MEMORY_REPORT"], "a") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX)
            stream.write(json.dumps(value) + "\n")
        if not value["complete"]:
            raise ValueError("incomplete direct guest memory observations")
    process.check_returncode()
    cases = []
    for row in vectors.read_text().strip().splitlines():
        name, entrypoint, input_hex, expected_hex = row.split("\t")
        cases.append({"name": name, "entrypoint": entrypoint,
                      "input_bytes": len(bytes.fromhex(input_hex)),
                      "output_bytes": len(bytes.fromhex(expected_hex))})
    evidence.write_text(json.dumps({"status": "PASS",
        "scope": "Guest ABI only; in-tree guest SDK, Rust Wasmtime 48.0.8",
        "wasm_sha256": hashlib.sha256(wasm.read_bytes()).hexdigest(), "cases": cases}, indent=2) + "\n")
    print(f"Audit WASM/native output agreement and rejected-mode serialization: {len(cases)} PASS")


if __name__ == "__main__":
    main()
