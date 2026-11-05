#!/usr/bin/env python3
"""Instrument the existing example test commands without changing their outcomes."""

import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib

import blake3


def register(wasm, manifest):
    if not wasm.is_file() or not manifest.is_file():
        return
    package = tomllib.loads(manifest.read_text()).get("package", {}).get("name")
    projects = json.loads(Path(os.environ["AVM_MEMORY_TEST_PROJECTS"]).read_text())
    if package not in projects:
        raise ValueError(f"unregistered contract package: {package}")
    value = {"code_hash": blake3.blake3(wasm.read_bytes()).hexdigest(),
             "project": projects[package], "manifest": str(manifest), "wasm": str(wasm)}
    with open(os.environ["AVM_MEMORY_TEST_BUILDS"], "a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        stream.write(json.dumps(value) + "\n")


def main():
    mode, arguments = sys.argv[1], sys.argv[2:]
    real = os.environ["AVM_MEMORY_TEST_BUILDER" if mode == "builder" else "AVM_MEMORY_TEST_STAND"]
    if mode == "builder" and arguments and arguments[0] == "build":
        if "--for-testing" not in arguments:
            arguments.append("--for-testing")
    if mode == "stand" and "--wasm" in arguments:
        wasm = Path(arguments[arguments.index("--wasm") + 1]).resolve()
        for parent in wasm.parents:
            manifest = parent / "Cargo.toml"
            if manifest.is_file() and "package" in tomllib.loads(manifest.read_text()):
                register(wasm, manifest)
                break
    process = subprocess.run([real, *arguments], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    sys.stdout.buffer.write(process.stdout)
    sys.stderr.buffer.write(process.stderr)
    if process.returncode == 0 and mode == "builder" and arguments and arguments[0] == "build":
        manifest = Path(arguments[arguments.index("--manifest-path") + 1]).resolve()
        register(manifest.parent / "target/avm/contract.wasm", manifest)
    return process.returncode


if __name__ == "__main__":
    sys.exit(main())
