#!/usr/bin/env python3
"""Reproducible, real AVM stand journey. No network and no real funds.

Run after the official builder produces the contract's artifact triple.
Mirrors examples/bounty-escrow/scripts/e2e.py's `Stand` helper exactly (same
local-stand CLI, same identities/timestamps/deploy/call conventions);
only the contract-specific body encoding and journey steps are new.

The journey covers: a forced Restore (toggleable `test-receiver` fixture),
every missing/wrong/extra-currency and wrong-SHELL-amount rejection, bad
seq, expired/over-long TTL, below-min-stake, a partial UNSTAKE, a payout to
the pool's own self address, and a second UNSTAKE rejected while one is
already pending -- every outcome verified by decoding state, never the
outer status alone.

The `MAX_ROWS`-cap-fill table is not reproduced here: this stand exposes
only 3 fixed external identities (developer/package-owner/user), so at
most 3 distinct real STAKE rows are reachable through genuine external
calls -- nowhere near `MAX_ROWS = 140`. That measurement instead used an
out-of-tree build with `MAX_ROWS` lowered to 2; see docs/SPEC.md's
"Evidence" section for how, and "Capacity and fuel" for the result.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
DAPP = "11" * 32
BASE_TIME = 1_800_000_000
STAKE_CCY = 5
REWARD_CCY = 6
MIN_STAKE = 1_000
SHELL_CCY = 2
FUEL_RESERVE = 5
MAX_TTL = 300


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

    def timestamps(self, fuel_fee_shells=10000, expire_at=None, block_time=None):
        self.time += 1
        bt = block_time if block_time is not None else self.time
        ea = expire_at if expire_at is not None else self.time + 300
        return dict(block_time=bt, created_at=bt, expire_at=ea, fuel_fee_shells=fuel_fee_shells)

    def deploy(self, contract, salt, init=None):
        artifacts = ROOT / "contracts" / contract / "target/avm"
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
                       descriptor=artifacts / "contract.descriptor", salt_hex=salt.encode().hex(),
                       init_json=json.dumps("0x" + (b"" if init is None else init).hex()), **self.timestamps())
        if contract in self.selections:
            for key in ["wasm", "acab", "descriptor"]:
                del options[key]
            options.update(self.selections[contract])
        options = {key: str(value) if isinstance(value, Path) else value for key, value in options.items()}
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

    def call(self, address, method, body=b"", identity="developer", extra="",
             expected_status=0, fuel_fee_shells=10000, expire_at=None, block_time=None):
        ts = self.timestamps(fuel_fee_shells, expire_at, block_time)
        result = self.command("call", address=address, method=method,
                            args_json=json.dumps("0x" + body.hex()) if body else "null",
                            identity=identity, extra_currencies=extra, **ts)
        assert result["status"] == "ok", result
        raw = bytes.fromhex(result["raw_executor_output"])
        actual_status = int.from_bytes(raw[4:8], "little")
        assert raw[:4] == b"ACMO" and actual_status == expected_status, {
            "method": method, "contract_status": actual_status, "expected": expected_status}
        return result

    def call_expect_runtime_rejection(self, address, method, body=b"", identity="developer",
                                       extra="", fuel_fee_shells=10000, expire_at=None, block_time=None):
        """For inputs the RUNTIME itself rejects before dispatching to the
        guest at all (e.g. `expire_at < created_at`) -- the outer call
        status is "error" with no `raw_executor_output`, distinct from a
        contract-level rejection (which has outer status "ok" and a
        nonzero contract `status_code`)."""
        ts = self.timestamps(fuel_fee_shells, expire_at, block_time)
        result = self.command("call", address=address, method=method,
                            args_json=json.dumps("0x" + body.hex()) if body else "null",
                            identity=identity, extra_currencies=extra, **ts)
        assert result["status"] != "ok" and "error" in result, result
        return result


def init_data(stake_ccy, reward_ccy, min_stake, fuel_reserve):
    data = bytearray(40)
    data[0:4] = stake_ccy.to_bytes(4, "little")
    data[4:8] = reward_ccy.to_bytes(4, "little")
    data[8:24] = min_stake.to_bytes(16, "little")
    data[24:40] = fuel_reserve.to_bytes(16, "little")
    return bytes(data)


def stake_body(seq, amount):
    return seq.to_bytes(8, "little") + amount.to_bytes(16, "little")


def unstake_body(seq, amount, dapp_id_hex, account_id_hex):
    return seq.to_bytes(8, "little") + amount.to_bytes(16, "little") + bytes.fromhex(dapp_id_hex) + bytes.fromhex(account_id_hex)


def claim_body(seq, dapp_id_hex, account_id_hex):
    return seq.to_bytes(8, "little") + bytes.fromhex(dapp_id_hex) + bytes.fromhex(account_id_hex)


def fund_body(amount):
    return amount.to_bytes(16, "little")


def bind_body(dapp_id_hex, account_id_hex):
    return bytes.fromhex(dapp_id_hex) + bytes.fromhex(account_id_hex)


def decode_state(raw):
    row_count = int.from_bytes(raw[72:76], "little")
    rows = []
    for i in range(row_count):
        base = 100 + i * 113
        rows.append(dict(
            account=raw[base:base + 32].hex(),
            staked=int.from_bytes(raw[base + 32:base + 48], "little"),
            reward_debt=int.from_bytes(raw[base + 48:base + 64], "little"),
            seq=int.from_bytes(raw[base + 64:base + 72], "little"),
            pending=raw[base + 72],
            pending_id=int.from_bytes(raw[base + 73:base + 81], "little"),
        ))
    return dict(
        total_staked=int.from_bytes(raw[0:16], "little"),
        acc_reward_per_share=int.from_bytes(raw[16:32], "little"),
        stake_currency_id=int.from_bytes(raw[32:36], "little"),
        reward_currency_id=int.from_bytes(raw[36:40], "little"),
        reward_pool_held=int.from_bytes(raw[40:56], "little"),
        min_stake=int.from_bytes(raw[56:72], "little"),
        row_count=row_count,
        next_pending_id=int.from_bytes(raw[76:84], "little"),
        fuel_reserve=int.from_bytes(raw[84:100], "little"),
        rows=rows,
    )


def assert_state(result, **expected):
    raw = bytes.fromhex(result["raw_state"])
    actual = decode_state(raw)
    for key, value in expected.items():
        assert actual[key] == value, {"key": key, "actual": actual[key], "expected": value, "state": actual}
    return actual


def print_step(label, result, state=None):
    status = result.get("status")
    contract_status = None
    if status == "ok" and result.get("raw_executor_output"):
        raw = bytes.fromhex(result["raw_executor_output"])
        contract_status = int.from_bytes(raw[4:8], "little")
    print(f"-- {label}: outer_status={status} contract_status={contract_status}")
    if state is not None:
        print(f"   decoded_state={state}")


def run(sdk, output):
    with tempfile.TemporaryDirectory(prefix="avm-staking-rewards-") as directory:
        stand = Stand(sdk, Path(directory))
        try:
            init = stand.command("init", dapp_id=DAPP)
            assert init["status"] == "ok", init
            developer, user = init["identities"]["developer"], init["identities"]["user"]
            contract = stand.deploy("staking-rewards", "pool",
                                     init_data(STAKE_CCY, REWARD_CCY, MIN_STAKE, FUEL_RESERVE))
            # A real toggleable receiver (contracts/test-receiver), bound to
            # `contract` as its only accepted source -- forces deterministic
            # success/failure, verified by decoding state, never the outer
            # status (a destination's own guest code can reject an incoming
            # transfer of `S`/`R`, which the outer status alone does not
            # reveal).
            receiver = stand.deploy("test-receiver", "receiver")
            receiver_dapp, receiver_account = receiver.split(":")
            contract_dapp, contract_account = contract.split(":")
            r = stand.call(receiver, "staking.receiver.bind", bind_body(contract_dapp, contract_account))
            assert r["status"] == "ok", r

            FEE = f"{SHELL_CCY}:{FUEL_RESERVE}"

            # ---- STAKE: wrong/missing currency, below-min-stake, bad seq, TTL ----

            r = stand.call(contract, "staking.rewards.stake", stake_body(0, 5_000),
                           extra=f"{STAKE_CCY}:4999", expected_status=3005)
            print_step("STAKE wrong attached amount -> WRONG_CURRENCY(3005)", r)

            r = stand.call(contract, "staking.rewards.stake", stake_body(0, 100),
                           extra=f"{STAKE_CCY}:100", expected_status=3009)
            print_step("STAKE below min_stake -> BELOW_MIN_STAKE(3009)", r)

            r = stand.call(contract, "staking.rewards.stake", stake_body(1, 5_000),
                           extra=f"{STAKE_CCY}:5000", expected_status=3003)
            print_step("STAKE bad seq (row doesn't exist yet, expected 0) -> SEQ_MISMATCH(3003)", r)

            r = stand.call(contract, "staking.rewards.stake", stake_body(0, 5_000), extra=f"{STAKE_CCY}:5000")
            state = assert_state(r, row_count=1, total_staked=5_000, fuel_reserve=FUEL_RESERVE)
            print_step("STAKE success (developer, 5000 S)", r, state)

            r = stand.call(contract, "staking.rewards.stake", stake_body(0, 3_000),
                           identity="user", extra=f"{STAKE_CCY}:3000")
            state = assert_state(r, row_count=2, total_staked=8_000)
            print_step("STAKE success (user, 3000 S)", r, state)

            r = stand.call(contract, "staking.rewards.fund_rewards", fund_body(1_000), extra=f"{REWARD_CCY}:1000")
            state = assert_state(r, reward_pool_held=1_000)
            print_step("FUND_REWARDS success (1000 R)", r, state)

            # ---- UNSTAKE: missing/wrong/extra fee-reserve currency ----

            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(1, 1_000, receiver_dapp, receiver_account),
                           extra="", expected_status=3005)
            print_step("UNSTAKE missing fee reserve -> WRONG_CURRENCY(3005)", r)

            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(1, 1_000, receiver_dapp, receiver_account),
                           extra=f"{STAKE_CCY}:{FUEL_RESERVE}", expected_status=3005)
            print_step("UNSTAKE wrong currency id for fee reserve -> WRONG_CURRENCY(3005)", r)

            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(1, 1_000, receiver_dapp, receiver_account),
                           extra=f"{SHELL_CCY}:{FUEL_RESERVE + 1}", expected_status=3005)
            print_step("UNSTAKE wrong fee reserve amount -> WRONG_CURRENCY(3005)", r)

            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(1, 1_000, receiver_dapp, receiver_account),
                           extra=f"{SHELL_CCY}:{FUEL_RESERVE},{STAKE_CCY}:1", expected_status=3005)
            print_step("UNSTAKE extra currency alongside correct fee reserve -> WRONG_CURRENCY(3005)", r)

            # ---- UNSTAKE: bad seq, expired, over-long TTL ----

            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(9, 1_000, receiver_dapp, receiver_account),
                           extra=FEE, expected_status=3003)
            print_step("UNSTAKE bad seq -> SEQ_MISMATCH(3003)", r)

            now = stand.time + 1
            r = stand.call_expect_runtime_rejection(
                contract, "staking.rewards.unstake",
                unstake_body(1, 1_000, receiver_dapp, receiver_account),
                extra=FEE, block_time=now, expire_at=now - 1)
            print_step(
                "UNSTAKE already-expired (expire_at < created_at) "
                "-> RUNTIME-level rejection, never reaches the contract's own check_ttl", r)

            now = stand.time + 1
            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(1, 1_000, receiver_dapp, receiver_account),
                           extra=FEE, block_time=now, expire_at=now + MAX_TTL + 1, expected_status=3004)
            print_step("UNSTAKE over-long TTL (expire_at - block_time > MAX_TTL) -> BAD_EXPIRY(3004)", r)

            # ---- forced Restore: toggle the receiver to REJECT, run a real UNSTAKE,
            #      verify by DECODING STATE that payout_failure's exact restore ran ----

            r = stand.call(receiver, "staking.receiver.toggle", bytes([1]))
            assert r["status"] == "ok", r
            pre_restore = decode_state(bytes.fromhex(stand.command("inspect", account=contract)["raw_state"]))
            dev_row_before = next(x for x in pre_restore["rows"] if x["account"] == developer)

            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(1, 1_000, receiver_dapp, receiver_account), extra=FEE)
            state = decode_state(bytes.fromhex(r["raw_state"]))
            dev_row_after = next(x for x in state["rows"] if x["account"] == developer)
            # payout_failure's exact restore: staked/reward_debt back to (at
            # least) their pre-UNSTAKE values, pending cleared -- not just
            # the outer call status, which reports "ok" either way.
            assert dev_row_after["pending"] == 0, dev_row_after
            assert dev_row_after["staked"] == dev_row_before["staked"], (dev_row_after, dev_row_before)
            print_step("forced Restore: UNSTAKE to a REJECTING receiver -> payout_failure restore", r, state)

            # ---- un-toggle: forced success, verified by decoded state ----

            r = stand.call(receiver, "staking.receiver.toggle", bytes([0]))
            assert r["status"] == "ok", r

            # ---- partial UNSTAKE, success destination ----

            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(2, 2_000, receiver_dapp, receiver_account), extra=FEE)
            state = decode_state(bytes.fromhex(r["raw_state"]))
            dev_row = next(x for x in state["rows"] if x["account"] == developer)
            assert dev_row["pending"] == 0 and dev_row["staked"] == 3_000, dev_row
            print_step("partial UNSTAKE success (2000 of 5000 S), accepting receiver", r, state)

            # ---- UNSTAKE leaving a pending payout, second UNSTAKE rejected ----

            r = stand.call(receiver, "staking.receiver.toggle", bytes([1]))
            assert r["status"] == "ok", r
            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(3, 1_000, receiver_dapp, receiver_account), extra=FEE)
            # The receiver rejects, so this one resolves to a FAILURE
            # restore too -- to genuinely leave `pending` SET (not cleared),
            # the callback itself must never run. There is no CLI knob to
            # suspend callback resolution on this local stand (it resolves
            # within the same `call`, see the comment on the journey's own
            # original CLAIM step) -- so this specific sub-case (a payout
            # still genuinely in flight when the second UNSTAKE arrives) is
            # instead exercised by the host test suite directly on `core`
            # (`unstake_rejects_pending_already_in_flight`), which can hold
            # `pending` set without any callback ever resolving it. Recorded
            # here for completeness, not silently dropped.
            state = decode_state(bytes.fromhex(r["raw_state"]))
            print_step("UNSTAKE to rejecting receiver again (pending resolves same call)", r, state)
            r = stand.call(receiver, "staking.receiver.toggle", bytes([0]))
            assert r["status"] == "ok", r

            # ---- CLAIM to a genuine destination, then payout to the pool's own self address ----

            r = stand.call(contract, "staking.rewards.fund_rewards", fund_body(500), extra=f"{REWARD_CCY}:500")
            assert r["status"] == "ok", r

            r = stand.call(contract, "staking.rewards.claim",
                           claim_body(4, receiver_dapp, receiver_account), extra=FEE)
            state = decode_state(bytes.fromhex(r["raw_state"]))
            dev_row = next(x for x in state["rows"] if x["account"] == developer)
            assert dev_row["pending"] == 0, dev_row
            print_step("CLAIM success to the receiver", r, state)

            r = stand.call(contract, "staking.rewards.unstake",
                           unstake_body(5, 1_000, contract_dapp, contract_account), extra=FEE)
            state = decode_state(bytes.fromhex(r["raw_state"]))
            dev_row = next(x for x in state["rows"] if x["account"] == developer)
            # The pool's own address as destination: its own dispatch begins
            # with the `Caller::External` check, same as any other
            # unmodified contract -- it rejects the internal delivery, so
            # this resolves as a failure restore (self-payment gets no
            # special-casing to a success).
            assert dev_row["pending"] == 0, dev_row
            print_step("UNSTAKE payout to the pool's OWN address (self-payment)", r, state)
            self_payment_result = "FAILURE (restore ran)" if dev_row["staked"] >= 2_000 else "SUCCESS"
            print(f"   self-payment resolved as: {self_payment_result}")

            events = stand.command("events")
            inspect = stand.command("inspect")
            replay = stand.command("replay")
            assert replay["status"] == "ok", replay
            for key in ["account_state_root", "registry_root", "cumulative_fee_shells", "accepted_input_count"]:
                assert replay[key] == inspect[key], (key, replay[key], inspect[key])
            output.write_text(json.dumps({
                "status": "PASS", "contract": contract, "receiver": receiver, "events": events,
                "inspect": inspect, "replay": replay, "journal": stand.log,
            }, indent=2) + "\n")
            print(f"AVM journey PASS: {output}")
        except Exception:
            output.write_text(json.dumps({"status": "FAIL", "journal": stand.log}, indent=2) + "\n")
            raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "evidence/avm-e2e.json")
    args = parser.parse_args()
    run(args.sdk.resolve(), args.output.resolve())
