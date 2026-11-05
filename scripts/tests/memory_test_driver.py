#!/usr/bin/env python3
"""Instrument the existing example test commands without changing their outcomes."""

import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib
import shutil

import blake3

BUILD_INPUTS = ("AVM_E2E_FAILURE_FLOW", "AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY", "PROVENANCE_PERMISSIVE_CHILD",
                "CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_PROFILE_RELEASE_LTO", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "CARGO_PROFILE_RELEASE_STRIP")


def register(wasm, manifest):
    if not wasm.is_file() or not manifest.is_file():
        return
    package = tomllib.loads(manifest.read_text()).get("package", {}).get("name")
    projects = json.loads(Path(os.environ["AVM_MEMORY_TEST_PROJECTS"]).read_text())
    if package not in projects:
        raise ValueError(f"unregistered contract package: {package}")
    value = {"code_hash": blake3.blake3(wasm.read_bytes()).hexdigest(),
             "project": projects[package], "manifest": str(manifest), "wasm": str(wasm),
             "environment": {name: os.environ[name] for name in BUILD_INPUTS if name in os.environ}}
    snapshots = Path(os.environ["AVM_MEMORY_TEST_BUILDS"]).parent / "modules"
    snapshots.mkdir(exist_ok=True)
    shutil.copyfile(wasm, snapshots / (value["code_hash"] + ".wasm"))
    with open(os.environ["AVM_MEMORY_TEST_BUILDS"], "a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        stream.write(json.dumps(value) + "\n")


def main():
    mode, arguments = sys.argv[1], sys.argv[2:]
    real = os.environ[{"builder": "AVM_MEMORY_TEST_BUILDER", "stand": "AVM_MEMORY_TEST_STAND", "optimizer": "AVM_MEMORY_TEST_OPTIMIZER"}[mode]]
    environment = os.environ.copy()
    optimizer_input = None
    if mode == "optimizer" and "-o" in arguments:
        optimizer_input = next(Path(value) for value in arguments[:arguments.index("-o")] if Path(value).is_file())
        input_hash = blake3.blake3(optimizer_input.read_bytes()).hexdigest()
    if mode == "stand" and environment.get("AVM_MEMORY_TEST_VECTORS"):
        vectors = Path(environment["AVM_MEMORY_TEST_VECTORS"])
        vectors.mkdir(exist_ok=True)
        environment["AVM_COMBINED_EXECUTION_PATH"] = str(vectors / f"stand-{os.getpid()}.tsv")
        environment["AVM_MEMORY_REPORT"] = str(vectors / f"stand-{os.getpid()}.memory.jsonl")
    if mode == "builder" and arguments and arguments[0] == "build":
        if "--for-testing" not in arguments and environment.get("AVM_MEMORY_TEST_DEPLOYMENT") != "1":
            arguments.append("--for-testing")
    if mode == "stand" and "--wasm" in arguments:
        wasm = Path(arguments[arguments.index("--wasm") + 1]).resolve()
        for parent in wasm.parents:
            manifest = parent / "Cargo.toml"
            if manifest.is_file() and "package" in tomllib.loads(manifest.read_text()):
                register(wasm, manifest)
                break
    process = subprocess.run([real, *arguments], stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment)
    sys.stdout.buffer.write(process.stdout)
    sys.stderr.buffer.write(process.stderr)
    if process.returncode == 0 and mode == "builder" and arguments and arguments[0] == "build":
        manifest = Path(arguments[arguments.index("--manifest-path") + 1]).resolve()
        register(manifest.parent / "target/avm/contract.wasm", manifest)
    if process.returncode == 0 and optimizer_input is not None:
        output = Path(arguments[arguments.index("-o") + 1])
        records = [json.loads(line) for line in Path(os.environ["AVM_MEMORY_TEST_BUILDS"]).read_text().splitlines()]
        parent = next(row for row in reversed(records) if row["code_hash"] == input_hash)
        register(output, Path(parent["manifest"]))
        record = {"input_hash": input_hash, "output_hash": blake3.blake3(output.read_bytes()).hexdigest(),
                  "flags": [value for value in arguments[:arguments.index("-o")] if value != str(optimizer_input)],
                  "manifest": parent["manifest"]}
        with open(Path(os.environ["AVM_MEMORY_TEST_BUILDS"]).with_name("optimizations.jsonl"), "a") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX)
            stream.write(json.dumps(record) + "\n")
    if mode == "stand" and environment.get("AVM_MEMORY_TEST_VECTORS"):
        usage = Path(environment["AVM_MEMORY_REPORT"])
        if usage.is_file():
            with open(os.environ["AVM_MEMORY_REPORT"], "a") as stream:
                fcntl.flock(stream, fcntl.LOCK_EX)
                stream.write(usage.read_text())
    return process.returncode


if __name__ == "__main__":
    sys.exit(main())
