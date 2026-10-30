#!/usr/bin/env python3
"""Black-box release gate. Only bundle files and public CLI inputs are used.

Run on the qualification pod with an isolated writable copy of the verified
bundle. Output goes to the supplied evidence directory, never to source Git.
No missing binary, missing case, or empty test selection is a success.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess

import blake3 as reference_blake3


def sha(data):
    return hashlib.sha256(data).hexdigest()


def blake3(data):
    return reference_blake3.blake3(data).digest()


def u32(data, offset):
    return struct.unpack_from("<I", data, offset)[0]


def read_state(path):
    """Independent read-only decoder of the published developer-state prefix."""
    data = path.read_bytes()
    assert data[:8] == b"AVMLSF01"
    position = 308  # magic/version/config/DApp/identities/roots/clock/fees/counters
    for width in (33, 32, 32):
        count = u32(data, position)
        position += 4
        for _ in range(count):
            position += width
            size = u32(data, position)
            position += 4 + size
    count = u32(data, position)
    position += 4
    for _ in range(count):
        position += 64 + 8
        count_extra = u32(data, position)
        position += 4 + count_extra * 20
    count = u32(data, position)
    position += 4
    accounts = {}
    for _ in range(count):
        address = data[position:position + 64]
        position += 64
        account_hash = data[position:position + 32]
        position += 32 + 8 + 4 + 16 + 1
        size = u32(data, position)
        position += 4
        image = data[position:position + size]
        position += size
        assert blake3(image) == account_hash
        extras = int.from_bytes(image[33:35], "little")
        start = 35 + 20 * extras
        state_size = u32(image, start)
        assert start + 4 + state_size == len(image)
        accounts[address.hex()[:64] + ":" + address.hex()[64:]] = {
            "selected_code_id": image[1:33].hex(),
            "image": image.hex(),
            "data": image[start + 4:],
        }
    nonces = u32(data, position)
    position += 4 + nonces * 104
    assert u32(data, position) == 0, "successful public operations must drain the queue"
    position += 4
    receipts = u32(data, position)
    position += 4 + receipts * 136
    count = u32(data, position)
    position += 4
    assert count == receipts
    transcript = []
    for _ in range(count):
        size = u32(data, position)
        position += 4
        wire = data[position:position + size]
        assert len(wire) == size
        transcript.append(wire)
        position += size
    return accounts, data[204:236].hex(), transcript


def read_accounts(path):
    return read_state(path)[:2]


def wasm_facts(data, batch):
    """Independent section decoder; no builder/admission implementation is imported."""
    def leb(position):
        value = 0
        for shift in range(0, 35, 7):
            byte = data[position]
            position += 1
            value |= (byte & 127) << shift
            if byte < 128:
                return value, position
        raise AssertionError("oversized WASM integer")

    assert data[:8] == b"\0asm\1\0\0\0"
    position, exports, memory = 8, {}, None
    while position < len(data):
        section = data[position]
        size, position = leb(position + 1)
        end = position + size
        assert end <= len(data) and section != 0, "custom/truncated section survived admission"
        if section == 2:
            count, _ = leb(position)
            assert count == 0, "imports are forbidden"
        if section == 5:
            count, cursor = leb(position)
            flags, cursor = leb(cursor)
            minimum, cursor = leb(cursor)
            maximum, cursor = leb(cursor)
            assert count == 1 and flags == 1 and cursor == end
            assert 1 <= minimum <= maximum == 512
            memory = {"minimum_bytes": minimum * 65536, "maximum_bytes": maximum * 65536}
        if section == 7:
            count, cursor = leb(position)
            for _ in range(count):
                length, cursor = leb(cursor)
                name = data[cursor:cursor + length].decode()
                cursor += length
                kind = data[cursor]
                index, cursor = leb(cursor + 1)
                assert name not in exports
                exports[name] = {"kind": kind, "index": index}
            assert cursor == end
        position = end
    required = {"memory", "avm_guest_alloc", "avm_guest_free", "init", "process_message"}
    if batch:
        required.add("process_batch")
    assert set(exports) == required and memory is not None
    assert exports["memory"] == {"kind": 2, "index": 0}
    assert all(value["kind"] == 0 for key, value in exports.items() if key != "memory")
    return {"memory": memory, "exports": exports}


class Journey:
    def __init__(self, bundle, evidence, label):
        self.bundle = bundle
        self.evidence = evidence / label
        self.evidence.mkdir(parents=True)
        self.state = self.evidence / "developer.state"
        self.profile = bundle / "profiles/wasmtime-release-profile.v1"
        self.sequence = 0
        self.time = 1_800_000_000
        self.label = label
        self.command_history = []
        self.initial_state = None
        self.identities = None
        self.current = json.loads((bundle / "BUILD-MANIFEST.json").read_bytes())["schema"] == "gosh.avm-sdk.release.v2"

    def run(self, command, expected=0, **options):
        argv = [str(self.bundle / "bin/avm-local-stand"), command,
                "--state", str(self.state), "--wasmtime-profile", str(self.profile)]
        for key, value in options.items():
            argv.extend(["--" + key.replace("_", "-"), str(value)])
        process = subprocess.run(argv, capture_output=True)
        self.sequence += 1
        stem = self.evidence / f"{self.sequence:03}-{command}"
        stem.with_suffix(".stdout").write_bytes(process.stdout)
        stem.with_suffix(".stderr").write_bytes(process.stderr)
        stem.with_suffix(".argv.json").write_text(json.dumps(argv))
        assert process.returncode == expected, (argv, process.returncode, process.stderr.decode(), process.stdout.decode())
        value = json.loads(process.stdout)
        expected_status = "ok" if expected == 0 else "error"
        if expected == 2 and value.get("error") == "funding_required":
            expected_status = "precondition"
        assert value["status"] == expected_status, value
        if command == "init" and expected == 0:
            self.initial_state = self.state.read_bytes()
            self.identities = value["identities"]
        elif command in {"fund-user", "deploy", "publish-version", "call", "upgrade"}:
            self.command_history.append((command, expected, options.copy(), value))
        return value

    def timing(self, fee=1):
        self.time += 1
        return dict(block_time=self.time, created_at=self.time,
                    expire_at=self.time + 3600, fuel_fee_shells=fee)

    def deploy(self, example, command="deploy", **extra):
        artifacts = self.bundle / "examples" / example / "target/avm"
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
                       descriptor=artifacts / "contract.descriptor", salt_hex=example.encode().hex(),
                       init_json="null", **self.timing(), **extra)
        before = self.state.read_bytes()
        missing = self.run(command, expected=2, **options)
        assert missing["error"] == "funding_required"
        assert self.state.read_bytes() == before, "unfunded deploy must have no state effects"
        address = missing["address"]
        # The 64-KiB batch fixture incurs rent at each later logical second.
        # Fund the entire journey so its error probe reaches real guest code.
        self.run("fund-user", address=address, shells=1000000)
        deployed = self.run(command, **options)
        assert int(deployed["receipt"]["charged_shells"]) > 0
        assert address in read_accounts(self.state)[0]
        return address

    def call(self, address, method, args=None, **extra):
        return self.run("call", address=address, method=method, args_json=json.dumps(args),
                        identity="user",
                        extra_currencies=extra.pop("extra_currencies", ""),
                        **self.timing(), **extra)

    def replay(self):
        from fixture_oracle import decode_state, expected_journey, verify_contents

        def independent(stand, inspected):
            contents, header_roots, inputs, times = decode_state(stand.state.read_bytes())
            assert header_roots["registry_root"] == inspected["registry_root"]
            expected = expected_journey(stand.bundle, self.label, self.identities, inputs, times, self.current)
            roots = {**header_roots, "account_state_root": inspected["account_state_root"]}
            return verify_contents(contents, expected, roots, self.current)

        inspected = self.run("inspect")
        roots = independent(self, inspected)
        accounts, ring = read_accounts(self.state)
        before = self.state.read_bytes()
        replayed = self.run("replay")
        for key in ("registry_root", "account_state_root", "cumulative_fee_shells", "accepted_input_count"):
            assert replayed[key] == inspected[key], key
        assert replayed["receipt_count"] == len(inspected["receipts"]) > 0
        assert read_accounts(self.state) == (accounts, ring)
        assert self.state.read_bytes() == before
        # Replay the public command inputs into a different state, starting from
        # the exact empty CLI-created identity/config seed. No internal message
        # injection, root edits, private executor or prebuilt WASM is involved.
        assert self.initial_state is not None and self.identities is not None
        restored = Journey(self.bundle, self.evidence.parent, self.label + "-command-replay")
        descriptor = os.open(restored.state, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(self.initial_state)
        for command, expected_status, options, original_result in self.command_history:
            assert restored.run(command, expected=expected_status, **options) == original_result
        restored_inspected = restored.run("inspect")
        restored_roots = independent(restored, restored_inspected)
        restored.run("replay")
        assert restored.state.read_bytes() == before
        assert self.state.read_bytes() == before
        (self.evidence / "roots.json").write_text(json.dumps({
            **roots, "public_command_replay_roots": restored_roots,
            "independent_oracle": "bounded-public-journeys-v1",
            "verified_contents": ["full account bodies and balances", "full account-index metadata",
                                  "all registry records and code/ABI blobs", "all canonical event bytes",
                                  "all canonical ring records"],
            "receipts": inspected["receipts"], "cumulative_fee_shells": inspected["cumulative_fee_shells"],
        }, indent=2))


def run(bundle, evidence, incompatible_state=None, wrong_rustc=None):
    evidence.mkdir(parents=True, exist_ok=False)
    manifest = json.loads((bundle / "BUILD-MANIFEST.json").read_bytes())
    current = manifest["schema"] == "gosh.avm-sdk.release.v2"
    if current:
        assert incompatible_state is not None, "candidate qualification requires a populated historical state"
        assert wrong_rustc is not None, "candidate qualification requires a real different compiler"
        version = subprocess.run([str(wrong_rustc), "-vV"], capture_output=True, check=True)
        (evidence / "wrong-rustc-version.txt").write_bytes(version.stdout)
        assert b"release: " in version.stdout and b"release: 1.93.0\n" not in version.stdout
        negative = bundle / "examples/compiler-mismatch"
        shutil.copytree(bundle / "examples/contract-a-v1", negative, ignore=shutil.ignore_patterns("target"))
        env = os.environ.copy()
        env["RUSTC"] = str(wrong_rustc)
        result = subprocess.run([str(bundle / "bin/avm-contract-build"), "build", "--manifest-path", str(negative / "Cargo.toml")], env=env, capture_output=True)
        (evidence / "compiler-mismatch.stderr").write_bytes(result.stderr)
        assert result.returncode != 0 and b"required rustc 1.93.0" in result.stderr
        assert not (negative / "target").exists()
    if incompatible_state is not None:
        before = incompatible_state.read_bytes()
        command = [str(bundle / "bin/avm-local-stand"), "inspect", "--state", str(incompatible_state),
                   "--wasmtime-profile", str(bundle / "profiles/wasmtime-release-profile.v1")]
        rejection = subprocess.run(command, capture_output=True)
        (evidence / "old-state-new-profile.stdout").write_bytes(rejection.stdout)
        (evidence / "old-state-new-profile.stderr").write_bytes(rejection.stderr)
        assert rejection.returncode == 2
        assert json.loads(rejection.stdout)["status"] == "error"
        assert incompatible_state.read_bytes() == before
    version = Journey(bundle, evidence, "version-upgrade")
    initialized = version.run("init", dapp_id="11" * 32)
    owner = initialized["identities"]["package_owner"]
    assert len(owner) == 64 and bytes.fromhex(owner).hex() == owner
    fixture_owner = "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394"
    assert owner != fixture_owner, "qualification must use the public CLI's generated identity"
    (evidence / "example-owner.json").write_text(json.dumps({
        "package_owner_public_key": owner,
        "compiled_owner": owner if current else fixture_owner,
        "configuration": "AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY" if current else "historical fixed fixture key",
    }, indent=2))
    if current:
        negative = bundle / "examples/invalid-package-owner"
        shutil.copytree(bundle / "examples/contract-a-v1", negative, ignore=shutil.ignore_patterns("target"))
        for label, invalid_owner in (("length", "81"), ("hex", "zz" * 32)):
            env = os.environ.copy()
            env["AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY"] = invalid_owner
            result = subprocess.run([str(bundle / "bin/avm-contract-build"), "build", "--manifest-path", str(negative / "Cargo.toml")], env=env, capture_output=True)
            (evidence / f"invalid-owner-{label}.stderr").write_bytes(result.stderr)
            assert result.returncode != 0
            assert b"AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY must be 64 lowercase hex digits" in result.stderr
            assert not (negative / "target/avm/contract.wasm").exists()
    artifacts, admission = {}, {}
    for example in ("contract-a-v1", "contract-a-v2", "contract-b", "contract-batch-patch"):
        root = bundle / "examples" / example
        env = os.environ.copy()
        if current and example == "contract-a-v1":
            env["AVM_EXAMPLE_PACKAGE_OWNER_PUBKEY"] = owner
        process = subprocess.run([str(bundle / "bin/avm-contract-build"), "build", "--manifest-path", str(root / "Cargo.toml")], env=env, capture_output=True)
        (evidence / f"{example}-build.stdout").write_bytes(process.stdout)
        (evidence / f"{example}-build.stderr").write_bytes(process.stderr)
        assert process.returncode == 0, process.stderr.decode()
        directory = root / "target/avm"
        assert sorted(p.name for p in directory.iterdir()) == ["contract.acab", "contract.descriptor", "contract.wasm"]
        values = {p.name: p.read_bytes() for p in directory.iterdir()}
        expected = [("contract.wasm", blake3(values["contract.wasm"])),
                    ("contract.acab", blake3(b"AVM_ABI" + values["contract.acab"])),
                    ("contract.descriptor", blake3(values["contract.descriptor"]))]
        assert process.stdout.decode().splitlines() == [f"{name} {digest.hex()}" for name, digest in expected]
        assert values["contract.wasm"][:8] == b"\0asm\1\0\0\0"
        assert values["contract.acab"][:4] == b"ACAB"
        assert len(values["contract.descriptor"]) == 66
        artifacts[example] = {name: sha(data) for name, data in values.items()}
        admission[example] = wasm_facts(values["contract.wasm"], example == "contract-batch-patch")
    assert artifacts["contract-a-v1"]["contract.acab"] == artifacts["contract-a-v2"]["contract.acab"]
    assert artifacts["contract-a-v1"]["contract.descriptor"] == artifacts["contract-a-v2"]["contract.descriptor"]
    assert artifacts["contract-a-v1"]["contract.wasm"] != artifacts["contract-a-v2"]["contract.wasm"]
    (evidence / "artifacts.json").write_text(json.dumps(artifacts, indent=2))
    (evidence / "admission.json").write_text(json.dumps(admission, indent=2))

    address = version.deploy("contract-a-v1")
    call = version.call(address, "causal.a.version")
    assert bytes.fromhex(call["raw_state"])[16] == 1
    assert bytes.fromhex(call["raw_executor_output"])[:4] == b"ACMO"
    assert u32(bytes.fromhex(call["raw_executor_output"]), 4) == 0
    assert call["state_decode_error"] is None
    # External routes require a positive fee before WASM can start. Real guest
    # FUEL exhaustion remains a separate status-31 failure-flow assertion below.
    before_unpaid = version.state.read_bytes()
    unpaid = version.run("call", expected=2, address=address, method="causal.a.version",
                         args_json="null", identity="user", extra_currencies="",
                         **version.timing(fee=0))
    assert unpaid["error"] == "call rejected canonical operation: local call charge failed: RouteRequiresFee", unpaid
    assert version.state.read_bytes() == before_unpaid, "unpaid external call must not publish effects"
    upgraded = version.deploy("contract-a-v2", command="publish-version")
    selected = read_accounts(version.state)[0][upgraded]["selected_code_id"]
    before = read_accounts(version.state)[0][address]["data"]
    # Authorization is exercised by real WASM, not merely CLI argument parsing.
    before_selection = read_accounts(version.state)[0][address]["selected_code_id"]
    rejected = version.run("upgrade", expected=2, address=address, selected_code_id=selected,
                migration_payload_hex=b"avm-a-v2-migrate-state!!".hex(),
                identity="user", **version.timing())
    assert "did not switch" in rejected["error"]
    assert read_accounts(version.state)[0][address]["data"] == before
    assert read_accounts(version.state)[0][address]["selected_code_id"] == before_selection
    rejected = version.run("upgrade", expected=2, address=address, selected_code_id=selected,
                migration_payload_hex=b"avm-a-v2-migrate-state!?".hex(),
                identity="package-owner", **version.timing())
    assert "did not switch" in rejected["error"]
    assert read_accounts(version.state)[0][address]["data"] == before
    assert read_accounts(version.state)[0][address]["selected_code_id"] == before_selection
    upgrade = version.run("upgrade", expected=0 if current else 2, address=address, selected_code_id=selected,
                migration_payload_hex=b"avm-a-v2-migrate-state!!".hex(),
                identity="package-owner", **version.timing())
    if current:
        assert read_accounts(version.state)[0][address]["selected_code_id"] == selected
        assert bytes.fromhex(version.call(address, "causal.a.version")["raw_state"])[16] == 2
    else:
        # The immutable historical example authorizes only its fixed fixture
        # owner. Its public CLI generates a different key; record that defect,
        # without altering the old contract or claiming an authorized upgrade.
        assert "did not switch" in upgrade["error"]
        assert read_accounts(version.state)[0][address]["data"] == before
        assert read_accounts(version.state)[0][address]["selected_code_id"] == before_selection
        assert bytes.fromhex(version.call(address, "causal.a.version")["raw_state"])[16] == 1
    version.replay()

    for destination_example, callback in [("contract-b", True), ("contract-batch-patch", False)]:
        journey = Journey(bundle, evidence, destination_example)
        journey.run("init", dapp_id="11" * 32)
        sender = journey.deploy("contract-a-v1")
        receiver = journey.deploy(destination_example, command="publish-version")
        ring_before = read_accounts(journey.state)[1]
        events_before = journey.run("events")["events"]
        method = "causal.b.receive" if callback else "causal.batch.patch"
        body = bytes.fromhex(receiver.replace(":", "")) + blake3(method.encode())[:8]
        body += blake3(b"causal.a.callback")[:8] if callback else bytes(8)
        journey.call(sender, "causal.a.forward", "0x" + body.hex(), extra_currencies="0:100,7:40")
        accounts, ring = read_accounts(journey.state)
        state = accounts[receiver]["data"]
        if callback:
            assert int.from_bytes(state[:8], "little") == 1
            sender_state = accounts[sender]["data"]
            assert int.from_bytes(sender_state[8:16], "little") == 1, "requested callback was not delivered"
            assert u32(sender_state, 16) == 0 and u32(sender_state, 20) == 16
            assert sender_state[24:40] == bytes([0x99]) * 16
            events = journey.run("events")
            assert events["count"] == len(events_before) + 2 == len(events["events"])
            assert events["events"][:len(events_before)] == events_before
            assert ring != ring_before
        else:
            assert len(state) == 65540 and u32(state, 0) == 65536
            assert int.from_bytes(state[4:12], "little") == 1
            assert state[12:] == bytes(65528)
            failed = journey.call(receiver, "causal.batch.patch")
            assert u32(bytes.fromhex(failed["raw_executor_output"]), 4) == 1401
            assert read_accounts(journey.state)[0][receiver]["data"] == state
        journey.replay()

    # Build the public receiver's existing failure switches with the packaged
    # builder. The original four builds above stay unmodified and independently
    # identified. No private fixture or prebuilt WASM is substituted.
    negative = bundle / "examples/contract-b-failure"
    shutil.copytree(bundle / "examples/contract-b", negative, ignore=shutil.ignore_patterns("target"))
    env = os.environ.copy()
    env["AVM_E2E_FAILURE_FLOW"] = "1"
    built = subprocess.run([str(bundle / "bin/avm-contract-build"), "build", "--manifest-path", str(negative / "Cargo.toml")], env=env, capture_output=True)
    (evidence / "failure-build.stdout").write_bytes(built.stdout)
    (evidence / "failure-build.stderr").write_bytes(built.stderr)
    assert built.returncode == 0, built.stderr.decode()
    failure_wasm = (negative / "target/avm/contract.wasm").read_bytes()
    assert sha(failure_wasm) != artifacts["contract-b"]["contract.wasm"]
    wasm_facts(failure_wasm, False)
    (evidence / "failure-artifacts.json").write_text(json.dumps({p.name: sha(p.read_bytes()) for p in (negative / "target/avm").iterdir()}, indent=2))
    for label, method, handler, status in [
        ("trap", "causal.b.drain", (0xc2c6c51bc610249d).to_bytes(8, "little"), 30),
        ("out-of-fuel", "causal.b.drain", blake3(b"causal.a.version")[:8], 31),
        ("invalid-event", "causal.b.receive", (0xc2c6c51bc610249d).to_bytes(8, "little"), 44),
    ]:
        journey = Journey(bundle, evidence, label)
        journey.run("init", dapp_id="11" * 32)
        sender = journey.deploy("contract-a-v1")
        receiver = journey.deploy("contract-b-failure", command="publish-version")
        before_accounts, before_ring, before_wires = read_state(journey.state)
        before_events = journey.run("events")["events"]
        body = bytes.fromhex(receiver.replace(":", "")) + blake3(method.encode())[:8] + handler
        called = journey.call(sender, "causal.a.forward", "0x" + body.hex(), extra_currencies="0:100,7:40")
        assert u32(bytes.fromhex(called["raw_executor_output"]), 4) == 0
        accounts, ring, wires = read_state(journey.state)
        assert len(wires) == len(before_wires) + 3
        delivered, callback = wires[-2:]
        assert delivered[48:112] == bytes.fromhex(sender.replace(":", ""))
        assert delivered[112:176] == bytes.fromhex(receiver.replace(":", ""))
        assert callback[:4] == b"\0\1\2\0" and len(callback) == 316
        assert callback[8:16] == handler
        assert callback[16:32] == (1).to_bytes(16, "little")
        assert callback[48:112] == delivered[112:176] and callback[112:176] == delivered[48:112]
        assert callback[176:208] == blake3(delivered)
        assert callback[208:272] == delivered[48:112]
        assert u32(callback, 272) == status
        assert callback[276:] == bytes(4) + (37).to_bytes(16, "little") + (7).to_bytes(4, "little") + (11).to_bytes(16, "little")
        assert accounts[receiver]["data"] == before_accounts[receiver]["data"]
        assert accounts[receiver]["selected_code_id"] == before_accounts[receiver]["selected_code_id"]
        assert accounts[sender]["data"] == (1).to_bytes(8, "little") + bytes(64)
        assert ring == before_ring and journey.run("events")["events"] == before_events
        journey.replay()

    invalid = Journey(bundle, evidence, "invalid-profile")
    invalid.profile = invalid.evidence / "invalid.v1"
    invalid.profile.write_bytes((bundle / "profiles/wasmtime-release-profile.v1").read_bytes() + b"unapproved=true\n")
    # Current init is allowed to reject early; if it accepts, actual deploy must reject.
    argv = [str(bundle / "bin/avm-local-stand"), "init", "--state", str(invalid.state),
            "--wasmtime-profile", str(invalid.profile), "--dapp-id", "11" * 32]
    process = subprocess.run(argv, capture_output=True)
    if process.returncode == 0:
        root = bundle / "examples/contract-a-v1/target/avm"
        options = dict(wasm=root / "contract.wasm", acab=root / "contract.acab",
                       descriptor=root / "contract.descriptor", salt_hex="01", init_json="null", **invalid.timing())
        rejection = invalid.run("deploy", expected=2, **options)
        if rejection.get("error") == "funding_required":
            invalid.run("fund-user", address=rejection["address"], shells=100000)
            before = invalid.state.read_bytes()
            rejection = invalid.run("deploy", expected=2, **options)
            assert invalid.state.read_bytes() == before
        assert "profile" in rejection["error"].lower(), rejection
    else:
        assert process.returncode == 2
    (evidence / "passed.json").write_text(json.dumps({
        "passed": True, "bundle": str(bundle), "authorized_upgrade_passed": current,
        "historical_limitations": [] if current else [
            "Fixed example owner differs from the public CLI-generated owner; upgrade rejection was verified."
        ],
        "internal_event_reads": {
            "capability": "UNSUPPORTED_IN_REVIEWED_ABI",
            "package_execution": "NOT_EXECUTED_THROUGH_PUBLIC_PACKAGE", "passed": None,
            "reason": "Current guest OutboundIntent wire has no read-request field; external inputs forbid it. Source block-start visibility/burn/fallback and injection-rejection assertions remain mandatory. Packaged event coverage is emission/listing only.",
        },
    }, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--incompatible-state", type=Path)
    parser.add_argument("--wrong-rustc", type=Path)
    arguments = parser.parse_args()
    run(arguments.bundle.resolve(), arguments.evidence.resolve(), arguments.incompatible_state, arguments.wrong_rustc)
