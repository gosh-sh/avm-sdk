#!/usr/bin/env python3
"""Check measured deployment maxima and rejected growth through real SDK commands."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from contract_memory import check_built_memory, memory_limits
from packaged_lifecycle import read_state


def shells(state, address):
    """The SHELL (currency 2) balance of one account of the developer state."""
    image = bytes.fromhex(read_state(state)[0][address]["image"])
    for offset in range(35, 35 + 20 * int.from_bytes(image[33:35], "little"), 20):
        if int.from_bytes(image[offset:offset + 4], "little") == 2:
            return int.from_bytes(image[offset + 4:offset + 20], "little")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("sdk", "builder", "stand", "profile", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    commands = []
    now = 1_800_000_000
    with tempfile.TemporaryDirectory(prefix="avm-memory-") as temporary:
        consumer = Path(temporary) / "consumer"
        shutil.copytree(args.sdk / "crates/avm-contract-sdk", consumer / "crates/avm-contract-sdk", ignore=shutil.ignore_patterns("target"))
        for name in ("Cargo.toml", "Cargo.lock", "LICENSE.md"):
            shutil.copy2(args.sdk / name, consumer / name)
        fixture = consumer / "scripts/tests/fixtures/memory-budget"
        shutil.copytree(args.sdk / "scripts/tests/fixtures/memory-budget", fixture, ignore=shutil.ignore_patterns("target"))
        manifest = fixture / "avm-contract.toml"
        text = manifest.read_text()
        prefix = "[build]\nmax_memory_bytes = 2097152\n\n"
        assert text.startswith(prefix)
        descriptor = text[len(prefix):]
        manifest.write_text(descriptor)
        report = Path(temporary) / "memory-runs.jsonl"
        environment = os.environ.copy()
        environment["AVM_MEMORY_REPORT"] = str(report)
        state = Path(temporary) / "developer.state"

        def run(argv, label, expected=0):
            process = subprocess.run([str(value) for value in argv], capture_output=True, env=environment)
            (args.output / (label + ".stdout")).write_bytes(process.stdout)
            (args.output / (label + ".stderr")).write_bytes(process.stderr)
            commands.append({"argv": [str(value) for value in argv], "exit": process.returncode})
            (args.output / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
            assert process.returncode == expected, process.stderr.decode() + process.stdout.decode()
            return process

        def build(label, testing=False, expected=0):
            argv = [args.builder.resolve(), "build", "--manifest-path", fixture / "Cargo.toml"]
            if testing:
                argv.append("--for-testing")
            return run(argv, label, expected)

        def stand(name, label, expected=0, **options):
            argv = [args.stand.resolve(), name, "--state", state, "--wasmtime-profile", args.profile.resolve()]
            for key, value in options.items():
                argv.append("--" + key.replace("_", "-"))
                if value is not True:
                    argv.append(str(value))
            process = run(argv, label, expected)
            return json.loads(process.stdout), process.stderr.decode()

        def timing():
            nonlocal now
            now += 1
            return dict(block_time=now, created_at=now, expire_at=now + 3600, fuel_fee_shells=1)

        def deploy(label):
            artifacts = fixture / "target/avm"
            options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab", descriptor=artifacts / "contract.descriptor",
                           salt_hex=label.encode().hex(), init_json="null", **timing())
            quoted, _ = stand("deploy", label + "-precondition", expected=2, **options)
            assert quoted["error"] == "funding_required"
            address = quoted["address"]
            stand("fund-user", label + "-fund", address=address, shells=1_000_000)
            value, diagnostic = stand("deploy", label + "-deploy", check_memory=True, **options)
            assert value["memory_check"]["passed"]
            assert set(value["memory_check"]) == {"passed", "runs"}
            return address, diagnostic

        missing = build("missing-measurement", expected=1)
        assert b"avm-memory.json" in missing.stderr and b"max_memory_bytes" in missing.stderr
        assert not (fixture / "target").exists()
        testing = build("measurement-build", testing=True)
        artifacts = fixture / "target/avm"
        assert (artifacts / "NOT-FOR-DEPLOYMENT").is_file()
        assert b"NOT FOR DEPLOYMENT" in testing.stderr
        assert "artifact_purpose: memory-test-only" in (fixture / "target/avm-build-provenance.txt").read_text()
        unmeasured = run([args.builder.resolve(), "record-memory", "--manifest-path", fixture / "Cargo.toml", "--report", report], "bootstrap-is-not-a-measurement", expected=1)
        assert b"bootstrap resources are not a measurement" in unmeasured.stderr
        manifest.write_text("[build]\nstack_bytes = 16384\narena_bytes = 65536\n\n" + descriptor)
        build("bounded-resource-test-build", testing=True)
        initial, testing_maximum = memory_limits((artifacts / "contract.wasm").read_bytes())
        initialized, _ = stand("init", "init", dapp_id="11" * 32)
        user_key = initialized["identities"]["user"]
        address, _ = deploy("measure")
        control, _ = stand("call", "control-growth", address=address, method="memory.probe", args_json="0", identity="user", check_memory=True, **timing())
        # The input is read in place, so the probe sees the initial memory; the output then
        # takes a heap page. One grown page therefore peaks one page above the control call.
        # The data starts with the owner key, the deploy's owner_id (the user).
        control_data = read_state(state)[0][address]["data"]
        assert control_data[:32] == bytes.fromhex(user_key)
        control_pages = int.from_bytes(control_data[32:], "little")
        assert control_pages * 65_536 == control["memory_check"]["runs"][0]["initial_bytes"]
        control_peak = control["memory_check"]["runs"][0]["peak_bytes"]
        grown, _ = stand("call", "successful-growth", address=address, method="memory.probe", args_json="1", identity="user", check_memory=True, **timing())
        assert grown["memory_check"]["runs"][0]["peak_bytes"] == control_peak + 65_536
        run([args.builder.resolve(), "record-memory", "--manifest-path", fixture / "Cargo.toml", "--report", report], "record")
        record = json.loads((fixture / "avm-memory.json").read_text())
        peak = record["peak_memory_bytes"]
        assert peak == control_peak + 65_536 and record["test_calls"] >= 3
        assert record["version"] == 2 and record["stack_bytes"] == 16384 and record["arena_bytes"] == 65536
        manifest.write_text(descriptor)
        deployment = build("deployment-build")
        provenance = (fixture / "target/avm-build-provenance.txt").read_text()
        check_built_memory((artifacts / "contract.wasm").read_bytes(), peak, provenance)
        assert not (artifacts / "NOT-FOR-DEPLOYMENT").exists()
        assert "artifact_purpose: deployment" in provenance and "memory_measurement_blake3:" in provenance
        assert b"largest state and inputs" in deployment.stderr
        measured_address, warning = deploy("measured")
        _, warning = stand("call", "near-limit-growth", address=measured_address, method="memory.probe", args_json="1", identity="user", check_memory=True, **timing())
        assert "80%" in warning
        rejected, diagnostic = stand("call", "rejected-growth", expected=2, address=measured_address, method="memory.probe", args_json="1000", identity="user", check_memory=True, **timing())
        assert rejected["status"] == "ok" and not rejected["memory_check"]["passed"]
        assert rejected["memory_check"]["runs"][0]["rejected_growths"] == 1
        assert "memory.grow was rejected" in diagnostic
        assert int.from_bytes(read_state(state)[0][measured_address]["data"][32:], "little") == 0xFFFFFFFF
        before_overflow = read_state(state)[0][measured_address]["data"]
        before_overflow_shells = shells(state, measured_address)
        trapped, _ = stand("call", "stack-overflow", expected=2, address=measured_address, method="memory.probe", args_json=str(0x8000_0000 | 1024), identity="user", check_memory=True, **timing())
        assert read_state(state)[0][measured_address]["data"] == before_overflow
        # The call was admitted: the trap commits its charge and keeps none of
        # its guest effects.
        assert trapped["error"] == "execution_failed" and trapped["status_code"] == 30
        assert "VM_WASM_TRAP" in trapped["detail"] and trapped["state_committed"]
        charged = int(trapped["receipt"]["charged_shells"])
        assert charged > 0 and shells(state, measured_address) == before_overflow_shells - charged
        repeated, _ = stand("call", "stack-overflow-repeat", expected=2, address=measured_address, method="memory.probe", args_json=str(0x8000_0000 | 1024), identity="user", check_memory=True, **timing())
        assert (repeated["error"], repeated["status_code"]) == (trapped["error"], trapped["status_code"])
        assert read_state(state)[0][measured_address]["data"] == before_overflow
        before = (fixture / "avm-memory.json").read_bytes()
        manifest.write_text(f"[build]\nmax_memory_bytes = {peak}\nstack_bytes = 16384\narena_bytes = 65536\n\n" + descriptor)
        build("explicit-test-build", testing=True)
        failure = run([args.builder.resolve(), "record-memory", "--manifest-path", fixture / "Cargo.toml", "--report", report], "rejected-record", expected=1)
        assert b"growth was rejected" in failure.stderr
        assert (fixture / "avm-memory.json").read_bytes() == before
        empty, _ = stand("inspect", "empty-capture", expected=2, check_memory=True)
        assert not empty["memory_check"]["passed"]
        (args.output / "result.json").write_text(json.dumps({"passed": True, "initial_memory_bytes": initial,
            "peak_memory_bytes": peak, "deployment_maximum_bytes": peak, "testing_maximum_bytes": testing_maximum,
            "test_calls": record["test_calls"], "marker_and_purpose_verified": True, "warning_verified": True,
            "rejected_growth_preserved": True, "failed_record_preserved_previous_measurement": True,
            "rust_stack_overflow_trap_repeated": True, "overflow_preserved_account_data": True,
            "temporary_state_removed_on_exit": True}, indent=2) + "\n")
    print("PASS: measured deployment maximum, marker, provenance, warning and rejected growth")


if __name__ == "__main__":
    main()
