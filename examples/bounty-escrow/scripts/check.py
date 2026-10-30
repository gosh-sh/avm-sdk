#!/usr/bin/env python3
"""Build the guests and run the local regression journey with explicit tools."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path)
    for name in ("builder", "stand", "profile", "out"):
        parser.add_argument("--" + name, type=Path, required=name == "out")
    args = parser.parse_args()
    if args.sdk:
        args.builder = args.builder or args.sdk / "bin/avm-contract-build"
        args.stand = args.stand or args.sdk / "bin/avm-local-stand"
        args.profile = args.profile or args.sdk / "profiles/wasmtime-release-profile.v1"
    if not all((args.builder, args.stand, args.profile)):
        parser.error("supply --sdk or --builder, --stand and --profile")
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    scratch = out / "tmp"
    scratch.mkdir(exist_ok=True)
    env = os.environ.copy()
    env.update(TMPDIR=str(scratch), TMP=str(scratch), TEMP=str(scratch), CARGO_TARGET_DIR=str(out / "target"))
    runtime = out / "runtime"
    inputs = {}
    for name, source in (("bin/avm-contract-build", args.builder),
                         ("bin/avm-local-stand", args.stand),
                         ("profiles/wasmtime-release-profile.v1", args.profile)):
        source = source.resolve(strict=True)
        target = runtime / name
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.is_symlink():
            target.unlink()
        target.symlink_to(source)
        inputs[name] = {"path": str(source), "sha256": hashlib.sha256(source.read_bytes()).hexdigest()}
    (out / "inputs.json").write_text(json.dumps(inputs, indent=2) + "\n")
    commands = []

    def run(label, command):
        command = [str(item) for item in command]
        result = subprocess.run(command, cwd=ROOT, env=env, text=True, capture_output=True)
        (out / (label + ".log")).write_text(result.stdout + result.stderr)
        commands.append({"label": label, "command": command, "exit_code": result.returncode})
        (out / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        if result.returncode:
            raise SystemExit(f"{label} failed; see {out / (label + '.log')}")

    run("format", ["cargo", "+1.93.0", "fmt", "--all", "--", "--check"])
    run("native-clippy", ["cargo", "+1.93.0", "clippy", "--workspace", "--all-targets", "--locked", "--offline", "--", "-D", "warnings"])
    run("native-tests", ["cargo", "+1.93.0", "test", "--workspace", "--locked", "--offline"])
    for contract in ("escrow", "receiver"):
        run("build-" + contract, [args.builder, "build", "--manifest-path", ROOT / "contracts" / contract / "Cargo.toml"])
        run("wasm-clippy-" + contract, ["cargo", "+1.93.0", "clippy", "-p", "avm-bounty-" + contract, "--target", "wasm32-unknown-unknown", "--locked", "--offline", "--", "-D", "warnings"])
    run("canonical-ids", ["python3", "scripts/sync_ids.py", "--check"])
    run("artifact-inspection", ["python3", "scripts/verify_artifacts.py", "--out", out / "artifacts.json"])
    run("journey", ["python3", "scripts/e2e.py", "--sdk", runtime, "--output", out / "journey.json"])
    (out / "summary.json").write_text(json.dumps({"status": "PASS", "scope": "local correctness only", "principal_currency_id": 0, "commands": commands, "inputs": inputs}, indent=2) + "\n")
    print("Bounty escrow local checks: PASS")


if __name__ == "__main__":
    main()
