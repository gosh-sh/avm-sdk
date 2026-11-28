#!/usr/bin/env python3
"""Check and measure the lending path against an adapted PR #32 WASM on SDK 0.6.5."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

WALLET_FEE_SHELLS = 100_000
PR_HEAD = "b02056417049f3d8dc1b71a57fbb76af9b1f1d1e"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def require(condition, detail):
    if not condition:
        raise RuntimeError(detail)


def run(sdk, market_dir, wallet_dir, work, corpus=None, full_registry=False, admin_scenario=False, reserve_capacity=None):
    spec = importlib.util.spec_from_file_location("pr32_e2e", market_dir / "scripts/e2e.py")
    original = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(original)
    profile = sdk / "profiles/wasmtime-release-profile.v1"
    release = json.loads((sdk / "BUILD-MANIFEST.json").read_text())
    require(release["bundle_version"] == "0.6.5", release)
    require(digest(profile) == release["wasmtime_profile_sha256"], "profile digest mismatch")
    state = work / "lending.state"
    trace = work / "execution.tsv"
    memory = work / "memory.jsonl"
    env = os.environ.copy()
    env.pop("AVM_COMBINED_EXECUTION_PATH", None)
    env.pop("AVM_MEMORY_REPORT", None)
    clock = 1_800_000_000
    commands = []
    measurements = []
    checkpoints = {}
    withdrawals = []
    capacity = {}
    admin_results = {}
    filler_accounts = {}
    peak_by_contract = {}

    def record_memory(runs):
        for observed in runs:
            code_hash = observed["code_hash"]
            current = peak_by_contract.get(code_hash, 0)
            peak_by_contract[code_hash] = max(current, observed["peak_bytes"])

    methods = dict(re.findall(r"pub const (\w+): u64 = (0x[0-9a-f]+);",
                              (market_dir / "contracts/lending/src/ids.rs").read_text()))

    def stand(action, capture=False, rejected_status=None, **options):
        argv = [sdk / "bin/avm-local-stand", action, "--state", state,
                "--wasmtime-profile", profile]
        for key, value in options.items():
            argv.append("--" + key.replace("_", "-"))
            if value is not True:
                argv.append(str(value))
        active = env
        if capture:
            trace.unlink(missing_ok=True)
            memory.unlink(missing_ok=True)
            active = env | {"AVM_COMBINED_EXECUTION_PATH": str(trace),
                            "AVM_MEMORY_REPORT": str(memory)}
        result = subprocess.run(argv, env=active, capture_output=True, text=True)
        commands.append(dict(action=action, options={k: str(v) for k, v in options.items()},
                             returncode=result.returncode))
        if capture and corpus is not None:
            for source, extension in ((trace, ".tsv"), (memory, ".memory.jsonl")):
                (corpus / f"stand-{len(commands):04d}{extension}").write_bytes(source.read_bytes())
        if rejected_status is None:
            require(result.returncode == 0, f"{action} {options}: {result.stdout}\n{result.stderr}")
        reply = json.loads(result.stdout)
        if rejected_status is not None:
            require(result.returncode == 2 and reply.get("error") == "execution_failed"
                    and reply.get("state_committed") is True
                    and reply.get("status_code") == rejected_status, reply)
        else:
            require(reply.get("status") == "ok" or reply.get("quoted") is True, reply)
        return reply

    def stamps():
        nonlocal clock
        clock += 1
        return dict(block_time=clock, created_at=clock, expire_at=clock + 3600,
                    fuel_fee_shells=WALLET_FEE_SHELLS)

    def deploy(folder, salt, owner, init, extra=None, committed=None):
        artifacts = folder / "target/avm"
        code = committed if committed is not None else dict(
            wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
            descriptor=artifacts / "contract.descriptor")
        options = dict(**code, salt_hex=salt, owner_id=owner,
                       init_json=json.dumps(init), **stamps())
        quote = stand("deploy", quote_address=True, **options)
        address = quote["address"]
        funding = {"extra_currencies": extra} if extra is not None else {}
        stand("fund-user", address=address, shells=100_000_000, **funding)
        stand("deploy", capture=True, **options)
        report = json.loads(memory.read_text())
        require(report["complete"], report)
        init_run, = report["runs"]
        record_memory(report["runs"])
        return address, init_run["code_hash"], quote

    def callback_status(receipt):
        marker = b"status_code"
        events = [bytes.fromhex(event) for event in receipt["events"]]
        callback, = [event for event in events if b"wallet.callback" in event]
        encoded = callback.split(marker, 1)[1][13:21]
        return int.from_bytes(encoded, "little")

    def captures():
        executions = [dict(field.split("=", 1) for field in row.split("\t"))
                      for row in trace.read_text().splitlines()]
        report = json.loads(memory.read_text())
        require(report["complete"] and len(report["runs"]) == len(executions), report)
        record_memory(report["runs"])
        return executions, report["runs"]

    def guest_input(guest):
        data = bytes.fromhex(guest["input"])
        require(data[:4] == b"ACMI", data[:4])
        # ACMI prefix: magic, context, caller and destination, code and state mode.
        count = int.from_bytes(data[260:262], "little")
        position = 262 + count * 20
        balances = currency_amounts(data[262:position])
        state_len = int.from_bytes(data[position:position + 4], "little")
        position += 4 + state_len
        message_len = int.from_bytes(data[position:position + 4], "little")
        return balances, data[position + 4:position + 4 + message_len]

    def currency_amounts(data):
        require(len(data) % 20 == 0, len(data))
        return {int.from_bytes(data[i:i + 4], "little"): int.from_bytes(data[i + 4:i + 20], "little")
                for i in range(0, len(data), 20)}

    def internal_transfer(guest, flags):
        balances, message = guest_input(guest)
        require(message[:2] == bytes([0, flags]), message[:2])
        count = int.from_bytes(message[2:4], "little")
        start = 176 + (100 if flags == 1 else 8)
        return balances, message, currency_amounts(message[start:start + count * 20])

    def guest_status(guest):
        require(guest["entrypoint"] == "process_message" and guest["result"] == "ok"
                and guest["vm_status"] == "0", {k: guest[k] for k in ("entrypoint", "result", "vm_status")})
        output = bytes.fromhex(guest["output"])
        require(output[:4] == b"ACMO", output[:8])
        return int.from_bytes(output[4:8], "little")

    def send(wallet, signer, method, amount=None, currencies=(), expected=0, label=None, withdrawal=None, payload=None):
        body = b"" if amount is None else amount.to_bytes(16, "little")
        if payload is not None:
            body = payload
        if method == "WITHDRAW":
            body = body + original.address_bytes(payee)
            body = original.le(original.COLLATERAL, 4) + body
        args = (original.address_bytes(market) + int(methods[method], 16).to_bytes(8, "little")
                + WALLET_FEE_SHELLS.to_bytes(16, "little") + bytes([1, len(currencies)])
                + b"".join(key.to_bytes(4, "little") + value.to_bytes(16, "little")
                           for key, value in currencies) + body)
        output = stand("call", capture=True, address=wallet, method="wallet.send",
                       identity=signer, args_json=json.dumps("0x" + args.hex()), **stamps())
        actual = callback_status(output)
        require(actual == expected, (method, expected, actual))
        executions, runs = captures()
        hashes = [run["code_hash"] for run in runs]
        require(hashes[0] == wallet_code_hash and hashes[1] == market_code_hash, hashes)
        if withdrawal is not None:
            require(len(hashes) == 5 and hashes.count(wallet_code_hash) == 2
                    and hashes.count(market_code_hash) == 2 and hashes.count(payee_code_hash) == 1, hashes)
            market_callback = hashes.index(market_code_hash, 2)
            receiver = hashes.index(payee_code_hash)
            require(receiver < market_callback, hashes)
            require(guest_status(executions[receiver]) == withdrawal, hashes)
            require(guest_status(executions[market_callback]) == 0, hashes)
            coin = original.COLLATERAL
            receiver_balances, send_message, sent = internal_transfer(executions[receiver], 4)
            market_balances, callback_message, returned = internal_transfer(executions[market_callback], 1)
            require(send_message[48:112] == original.address_bytes(market)
                    and send_message[112:176] == original.address_bytes(payee), "payout route mismatch")
            require(callback_message[48:112] == original.address_bytes(payee)
                    and callback_message[112:176] == original.address_bytes(market), "callback route mismatch")
            require(int.from_bytes(callback_message[272:276], "little") == withdrawal, "callback cause status")
            expected_return = amount if withdrawal != 0 else 0
            require(sent.get(coin, 0) == receiver_balances.get(coin, 0) == amount, sent)
            require(returned.get(coin, 0) == market_balances.get(coin, 0) == expected_return, returned)
            withdrawals.append(dict(
                sent_principal=sent[coin], returned_principal=returned.get(coin, 0),
                collateral_balance_at_market_callback=market_balances.get(coin, 0),
                label=label, receiver_status=withdrawal, market_withdraw_status=actual,
                market_callback_status=0, market_withdraw_fuel=int(executions[1]["fuel_used"]),
                market_callback_fuel=int(executions[market_callback]["fuel_used"]),
                receiver_fuel=int(executions[receiver]["fuel_used"]),
                wallet_send_fuel=int(executions[0]["fuel_used"]),
                wallet_callback_fuel=int(executions[hashes.index(wallet_code_hash, 2)]["fuel_used"]),
                market_withdraw_peak_bytes=runs[1]["peak_bytes"],
                market_callback_peak_bytes=runs[market_callback]["peak_bytes"],
                receiver_peak_bytes=runs[receiver]["peak_bytes"],
                event_receipts=output["events"],
                guest_statuses=[guest_status(item) for item in executions],
            ))
            require(all(run["rejected_growths"] == 0 for run in runs), runs)
            return
        require(hashes == [wallet_code_hash, market_code_hash, wallet_code_hash], hashes)
        market_run, guest = runs[1], executions[1]
        if method == "DEPOSIT" and expected != 0:
            wallet_balances, callback_message, returned = internal_transfer(executions[2], 1)
            require(int.from_bytes(callback_message[272:276], "little") == expected, "wallet callback status")
            for coin, amount in currencies:
                require(returned.get(coin, 0) == amount == wallet_balances.get(coin, 0),
                        (wallet_balances, returned, currencies))
            capacity["refunded_currencies"] = {str(coin): returned[coin] for coin, _ in currencies}
        require(market_run["rejected_growths"] == 0, market_run)
        require(guest_status(guest) == expected, (method, expected, guest_status(guest)))
        measurements.append(dict(
            label=label or method.lower(), method="lend." + method.lower(), status=actual,
            wallet_send_fuel=int(executions[0]["fuel_used"]),
            market_compute_fuel=int(guest["fuel_used"]),
            wallet_callback_fuel=int(executions[2]["fuel_used"]),
            market_guest_input_bytes=len(bytes.fromhex(guest["input"])),
            market_peak_linear_memory_bytes=market_run["peak_bytes"],
            market_declared_max_memory_bytes=market_run["declared_max_bytes"],
            market_code_hash=market_run["code_hash"],
        ))

    def market_state():
        return bytes.fromhex(stand("inspect", account=market)["raw_state"])

    def checkpoint(label, expected):
        raw = market_state()
        view = original.market_view(raw)
        if filler_accounts:
            require(len(view["accounts"]) == capacity["setup_accounts"], (label, len(view["accounts"])))
            for key, expected_account in filler_accounts.items():
                require(view["accounts"].get(key) == expected_account, (label, key))
        borrower = view["accounts"].get(borrower_key, dict(bal=[0, 0], col=0, scaled=0))
        lender = view["accounts"].get(lender_key, dict(shares=0))
        debt = original.ceil_div(borrower["scaled"] * view["index"], original.ONE)
        checkpoint_data = {
            "occupied_accounts": len(view["accounts"]),
            "available_collateral": borrower["bal"][0], "locked_collateral": borrower["col"],
            "available_debt_asset": borrower["bal"][1], "debt": debt,
            "cash": view["cash"], "total_locked_collateral": view["collateral"],
            "lender_shares": lender["shares"], "pending_payouts": view["pending"],
        }
        for key, value in expected.items():
            require(checkpoint_data[key] == value, (label, checkpoint_data, expected))
        require(checkpoint_data["pending_payouts"] == 0, checkpoint_data)
        checkpoints[label] = checkpoint_data
        return raw

    identities = stand("init", dapp_id="11" * 32)["identities"]
    market, market_code_hash, _ = deploy(market_dir / "contracts/lending", "01", identities["developer"],
                                "0x" + original.init_data(identities["developer"]).hex())
    lender_wallet, wallet_code_hash, wallet_quote = deploy(wallet_dir, "02", identities["developer"], None, "9:1000000")
    committed = {key: wallet_quote[key] for key in ("package_id", "selected_code_id")}
    borrower_wallet, _, _ = deploy(wallet_dir, "03", identities["package_owner"], None,
                                  "7:100000,9:100", committed=committed)
    payee, payee_code_hash, _ = deploy(market_dir / "contracts/payee", "04",
                                         identities["developer"], None)

    def payee_control(method, data):
        reply = stand("call", capture=True, address=payee, identity="developer", method=method,
                      args_json=json.dumps("0x" + data.hex()), **stamps())
        require(bytes.fromhex(reply["raw_executor_output"])[:8] == b"ACMO\0\0\0\0", reply)
        executions, runs = captures()
        guest, = executions
        payee_run, = runs
        require(payee_run["code_hash"] == payee_code_hash and guest_status(guest) == 0, payee_run)
        return guest_input(guest)[0].get(original.COLLATERAL, 0)

    payee_control("lend.payee.bind", original.address_bytes(market))
    lender_key = hashlib.sha256(original.address_bytes(lender_wallet)).hexdigest()
    borrower_key = hashlib.sha256(original.address_bytes(borrower_wallet)).hexdigest()

    receipt = stand("call", capture=True, address=market, identity="developer",
                    method="lend.set_price", args_json=str(2 * original.ONE), **stamps())
    require(bytes.fromhex(receipt["raw_executor_output"])[:8] == b"ACMO\0\0\0\0", receipt)
    trace_row, = trace.read_text().splitlines()
    guest = dict(field.split("=", 1) for field in trace_row.split("\t"))
    report = json.loads(memory.read_text())
    market_run, = report["runs"]
    record_memory(report["runs"])
    require(report["complete"] and market_run["code_hash"] == market_code_hash
            and market_run["rejected_growths"] == 0
            and guest["entrypoint"] == market_run["entrypoint"] == "process_message"
            and guest["result"] == "ok" and guest["vm_status"] == "0", report)
    measurements.append(dict(
        label="set_price", method="lend.set_price", status=0,
        wallet_send_fuel=None, wallet_callback_fuel=None,
        market_compute_fuel=int(guest["fuel_used"]),
        market_guest_input_bytes=len(bytes.fromhex(guest["input"])),
        market_peak_linear_memory_bytes=market_run["peak_bytes"],
        market_declared_max_memory_bytes=market_run["declared_max_bytes"],
        market_code_hash=market_code_hash,
    ))

    send(lender_wallet, "developer", "DEPOSIT", currencies=((9, 1_000_000),), label="lender_deposit")
    send(lender_wallet, "developer", "SUPPLY", 1_000_000)
    checkpoint("supplied", dict(cash=1_000_000, lender_shares=1_000_000))
    send(borrower_wallet, "package-owner", "DEPOSIT", currencies=((7, 100_000), (9, 100)),
         label="borrower_deposit")
    checkpoint("deposited", dict(available_collateral=100_000, available_debt_asset=100))

    if full_registry or reserve_capacity:
        state_rs = (market_dir / "contracts/lending/src/state.rs").read_text()
        account_limit = int(re.search(r"pub const ACCOUNT_SLOTS: usize = (\d+);", state_rs)[1])
        source = (market_dir / "contracts/lending/src/lib.rs").read_text()
        capacity_error = int(re.search(r"pub const ERR_CAPACITY: u32 = (\d+);", source)[1])
        coin = original.DEBT
        setup_accounts = account_limit - (reserve_capacity == "existing")
        for index in range(2, setup_accounts):
            wallet, _, _ = deploy(wallet_dir, f"{index + 16:02x}", identities["developer"],
                                  None, f"{coin}:1", committed=committed)
            send(wallet, "developer", "DEPOSIT", currencies=((coin, 1),),
                 label=f"fill_slot_{index + 1}")
            key = hashlib.sha256(original.address_bytes(wallet)).hexdigest()
            require(key not in filler_accounts and key not in (lender_key, borrower_key), "reused holder")
            filler_accounts[key] = dict(bal=[0, 1], col=0, shares=0, scaled=0)
        capacity.update(account_limit=account_limit, filler_accounts=len(filler_accounts),
                        setup_accounts=setup_accounts, overflow_status=capacity_error)
        before = checkpoint("registry_prepared", dict(occupied_accounts=setup_accounts))
        if full_registry:
            overflow, _, _ = deploy(wallet_dir, "ff", identities["developer"], None,
                                     f"{coin}:1", committed=committed)
            send(overflow, "developer", "DEPOSIT", currencies=((coin, 1),),
                 expected=capacity_error, label="overflow_deposit")
            after = checkpoint("overflow_rejected", dict(occupied_accounts=account_limit))
            require(after == before, "overflow attempt changed contract state")
            capacity["overflow_market_state_sha256"] = hashlib.sha256(after).hexdigest()
            capacity["overflow_preserved_market_state"] = True
    send(borrower_wallet, "package-owner", "ADD_COLLATERAL", 100_000)
    checkpoint("locked", dict(available_collateral=0, locked_collateral=100_000))
    send(borrower_wallet, "package-owner", "BORROW", 120_000)
    before_rejection = checkpoint("borrowed", dict(debt=120_000, cash=880_000,
                                                   available_debt_asset=120_100))
    def run_admin_scenario(original_state):
        nonlocal clock
        admin_key = identities["developer"]
        valid_params = dict(original.PARAMS, ltv=6_900, base=500)
        month = 30 * 24 * 3600

        def encode_params(config, paused=0):
            return (b"".join(original.le(config[key], 2) for key in original.PARAMS)
                    + original.le(original.MAX_AGE, 4) + bytes([paused]))

        def admin_call(method, label, data=None, expected=0):
            reply = stand("call", capture=True, address=market, identity="developer", method=method,
                          rejected_status=expected or None, args_json=json.dumps(None if data is None else "0x" + data.hex()), **stamps())
            executions, runs = captures()
            execution, = executions
            observed, = runs
            require(observed["code_hash"] == market_code_hash and observed["rejected_growths"] == 0, observed)
            require(guest_status(execution) == expected, (label, expected, guest_status(execution)))
            measurements.append(dict(
                label=label, method=method, status=expected,
                market_compute_fuel=int(execution["fuel_used"]),
                market_peak_linear_memory_bytes=observed["peak_bytes"],
                market_declared_max_bytes=observed["declared_max_bytes"],
                market_guest_input_bytes=len(bytes.fromhex(execution["input"])),
            ))
            return reply

        send(borrower_wallet, "package-owner", "SET_PARAMS", expected=5001,
             label="set_params_unauthorized", payload=encode_params(valid_params))
        require(market_state() == original_state, "unauthorized set_params changed state")
        invalid_params = dict(valid_params, ltv=valid_params["liq"])
        admin_call("lend.set_params", "set_params_invalid", encode_params(invalid_params), expected=5000)
        require(market_state() == original_state, "invalid parameters changed state")
        # Cover storage rent during the simulated month using local test SHELL.
        stand("fund-user", address=market, shells=50_000_000_000)
        stand("fund-user", address=borrower_wallet, shells=2_000_000_000)
        clock_at_borrow = original.market_view(original_state)["accrual"]
        clock = clock_at_borrow + month - 1
        event = admin_call("lend.set_params", "set_params_valid", encode_params(valid_params))
        require(original.event_names(event) == ["lend.params"], original.event_names(event))
        state = market_state()
        expected_fields = encode_params(valid_params)
        require(state[40:58] == expected_fields[:18] and state[60:64] == expected_fields[18:22]
                and state[80] == expected_fields[22], "set_params configuration mismatch")
        require(state[:40] == original_state[:40] and state[64:80] == original_state[64:80],
                "set_params changed immutable config")
        before = original.market_view(state)

        def accrue(view, config, now):
            scale, index = view["scaled"], view["index"]
            debt = original.ceil_div(scale * index, original.ONE)
            utilization = debt * original.BPS // (view["cash"] + debt)
            require(utilization <= config["kink"], "fixture must exercise the lower rate segment")
            annual_rate = config["base"] + config["slope1"] * utilization // config["kink"]
            elapsed = now - view["accrual"]
            next_index = index + index * annual_rate * elapsed // (original.YEAR * original.BPS)
            next_debt = original.ceil_div(scale * next_index, original.ONE)
            next_reserves = view["reserves"] + (next_debt - debt) * config["reserve_factor"] // original.BPS
            return next_index, next_debt, next_reserves, annual_rate

        prior = original.market_view(original_state)
        index, debt, reserves, old_rate = accrue(prior, original.PARAMS, clock)
        require(before["index"] == index and before["reserves"] == reserves and reserves > 0,
                ("old rate until set_params", before, index, reserves))
        require(before["accounts"] == prior["accounts"] and before["cash"] == prior["cash"],
                "set_params changed holder balances or cash")
        require(admin_key not in before["accounts"], "fixture expects empty admin balance")
        send(borrower_wallet, "package-owner", "COLLECT_RESERVES", expected=5001,
             label="collect_reserves_unauthorized")
        require(market_state() == state, "unauthorized collect_reserves changed state")
        next_index, debt_at_collect, reserves_at_collect, new_rate = accrue(before, valid_params, clock + 1)
        amount = min(reserves_at_collect, before["cash"])
        if reserve_capacity == "missing":
            require(len(before["accounts"]) == capacity["account_limit"], before)
            admin_call("lend.collect_reserves", "collect_full_admin_missing",
                       expected=capacity["overflow_status"])
            rejected_state = market_state()
            require(rejected_state == state, "capacity failure changed market contract state")
            admin_results["full_registry_collection"] = dict(
                admin_present=False, accounts_before=len(before["accounts"]),
                accounts_after=len(original.market_view(rejected_state)["accounts"]),
                status=capacity["overflow_status"], collected_ecc9=0,
                reserves_before=before["reserves"], reserves_after=before["reserves"],
                cash_before=before["cash"], cash_after=before["cash"],
                preserved_market_state=True,
            )
            return
        collect = admin_call("lend.collect_reserves", "collect_reserves")
        require(original.event_names(collect) == ["lend.action"], original.event_names(collect))
        collected = market_state()
        after = original.market_view(collected)
        admin_account = after["accounts"].get(admin_key)
        require(admin_account == dict(bal=[0, amount], col=0, shares=0, scaled=0), admin_account)
        require(after["cash"] == before["cash"] - amount and after["reserves"] == reserves_at_collect - amount
                and after["index"] == next_index and after["scaled"] == before["scaled"]
                and after["shares"] == before["shares"], after)
        require(all(after["accounts"][key] == value for key, value in before["accounts"].items()), after["accounts"])
        assets_before = before["cash"] + debt_at_collect - reserves_at_collect
        assets_after = after["cash"] + debt_at_collect - after["reserves"]
        require(assets_after == assets_before, (assets_before, assets_after))
        admin_call("lend.collect_reserves", "collect_reserves_empty", expected=5016)
        require(market_state() == collected, "empty collect_reserves changed state")
        admin_results.update(
            previous_params=original.PARAMS, current_params=valid_params, params_event=event["events"],
            collect_event=collect["events"], old_annual_rate_bps=old_rate, new_annual_rate_bps=new_rate,
            debt_at_start=120_000, debt_at_parameter_update=debt, debt_at_collection=debt_at_collect,
            cash_before=before["cash"], cash_after=after["cash"], collected_ecc9=amount,
            reserves_before_collection=reserves_at_collect, reserves_after_collection=after["reserves"],
            admin_ecc9_before=0, admin_ecc9_after=admin_account["bal"][1],
            lender_net_assets_before=assets_before, lender_net_assets_after=assets_after,
            lender_shares=after["shares"], rejected_calls_preserved_state=True,
        )

        if reserve_capacity == "existing":
            require(len(before["accounts"]) == capacity["account_limit"] - 1
                    and len(after["accounts"]) == capacity["account_limit"], "bootstrap must fill final slot")
            clock = after["accrual"] + month - 1
            admin_call("lend.set_params", "accrue_second_month", encode_params(valid_params))
            full = original.market_view(market_state())
            require(full["accounts"] == after["accounts"] and full["cash"] == after["cash"], full)
            second_index, _, second_reserves, _ = accrue(after, valid_params, clock)
            require(full["index"] == second_index and full["reserves"] == second_reserves > 0,
                    (full, second_index, second_reserves))
            final_index, final_debt, final_reserves, _ = accrue(full, valid_params, clock + 1)
            second_amount = min(full["cash"], final_reserves)
            payout = admin_call("lend.collect_reserves", "collect_full_admin_existing")
            require(original.event_names(payout) == ["lend.action"], payout)
            final = original.market_view(market_state())
            expected_accounts = full["accounts"] | {
                admin_key: dict(bal=[0, amount + second_amount], col=0, shares=0, scaled=0)
            }
            require(len(full["accounts"]) == len(final["accounts"]) == capacity["account_limit"], final)
            require(final["accounts"] == expected_accounts and final["shares"] == full["shares"]
                    and final["scaled"] == full["scaled"] and final["index"] == final_index, final)
            require(final["reserves"] == final_reserves - second_amount
                    and final["cash"] == full["cash"] - second_amount, final)
            before_assets = full["cash"] + final_debt - final_reserves
            after_assets = final["cash"] + final_debt - final["reserves"]
            require(second_amount > 0 and before_assets == after_assets, (before_assets, after_assets))
            admin_results["full_registry_collection"] = dict(
                admin_present=True, accounts_before=len(full["accounts"]),
                accounts_after=len(final["accounts"]), status=0, collected_ecc9=second_amount,
                reserves_before=final_reserves, reserves_after=final["reserves"],
                cash_before=full["cash"], cash_after=final["cash"],
                admin_balance_before=amount, admin_balance_after=amount + second_amount,
                lender_net_assets_before=before_assets, lender_net_assets_after=after_assets,
            )

    if admin_scenario or reserve_capacity:
        run_admin_scenario(before_rejection)
    else:
        send(borrower_wallet, "package-owner", "REMOVE_COLLATERAL", 100_000, expected=5007,
             label="remove_rejected")
        require(checkpoint("rejected", dict(locked_collateral=100_000)) == before_rejection,
                "Rejected unlock changed market contract state")
        send(borrower_wallet, "package-owner", "REPAY", 120_100)
        # A few seconds accrue between blocks. With the fixture's 320 bps APR, the
        # index increases by less than one whole debt unit, rounded up to one.
        checkpoint("repaid", dict(debt=0, cash=1_000_001, available_debt_asset=99,
                                 locked_collateral=100_000))
        send(borrower_wallet, "package-owner", "REMOVE_COLLATERAL", 100_000)
        checkpoint("unlocked", dict(debt=0, available_collateral=100_000, locked_collateral=0,
                                   total_locked_collateral=0, cash=1_000_001, lender_shares=1_000_000))

        def payout_checkpoint(label, available, received, expected_count, expected_id=None):
            checkpoint(label, dict(available_collateral=available, locked_collateral=0, debt=0,
                                   cash=1_000_001, available_debt_asset=99))
            view = original.payee_view(bytes.fromhex(stand("inspect", account=payee)["raw_state"]))
            require(view["count"] == expected_count, view)
            if expected_id is not None:
                require(view["last"] == original.receipt(original.COLLATERAL, expected_id, received), view)
            checkpoints[label].update(payee_received_count=view["count"], payee_last_receipt=view["last"].hex())

        require(payee_control("lend.payee.reject", b"\x01") == 0, "initial recipient balance")
        payout_checkpoint("withdraw_before", 100_000, 0, 0)
        checkpoints["withdraw_before"]["payee_ecc7_balance"] = 0
        send(borrower_wallet, "package-owner", "WITHDRAW", 100_000, label="withdraw_rejected",
             withdrawal=2202)
        payout_checkpoint("withdraw_rejected", 100_000, 0, 0)
        require(payee_control("lend.payee.reject", b"\0") == 0, "rejected payout credited recipient")
        checkpoints["withdraw_rejected"]["payee_ecc7_balance"] = 0
        send(borrower_wallet, "package-owner", "WITHDRAW", 100_000, label="withdraw_retry_accepted",
             withdrawal=0)
        payout_checkpoint("withdraw_retry_accepted", 0, 100_000, 1, expected_id=1)
        require(payee_control("lend.payee.reject", b"\0") == 100_000, "incorrect final recipient balance")
        checkpoints["withdraw_retry_accepted"]["payee_ecc7_balance"] = 100_000
        send(borrower_wallet, "package-owner", "WITHDRAW", 100_000,
             expected=5005, label="withdraw_duplicate_rejected")
        payout_checkpoint("withdraw_duplicate_rejected", 0, 100_000, 1, expected_id=1)
        require(payee_control("lend.payee.reject", b"\0") == 100_000, "duplicate payout credited recipient")
        checkpoints["withdraw_duplicate_rejected"]["payee_ecc7_balance"] = 100_000

    inspect = stand("inspect")
    replay = stand("replay")
    require(inspect["queue"]["message_count"] == 0, inspect["queue"])
    replay_keys = ("account_state_root", "registry_root", "cumulative_fee_shells", "accepted_input_count")
    require(all(inspect[k] == replay[k] for k in replay_keys), "Replay diverged")
    wasm = market_dir / "contracts/lending/target/avm/contract.wasm"
    return {
        "payee_wasm_sha256": digest(market_dir / "contracts/payee/target/avm/contract.wasm"),
        "pr_head": PR_HEAD, "sdk_release": release, "market_wasm_bytes": wasm.stat().st_size,
        "market_wasm_sha256": digest(wasm), "profile_sha256": digest(profile),
        "addresses": dict(market=market, lender_wallet=lender_wallet, borrower_wallet=borrower_wallet, payee=payee),
        "capacity": capacity, "admin": admin_results,
        "market_peak_including_deploy_bytes": peak_by_contract[market_code_hash],
        "measurements": measurements, "withdrawals": withdrawals, "checkpoints": checkpoints,
        "replay": {k: replay[k] for k in replay_keys}, "commands": commands,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=lambda p: Path(p).resolve(), required=True)
    parser.add_argument("--market", type=lambda p: Path(p).resolve(), required=True)
    parser.add_argument("--wallet", type=lambda p: Path(p).resolve(), required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--capture-vectors", type=Path)
    parser.add_argument("--full-registry", action="store_true")
    parser.add_argument("--admin", action="store_true")
    parser.add_argument("--reserve-capacity", choices=("missing", "existing"))
    args = parser.parse_args()
    if sum(map(bool, (args.admin, args.full_registry, args.reserve_capacity))) > 1:
        parser.error("choose one scenario")
    with tempfile.TemporaryDirectory(prefix="avm-lending-fuel-") as temp:
        outcome = run(args.sdk, args.market, args.wallet, Path(temp), args.capture_vectors, args.full_registry, args.admin, args.reserve_capacity)
    args.output.write_text(json.dumps(outcome, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(outcome["admin"] if args.admin or args.reserve_capacity else outcome["withdrawals"], indent=2))


if __name__ == "__main__":
    main()
