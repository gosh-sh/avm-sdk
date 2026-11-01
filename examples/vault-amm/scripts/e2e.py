#!/usr/bin/env python3
"""Reproducible journey on the real AVM local stand. No network, no real funds.

Run after `avm-contract-build build` produced both artifact triples:

    python3 scripts/e2e.py --sdk ~/.local/share/avm-sdk/current

It deploys the AMM and a receiver, binds them, then deposits, adds liquidity,
swaps exact-in and exact-out, checks the TWAP accumulators, reads the status view,
withdraws to the receiver (accepted and rejected payouts) and replays the journal. After every step the decoded AMM ledger is checked against the amounts
the script computed on its own. The stand does not expose per-account ECC
balances, so the vault's real balances are checked indirectly: the contract's
own solvency check compares post-credit `input.extra` on external calls (including the new ledger on deposits) and
fails the call with 3012 if the vault holds less than the ledger promises.
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
NEVER = 0xFFFFFFFF
SHELL = 2
C0, C1 = 0, 7
FUEL_RESERVE = 10_000

ERR_INSOLVENT = 3012


def u128(raw, offset):
    return int.from_bytes(raw[offset:offset + 16], "little")


def u64(raw, offset):
    return int.from_bytes(raw[offset:offset + 8], "little")


def le(value, size):
    return int(value).to_bytes(size, "little")


class Stand:
    def __init__(self, sdk, work):
        self.binary = str(sdk / "bin/avm-local-stand")
        self.profile = str(sdk / "profiles/wasmtime-release-profile.v1")
        self.state = work / "journey.state"
        self.log = []
        self.time = BASE_TIME
        self.selections = {}
        self.fuel = []

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
        if contract in self.selections:
            for key in ["wasm", "acab", "descriptor"]:
                del options[key]
            options.update(self.selections[contract])
        options = {key: str(value) if isinstance(value, Path) else value for key, value in options.items()}
        quote = self.command("deploy", quote_address=True, **options)
        assert quote.get("quoted") is True, quote
        address = quote["address"]
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

    def call(self, address, method, body=b"", identity="user", extra="", expected=0):
        stamps = self.timestamps()
        options = dict(address=address, method=method, args_json=json.dumps("0x" + body.hex()) if body else "null",
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
        assert self.fuel[-1][1] == 1, {"method": method, "fuel_bound": self.fuel[-1][1]}
        result = self.command("call", **options, **stamps)
        assert result["status"] == "ok", result
        raw = bytes.fromhex(result["raw_executor_output"])
        status = int.from_bytes(raw[4:8], "little")
        assert raw[:4] == b"ACMO" and status == expected, {"method": method, "status": status, "expected": expected}
        return result

    def account_state(self, address):
        reply = self.command("inspect", account=address)
        assert reply["status"] == "ok" and reply["state_decode_error"] is None, reply
        return bytes.fromhex(reply["state"][2:])


def address_bytes(value):
    return bytes.fromhex(value.replace(":", ""))


def config(admin):
    data = bytearray(64)
    data[:32] = bytes.fromhex(admin)
    data[32:36] = le(C0, 4)
    data[36:40] = le(C1, 4)
    data[40:42] = le(30, 2)  # swap fee, bps
    data[42:44] = le(0, 2)   # protocol fee, bps
    data[48:64] = le(FUEL_RESERVE, 16)
    return bytes(data)


def amm_view(raw):
    """Decodes the fixed 4376-byte AMM state image."""
    assert len(raw) == 4376, len(raw)
    accounts = {}
    for index in range(32):
        base = 728 + 80 * index
        key = raw[base:base + 32]
        if key == bytes(32):
            break
        accounts[key.hex()] = ([u128(raw, base + 32), u128(raw, base + 48)], u128(raw, base + 64))
    pending = [raw[3288 + 136 * i] for i in range(8)].count(1)
    return dict(reserve=[u128(raw, 72), u128(raw, 88)], fees=[u128(raw, 104), u128(raw, 120)],
                lp_total=u128(raw, 136), last_ts=int.from_bytes(raw[152:156], "little"),
                cumulative=[u128(raw, 160), u128(raw, 176)], ops=u64(raw, 192), seq=u64(raw, 200),
                receipts=u64(raw, 208), accounts=accounts, pending=pending)


def twap_after(pre, now):
    """What the accumulators must hold after a trade at `now`: the elapsed time folded in at the
    pre-trade price (UQ64.64, wrapping), computed here independently of the contract."""
    if pre["lp_total"] == 0 or now <= pre["last_ts"]:
        return pre["cumulative"], pre["last_ts"]
    elapsed = now - pre["last_ts"]
    q64, mask = 1 << 64, (1 << 128) - 1
    price0 = pre["reserve"][1] * q64 // pre["reserve"][0]
    price1 = pre["reserve"][0] * q64 // pre["reserve"][1]
    return [(pre["cumulative"][0] + price0 * elapsed) & mask,
            (pre["cumulative"][1] + price1 * elapsed) & mask], now


def ceil_div(a, b):
    return -(-a // b)


def receiver_view(raw):
    assert len(raw) == 144, len(raw)
    return dict(reject=raw[96], count=u64(raw, 104), last=raw[112:144])


def event_names(result):
    names = []
    for event in result["events"]:
        blob = bytes.fromhex(event)
        for name in ("amm.deposited", "amm.liquidity_added", "amm.liquidity_removed", "amm.swapped",
                     "amm.withdraw_pending", "amm.withdraw_settled", "amm.withdraw_failed", "amm.created",
                     "amm.status"):
            if name.encode() in blob:
                names.append(name)
    return names


def receipt(currency, ident, amount):
    return b"VAM1" + le(currency, 4) + le(ident, 8) + le(amount, 16)


def run(sdk, output):
    with tempfile.TemporaryDirectory(prefix="avm-vault-amm-") as directory:
        stand = Stand(sdk, Path(directory))
        try:
            init = stand.command("init", dapp_id=DAPP)
            assert init["status"] == "ok", init
            admin, alice, bob = (init["identities"][k] for k in ("developer", "user", "package_owner"))

            receiver = stand.deploy("receiver", "payee")
            amm = stand.deploy("amm", "vault-amm", config(admin))
            view = amm_view(stand.account_state(amm))
            assert view["reserve"] == [0, 0] and view["accounts"] == {} and view["ops"] == 0

            # Bind the payee to the pool, then give the pool its FUEL reserve in SHELL.
            stand.call(receiver, "vault.receiver.bind", address_bytes(amm), identity="developer")
            stand.call(amm, "vault.amm.topup", extra=f"{SHELL}:100000")

            # Deposits: the ledger is credited, nothing is pooled yet.
            stand.call(amm, "vault.amm.deposit", extra=f"{C0}:1000000,{C1}:1000000", identity="user")
            view = amm_view(stand.account_state(amm))
            assert view["accounts"][alice] == ([1_000_000, 1_000_000], 0), view
            assert view["reserve"] == [0, 0]

            # First liquidity: sqrt(1e6 * 1e6) = 1e6 shares, 1000 of them locked.
            body = le(1_000_000, 16) + le(1_000_000, 16) + le(0, 16) + le(0, 16) + le(NEVER, 4)
            result = stand.call(amm, "vault.amm.add_liquidity", body, identity="user")
            assert "amm.liquidity_added" in event_names(result), event_names(result)
            view = amm_view(stand.account_state(amm))
            assert view["reserve"] == [1_000_000, 1_000_000] and view["lp_total"] == 1_000_000, view
            assert view["accounts"][alice] == ([0, 0], 999_000), view

            # Bob deposits 10_000 of currency 0 and swaps it: 10_000 in, 9_871 out at 30 bps.
            stand.call(amm, "vault.amm.deposit", extra=f"{C0}:10000", identity="package-owner")
            body = b"\x00" + le(10_000, 16) + le(9_871, 16) + le(NEVER, 4)
            before_swap = amm_view(stand.account_state(amm))
            result = stand.call(amm, "vault.amm.swap_exact_in", body, identity="package-owner")
            assert "amm.swapped" in event_names(result), event_names(result)
            view = amm_view(stand.account_state(amm))
            assert view["reserve"] == [1_010_000, 990_129], view
            # The TWAP accumulators folded in the elapsed time at the pre-trade price.
            assert (view["cumulative"], view["last_ts"]) == twap_after(before_swap, stand.time), (view, before_swap)
            assert view["cumulative"] != before_swap["cumulative"]
            assert view["accounts"][bob] == ([0, 9_871], 0), view

            # A swap whose bound cannot be met fails without effects (3006 = slippage).
            before = stand.account_state(amm)
            stand.call(amm, "vault.amm.deposit", extra=f"{C0}:500", identity="package-owner")
            body = b"\x00" + le(500, 16) + le(10**9, 16) + le(NEVER, 4)
            stand.call(amm, "vault.amm.swap_exact_in", body, identity="package-owner", expected=3006)
            assert amm_view(stand.account_state(amm))["accounts"][bob] == ([500, 9_871], 0)

            # Withdraw Bob's currency 1 to the bound receiver; delivery, callback and settlement run.
            payee = address_bytes(receiver)
            body = le(C1, 4) + le(9_871, 16) + payee
            result = stand.call(amm, "vault.amm.withdraw", body, identity="package-owner")
            names = event_names(result)
            assert "amm.withdraw_pending" in names, names
            view = amm_view(stand.account_state(amm))
            assert view["accounts"][bob] == ([500, 0], 0), view
            assert view["pending"] == 0, "the callback must settle the payout in the same journey step"
            got = receiver_view(stand.account_state(receiver))
            assert got["count"] == 1 and got["last"] == receipt(C1, 0, 9_871), got

            # A rejecting receiver bounces the payout; the holder is re-credited.
            stand.call(receiver, "vault.receiver.reject", b"\x01", identity="developer")
            body = le(C0, 4) + le(500, 16) + payee
            stand.call(amm, "vault.amm.withdraw", body, identity="package-owner")
            view = amm_view(stand.account_state(amm))
            assert view["accounts"][bob] == ([500, 0], 0), ("bounce must re-credit", view)
            assert view["pending"] == 0
            assert receiver_view(stand.account_state(receiver))["count"] == 1

            # The same payout succeeds once the receiver accepts again.
            stand.call(receiver, "vault.receiver.reject", b"\x00", identity="developer")
            stand.call(amm, "vault.amm.withdraw", body, identity="package-owner")
            view = amm_view(stand.account_state(amm))
            assert bob not in view["accounts"], "an empty holder is compacted away"
            got = receiver_view(stand.account_state(receiver))
            assert got["count"] == 2 and got["last"] == receipt(C0, 2, 500), got

            # Adversarial receiver accepts principal but emits an empty receipt.
            # More than eight such payouts must not exhaust the global pending slots.
            stand.call(receiver, "vault.receiver.reject", b"\x02", identity="developer")
            stand.call(amm, "vault.amm.deposit", extra=f"{C0}:9", identity="package-owner")
            for _ in range(9):
                before_count = receiver_view(stand.account_state(receiver))["count"]
                stand.call(amm, "vault.amm.withdraw", le(C0, 4) + le(1, 16) + payee,
                           identity="package-owner")
                assert amm_view(stand.account_state(amm))["pending"] == 0
                assert receiver_view(stand.account_state(receiver))["count"] == before_count + 1
            # A different holder can still exit after the adversarial payouts.
            stand.call(amm, "vault.amm.deposit", extra=f"{C0}:1", identity="user")
            stand.call(amm, "vault.amm.withdraw", le(C0, 4) + le(1, 16) + payee, identity="user")
            assert amm_view(stand.account_state(amm))["pending"] == 0
            stand.call(receiver, "vault.receiver.reject", b"\x00", identity="developer")

            # Alice leaves the pool and withdraws part of it.
            body = le(500_000, 16) + le(0, 16) + le(0, 16) + le(NEVER, 4)
            stand.call(amm, "vault.amm.remove_liquidity", body, identity="user")
            view = amm_view(stand.account_state(amm))
            assert view["lp_total"] == 500_000 and view["reserve"] == [505_000, 495_065], view

            # Exact-out swap: Alice buys exactly 10_000 of currency 1 and pays the input the double
            # round-up formula gives; a bound one unit too tight is refused without effects.
            stand.call(amm, "vault.amm.deposit", extra=f"{C0}:100000", identity="user")
            stand.advance(1_000)
            pre = amm_view(stand.account_state(amm))
            want_out = 10_000
            r_in, r_out = pre["reserve"]
            need = ceil_div(ceil_div(r_in * want_out, r_out - want_out) * 10_000, 10_000 - 30)
            body = b"\x00" + le(want_out, 16) + le(need - 1, 16) + le(NEVER, 4)
            stand.call(amm, "vault.amm.swap_exact_out", body, identity="user", expected=3006)
            assert amm_view(stand.account_state(amm)) == pre, "a refused swap changes nothing"
            body = b"\x00" + le(want_out, 16) + le(need, 16) + le(NEVER, 4)
            result = stand.call(amm, "vault.amm.swap_exact_out", body, identity="user")
            assert "amm.swapped" in event_names(result), event_names(result)
            post = amm_view(stand.account_state(amm))
            assert post["reserve"] == [r_in + need, r_out - want_out], (post, need)
            assert post["reserve"][0] * post["reserve"][1] >= r_in * r_out, "k never decreases"
            (b0, b1), shares = pre["accounts"][alice]
            assert post["accounts"][alice] == ([b0 - need, b1 + want_out], shares), post["accounts"][alice]
            assert (post["cumulative"], post["last_ts"]) == twap_after(pre, stand.time)
            assert post["cumulative"][0] > pre["cumulative"][0] and post["cumulative"][1] > pre["cumulative"][1]

            # The status view reports the pool without changing it.
            before_status = amm_view(stand.account_state(amm))
            result = stand.call(amm, "vault.amm.status", identity="developer")
            assert "amm.status" in event_names(result), event_names(result)
            blob = b"".join(bytes.fromhex(event) for event in result["events"])
            for value in (post["reserve"][0], post["reserve"][1], post["lp_total"]):
                assert value < 1 << 64 and le(value, 8) in blob, value  # small u128 fields use 8 bytes
            after_status = amm_view(stand.account_state(amm))
            assert after_status["reserve"] == before_status["reserve"] and after_status["lp_total"] == before_status["lp_total"]
            view = after_status

            events = stand.command("events")
            inspect = stand.command("inspect")
            replay = stand.command("replay")
            assert replay["status"] == "ok", replay
            assert inspect["queue"]["message_count"] == 0, inspect["queue"]
            for key in ["account_state_root", "registry_root", "cumulative_fee_shells", "accepted_input_count"]:
                assert replay[key] == inspect[key], (key, replay[key], inspect[key])
            summary = {"status": "PASS", "amm": amm, "receiver": receiver, "final": view,
                       "fuel_reserve_needed_shells": stand.fuel, "events": events, "inspect": inspect,
                       "replay": replay}
            output.write_text(json.dumps(summary, indent=2, default=str) + "\n")
            print(f"vault-amm journey PASS: {output}")
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
