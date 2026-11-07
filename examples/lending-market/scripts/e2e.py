#!/usr/bin/env python3
"""Reproducible journey on the real AVM local stand. No network, no real funds.

Run after `avm-contract-build build` produced both artifact triples:

    python3 scripts/e2e.py --sdk ~/.local/share/avm-sdk/current

The journey deploys the lending market and a payee, tops up the market's FUEL,
feeds a price, and walks three stand identities through the whole life of a loan:
a lender supplies, a borrower locks collateral and borrows, thirty days of
interest accrue, the price falls in steps of at most 20% until the position can
be liquidated, a liquidator closes it in rounds until the collateral is gone, the
remaining bad debt is written off, the lender redeems its shares, and withdrawals
go to the payee (delivered, bounced by a payee that rejects, and delivered again).

The script carries its own model of the accounting rules (interest index, share
price, rounding directions, liquidation and write-off). At the checkpoints,
decoded accounting fields and accounts are compared with the model. The stand
journal is replayed at the end.
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
SHELL, COLLATERAL, DEBT = 2, 7, 9
FUEL_RESERVE = 10_000
STATE_BYTES = 4_528

ONE = 10**18
BPS = 10_000
YEAR = 31_536_000
PARAMS = dict(ltv=7_000, liq=8_000, bonus=500, reserve_factor=1_000, kink=8_000, base=200,
              slope1=800, slope2=6_000, max_move=2_000)
MAX_AGE = 90 * 24 * 3600

ERR_HEALTHY, ERR_PRICE_MOVE = 5008, 5010

EVENTS = ["lend.created", "lend.action", "lend.price", "lend.params"]


def le(value, size):
    return int(value).to_bytes(size, "little")


def u32(raw, offset):
    return int.from_bytes(raw[offset:offset + 4], "little")


def u64(raw, offset):
    return int.from_bytes(raw[offset:offset + 8], "little")


def u128(raw, offset):
    return int.from_bytes(raw[offset:offset + 16], "little")


def ceil_div(a, b):
    return -(-a // b)


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
        # Probe the expected call status on copies of the state. Each SHELL buys
        # 1_000_000 FUEL. The main call also uses 1 SHELL and checks settlement.
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


def init_data(admin):
    p = PARAMS
    return (bytes.fromhex(admin) + le(COLLATERAL, 4) + le(DEBT, 4)
            + b"".join(le(p[k], 2) for k in ("ltv", "liq", "bonus", "reserve_factor", "kink", "base",
                                             "slope1", "slope2", "max_move"))
            + b"\0\0" + le(MAX_AGE, 4) + le(FUEL_RESERVE, 16))


def market_view(raw):
    """Decodes the fixed 4528-byte market state image."""
    assert len(raw) == STATE_BYTES, len(raw)
    accounts = {}
    for index in range(24):
        base = 752 + 112 * index
        key = raw[base:base + 32]
        if key == bytes(32):
            break
        accounts[key.hex()] = dict(bal=[u128(raw, base + 32), u128(raw, base + 48)], col=u128(raw, base + 64),
                                   shares=u128(raw, base + 80), scaled=u128(raw, base + 96))
    pending = sum(raw[3440 + 136 * i] for i in range(8))
    return dict(paused=raw[80], price=u128(raw, 88), price_ts=u32(raw, 104), cash=u128(raw, 112),
                reserves=u128(raw, 128), shares=u128(raw, 144), scaled=u128(raw, 160), index=u128(raw, 176),
                accrual=u32(raw, 192), collateral=u128(raw, 200), ops=u64(raw, 216), seq=u64(raw, 224),
                accounts=accounts, pending=pending)


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
    return b"LND1" + le(currency, 4) + le(ident, 8) + le(amount, 16)


class Model:
    """The accounting rules, written down independently of the contract."""

    def __init__(self, deployed_at):
        self.cash = self.reserves = self.shares = self.scaled = self.collateral = 0
        self.index = ONE
        self.accrual = deployed_at
        self.price = self.price_ts = 0
        self.accounts = {}

    def account(self, key):
        return self.accounts.setdefault(key, dict(bal=[0, 0], col=0, shares=0, scaled=0))

    def debt_of(self, scaled):
        return ceil_div(scaled * self.index, ONE)

    def value(self):
        return self.cash + self.debt_of(self.scaled) - self.reserves

    def available(self):
        return max(self.cash - self.reserves, 0)

    def rate_bps(self, util):
        p = PARAMS
        rate = p["base"] + p["slope1"] * min(util, p["kink"]) // p["kink"]
        if util > p["kink"]:
            rate += p["slope2"] * (util - p["kink"]) // (BPS - p["kink"])
        return rate

    def accrue(self, now):
        if now <= self.accrual:
            return
        dt, self.accrual = now - self.accrual, now
        if self.scaled == 0:
            return
        before = self.debt_of(self.scaled)
        util = before * BPS // (self.cash + before)
        self.index += self.index * (self.rate_bps(util) * dt) // (YEAR * BPS)
        interest = max(self.debt_of(self.scaled) - before, 0)
        self.reserves += interest * PARAMS["reserve_factor"] // BPS

    def compact(self):
        self.accounts = {k: a for k, a in self.accounts.items()
                         if a["bal"] != [0, 0] or a["col"] or a["shares"] or a["scaled"]}

    def limit(self, collateral, bps):
        return (collateral * self.price // ONE) * bps // BPS

    # Each operation below mirrors one successful call at block time `now`.
    def set_price(self, now, price):
        self.accrue(now)
        self.price, self.price_ts = price, now

    def deposit(self, now, key, side, amount):
        self.accrue(now)
        self.account(key)["bal"][side] += amount

    def supply(self, now, key, amount):
        self.accrue(now)
        minted = amount if self.shares == 0 else amount * self.shares // self.value()
        acc = self.account(key)
        acc["bal"][1] -= amount
        acc["shares"] += minted
        self.shares += minted
        self.cash += amount
        return minted

    def add_collateral(self, now, key, amount):
        self.accrue(now)
        acc = self.account(key)
        acc["bal"][0] -= amount
        acc["col"] += amount
        self.collateral += amount

    def borrow(self, now, key, amount):
        self.accrue(now)
        scaled = ceil_div(amount * ONE, self.index)
        acc = self.account(key)
        acc["scaled"] += scaled
        acc["bal"][1] += amount
        self.scaled += scaled
        self.cash -= amount
        assert self.debt_of(acc["scaled"]) <= self.limit(acc["col"], PARAMS["ltv"]), "model: borrow over ltv"

    def unhealthy(self, key):
        acc = self.accounts[key]
        debt = self.debt_of(acc["scaled"])
        return debt > 0 and debt > self.limit(acc["col"], PARAMS["liq"])

    def liquidate(self, now, liquidator, borrower, wanted):
        self.accrue(now)
        target = self.accounts[borrower]
        debt = self.debt_of(target["scaled"])
        assert self.unhealthy(borrower), "model: position is healthy"
        repaid = min(wanted, debt - (debt >> 1))
        bonus = BPS + PARAMS["bonus"]
        seized = (repaid * bonus // BPS) * ONE // self.price
        if seized > target["col"]:
            seized = target["col"]
            repaid = (seized * self.price // ONE) * BPS // bonus
        assert repaid > 0 and seized > 0
        self.account(liquidator)["bal"][1] -= repaid
        removed = target["scaled"] if repaid >= debt else min(repaid * ONE // self.index, target["scaled"])
        target["scaled"] -= removed
        self.scaled -= removed
        self.cash += repaid
        target["col"] -= seized
        self.collateral -= seized
        self.account(liquidator)["bal"][0] += seized
        return repaid, seized

    def write_off(self, now, borrower):
        self.accrue(now)
        target = self.accounts[borrower]
        assert target["scaled"] > 0 and target["col"] == 0
        before = self.debt_of(self.scaled)
        self.scaled -= target["scaled"]
        target["scaled"] = 0
        loss = before - self.debt_of(self.scaled)
        absorbed = min(loss, self.reserves)
        self.reserves -= absorbed
        return loss, absorbed

    def redeem(self, now, key, shares):
        self.accrue(now)
        assets = shares * self.value() // self.shares
        assert assets <= self.available()
        acc = self.account(key)
        acc["shares"] -= shares
        acc["bal"][1] += assets
        self.shares -= shares
        self.cash -= assets
        return assets

    def withdraw_delivered(self, now, key, side, amount):
        self.accrue(now)
        self.account(key)["bal"][side] -= amount

    def withdraw_bounced(self, now):
        self.accrue(now)  # the principal returns to the holder: nothing changes


def run(sdk, output):
    with tempfile.TemporaryDirectory(prefix="avm-lending-market-") as directory:
        stand = Stand(sdk, Path(directory))
        try:
            init = stand.command("init", dapp_id=DAPP)
            assert init["status"] == "ok", init
            ids = init["identities"]
            alice, bob, carol = ids["developer"], ids["user"], ids["package_owner"]
            # alice: admin and liquidator, bob: lender, carol: borrower
            who = {alice: "developer", bob: "user", carol: "package-owner"}

            payee = stand.deploy("payee", "lending-payee")
            market = stand.deploy("lending", "lending-market", init_data(alice))
            # Execution fees and storage rent are charged to the contracts' own SHELL. Rent is
            # 1 SHELL per byte per second, so the market's 4528-byte state needs about 1.2e10 SHELL
            # for the thirty days this journey lets pass; top both accounts up far beyond that.
            for address, shells in ((market, 100_000_000_000), (payee, 10_000_000_000)):
                funding = stand.command("fund-user", address=address, shells=shells)
                assert funding["status"] == "ok", funding
            model = Model(stand.time)  # State::new records the deploy block time as last accrual
            recipient = address_bytes(payee)

            def view():
                return market_view(stand.account_state(market))

            def check(label):
                """The decoded contract state must equal the model, field by field."""
                got = view()
                want = dict(price=model.price, price_ts=model.price_ts, cash=model.cash, reserves=model.reserves,
                            shares=model.shares, scaled=model.scaled, index=model.index, accrual=model.accrual,
                            collateral=model.collateral)
                model.compact()
                for name, value in want.items():
                    assert got[name] == value, (label, name, got[name], value)
                assert got["accounts"] == model.accounts, (label, got["accounts"], model.accounts)
                assert got["pending"] == 0 and got["paused"] == 0, (label, got)
                return got

            def call(method, body=b"", who_key=alice, **kw):
                return stand.call(market, method, body, identity=who[who_key], **kw)

            state = check("deploy")
            assert state["accrual"] == stand.time and state["index"] == ONE and state["ops"] == 1, state

            # FUEL for payouts, then the first price (any value is accepted while none is set).
            result = call("lend.topup", extra=f"{SHELL}:100000")
            assert "lend.action" in event_names(result), event_names(result)
            model.accrue(stand.time)  # every call accrues interest first, even one that moves no funds
            check("topup")
            stand.call(payee, "lend.payee.bind", address_bytes(market), identity="developer")
            price = 2 * ONE
            call("lend.set_price", price)
            model.set_price(stand.time, price)
            check("first price")
            call("lend.set_price", price + 1, who_key=bob, expected=5001)  # only the admin feeds prices

            # Funds enter through deposit. The contract checks its solvency after crediting
            # the deposit against `input.extra`, so these passing shows `input.extra` is the
            # post-credit balance.
            for key, side, amount in ((bob, 1, 1_000_000), (carol, 0, 100_000), (alice, 1, 500_000)):
                currency = COLLATERAL if side == 0 else DEBT
                call("lend.deposit", who_key=key, extra=f"{currency}:{amount}")
                model.deposit(stand.time, key, side, amount)
                check("deposit")

            call("lend.supply", 1_000_000, who_key=bob)
            assert model.supply(stand.time, bob, 1_000_000) == 1_000_000  # first supplier: 1 share per unit
            check("supply")
            call("lend.add_collateral", 100_000, who_key=carol)
            model.add_collateral(stand.time, carol, 100_000)
            check("add collateral")

            # 100_000 collateral at price 2 is worth 200_000; 70% ltv allows a debt of 140_000.
            call("lend.borrow", 140_001, who_key=carol, expected=5007)
            call("lend.borrow", 120_000, who_key=carol)
            model.borrow(stand.time, carol, 120_000)
            borrowed_at = stand.time
            got = check("borrow")
            assert model.debt_of(got["accounts"][carol]["scaled"]) == 120_000

            # Thirty days pass. Utilisation is 120_000 / 1_000_000 = 12%, so the annual rate is
            # 200 + 800 * 1200 / 8000 = 320 bps and the index grows by 320 * dt / (year * 10_000).
            stand.advance(30 * 24 * 3600)
            call("lend.set_price", price)  # also refreshes the price age; accrual happens first
            model.set_price(stand.time, price)
            dt = stand.time - borrowed_at
            got = check("interest")
            assert got["index"] == ONE + ONE * (320 * dt) // (YEAR * BPS), got["index"]
            debt = model.debt_of(got["accounts"][carol]["scaled"])
            assert debt == ceil_div(120_000 * got["index"], ONE) and debt > 120_300, debt
            interest = debt - 120_000
            assert got["reserves"] == interest * PARAMS["reserve_factor"] // BPS, got["reserves"]

            # The price may move by at most 20% per update.
            call("lend.set_price", 1_580_000_000_000_000_000, expected=ERR_PRICE_MOVE)
            check("refused move")
            price = 1_600_000_000_000_000_000
            call("lend.set_price", price)
            model.set_price(stand.time, price)
            check("price 1.6")
            assert not model.unhealthy(carol), "collateral 160_000 * 80% still covers the debt"
            call("lend.liquidate", bytes.fromhex(carol) + le(1, 16), who_key=alice, expected=ERR_HEALTHY)
            check("healthy position refused")

            price = 1_280_000_000_000_000_000
            call("lend.set_price", price)
            model.set_price(stand.time, price)
            check("price 1.28")
            assert model.unhealthy(carol)

            liquidations = []

            def liquidate(label):
                repaid, seized = model.liquidate(stand.time + 1, alice, carol, 10**30)
                result = call("lend.liquidate", bytes.fromhex(carol) + le(10**30, 16), who_key=alice)
                assert stand.time == model.accrual, "liquidation accrued at the call's block time"
                assert "lend.action" in event_names(result)
                check(label)
                liquidations.append((repaid, seized))

            liquidate("liquidation 1")
            price = 1_024_000_000_000_000_000
            call("lend.set_price", price)
            model.set_price(stand.time, price)
            check("price 1.024")
            liquidate("liquidation 2")
            price = 819_200_000_000_000_000
            call("lend.set_price", price)
            model.set_price(stand.time, price)
            check("price 0.8192")
            while model.accounts[carol]["col"] > 0:
                liquidate(f"liquidation {len(liquidations) + 1}")
                assert len(liquidations) < 12, "liquidation should exhaust the collateral"
            assert model.accounts[carol]["col"] == 0 and model.accounts[carol]["scaled"] > 0
            # Each round closed at most half of the debt that was owed at the time.
            assert all(repaid > 0 and seized > 0 for repaid, seized in liquidations)

            # Bad debt: anyone may write it off. The reserves absorb it first, suppliers the rest.
            model.accrue(stand.time + 1)
            value_before = model.value()
            loss, absorbed = model.write_off(stand.time + 1, carol)
            result = call("lend.write_off", bytes.fromhex(carol), who_key=bob)
            assert "lend.action" in event_names(result)
            got = check("write off")
            assert absorbed == min(loss, got["reserves"] + absorbed) and got["scaled"] == 0
            assert model.value() == value_before - (loss - absorbed), "suppliers bear what the reserves did not cover"

            # Bob redeems every share; the share price fell by the unabsorbed loss.
            shares = got["accounts"][bob]["shares"]
            assets = model.redeem(stand.time + 1, bob, shares)
            call("lend.redeem", shares, who_key=bob)
            got = check("redeem")
            assert assets < 1_000_000 and got["shares"] == 0, (assets, got)

            # Withdrawals go to the payee as acknowledged payouts.
            delivered = 0

            def withdraw(key, currency, amount, accepted):
                nonlocal delivered
                body = le(currency, 4) + le(amount, 16) + recipient
                ident = view()["seq"]
                now = stand.time + 1
                call("lend.withdraw", body, who_key=key)
                if accepted:
                    model.withdraw_delivered(now, key, 0 if currency == COLLATERAL else 1, amount)
                    delivered += 1
                    got = payee_view(stand.account_state(payee))
                    assert got["count"] == delivered and got["last"] == receipt(currency, ident, amount), got
                else:
                    model.withdraw_bounced(now)
                    assert payee_view(stand.account_state(payee))["count"] == delivered
                check("withdraw delivered" if accepted else "withdraw bounced")

            withdraw(bob, DEBT, assets, accepted=True)
            stand.call(payee, "lend.payee.reject", b"\x01", identity="developer")
            withdraw(alice, DEBT, 1_000, accepted=False)
            assert "lend.action" in stand.events_seen
            stand.call(payee, "lend.payee.reject", b"\x00", identity="developer")
            withdraw(alice, COLLATERAL, 5_000, accepted=True)
            final = view()
            assert final["seq"] == 3 and final["pending"] == 0, final

            assert set(EVENTS) - stand.events_seen - {"lend.created", "lend.params"} == set()
            events = stand.command("events")
            assert "lend.created".encode().hex() in json.dumps(events), "the deploy emitted lend.created"
            inspect = stand.command("inspect")
            replay = stand.command("replay")
            assert replay["status"] == "ok", replay
            assert inspect["queue"]["message_count"] == 0, inspect["queue"]
            for key in ["account_state_root", "registry_root", "cumulative_fee_shells", "accepted_input_count"]:
                assert replay[key] == inspect[key], (key, replay[key], inspect[key])
            summary = {"status": "PASS", "market": market, "payee": payee, "debt_after_30_days": debt,
                       "liquidations": liquidations, "write_off": {"loss": loss, "absorbed": absorbed},
                       "redeemed": assets, "final": final, "fuel_probe_reserve_shells": stand.fuel,
                       "events_seen": sorted(stand.events_seen), "events": events, "inspect": inspect,
                       "replay": replay}
            output.write_text(json.dumps(summary, indent=2, default=str) + "\n")
            print(f"lending-market journey PASS: {output}")
            print(f"debt after 30 days: {debt} (borrowed 120000); liquidations: {len(liquidations)}; "
                  f"write-off loss {loss}, absorbed by reserves {absorbed}")
            print("FUEL probe (smallest tried reserve with expected status in SHELL, 1 SHELL = 1,000,000 FUEL; cap 10):")
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
