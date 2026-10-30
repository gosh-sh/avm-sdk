#!/usr/bin/env python3
"""Check the separate new-install compact audit with the selected SDK tools."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

EXP = Path(__file__).resolve().parents[1]
ROOT = EXP.parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--optimizer", type=Path, required=True)
    args = parser.parse_args()
    sdk, out = args.sdk.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    scratch = out / "tmp"
    scratch.mkdir(exist_ok=True)
    env = os.environ.copy()
    env.update(TMPDIR=str(scratch), TMP=str(scratch), TEMP=str(scratch), CARGO_TARGET_DIR=str(out / "target"),
               COMPACT_VECTORS=str(out / "guest-vectors.tsv"))
    commands = []

    def run(label, command):
        command = [str(part) for part in command]
        result = subprocess.run(command, cwd=EXP, env=env, text=True, capture_output=True)
        (out / (label + ".log")).write_text(result.stdout + result.stderr)
        commands.append({"label": label, "command": command, "exit_code": result.returncode})
        (out / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        if result.returncode:
            raise SystemExit(f"{label} failed; see {out / (label + '.log')}")

    run("format", ["cargo", "+1.93.0", "fmt", "--", "--check"])
    run("native-tests", ["cargo", "+1.93.0", "test", "--locked", "--offline"])
    run("native-clippy", ["cargo", "+1.93.0", "clippy", "--all-targets", "--locked", "--offline", "--", "-D", "warnings"])
    run("wasm-clippy", ["cargo", "+1.93.0", "clippy", "--lib", "--target", "wasm32-unknown-unknown", "--locked", "--offline", "--", "-D", "warnings"])
    run("build", [sdk / "bin/avm-contract-build", "build", "--manifest-path", EXP / "Cargo.toml"])
    run("optimization", ["python3", ROOT / "scripts/optimize.py", "--wasm-opt", args.optimizer.resolve(), "--out", out / "optimization.json", "--compact"])
    run("guest-vectors", ["node", EXP / "scripts/guest_modes.mjs", EXP / "target/avm/contract.wasm", out / "guest-vectors.tsv", out / "guest-vectors.json"])
    run("journey", ["python3", EXP / "scripts/e2e.py", "--sdk", sdk, "--output", out / "journey.json"])
    run("runtime-probes", ["python3", EXP / "scripts/runtime_probes.py", "--sdk", sdk, "--output", out / "runtime-probes.json"])
    artifacts = {name: hashlib.sha256((EXP / "target/avm" / name).read_bytes()).hexdigest()
                 for name in ("contract.wasm", "contract.acab", "contract.descriptor")}
    (out / "summary.json").write_text(json.dumps({"status": "PASS", "scope": "local correctness; separate new installs only", "artifacts": artifacts, "commands": commands}, indent=2) + "\n")
    print("Compact audit local checks: PASS")


if __name__ == "__main__":
    main()
