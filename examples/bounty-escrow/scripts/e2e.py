#!/usr/bin/env python3
"""Reproducible, real AVM stand journey. No network and no real funds.

Run after the official builder produces both artifact triples.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
DAPP = "11" * 32
BASE_TIME = 1_800_000_000

class Stand:
    def __init__(self, sdk, work):
        self.binary = str(sdk / "bin/avm-local-stand")
        self.profile = str(sdk / "profiles/wasmtime-release-profile.v1")
        self.state = work / "journey.state"
        self.log = []
        self.time = BASE_TIME
        self.selections = {}

    def command(self, operation, **options):
        cmd = [self.binary, operation, "--state", str(self.state), "--wasmtime-profile", self.profile]
        for key, value in options.items():
            cmd += ["--" + key.replace("_", "-")] + ([] if value is True else [str(value)])
        proc = subprocess.run(cmd, capture_output=True, text=True)
        try:
            result = json.loads(proc.stdout)
        except json.JSONDecodeError:
            raise RuntimeError(f"{operation}: {proc.stderr}\n{proc.stdout}") from None
        self.log.append({"operation": operation, "options": options, "exit_code": proc.returncode, "result": result})
        return result

    def timestamps(self):
        self.time += 1
        return dict(block_time=self.time, created_at=self.time, expire_at=self.time+3600, fuel_fee_shells=10000)

    def deploy(self, contract, salt, init=None):
        artifacts = ROOT / "contracts" / contract / "target/avm"
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
                       descriptor=artifacts / "contract.descriptor", salt_hex=salt.encode().hex(),
                       init_json=json.dumps("0x"+(b"" if init is None else init).hex()), **self.timestamps())
        if contract in self.selections:
            for key in ["wasm", "acab", "descriptor"]:
                del options[key]
            options.update(self.selections[contract])
        # Convert paths to text for the evidence journal.
        options = {key: str(value) if isinstance(value, Path) else value for key,value in options.items()}
        precondition = self.command("deploy", quote_address=True, **options)
        assert precondition.get("quoted") is True, precondition
        address = precondition["address"]
        funding = self.command("fund-user", address=address, shells=10_000_000)
        assert funding["status"] == "ok", funding
        result = self.command("deploy", **options)
        assert result["status"] == "ok", result
        if contract not in self.selections:
            snapshot = self.command("inspect")
            existing = {v["selected_code_id"] for v in self.selections.values()}
            selection = next(item for item in snapshot["registry_entries"]
                             if item["key"].startswith("02") and item["value"][:64] not in existing)
            self.selections[contract] = {"selected_code_id": selection["value"][:64],
                                         "package_id": selection["value"][64:128]}
        return address

    def call(self, address, method, body=b"", identity="developer", value=0, extra="", expected_status=0):
        result = self.command("call", address=address, method=method,
                            args_json=json.dumps("0x"+body.hex()) if body else "null",
                            identity=identity, extra_currencies=(f"0:{value}," if value and extra else f"0:{value}" if value else "")+extra, **self.timestamps())
        assert result["status"] == "ok", result
        raw = bytes.fromhex(result["raw_executor_output"])
        actual_status = int.from_bytes(raw[4:8], "little")
        assert raw[:4] == b"ACMO" and actual_status == expected_status, {"method": method, "contract_status": actual_status, "expected": expected_status}
        return result

def assert_state(result, funded, paid, refunded, pending, attempts, phases):
    raw = bytes.fromhex(result["raw_state"])
    assert len(raw) == 2768, len(raw)
    actual = dict(funded=int.from_bytes(raw[360:376],"little"),
                  paid=int.from_bytes(raw[376:392],"little"),
                  refunded=int.from_bytes(raw[392:408],"little"),
                  pending=raw[416], attempts=int.from_bytes(raw[408:416],"little"),
                  phases=list(raw[448:451]))
    expected = dict(funded=funded, paid=paid, refunded=refunded, pending=pending, attempts=attempts, phases=phases)
    assert actual == expected, {"actual":actual,"expected":expected}
    return actual

def assert_event(result, name):
    assert any(name.encode() in bytes.fromhex(event) for event in result["events"]), name

def address_bytes(value):
    return bytes.fromhex(value.replace(":", ""))

def config(owner, worker, refund, pay):
    data = bytearray(360)
    data[:32] = bytes.fromhex(owner)
    data[32:64] = bytes.fromhex(worker)
    data[64:128] = address_bytes(refund)
    data[128:192] = address_bytes(pay)
    data[192:208] = (1000).to_bytes(16, "little")
    data[208:212] = (BASE_TIME+100).to_bytes(4, "little")
    data[212] = 3
    for i,amount in enumerate([300,400,300]):
        data[216+i*16:232+i*16] = amount.to_bytes(16,"little")
    data[344:360] = (10000).to_bytes(16,"little")
    return bytes(data)

def run(sdk, output):
    with tempfile.TemporaryDirectory(prefix="avm-bounty-") as directory:
        stand = Stand(sdk, Path(directory))
        try:
            init = stand.command("init", dapp_id=DAPP)
            assert init["status"] == "ok", init
            owner, worker = init["identities"]["developer"], init["identities"]["user"]
            pay = stand.deploy("receiver", "worker")
            refund = stand.deploy("receiver", "client")
            escrow = stand.deploy("escrow", "bounty", config(owner, worker, refund, pay))
            stand.call(escrow, "bounty.escrow.status")
            stand.call(escrow, "bounty.escrow.fund", extra="7:1000", expected_status=2004)
            # Bind both fixtures using their observed init authority in the
            # pinned stand. Escrow is initialized by package-owner here.
            for receiver,identity in [(pay,"developer"),(refund,"developer")]:
                result = stand.call(receiver, "bounty.receiver.bind", address_bytes(escrow), identity=identity)
                assert result["status"] == "ok", result
            result = stand.call(escrow, "bounty.escrow.fund", value=1000)
            assert result["status"] == "ok", result
            assert_state(result,1000,0,0,0,0,[0,0,0])
            stand.call(escrow,"bounty.escrow.fund",value=1000,expected_status=2003)
            stand.call(escrow,"bounty.escrow.refund",expected_status=2005)
            # Intentionally rejected destination: actual runtime rollback and
            # callback principal return, followed by an approved payment retry.
            result = stand.call(pay, "bounty.receiver.reject", b"\x01", identity="developer")
            assert result["status"] == "ok", result
            body = b"\x00" + b"\x42"*32
            stand.call(escrow,"bounty.escrow.submit",body,identity="developer",expected_status=2001)
            for method,identity in [("submit","user"),("approve","developer")]:
                result = stand.call(escrow,"bounty.escrow."+method,body,identity)
                assert result["status"] == "ok", result
            result = stand.call(escrow,"bounty.escrow.pay",b"\x00",identity="user")
            assert result["status"] == "ok", result
            assert_state(result,1000,0,0,0,1,[2,0,0])
            assert_event(result,"escrow.transfer_failed")
            result = stand.call(pay,"bounty.receiver.reject",b"\x00",identity="developer")
            assert result["status"] == "ok", result
            result = stand.call(escrow,"bounty.escrow.pay",b"\x00",identity="user")
            assert result["status"] == "ok", result
            assert_state(result,1000,300,0,0,2,[3,0,0])
            assert_event(result,"escrow.transfer_settled")
            stand.call(escrow,"bounty.escrow.pay",b"\x00",identity="user",expected_status=2003)
            # Approval before the deadline remains payable after it; refund
            # excludes the approved 400 and returns only the unapproved 300.
            body = b"\x01" + b"\x43"*32
            for method,identity in [("submit","user"),("approve","developer")]:
                result = stand.call(escrow,"bounty.escrow."+method,body,identity)
                assert result["status"] == "ok", result
            stand.time = BASE_TIME+100
            stand.call(refund,"bounty.receiver.reject",b"\x01",identity="developer")
            result = stand.call(escrow,"bounty.escrow.refund")
            assert result["status"] == "ok", result
            assert_state(result,1000,300,0,0,3,[3,2,0])
            assert_event(result,"escrow.transfer_failed")
            stand.call(refund,"bounty.receiver.reject",b"\x00",identity="developer")
            result = stand.call(escrow,"bounty.escrow.refund")
            assert_state(result,1000,300,300,0,4,[3,2,4])
            result = stand.call(escrow,"bounty.escrow.pay",b"\x01",identity="user")
            assert result["status"] == "ok", result
            assert_state(result,1000,700,300,0,5,[3,3,4])
            stand.call(escrow,"bounty.escrow.refund",expected_status=2003)
            stand.call(escrow,"bounty.escrow.submit",b"\x02"+b"\x44"*32,identity="user",expected_status=2005)
            result = stand.call(escrow,"bounty.escrow.status")
            assert result["status"] == "ok", result
            final = assert_state(result,1000,700,300,0,5,[3,3,4])
            events = stand.command("events")
            inspect = stand.command("inspect")
            replay = stand.command("replay")
            assert replay["status"] == "ok", replay
            assert inspect["queue"]["message_count"] == 0, inspect["queue"]
            for key in ["account_state_root", "registry_root", "cumulative_fee_shells", "accepted_input_count"]:
                assert replay[key] == inspect[key], (key,replay[key],inspect[key])
            output.write_text(json.dumps({"status":"PASS", "accounting":final, "escrow":escrow, "events":events,
                                         "inspect":inspect, "replay":replay, "journal":stand.log}, indent=2)+"\n")
            print(f"AVM journey PASS: {output}")
        except Exception:
            output.write_text(json.dumps({"status":"FAIL", "journal":stand.log}, indent=2)+"\n")
            raise

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "evidence/avm-e2e.json")
    args = parser.parse_args()
    run(args.sdk.resolve(), args.output.resolve())
