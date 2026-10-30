#!/usr/bin/env python3
"""Candidate tooling integration gate: actual guest WASM and public CLI, no state injection.

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
    stand("fund-user", "--address", address, "--shells", 17)
    pending = next(item for item in stand("inspect")["funding"] if item["address"] == address)
    assert pending["extra"] == [{"currency_id": 2, "amount": "10000017"}]
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
                        "--extra-currencies", "", *timing())
        raw = bytes.fromhex(receipt["raw_executor_output"])
        assert raw[:4] == b"ACMO" and struct.unpack_from("<I", raw, 4)[0] == 0
        return receipt

    call("tools.add", {"a": 5, "b": 7})
    assert inspect() == struct.pack("<Q", 12), "generated internal dispatch did not run"
    before_topup = stand("inspect")
    before_account = stand("inspect", "--account", address)
    for shells in (13, 17):
        assert stand("fund-user", "--address", address, "--shells", shells)["status"] == "ok"
    after_topup = stand("inspect")
    after_account = stand("inspect", "--account", address)
    assert after_account["account_hash"] != before_account["account_hash"]
    assert {k: v for k, v in after_account.items() if k != "account_hash"} == {
        k: v for k, v in before_account.items() if k != "account_hash"}
    for key in ("registry_root", "receipts", "cumulative_fee_shells", "accepted_input_count"):
        assert after_topup[key] == before_topup[key], key
    assert len(before_topup["accounts"]) == len(after_topup["accounts"])
    for old, new in zip(before_topup["accounts"], after_topup["accounts"]):
        assert {k: v for k, v in old.items() if k != "account_hash"} == {
            k: v for k, v in new.items() if k != "account_hash"}

    # Publish a distinct, unchanged example into this already populated stand.
    second_contract = args.sdk / "examples/contract-b"
    run([args.builder, "build", "--manifest-path", second_contract / "Cargo.toml"])
    second_artifact = second_contract / "target/avm"
    assert (second_artifact / "contract.wasm").read_bytes() != (artifact / "contract.wasm").read_bytes()
    now += 10
    second_deploy = ["--wasm", second_artifact / "contract.wasm", "--acab", second_artifact / "contract.acab",
                     "--descriptor", second_artifact / "contract.descriptor", "--salt-hex", "1751",
                     "--init-json", '"0x"', *timing()]
    second_quote = stand("deploy", *second_deploy, "--quote-address")
    assert second_quote["package_id"] != quote["package_id"]
    second_address = second_quote["address"]
    stand("fund-user", "--address", second_address, "--shells", 10_000_000)
    before_second = stand("inspect")
    assert stand("deploy", *second_deploy)["status"] == "ok"
    after_second = stand("inspect")
    old_registry = {row["key"]: row["value"] for row in before_second["registry_entries"]}
    new_registry = {row["key"]: row["value"] for row in after_second["registry_entries"]}
    assert all(new_registry[key] == value for key, value in old_registry.items())
    assert len(new_registry) == len(old_registry) + 5
    assert len(after_second["code_blobs"]) == len(before_second["code_blobs"]) + 1
    assert stand("inspect", "--account", address) == after_account
    assert bytes.fromhex(stand("inspect", "--account", second_address)["raw_state"]) == bytes(128)
    assert int(after_second["cumulative_fee_shells"]) - int(before_second["cumulative_fee_shells"]) == 11011
    assert after_second["counters"]["init_executor_calls"] == before_second["counters"]["init_executor_calls"] + 1
    call("tools.add", {"a": 8, "b": 9})
    assert inspect() == struct.pack("<Q", 17), "first package must remain executable"
    assert stand("replay")["status"] == "ok"
    summary = {"status": "passed", "paths": ["constructor", "manifest_bindings", "abi_inspection_and_body_decode", "generated_external_internal_dispatch", "read_only_account_inspection", "quote_fund_deploy", "repeated_placeholder_and_executable_topups", "independent_package_in_populated_stand", "original_package_continuation", "replay"],
               "state_sha256": hashlib.sha256(state.read_bytes()).hexdigest(),
               "wasm_sha256": hashlib.sha256((artifact / "contract.wasm").read_bytes()).hexdigest(),
               "second_wasm_sha256": hashlib.sha256((second_artifact / "contract.wasm").read_bytes()).hexdigest()}
    (args.out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
