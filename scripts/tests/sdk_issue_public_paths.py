#!/usr/bin/env python3
"""Candidate-only pod gate: actual guest WASM and public CLI, no state injection.

Pass a writable exported SDK source copy and isolated evidence directory. This
is not a release gate result until its exact SDK/runtime heads are recorded.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess


def main():
    parser = argparse.ArgumentParser()
    for name in ("sdk", "builder", "stand", "profile", "out"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    evidence = []

    def run(command, expected=0):
        result = subprocess.run([str(part) for part in command], text=True, capture_output=True)
        evidence.append({"command": [str(part) for part in command], "exit": result.returncode,
                         "stdout": result.stdout, "stderr": result.stderr})
        (args.out / "commands.json").write_text(json.dumps(evidence, indent=2) + "\n")
        assert (result.returncode == 0) == (expected == 0), result.stderr + result.stdout
        return result.stdout

    contract = args.sdk / "examples/contract-tools"
    run([args.builder, "build", "--manifest-path", contract / "Cargo.toml"])
    artifact = contract / "target/avm"
    abi = json.loads(run([args.builder, "abi", "--acab", artifact / "contract.acab", "--json"]))
    methods = {method["name"]: method for method in abi["methods"]}
    assert methods["tools.add"]["input_fixed_size"] == 16
    assert {error["status_code"] for error in abi["errors"]} == {1750, 1751, 1752, 1753}
    body = args.out / "body.bin"
    body.write_bytes(struct.pack("<QQ", 5, 7))
    decoded = json.loads(run([args.builder, "abi", "--acab", artifact / "contract.acab", "--json", "--method", "tools.add", "--body-file", body]))
    assert decoded["decoded_body"] == {"a": 5, "b": 7}
    body.write_bytes(body.read_bytes() + b"\0")
    run([args.builder, "abi", "--acab", artifact / "contract.acab", "--method", "tools.add", "--body-file", body], expected=1)
    state = args.out / "public.state"
    now = 1_800_000_000

    def stand(command, *rest, expected=0):
        return json.loads(run([args.stand, command, "--state", state, "--wasmtime-profile", args.profile, *rest], expected))

    def timing():
        return ["--block-time", now, "--created-at", now, "--expire-at", now + 3600, "--fuel-fee-shells", 10000]

    stand("init", "--dapp-id", "75" * 32)
    deploy = ["--wasm", artifact / "contract.wasm", "--acab", artifact / "contract.acab",
              "--descriptor", artifact / "contract.descriptor", "--salt-hex", "1750",
              "--init-json", '"0x010203"', *timing()]
    before = state.read_bytes()
    quote = stand("deploy", *deploy, "--quote-address")
    assert quote["quoted"] is True and state.read_bytes() == before
    assert quote == stand("deploy", *deploy, "--quote-address")
    missing = stand("deploy", *deploy, expected=1)
    assert missing["error"] == "funding_required" and missing["address"] == quote["address"]
    assert state.read_bytes() == before
    address = quote["address"]
    stand("fund-user", "--address", address, "--shells", 10_000_000)
    assert all(account["address"] != address for account in stand("inspect")["accounts"])
    assert stand("deploy", *deploy)["status"] == "ok"

    def inspect():
        before = state.read_bytes()
        value = stand("inspect", "--account", address)
        assert state.read_bytes() == before, "inspection mutated persisted state"
        return bytes.fromhex(value["raw_state"])

    assert inspect() == b"\x01\x02\x03", "constructor must decode init_data, not ACDP"

    def call(method, value):
        nonlocal now
        now += 10
        receipt = stand("call", "--address", address, "--method", method,
                        "--args-json", json.dumps(value), "--identity", "developer",
                        "--value-main", 0, "--extra-currencies", "", *timing())
        raw = bytes.fromhex(receipt["raw_executor_output"])
        assert raw[:4] in (b"ACMO", b"ACM2") and struct.unpack_from("<I", raw, 4)[0] == 0
        return receipt

    call("tools.add", {"a": 5, "b": 7})
    assert inspect() == struct.pack("<Q", 12), "generated internal dispatch did not run"
    call("tools.cycle", True)
    same_block = inspect()
    assert same_block == bytes([1, 0, 0, 0, 0, 0, 0, 0]), "same-block reader saw a newly emitted event"
    snapshot = stand("inspect")
    assert snapshot["queue"]["message_count"] == 0
    call("tools.cycle", False)
    next_block = inspect()
    assert next_block[:8] == bytes([1, 0, 0, 0, 0, 0, 1, 0]), "next-block result must contain one event"
    assert struct.unpack_from("<Q", next_block, 56)[0] == 0x7175
    name_len, topic_count, fields_len = struct.unpack_from("<HHI", next_block, 64)
    assert next_block[72:72 + name_len] == b"tools.changed" and topic_count == 0
    fields = next_block[72 + name_len:]
    assert len(fields) == fields_len and fields.endswith(struct.pack("<Q", 17))
    assert stand("replay")["status"] == "ok"
    summary = {"status": "passed", "paths": ["constructor", "manifest_bindings", "abi_inspection_and_body_decode", "generated_external_internal_dispatch", "read_only_account_inspection", "quote_fund_deploy", "guest_internal_event_read_same_block_and_next_block", "replay"],
               "state_sha256": hashlib.sha256(state.read_bytes()).hexdigest(),
               "wasm_sha256": hashlib.sha256((artifact / "contract.wasm").read_bytes()).hexdigest()}
    (args.out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
