#!/usr/bin/env python3
"""Reproducible, real AVM stand journey. No network and no real funds.

Run after the official builder produces both artifact triples.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
WALLET = ROOT.parent / "wallet"
DAPP = "11" * 32
BASE_TIME = 1_800_000_000
# Method IDs of the escrow ABI, from the generated ids.rs.
METHOD_IDS = {name.lower(): int(value, 16) for name, value in re.findall(
    r"pub const (\w+): u64 = 0x([0-9a-f]+);", (ROOT / "contracts/escrow/src/ids.rs").read_text())}
STATUS_FIELD = b"status_code"  # a field of the wallet.callback event only

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

    def deploy(self, contract, salt, owner, init=None, extra=None):
        artifacts = (WALLET if contract == "wallet" else ROOT / "contracts" / contract) / "target/avm"
        init_json = "null" if contract == "wallet" else json.dumps("0x"+(b"" if init is None else init).hex())
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
                       descriptor=artifacts / "contract.descriptor", salt_hex=salt.encode().hex(),
                       init_json=init_json, owner_id=owner, **self.timestamps())
        if contract in self.selections:
            for key in ["wasm", "acab", "descriptor"]:
                del options[key]
            options.update(self.selections[contract])
        # Convert paths to text for the evidence journal.
        options = {key: str(value) if isinstance(value, Path) else value for key,value in options.items()}
        precondition = self.command("deploy", quote_address=True, **options)
        assert precondition.get("quoted") is True, precondition
        address = precondition["address"]
        funding = self.command("fund-user", address=address, shells=10_000_000,
                               **({"extra_currencies": extra} if extra else {}))
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

    def call(self, address, method, body=b"", identity="developer", expected_status=0):
        """A direct external, admitted only under the destination's owner key."""
        result = self.command("call", address=address, method=method,
                              args_json=json.dumps("0x"+body.hex()) if body else "null",
                              identity=identity, extra_currencies="", **self.timestamps())
        assert result["status"] == "ok", result
        raw = bytes.fromhex(result["raw_executor_output"])
        actual_status = int.from_bytes(raw[4:8], "little")
        assert raw[:4] == b"ACMO" and actual_status == expected_status, {"method": method, "contract_status": actual_status, "expected": expected_status}
        return result

    def send(self, wallet, identity, address, method, body=b"", extra=(), expected_status=0):
        """A call through the signer's wallet: wallet.send delivers an internal
        message carrying `extra` with a callback request. The destination's
        exact status comes from the wallet.callback event, its state from inspect."""
        args = address_bytes(address) + METHOD_IDS[method.rsplit(".", 1)[1]].to_bytes(8, "little")
        args += (10000).to_bytes(16, "little") + bytes([1, len(extra)])
        for currency_id, amount in extra:
            args += currency_id.to_bytes(4, "little") + amount.to_bytes(16, "little")
        args += body
        result = self.command("call", address=wallet, method="wallet.send",
                              args_json=json.dumps("0x"+args.hex()),
                              identity=identity, extra_currencies="", **self.timestamps())
        assert result["status"] == "ok", result
        callbacks = [bytes.fromhex(event) for event in result["events"] if STATUS_FIELD in bytes.fromhex(event)]
        assert len(callbacks) == 1, {"method": method, "callbacks": len(callbacks)}
        # The value follows 13 bytes after the field name; trailing zero bytes are not stored.
        field = callbacks[0].index(STATUS_FIELD) + len(STATUS_FIELD) + 13
        actual_status = int.from_bytes(callbacks[0][field:field+8], "little")
        assert actual_status == expected_status, {"method": method, "contract_status": actual_status, "expected": expected_status}
        result["raw_state"] = self.command("inspect", account=address)["raw_state"]
        return result

    def rejected(self, address, method, identity, error, extra=""):
        """An external the node rejects before execution with `error`."""
        result = self.command("call", address=address, method=method, args_json="null",
                              identity=identity, extra_currencies=extra, **self.timestamps())
        assert result["status"] == "error" and error in json.dumps(result), result

def assert_state(result, funded, paid, refunded, pending, attempts, phases):
    raw = bytes.fromhex(result["raw_state"])
    assert len(raw) == 2864, len(raw)
    actual = dict(funded=int.from_bytes(raw[456:472],"little"),
                  paid=int.from_bytes(raw[472:488],"little"),
                  refunded=int.from_bytes(raw[488:504],"little"),
                  pending=raw[512], attempts=int.from_bytes(raw[504:512],"little"),
                  phases=list(raw[544:547]))
    expected = dict(funded=funded, paid=paid, refunded=refunded, pending=pending, attempts=attempts, phases=phases)
    assert actual == expected, {"actual":actual,"expected":expected}
    return actual

def assert_event(result, name):
    assert any(name.encode() in bytes.fromhex(event) for event in result["events"]), name

def address_bytes(value):
    return bytes.fromhex(value.replace(":", ""))

def config(owner, owner_wallet, worker_wallet, refund, pay):
    data = bytearray(456)
    data[:32] = bytes.fromhex(owner)
    data[32:96] = address_bytes(owner_wallet)
    data[96:160] = address_bytes(worker_wallet)
    data[160:224] = address_bytes(refund)
    data[224:288] = address_bytes(pay)
    data[288:304] = (1000).to_bytes(16, "little")
    data[304:308] = (BASE_TIME+100).to_bytes(4, "little")
    data[308] = 3
    for i,amount in enumerate([300,400,300]):
        data[312+i*16:328+i*16] = amount.to_bytes(16,"little")
    data[440:456] = (10000).to_bytes(16,"little")
    return bytes(data)

def run(sdk, output):
    with tempfile.TemporaryDirectory(prefix="avm-bounty-") as directory:
        stand = Stand(sdk, Path(directory))
        try:
            init = stand.command("init", dapp_id=DAPP)
            assert init["status"] == "ok", init
            owner, worker = init["identities"]["developer"], init["identities"]["user"]
            stranger = init["identities"]["package_owner"]
            # One keyed wallet per identity. The owner wallet holds the ECC 0
            # principal for two funding attempts and ECC 7 for a wrong-currency one.
            owner_wallet = stand.deploy("wallet", "wallet-owner", owner, extra="0:2000,7:1000")
            worker_wallet = stand.deploy("wallet", "wallet-worker", worker)
            stranger_wallet = stand.deploy("wallet", "wallet-stranger", stranger)
            # Receivers and the escrow keep the developer (owner) key.
            pay = stand.deploy("receiver", "worker", owner)
            refund = stand.deploy("receiver", "client", owner)
            escrow = stand.deploy("escrow", "bounty", owner,
                                  config(owner, owner_wallet, worker_wallet, refund, pay))
            stand.call(escrow, "bounty.escrow.status")
            stand.send(owner_wallet, "developer", escrow, "bounty.escrow.fund", extra=[(7, 1000)], expected_status=2004)
            # The node admits an external only under the destination's owner
            # key and without value.
            stand.rejected(owner_wallet, "wallet.receive", "user", "AccountKeyNotAllowed")
            stand.rejected(escrow, "bounty.escrow.status", "user", "AccountKeyNotAllowed")
            stand.rejected(escrow, "bounty.escrow.fund", "developer", "ExternalValueNotAllowed", extra="0:1000")
            # Bind both fixtures with the owner key they were deployed with.
            for receiver,identity in [(pay,"developer"),(refund,"developer")]:
                result = stand.call(receiver, "bounty.receiver.bind", address_bytes(escrow), identity=identity)
                assert result["status"] == "ok", result
            # Only the owner wallet funds: a foreign wallet is rejected.
            stand.send(stranger_wallet, "package-owner", escrow, "bounty.escrow.fund", expected_status=2001)
            result = stand.send(owner_wallet, "developer", escrow, "bounty.escrow.fund", extra=[(0, 1000)])
            assert_state(result,1000,0,0,0,0,[0,0,0])
            stand.send(owner_wallet, "developer", escrow, "bounty.escrow.fund", extra=[(0, 1000)], expected_status=2003)
            stand.call(escrow,"bounty.escrow.refund",expected_status=2005)
            # Intentionally rejected destination: actual runtime rollback and
            # callback principal return, followed by an approved payment retry.
            result = stand.call(pay, "bounty.receiver.reject", b"\x01", identity="developer")
            assert result["status"] == "ok", result
            body = b"\x00" + b"\x42"*32
            stand.call(escrow,"bounty.escrow.submit",body,identity="developer",expected_status=2001)
            stand.send(stranger_wallet,"package-owner",escrow,"bounty.escrow.submit",body,expected_status=2001)
            stand.send(worker_wallet,"user",escrow,"bounty.escrow.submit",body)
            stand.call(escrow,"bounty.escrow.approve",body)
            result = stand.send(worker_wallet,"user",escrow,"bounty.escrow.pay",b"\x00")
            assert_state(result,1000,0,0,0,1,[2,0,0])
            assert_event(result,"escrow.transfer_failed")
            result = stand.call(pay,"bounty.receiver.reject",b"\x00",identity="developer")
            assert result["status"] == "ok", result
            result = stand.send(worker_wallet,"user",escrow,"bounty.escrow.pay",b"\x00")
            assert_state(result,1000,300,0,0,2,[3,0,0])
            assert_event(result,"escrow.transfer_settled")
            stand.send(worker_wallet,"user",escrow,"bounty.escrow.pay",b"\x00",expected_status=2003)
            # Approval before the deadline remains payable after it; refund
            # excludes the approved 400 and returns only the unapproved 300.
            body = b"\x01" + b"\x43"*32
            stand.send(worker_wallet,"user",escrow,"bounty.escrow.submit",body)
            stand.call(escrow,"bounty.escrow.approve",body)
            stand.time = BASE_TIME+100
            stand.call(refund,"bounty.receiver.reject",b"\x01",identity="developer")
            result = stand.call(escrow,"bounty.escrow.refund")
            assert result["status"] == "ok", result
            assert_state(result,1000,300,0,0,3,[3,2,0])
            assert_event(result,"escrow.transfer_failed")
            stand.call(refund,"bounty.receiver.reject",b"\x00",identity="developer")
            result = stand.call(escrow,"bounty.escrow.refund")
            assert_state(result,1000,300,300,0,4,[3,2,4])
            result = stand.send(worker_wallet,"user",escrow,"bounty.escrow.pay",b"\x01")
            assert_state(result,1000,700,300,0,5,[3,3,4])
            stand.call(escrow,"bounty.escrow.refund",expected_status=2003)
            stand.send(worker_wallet,"user",escrow,"bounty.escrow.submit",b"\x02"+b"\x44"*32,expected_status=2005)
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
