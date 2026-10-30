#!/usr/bin/env python3
"""Inspect WASM sections without third-party dependencies."""
from pathlib import Path
import argparse
import hashlib
import json

ROOT = Path(__file__).resolve().parents[1]

def leb(data, pos):
    value, shift = 0, 0
    while True:
        byte = data[pos]; pos += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, pos
        shift += 7
        if shift > 35:
            raise ValueError("oversized LEB")

def exports(data):
    assert data[:8] == b"\0asm\x01\0\0\0"
    pos = 8
    names = []
    while pos < len(data):
        kind = data[pos]; pos += 1
        size, pos = leb(data, pos)
        end = pos + size
        assert end <= len(data)
        if kind == 2:
            count, _ = leb(data, pos)
            assert count == 0, "AVM guest must have no host imports"
        if kind == 7:
            count, cursor = leb(data, pos)
            for _ in range(count):
                length, cursor = leb(data, cursor)
                name = data[cursor:cursor+length].decode(); cursor += length
                cursor += 1 # Export kind
                _, cursor = leb(data, cursor)
                names.append(name)
        pos = end
    for name in ["memory", "init", "process_message", "avm_guest_alloc", "avm_guest_free"]:
        assert name in names, (name,names)
    return names

parser = argparse.ArgumentParser()
parser.add_argument("--out", type=Path, required=True)
args = parser.parse_args()
result = {"status":"PASS", "contracts":{}}
for contract in ["vault", "receiver", "audit"]:
    directory = ROOT / "contracts" / contract / "target/avm"
    wasm = (directory / "contract.wasm").read_bytes()
    names = exports(wasm)
    if contract == "audit":
        assert "process_batch" in names
    result["contracts"][contract] = {
        "exports":names, "host_imports":0,
        "sha256":{name:hashlib.sha256((directory / name).read_bytes()).hexdigest()
                  for name in ["contract.wasm","contract.acab","contract.descriptor"]},
        "wasm_bytes":len(wasm),
    }
args.out.write_text(json.dumps(result,indent=2)+"\n")
print("WASM exports, absence of host imports, and artifact SHA-256: PASS")
