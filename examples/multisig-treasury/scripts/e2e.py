#!/usr/bin/env python3
"""Reproducible journey on the real AVM local stand. No network, no real funds.

Run after `avm-contract-build build` produced both artifact triples:

    python3 scripts/e2e.py --sdk ~/.local/share/avm-sdk/current

The three stand identities are the owners of a 2-of-3 treasury. The journey
deploys the treasury and a payee, binds them, deposits C7, C9 and SHELL, then
walks a transfer through propose, approve, timelock and execute until the
payee's callback settles it. It also covers revoking an approval, a payee that
rejects (the payout bounces and the proposal returns to the queue), changing
the delay and the threshold, a `SET_POLICY` fast lane with a window limit (one
transfer refused at the limit, a bounce that gives the allowance back, a new
window), adding and removing an owner, cancelling, freezing and unfreezing.
Selected decoded fields and expected outcomes are asserted at checkpoints; this
is not a complete independent state oracle. The journal is replayed at the end.
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
SHELL_DEPOSIT = 1_000_000  # FUEL for the many payouts of the journey
DELAY, GRACE, TTL = 60, 3_600, 86_400
STATE_BYTES = 3_264

(KIND_TRANSFER, KIND_ADD_OWNER, KIND_REMOVE_OWNER, KIND_SET_THRESHOLD, KIND_SET_DELAY,
 KIND_SET_POLICY, KIND_UNFREEZE) = 1, 2, 3, 4, 5, 6, 7
STATUS_OPEN, STATUS_QUEUED, STATUS_EXECUTING = 1, 2, 3

ERR_FROZEN, ERR_NOT_FOUND, ERR_TIMELOCK, ERR_LIMIT = 4003, 4005, 4007, 4008

EVENTS = ["msig.created", "msig.deposited", "msig.proposed", "msig.approved", "msig.revoked",
          "msig.queued", "msig.cancelled", "msig.frozen", "msig.unfrozen", "msig.owner_added",
          "msig.owner_removed", "msig.threshold_changed", "msig.delay_changed", "msig.policy_set",
          "msig.transfer_pending", "msig.transfer_settled", "msig.transfer_failed"]


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
        stamps = self.timestamps(fee=1)
        args = json.dumps(body) if isinstance(body, int) else json.dumps("0x" + body.hex()) if body else "null"
        options = dict(address=address, method=method, args_json=args, identity=identity,
                       extra_currencies=extra)
        # Smallest FUEL reserve (1 SHELL = 1_000_000 FUEL, capped at 10_000_000) that lets the call
        # return the expected status on a copy of state. The main call also uses 1 SHELL.
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
        assert self.fuel[-1][1] == 1, {"method": method, "fuel_bound": self.fuel[-1][1]}
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
    """Decodes the fixed 3264-byte treasury state image."""
    assert len(raw) == STATE_BYTES, len(raw)
    zero = bytes(32)
    owners = [raw[64 + 32 * i:96 + 32 * i] for i in range(8)]
    policies = {}
    for index in range(raw[3]):
        base = 832 + 64 * index
        policies[u32(raw, base)] = dict(secs=u32(raw, base + 4), fast=u128(raw, base + 8),
                                        limit=u128(raw, base + 24), start=u32(raw, base + 40),
                                        spent=u128(raw, base + 48))
    proposals = []
    for index in range(raw[56]):
        base = 1088 + 112 * index
        proposals.append(dict(id=u64(raw, base), kind=raw[base + 8], status=raw[base + 9],
                              proposer=raw[base + 10], approvals=raw[base + 11], created=u32(raw, base + 12),
                              eta=u32(raw, base + 16), expires=u32(raw, base + 20),
                              currency=u32(raw, base + 24), aux=u32(raw, base + 28),
                              amount=u128(raw, base + 32)))
    return dict(threshold=raw[0], frozen=raw[1], delay=u32(raw, 4), ops=u64(raw, 48),
                next_id=u64(raw, 32), owners=[o.hex() if o != zero else None for o in owners],
                balances={u32(raw, 576 + 32 * i): u128(raw, 592 + 32 * i) for i in range(raw[2])},
                policies=policies, proposals=proposals, pending=raw[57])


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
            recipient = address_bytes(payee)
            # Execution fees and storage rent are charged to the contract's own SHELL, and this
            # journey runs for a long time, so top both accounts up beyond the deploy funding.
            for address in (treasury, payee):
                funding = stand.command("fund-user", address=address, shells=1_000_000_000)
                assert funding["status"] == "ok", funding

            def view():
                return treasury_view(stand.account_state(treasury))

            def proposal(ident):
                found = [p for p in view()["proposals"] if p["id"] == ident]
                return found[0] if found else None

            def propose(who_name, kind, **fields):
                before = view()["next_id"]
                result = stand.call(treasury, "treasury.propose", propose_body(kind, **fields),
                                    identity=who[who_name])
                assert view()["next_id"] == before + 1
                return before, result

            def act(who_name, method, ident, expected=0):
                return stand.call(treasury, method, ident, identity=who[who_name], expected=expected)

            expected_c7 = [1_000_000]  # what the treasury should still hold of C7
            delivered = [0]            # payouts the payee should have accepted

            def transfer(amount, currency=C7, approvers=("bob",)):
                """Proposed by alice and approved by `approvers`; returns the proposal id."""
                ident, _ = propose("alice", KIND_TRANSFER, currency=currency, amount=amount, target=recipient)
                assert proposal(ident)["status"] == STATUS_OPEN and proposal(ident)["approvals"] == 0b001
                for name in approvers:
                    act(name, "treasury.approve", ident)
                return ident

            def wait_for(ident):
                """Execute is refused before the proposal's eta; then time passes."""
                queued = proposal(ident)
                assert queued["status"] == STATUS_QUEUED, queued
                if queued["eta"] > stand.time + 1:
                    act("carol", "treasury.execute", ident, expected=ERR_TIMELOCK)
                    stand.advance(queued["eta"] - stand.time)

            def pay(ident, amount, currency=C7):
                """Waits out the timelock, executes, and checks the callback settled the payout."""
                wait_for(ident)
                result = act("carol", "treasury.execute", ident)
                assert "msig.transfer_pending" in event_names(result), event_names(result)
                state = view()
                assert state["pending"] == 0 and proposal(ident) is None, "the callback settles in the same step"
                if currency == C7:
                    expected_c7[0] -= amount
                    assert state["balances"][C7] == expected_c7[0], state
                delivered[0] += 1
                got = payee_view(stand.account_state(payee))
                assert got["count"] == delivered[0] and got["last"] == receipt(currency, ident, amount), got

            def govern(kind, approvers=("bob",), **fields):
                """A governance proposal: it always waits out the delay, then carol executes it."""
                ident, _ = propose("alice", kind, **fields)
                for name in approvers:
                    act(name, "treasury.approve", ident)
                wait_for(ident)
                return ident, act("carol", "treasury.execute", ident)

            state = view()
            assert state["owners"][:3] == [alice, bob, carol] and state["threshold"] == 2, state
            assert state["delay"] == DELAY and state["balances"] == {} and state["proposals"] == [], state

            # Bind the payee to the treasury, then fund the treasury: C7, C9 and SHELL for FUEL.
            stand.call(payee, "treasury.payee.bind", address_bytes(treasury), identity="developer")
            result = stand.call(treasury, "treasury.deposit", extra=f"{SHELL}:{SHELL_DEPOSIT},{C7}:1000000,{C9}:500000",
                                identity="user")
            assert event_names(result).count("msig.deposited") == 3, event_names(result)
            assert view()["balances"] == {SHELL: SHELL_DEPOSIT, C7: 1_000_000, C9: 500_000}

            # A transfer: the payout is in flight until the payee's callback settles it.
            ident = transfer(1_000)
            queued = proposal(ident)
            assert queued["status"] == STATUS_QUEUED and queued["eta"] == stand.time + DELAY, queued
            pay(ident, 1_000)

            # Revoking an approval reopens a queued proposal; approving again requeues it.
            ident = transfer(300)
            first_eta = proposal(ident)["eta"]
            result = act("bob", "treasury.revoke", ident)
            assert "msig.revoked" in event_names(result), event_names(result)
            reopened = proposal(ident)
            assert (reopened["status"], reopened["approvals"], reopened["eta"]) == (STATUS_OPEN, 0b001, 0), reopened
            act("bob", "treasury.revoke", ident, expected=ERR_NOT_FOUND)  # nothing left to revoke
            stand.advance(100)
            act("carol", "treasury.approve", ident)
            assert proposal(ident)["status"] == STATUS_QUEUED and proposal(ident)["eta"] > first_eta, "the clock restarts"
            result = act("alice", "treasury.cancel", ident)
            assert "msig.cancelled" in event_names(result), event_names(result)
            assert proposal(ident) is None

            # A rejecting payee bounces the payout: funds return and the proposal is queued again.
            stand.call(payee, "treasury.payee.reject", b"\x01", identity="developer")
            ident = transfer(500)
            wait_for(ident)
            act("carol", "treasury.execute", ident)
            state = view()
            assert state["pending"] == 0 and state["balances"][C7] == expected_c7[0], state
            assert proposal(ident)["status"] == STATUS_QUEUED
            assert payee_view(stand.account_state(payee))["count"] == delivered[0]
            assert "msig.transfer_failed" in stand.events_seen, stand.events_seen
            # The same proposal pays out once the payee accepts again.
            stand.call(payee, "treasury.payee.reject", b"\x00", identity="developer")
            act("carol", "treasury.execute", ident)
            expected_c7[0] -= 500
            delivered[0] += 1
            assert proposal(ident) is None and view()["balances"][C7] == expected_c7[0]

            # The delay is itself governed, and later proposals use the new value.
            _, result = govern(KIND_SET_DELAY, aux=120)
            assert "msig.delay_changed" in event_names(result), event_names(result)
            assert view()["delay"] == 120
            ident = transfer(100)
            assert proposal(ident)["eta"] == stand.time + 120, "the new delay applies"
            pay(ident, 100)
            govern(KIND_SET_DELAY, aux=DELAY)
            assert view()["delay"] == DELAY

            # Raising the threshold demotes a queued proposal that no longer has enough approvals.
            waiting = transfer(200)
            assert proposal(waiting)["status"] == STATUS_QUEUED
            _, result = govern(KIND_SET_THRESHOLD, aux=3)
            assert "msig.threshold_changed" in event_names(result), event_names(result)
            assert view()["threshold"] == 3
            demoted = proposal(waiting)
            assert (demoted["status"], demoted["eta"], bin(demoted["approvals"]).count("1")) == (STATUS_OPEN, 0, 2), demoted
            act("carol", "treasury.approve", waiting)
            assert proposal(waiting)["status"] == STATUS_QUEUED
            pay(waiting, 200)
            # Lowering it needs the new threshold, and promotes what already has enough.
            govern(KIND_SET_THRESHOLD, approvers=("bob", "carol"), aux=2)
            assert view()["threshold"] == 2

            # SET_POLICY: C7 gets a fast lane up to 5_000 and at most 20_000 per 600-second window.
            _, result = govern(KIND_SET_POLICY, currency=C7, aux=600, amount=5_000, target=le(20_000, 16))
            assert "msig.policy_set" in event_names(result), event_names(result)
            policy = view()["policies"][C7]
            assert (policy["fast"], policy["limit"], policy["secs"], policy["spent"]) == (5_000, 20_000, 600, 0), policy

            small = transfer(4_000)
            assert proposal(small)["eta"] == stand.time, "inside the fast lane there is no delay"
            pay(small, 4_000)
            policy = view()["policies"][C7]
            assert policy["spent"] == 4_000 and policy["start"] == stand.time, policy
            window_start = policy["start"]

            pay(transfer(5_001), 5_001)  # above the fast limit the delay applies
            pay(transfer(9_000), 9_000)
            assert view()["policies"][C7]["spent"] == 18_001

            # Past the window limit the execution is refused and the proposal stays queued.
            over = transfer(2_000)
            wait_for(over)
            act("carol", "treasury.execute", over, expected=ERR_LIMIT)
            assert proposal(over)["status"] == STATUS_QUEUED and view()["policies"][C7]["spent"] == 18_001

            # A bounced payout gives its allowance back.
            stand.call(payee, "treasury.payee.reject", b"\x01", identity="developer")
            bounced = transfer(1_000)
            wait_for(bounced)
            act("carol", "treasury.execute", bounced)
            assert view()["policies"][C7]["spent"] == 18_001, "the bounce restores the allowance"
            assert proposal(bounced)["status"] == STATUS_QUEUED
            stand.call(payee, "treasury.payee.reject", b"\x00", identity="developer")

            # Once the window is over a new one opens, and both queued payouts go through.
            assert stand.time - window_start < 600
            stand.advance(600 - (stand.time - window_start))
            act("carol", "treasury.execute", over)
            expected_c7[0] -= 2_000
            delivered[0] += 1
            policy = view()["policies"][C7]
            assert (policy["spent"], policy["start"]) == (2_000, stand.time), policy
            act("carol", "treasury.execute", bounced)
            expected_c7[0] -= 1_000
            delivered[0] += 1
            assert view()["policies"][C7]["spent"] == 3_000
            assert view()["balances"][C7] == expected_c7[0]
            assert payee_view(stand.account_state(payee))["count"] == delivered[0]

            # Owners change only through proposals: add a fourth key, then remove it again.
            _, result = govern(KIND_ADD_OWNER, target=outsider_key)
            assert "msig.owner_added" in event_names(result), event_names(result)
            assert view()["owners"][3] == outsider_key.hex()
            _, result = govern(KIND_REMOVE_OWNER, target=outsider_key)
            assert "msig.owner_removed" in event_names(result), event_names(result)
            assert view()["owners"][3] is None

            # One owner freezes everything; only a threshold-approved unfreeze lifts it.
            waiting = transfer(7, currency=C9)
            wait_for(waiting)
            result = stand.call(treasury, "treasury.freeze", identity="package-owner")
            assert "msig.frozen" in event_names(result), event_names(result)
            assert view()["frozen"] == 1
            stand.call(treasury, "treasury.propose",
                       propose_body(KIND_TRANSFER, currency=C7, amount=1, target=recipient),
                       identity="developer", expected=ERR_FROZEN)
            act("carol", "treasury.execute", waiting, expected=ERR_FROZEN)
            unfreeze, _ = propose("bob", KIND_UNFREEZE)
            act("alice", "treasury.approve", unfreeze)
            assert proposal(unfreeze)["eta"] == stand.time, "unfreezing has no delay"
            result = act("carol", "treasury.execute", unfreeze)
            assert "msig.unfrozen" in event_names(result), event_names(result)
            assert view()["frozen"] == 0
            act("carol", "treasury.execute", waiting)
            final = view()
            assert final["balances"][C9] == 499_993 and final["pending"] == 0, final

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
            summary = {"status": "PASS", "treasury": treasury, "payee": payee, "final": final,
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
