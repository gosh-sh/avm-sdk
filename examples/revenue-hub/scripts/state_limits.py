#!/usr/bin/env python3
"""Black-box state-mode correctness checks with explicitly selected local tools.

No release qualification or mainnet execution is claimed.
Uses a deliberately non-financial fixture and fresh disposable stand state.
"""
import argparse
import hashlib
import json
from pathlib import Path
import tempfile
from e2e import Stand, DAPP

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests/fixtures/state-boundary/target/avm"


def deploy(stand, length, salt):
    options = {key: str(FIXTURE / name) for key, name in [
        ("wasm", "contract.wasm"), ("acab", "contract.acab"),
        ("descriptor", "contract.descriptor")]}
    if "boundary" in stand.selections:
        options = dict(stand.selections["boundary"])
    options.update(salt_hex=salt.encode().hex(),
                   init_json=json.dumps("0x" + length.to_bytes(4, "little").hex()),
                   **stand.timestamps())
    result = stand.command("deploy", quote_address=True, **options)
    assert result.get("quoted") is True, result
    address = result["address"]
    assert stand.command("fund-user", address=address, shells=10_000_000)["status"] == "ok"
    deployed = stand.command("deploy", **options)
    assert deployed["status"] == "ok", deployed
    if "boundary" not in stand.selections:
        entries = stand.command("inspect")["registry_entries"]
        entry = next(e for e in entries if e["key"].startswith("02"))
        stand.selections["boundary"] = {"selected_code_id": entry["value"][:64],
                                         "package_id": entry["value"][64:128]}
    return address


def account_size(stand, address):
    return next(a["canonical_byte_length"] for a in stand.command("inspect")["accounts"]
                if a["address"] == address)


def call_mode(stand, address, expected, force=0, extra=""):
    result = stand.call(address, "boundary.update", bytes([force]), extra=extra)
    raw = bytes.fromhex(result["raw_executor_output"])
    state = bytes.fromhex(result["raw_state"])
    assert raw[9] == state[4] == expected, (raw[:20].hex(), state[:8].hex(), expected)
    return {"mode": ["FullReplace", "DataPatchList"][raw[9]],
            "data_bytes": len(state), "output_bytes": len(raw)}


def mismatch(stand, address, force):
    result = stand.command("call", address=address, method="boundary.update",
                           args_json=json.dumps("0x" + bytes([force]).hex()),
                           identity="package-owner", extra_currencies="",
                           **stand.timestamps())
    assert result["status"] == "error", result
    # This is the exact released diagnostic, not reinterpreted as output overflow.
    assert "43" in str(result), result
    return result


def run(sdk, output):
    rows = []
    with tempfile.TemporaryDirectory(prefix="hub-state-limits-") as directory:
        stand = Stand(sdk, Path(directory))
        assert stand.command("init", dapp_id=DAPP)["status"] == "ok"
        # 39 fixed bytes + one 20-byte SHELL currency entry in these accounts.
        for canonical, expected in [(65_535, 0), (65_536, 0), (65_537, 1)]:
            data = canonical - 59
            address = deploy(stand, data, f"boundary-{canonical}")
            assert account_size(stand, address) == canonical
            follow = call_mode(stand, address, expected)
            forced = mismatch(stand, address, 2 if expected == 0 else 1)
            assert account_size(stand, address) == canonical
            again = call_mode(stand, address, expected)
            rows.append({"canonical_account_bytes": canonical, "data_bytes": data,
                         "first_update": follow, "wrong_mode": forced, "next_update": again})
        # Same data length, one more balance entry: it is the *previous* image
        # that selects the mode. Incoming new currency crosses the threshold,
        # and the subsequent update must switch to patches.
        address = deploy(stand, 65_477, "balance-transition")
        assert account_size(stand, address) == 65_536
        before = call_mode(stand, address, 0, extra="7:1")
        assert account_size(stand, address) == 65_556
        after = call_mode(stand, address, 1)
        replay = stand.command("replay")
        inspect = stand.command("inspect")
        assert replay["status"] == "ok"
        for key in ["account_state_root", "registry_root", "cumulative_fee_shells", "accepted_input_count"]:
            assert replay[key] == inspect[key]
        assert inspect["queue"]["message_count"] == 0
        result = {"status": "PASS", "scope": "Explicit-tool local stand correctness only",
                  "fixture_sha256": hashlib.sha256((FIXTURE / "contract.wasm").read_bytes()).hexdigest(),
                  "threshold_cases": rows,
                  "balance_transition": {"previous_canonical_bytes": 65_536,
                      "incoming_new_currency_call": before, "resulting_canonical_bytes": 65_556,
                      "following_call": after},
                  "replay_matches": True, "release_qualification": False,
                  "mainnet_deployment": False, "real_funds": False}
        output.write_text(json.dumps(result, indent=2) + "\n")
        print("Local canonical threshold/balance transition/mode mismatch/replay: PASS")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "evidence/state-limits.json")
    args = parser.parse_args()
    run(args.sdk.resolve(), args.output.resolve())
