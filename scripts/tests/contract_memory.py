#!/usr/bin/env python3
"""Verify contract memory declarations against actual compiled WASM sections."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tomllib


def memory_limits(module):
    if module[:8] != b"\0asm\x01\0\0\0":
        raise ValueError("expected a core WASM module")

    def integer(position, end):
        value = 0
        for shift in range(0, 35, 7):
            if position >= end:
                raise ValueError("truncated WASM integer")
            byte = module[position]
            position += 1
            if shift == 28 and byte & 0xF0:
                raise ValueError("invalid WASM u32 integer")
            value |= (byte & 127) << shift
            if byte < 128:
                return value, position
        raise ValueError("invalid WASM integer")

    cursor = 8
    found = None
    while cursor < len(module):
        section = module[cursor]
        length, start = integer(cursor + 1, len(module))
        end = start + length
        if end > len(module):
            raise ValueError("truncated WASM section")
        if section == 5:
            if found is not None:
                raise ValueError("duplicate memory section")
            count, position = integer(start, end)
            flags, position = integer(position, end)
            if count != 1 or flags not in (0, 1):
                raise ValueError("expected one unshared 32-bit memory")
            minimum, position = integer(position, end)
            maximum = None
            if flags == 1:
                maximum, position = integer(position, end)
                if minimum > maximum:
                    raise ValueError("memory minimum exceeds maximum")
            if position != end:
                raise ValueError("unexpected memory-section bytes")
            found = (minimum * 65536, None if maximum is None else maximum * 65536)
        cursor = end
    if found is None:
        raise ValueError("missing memory section")
    return found


def check_built_memory(module, expected, provenance):
    minimum, maximum = memory_limits(module)
    if maximum != expected:
        raise ValueError(f"compiled maximum {maximum} differs from declaration {expected}")
    flags = [flag for line in provenance.splitlines() if line.startswith("rustflags:") for flag in line.split() if flag.startswith("-Clink-arg=--max-memory=")]
    if flags != [f"-Clink-arg=--max-memory={expected}"]:
        raise ValueError("missing or ambiguous memory maximum in build provenance")
    return minimum, maximum


def declared_maximum(manifest):
    config = tomllib.loads(manifest.read_text())
    explicit = config.get("build", {}).get("max_memory_bytes")
    if explicit is not None:
        return explicit
    record = json.loads(manifest.with_name("avm-memory.json").read_text())
    if record["version"] != 1 or record["test_calls"] <= 0 or record["peak_memory_bytes"] <= 0:
        raise ValueError("invalid project memory measurement")
    return max(record["initial_memory_bytes"], ((record["peak_memory_bytes"] + 65535) // 65536) * 65536)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--builder", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    records = []
    args.output.mkdir(parents=True, exist_ok=True)
    for manifest in sorted((args.sdk / "examples").rglob("avm-contract.toml")):
        if "target" in manifest.relative_to(args.sdk).parts:
            continue
        expected = declared_maximum(manifest)
        relative = manifest.relative_to(args.sdk).as_posix()
        stem = relative.replace("/", "_")
        command = [str(args.builder.resolve()), "build", "--manifest-path", str(manifest.with_name("Cargo.toml").resolve())]
        process = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=os.environ.copy())
        (args.output / f"{stem}.stdout").write_bytes(process.stdout)
        (args.output / f"{stem}.stderr").write_bytes(process.stderr)
        if process.returncode:
            raise ValueError(f"contract build failed: {relative}; see retained stderr")
        target = manifest.parent / "target"
        minimum, maximum = check_built_memory((target / "avm/contract.wasm").read_bytes(), expected, (target / "avm-build-provenance.txt").read_text())
        if (target / "avm/NOT-FOR-DEPLOYMENT").exists():
            raise ValueError("deployment build retained a measurement-only marker")
        if "artifact_purpose: deployment" not in (target / "avm-build-provenance.txt").read_text():
            raise ValueError("deployment provenance purpose is missing")
        records.append({"manifest": relative, "minimum_bytes": minimum, "declared_max_bytes": maximum, "provenance_verified": True})
        (args.output / "examples.json").write_text(json.dumps({"examples": records}, indent=2) + "\n")
    if not records:
        raise ValueError("no contract manifests found")
    print(f"PASS: {len(records)} example memory declarations verified in WASM and provenance")


if __name__ == "__main__":
    main()
