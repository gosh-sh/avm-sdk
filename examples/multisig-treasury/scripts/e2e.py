#!/usr/bin/env python3
"""Reproducible journey on the real AVM local stand. No network, no real funds.

Run after `avm-contract-build build` produced both artifact triples:

    python3 scripts/e2e.py --sdk ~/.local/share/avm-sdk/current

The three stand identities are the owners of a 2-of-3 treasury. The journey
deploys the treasury and a payee, binds them, deposits C7, C9 and SHELL, then
walks a transfer through propose, approve, timelock and execute until the
payee's callback settles it. It also covers a payee that rejects (the payout
bounces and the proposal returns to the queue), the fast lane under a
`SET_POLICY` proposal, adding and removing an owner, cancelling, freezing and
unfreezing. After every step the decoded treasury state is compared with what
the script computed on its own, and the stand journal is replayed at the end.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
DAPP = "11" * 32
BASE_TIME = 1_800_000_000
SHELL, C7, C9 = 2, 7, 9
FUEL_RESERVE = 10_000
DELAY, GRACE, TTL = 60, 3_600, 86_400
STATE_BYTES = 3_136

KIND_TRANSFER, KIND_ADD_OWNER, KIND_REMOVE_OWNER, KIND_SET_POLICY, KIND_UNFREEZE = 1, 2, 3, 4, 5
STATUS_OPEN, STATUS_QUEUED, STATUS_EXECUTING = 1, 2, 3

ERR_FROZEN, ERR_TIMELOCK = 4003, 4007

EVENTS = ["msig.created", "msig.deposited", "msig.proposed", "msig.approved", "msig.queued",
          "msig.cancelled", "msig.frozen", "msig.unfrozen", "msig.owner_added",
          "msig.owner_removed", "msig.policy_set", "msig.transfer_pending",
          "msig.transfer_settled", "msig.transfer_failed"]


def le(value, size):
    return int(value).to_bytes(size, "little")


def u32(raw, offset):
    return int.from_bytes(raw[offset:offset + 4], "little")


def u64(raw, offset):
    return int.from_bytes(raw[offset:offset + 8], "little")


def u128(raw, offset):
    return int.from_bytes(raw[offset:offset + 16], "little")


class Stand:
    def __init__(self, sdk, work):
        self.binary = str(sdk / "bin/avm-local-stand")
        self.profile = str(sdk / "profiles/wasmtime-release-profile.v1")
        self.state = work / "journey.state"
        self.log = []
        self.time = BASE_TIME
        self.fuel = []
        self.events_seen = set()

    def command(self, operation, **options):
        cmd = [self.binary, operation, "--state", str(self.state), "--wasmtime-profile", self.profile]
        for key, value in options.items():
            cmd += ["--" + key.replace("_", "-")] + ([] if value is True else [str(value)])
        proc = subprocess.run(cmd, capture_output=True, text=True)
        try:
            result = json.loads(proc.stdout)
        except json.JSONDecodeError:
            raise RuntimeError(f"{operation}: {proc.stderr}\n{proc.stdout}") from None
        self.log.append({"operation": operation, "options": {k: str(v) for k, v in options.items()},
                         "exit_code": proc.returncode, "result": result})
        return result

    def timestamps(self, fee=100_000):
        self.time += 1
        return dict(block_time=self.time, created_at=self.time, expire_at=self.time + 3600, fuel_fee_shells=fee)

    def advance(self, seconds):
        self.time += seconds

    def deploy(self, contract, salt, init=b""):
        artifacts = ROOT / "contracts" / contract / "target/avm"
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
                       descriptor=artifacts / "contract.descriptor", salt_hex=salt.encode().hex(),
                       init_json=json.dumps("0x" + init.hex()), **self.timestamps())
        options = {key: str(value) if isinstance(value, Path) else value for key, value in options.items()}
        quote = self.command("deploy", quote_address=True, **options)
        assert quote.get("quoted") is True, quote
        address = quote["address"]
        funding = self.command("fund-user", address=address, shells=10_000_000)
        assert funding["status"] == "ok", funding
        result = self.command("deploy", **options)
        assert result["status"] == "ok", result
        return address

    def call(self, address, method, body=b"", identity="user", extra="", expected=0):
        stamps = self.timestamps()
        args = json.dumps(body) if isinstance(body, int) else json.dumps("0x" + body.hex()) if body else "null"
        options = dict(address=address, method=method, args_json=args,
                       identity=identity, extra_currencies=extra)
        # Smallest FUEL reserve (1 SHELL = 1_000_000 FUEL, capped at 10_000_000) that lets the call
        # finish, found on a copy of the state so the journey itself is unaffected.
        for shells in (1, 2, 3, 5, 10):
            probe = self.state.with_suffix(".probe")
            shutil.copyfile(self.state, probe)
            real, self.state = self.state, probe
            try:
                trial = self.command("call", **options, **dict(stamps, fuel_fee_shells=shells))
            finally:
                self.state = real
            if trial.get("status") == "ok":
                raw = bytes.fromhex(trial["raw_executor_output"])
                if raw[:4] == b"ACMO" and int.from_bytes(raw[4:8], "little") == expected:
                    self.fuel.append((method, shells))
                    break
        else:
            self.fuel.append((method, None))
        result = self.command("call", **options, **stamps)
        assert result["status"] == "ok", result
        raw = bytes.fromhex(result["raw_executor_output"])
        status = int.from_bytes(raw[4:8], "little")
        assert raw[:4] == b"ACMO" and status == expected, {"method": method, "status": status, "expected": expected}
        self.events_seen.update(event_names(result))
        return result

    def account_state(self, address):
        reply = self.command("inspect", account=address)
        assert reply["status"] == "ok" and reply["state_decode_error"] is None, reply
        return bytes.fromhex(reply["state"][2:])


def address_bytes(value):
    return bytes.fromhex(value.replace(":", ""))


def init_data(owners):
    return (bytes([2, len(owners)]) + le(DELAY, 4) + le(GRACE, 4) + le(TTL, 4) + le(FUEL_RESERVE, 16)
            + b"".join(bytes.fromhex(owner) for owner in owners))


def propose_body(kind, currency=0, aux=0, amount=0, target=b""):
    return bytes([kind]) + le(currency, 4) + le(aux, 4) + le(amount, 16) + target.ljust(64, b"\0")


def treasury_view(raw):
    """Decodes the fixed 3136-byte treasury state image."""
    assert len(raw) == STATE_BYTES, len(raw)
    zero = bytes(32)
    owners = [raw[64 + 32 * i:96 + 32 * i] for i in range(8)]
    proposals = []
    for index in range(raw[56]):
        base = 960 + 112 * index
        proposals.append(dict(id=u64(raw, base), kind=raw[base + 8], status=raw[base + 9],
                              proposer=raw[base + 10], approvals=raw[base + 11], created=u32(raw, base + 12),
                              eta=u32(raw, base + 16), expires=u32(raw, base + 20),
                              currency=u32(raw, base + 24), aux=u32(raw, base + 28),
                              amount=u128(raw, base + 32)))
    return dict(threshold=raw[0], frozen=raw[1], ops=u64(raw, 48), next_id=u64(raw, 32),
                owners=[o.hex() if o != zero else None for o in owners],
                balances={u32(raw, 576 + 32 * i): u128(raw, 592 + 32 * i) for i in range(raw[2])},
                policies={u32(raw, 832 + 32 * i): u128(raw, 848 + 32 * i) for i in range(raw[3])},
                proposals=proposals, pending=raw[57])


def payee_view(raw):
    assert len(raw) == 144, len(raw)
    return dict(reject=raw[96], count=u64(raw, 104), last=raw[112:144])


def event_names(result):
    found = []
    for event in result["events"]:
        blob = bytes.fromhex(event)
        found += [name for name in EVENTS if name.encode() in blob]
    return found


def receipt(currency, ident, amount):
    return b"TRS1" + le(currency, 4) + le(ident, 8) + le(amount, 16)


def run(sdk, output):
    with tempfile.TemporaryDirectory(prefix="avm-multisig-treasury-") as directory:
        stand = Stand(sdk, Path(directory))
        try:
            init = stand.command("init", dapp_id=DAPP)
            assert init["status"] == "ok", init
            ids = init["identities"]
            alice, bob, carol = ids["developer"], ids["user"], ids["package_owner"]  # slots 0, 1, 2
            who = {"alice": "developer", "bob": "user", "carol": "package-owner"}
            outsider_key = bytes([0x44]) * 32

            payee = stand.deploy("payee", "treasury-payee")
            treasury = stand.deploy("treasury", "multisig-treasury", init_data([alice, bob, carol]))
            view = treasury_view(stand.account_state(treasury))
            assert view["owners"][:3] == [alice, bob, carol] and view["threshold"] == 2, view
            assert view["balances"] == {} and view["proposals"] == [] and view["ops"] == 1, view

            # Bind the payee to the treasury, then fund the treasury: C7, C9 and SHELL for FUEL.
            stand.call(payee, "treasury.payee.bind", address_bytes(treasury), identity="developer")
            result = stand.call(treasury, "treasury.deposit", extra=f"{SHELL}:100000,{C7}:1000000,{C9}:500000",
                                identity="user")
            assert event_names(result).count("msig.deposited") == 3, event_names(result)
            view = treasury_view(stand.account_state(treasury))
            assert view["balances"] == {SHELL: 100_000, C7: 1_000_000, C9: 500_000}, view

            recipient = address_bytes(payee)

            def propose(who_name, kind, **fields):
                before = treasury_view(stand.account_state(treasury))["next_id"]
                result = stand.call(treasury, "treasury.propose", propose_body(kind, **fields),
                                    identity=who[who_name])
                after = treasury_view(stand.account_state(treasury))
                assert after["next_id"] == before + 1, after
                return before, result

            def act(who_name, method, ident, expected=0):
                return stand.call(treasury, method, ident, identity=who[who_name], expected=expected)

            def proposal(ident):
                found = [p for p in treasury_view(stand.account_state(treasury))["proposals"] if p["id"] == ident]
                return found[0] if found else None

            def settle_transfer(currency, amount):
                """propose (alice), approve (bob), wait out the delay, execute (carol)."""
                ident, _ = propose("alice", KIND_TRANSFER, currency=currency, amount=amount, target=recipient)
                assert proposal(ident)["status"] == STATUS_OPEN and proposal(ident)["approvals"] == 0b001
                result = act("bob", "treasury.approve", ident)
                assert "msig.queued" in event_names(result), event_names(result)
                queued = proposal(ident)
                assert queued["status"] == STATUS_QUEUED and queued["eta"] == stand.time + DELAY, queued
                act("carol", "treasury.execute", ident, expected=ERR_TIMELOCK)
                stand.advance(DELAY)
                return ident

            # A transfer: the payout is in flight until the payee's callback settles it.
            ident = settle_transfer(C7, 1_000)
            result = act("carol", "treasury.execute", ident)
            assert "msig.transfer_pending" in event_names(result), event_names(result)
            view = treasury_view(stand.account_state(treasury))
            assert view["pending"] == 0 and proposal(ident) is None, "the callback must settle in the same step"
            assert view["balances"][C7] == 999_000, view
            got = payee_view(stand.account_state(payee))
            assert got["count"] == 1 and got["last"] == receipt(C7, ident, 1_000), got

            # A rejecting payee bounces the payout: funds return and the proposal is queued again.
            stand.call(payee, "treasury.payee.reject", b"\x01", identity="developer")
            ident = settle_transfer(C7, 500)
            act("carol", "treasury.execute", ident)
            view = treasury_view(stand.account_state(treasury))
            queued = proposal(ident)
            assert view["pending"] == 0 and view["balances"][C7] == 999_000, view
            assert queued and queued["status"] == STATUS_QUEUED, queued
            assert payee_view(stand.account_state(payee))["count"] == 1
            assert "msig.transfer_failed" in stand.events_seen, stand.events_seen

            # The same proposal pays out once the payee accepts again.
            stand.call(payee, "treasury.payee.reject", b"\x00", identity="developer")
            act("carol", "treasury.execute", ident)
            view = treasury_view(stand.account_state(treasury))
            assert proposal(ident) is None and view["balances"][C7] == 998_500, view
            got = payee_view(stand.account_state(payee))
            assert got["count"] == 2 and got["last"] == receipt(C7, ident, 500), got

            # SET_POLICY gives C7 a fast lane for payouts up to 5_000; governance waits as always.
            policy, _ = propose("alice", KIND_SET_POLICY, currency=C7, aux=1, amount=5_000)
            act("bob", "treasury.approve", policy)
            stand.advance(DELAY)
            result = act("carol", "treasury.execute", policy)
            assert "msig.policy_set" in event_names(result), event_names(result)
            assert treasury_view(stand.account_state(treasury))["policies"] == {C7: 5_000}

            small, _ = propose("alice", KIND_TRANSFER, currency=C7, amount=4_000, target=recipient)
            act("bob", "treasury.approve", small)
            assert proposal(small)["eta"] == stand.time, "inside the fast lane there is no delay"
            act("carol", "treasury.execute", small)  # no waiting
            view = treasury_view(stand.account_state(treasury))
            assert view["balances"][C7] == 994_500 and payee_view(stand.account_state(payee))["count"] == 3, view

            large, _ = propose("alice", KIND_TRANSFER, currency=C7, amount=5_001, target=recipient)
            act("bob", "treasury.approve", large)
            assert proposal(large)["eta"] == stand.time + DELAY, "above the fast limit the delay applies"
            act("carol", "treasury.execute", large, expected=ERR_TIMELOCK)
            result = act("alice", "treasury.cancel", large)
            assert "msig.cancelled" in event_names(result), event_names(result)
            assert proposal(large) is None

            # Owners change only through proposals: add a fourth key, then remove it again.
            add, _ = propose("alice", KIND_ADD_OWNER, target=outsider_key)
            act("bob", "treasury.approve", add)
            stand.advance(DELAY)
            result = act("carol", "treasury.execute", add)
            assert "msig.owner_added" in event_names(result), event_names(result)
            view = treasury_view(stand.account_state(treasury))
            assert view["owners"][3] == outsider_key.hex(), view
            remove, _ = propose("alice", KIND_REMOVE_OWNER, target=outsider_key)
            act("bob", "treasury.approve", remove)
            stand.advance(DELAY)
            result = act("carol", "treasury.execute", remove)
            assert "msig.owner_removed" in event_names(result), event_names(result)
            assert treasury_view(stand.account_state(treasury))["owners"][3] is None

            # One owner freezes everything; only a threshold-approved unfreeze lifts it.
            waiting, _ = propose("alice", KIND_TRANSFER, currency=C9, amount=7, target=recipient)
            act("bob", "treasury.approve", waiting)
            stand.advance(DELAY)
            result = stand.call(treasury, "treasury.freeze", identity="package-owner")
            assert "msig.frozen" in event_names(result), event_names(result)
            assert treasury_view(stand.account_state(treasury))["frozen"] == 1
            stand.call(treasury, "treasury.propose", propose_body(KIND_TRANSFER, currency=C7, amount=1, target=recipient),
                       identity="developer", expected=ERR_FROZEN)
            act("carol", "treasury.execute", waiting, expected=ERR_FROZEN)
            unfreeze, _ = propose("bob", KIND_UNFREEZE)
            act("alice", "treasury.approve", unfreeze)
            assert proposal(unfreeze)["eta"] == stand.time, "unfreezing has no delay"
            result = act("carol", "treasury.execute", unfreeze)
            assert "msig.unfrozen" in event_names(result), event_names(result)
            assert treasury_view(stand.account_state(treasury))["frozen"] == 0
            act("carol", "treasury.execute", waiting)
            view = treasury_view(stand.account_state(treasury))
            assert view["balances"][C9] == 499_993 and view["pending"] == 0, view

            missing = set(EVENTS) - stand.events_seen - {"msig.created"}
            assert not missing, missing
            events = stand.command("events")
            assert "msig.created".encode().hex() in json.dumps(events), "the deploy emitted msig.created"
            inspect = stand.command("inspect")
            replay = stand.command("replay")
            assert replay["status"] == "ok", replay
            assert inspect["queue"]["message_count"] == 0, inspect["queue"]
            for key in ["account_state_root", "registry_root", "cumulative_fee_shells", "accepted_input_count"]:
                assert replay[key] == inspect[key], (key, replay[key], inspect[key])
            summary = {"status": "PASS", "treasury": treasury, "payee": payee, "final": view,
                       "fuel_reserve_needed_shells": stand.fuel, "events_seen": sorted(stand.events_seen),
                       "events": events, "inspect": inspect, "replay": replay}
            output.write_text(json.dumps(summary, indent=2, default=str) + "\n")
            print(f"multisig-treasury journey PASS: {output}")
            print("FUEL needed per call (smallest reserve in SHELL, 1 SHELL = 1,000,000 FUEL; cap 10):")
            for method, shells in stand.fuel:
                print(f"  {'> 10 (cap)' if shells is None else '<= %d M' % shells:>11}  {method}")
        except Exception:
            output.write_text(json.dumps({"status": "FAIL", "journal": stand.log}, indent=2) + "\n")
            raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "evidence/avm-e2e.json")
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    run(args.sdk.resolve(), args.output.resolve())
