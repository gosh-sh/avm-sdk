#!/usr/bin/env python3
"""Single source of truth for both ABI manifests and `ids.rs`.

Generates (or with --check verifies):
  contracts/lending/avm-contract.toml
  contracts/payee/avm-contract.toml
  contracts/lending/src/ids.rs

Method and event identifiers are `u64::from_le_bytes(blake3(name)[0..8])`. That
rule was recovered by reproducing every identifier of the checked-in
`bounty-escrow` and `fungible-token` examples; `--selftest` repeats the
reproduction so a builder change cannot go unnoticed. The official
`avm-contract-build` remains the authority: compare its ACAB with ids.rs.

Requires `pip install blake3`.
"""
import argparse
import pathlib
import re
import sys
import tomllib

import blake3

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXAMPLES = ROOT.parent
STATE_BYTES = 4528  # keep equal to state::STATE_BYTES (checked by a test)

# ----------------------------------------------------------------- the ABI ---

LENDING_TYPES = [
    ("state", "fixed_bytes", STATE_BYTES),
    ("amount", "u128", None),
    ("liquidate_args", "fixed_bytes", 48),
    ("withdraw_args", "fixed_bytes", 84),
    ("params_args", "fixed_bytes", 23),
    ("borrower_key", "fixed_bytes", 32),
    ("payout_receipt", "fixed_bytes", 32),
    ("counter", "u64", None),
]

LENDING_METHODS = [
    ("lend.deposit", None),
    ("lend.supply", "amount"),
    ("lend.redeem", "amount"),
    ("lend.add_collateral", "amount"),
    ("lend.remove_collateral", "amount"),
    ("lend.borrow", "amount"),
    ("lend.repay", "amount"),
    ("lend.liquidate", "liquidate_args"),
    ("lend.write_off", "borrower_key"),
    ("lend.withdraw", "withdraw_args"),
    ("lend.set_price", "amount"),
    ("lend.set_params", "params_args"),
    ("lend.collect_reserves", None),
    ("lend.topup", None),
    ("lend.callback", "payout_receipt"),
]

U, A, K = "counter", "amount", "borrower_key"  # field type shorthands
EVENTS = [
    ("lend.created", [("admin", K), ("collateral", U), ("debt", U), ("liq", U), ("ltv", U)]),
    ("lend.action", [("action", U), ("amount", A), ("aux", A), ("holder", K), ("other", K)]),
    ("lend.price", [("price", A), ("ts", U)]),
    ("lend.params", [("kink", U), ("liq", U), ("ltv", U), ("paused", U)]),
]

LENDING_ERRORS = [
    (5000, "invalid_args", "invalid or non-canonical arguments"),
    (5001, "unauthorized", "caller is not allowed to do this"),
    (5002, "invalid_state", "state is corrupt or the action is not valid now"),
    (5003, "paused", "the market is paused"),
    (5004, "invalid_value", "attached value is missing or not allowed"),
    (5005, "insufficient_balance", "ledger balance, collateral or shares too low"),
    (5006, "no_liquidity", "the market does not hold enough cash"),
    (5007, "unhealthy", "the position would exceed its loan-to-value limit"),
    (5008, "healthy", "nothing to liquidate or write off"),
    (5009, "stale_price", "the price is unset or too old"),
    (5010, "price_move", "price update exceeds the allowed move"),
    (5011, "capacity", "table is full"),
    (5012, "recipient_busy", "a payout to this recipient is already in flight"),
    (5013, "invalid_callback", "invalid, stale or mismatched callback"),
    (5014, "insolvent", "market balance or SHELL reserve insufficient"),
    (5015, "overflow", "arithmetic overflow"),
    (5016, "zero_amount", "the amount rounds to zero"),
]

PAYEE_TYPES = [
    ("state", "fixed_bytes", 144),
    ("address_bytes", "fixed_bytes", 64),
    ("reject_flag", "fixed_bytes", 1),
    ("payout_receipt", "fixed_bytes", 32),
]
PAYEE_METHODS = [
    ("lend.payee.bind", "address_bytes"),
    ("lend.payee.reject", "reject_flag"),
    ("lend.payee.receive", "payout_receipt"),
]
PAYEE_ERRORS = [
    (2200, "unauthorized", "unauthorized"),
    (2201, "invalid_args", "invalid args"),
    (2202, "payee_rejected", "payee rejected"),
]

# --------------------------------------------------------------- rendering ---


def ident(name: str) -> int:
    return int.from_bytes(blake3.blake3(name.encode()).digest()[:8], "little")


def toml_text(schema: str, types, init_type, methods, events, errors) -> str:
    ids = {name: index + 1 for index, (name, _, _) in enumerate(types)}
    out = ["[descriptor]", 'execution_mode = "message_isolated"',
           f'state_schema_id = "{blake3.blake3(schema.encode()).hexdigest()}"', "",
           "[abi]", "state_root_type_id = 1"]
    if init_type:
        out.append(f"init_input_type_id = {ids[init_type]}")
    out.append("")
    for index, (name, kind, length) in enumerate(types):
        out += ["[[abi.types]]", f"id = {index + 1}", f'kind = "{kind}"', f'name = "{name}"']
        if length is not None:
            out.append(f"len = {length}")
        out.append("")
    for name, input_type in methods:
        out += ["[[abi.methods]]", f'name = "{name}"']
        if input_type:
            out.append(f"input_type_id = {ids[input_type]}")
        out.append("")
    for name, fields in events:
        assert [f for f, _ in fields] == sorted(f for f, _ in fields), f"{name}: fields not ascending"
        out += ["[[abi.events]]", f'name = "{name}"', ""]
        for index, (field, type_name) in enumerate(fields):
            out += ["[[abi.events.fields]]", f"id = {index + 1}", f"type_id = {ids[type_name]}",
                    f'name = "{field}"', ""]
    for code, name, message in errors:
        out += ["[[abi.errors]]", f"status_code = {code}", f'name = "{name}"',
                f'message = "{message}"', ""]
    return "\n".join(out).rstrip("\n") + "\n"


def const_name(name: str, prefix: str) -> str:
    return name.removeprefix(prefix).replace(".", "_").upper()


def ids_rs() -> str:
    lines = ["// Generated by scripts/gen_abi.py from the ABI description. Do not edit.",
             "// id = u64::from_le_bytes(blake3(name)[0..8]); see scripts/gen_abi.py."]
    seen = {}

    def add(key: str, name: str):
        value = ident(name)
        if value == 0 or value in seen:
            raise SystemExit(f"identifier collision or zero for {name}")
        seen[value] = name
        lines.append(f"pub const {key}: u64 = 0x{value:016x};")

    for name, _ in LENDING_METHODS:
        add(const_name(name, "lend."), name)
    for name, _ in PAYEE_METHODS:
        add(const_name(name, "lend."), name)
    for name, _ in EVENTS:
        add("EVENT_" + const_name(name, "lend."), name)
    lines += ["", "/// Event name to class lookup used by the emitter.",
              "pub const EVENTS: &[(&str, u64)] = &["]
    for name, _ in EVENTS:
        lines.append(f'    ("{name}", EVENT_{const_name(name, "lend.")}),')
    lines.append("];")
    return "\n".join(lines) + "\n"


OUTPUTS = {
    "contracts/lending/avm-contract.toml": lambda: toml_text(
        "lending-market.state.v1", LENDING_TYPES, None, LENDING_METHODS, EVENTS,
        LENDING_ERRORS),
    "contracts/payee/avm-contract.toml": lambda: toml_text(
        "lending-payee.state.v1", PAYEE_TYPES, None, PAYEE_METHODS, [], PAYEE_ERRORS),
    "contracts/lending/src/ids.rs": ids_rs,
}


def selftest() -> None:
    """Reproduce every identifier of the official sibling examples."""
    checked = 0
    escrow_ids = (EXAMPLES / "bounty-escrow/contracts/escrow/src/ids.rs").read_text()
    names = {"FUND": "bounty.escrow.fund", "SUBMIT": "bounty.escrow.submit",
             "APPROVE": "bounty.escrow.approve", "PAY": "bounty.escrow.pay",
             "REFUND": "bounty.escrow.refund", "CALLBACK": "bounty.escrow.callback",
             "STATUS": "bounty.escrow.status", "TOPUP": "bounty.escrow.topup",
             "RECEIVE": "bounty.receiver.receive", "BIND": "bounty.receiver.bind",
             "REJECT": "bounty.receiver.reject", "EVENT_CREATED": "escrow.created",
             "EVENT_STATUS": "escrow.status", "EVENT_FUNDED": "escrow.funded",
             "EVENT_SUBMITTED": "escrow.submitted", "EVENT_APPROVED": "escrow.approved"}
    for const, name in names.items():
        match = re.search(rf"pub const {const}: u64 = 0x([0-9a-f]{{16}});", escrow_ids)
        assert match, const
        assert int(match.group(1), 16) == ident(name), f"{name} does not match the official builder"
        checked += 1
    token = (EXAMPLES / "fungible-token/src/lib.rs").read_text()
    for const, name in (("TRANSFER_METHOD_ID", "token.transfer"),
                        ("TRANSFERRED_EVENT_CLASS", "token.transferred")):
        match = re.search(rf"const {const}: u64 = 0x([0-9a-f_]+);", token)
        assert match, const
        assert int(match.group(1).replace("_", ""), 16) == ident(name), name
        checked += 1
    print(f"identifier rule reproduces {checked}/{checked} official identifiers: PASS")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if generated files are stale")
    parser.add_argument("--selftest", action="store_true", help="verify the id rule on siblings")
    args = parser.parse_args()
    if args.selftest:
        selftest()
    for path, render in OUTPUTS.items():
        target = ROOT / path
        text = render()
        if args.check:
            if not target.exists() or target.read_bytes() != text.encode():
                sys.exit(f"{path} is stale; run scripts/gen_abi.py")
            tomllib.loads(text) if path.endswith(".toml") else None
        elif not args.selftest:
            target.write_bytes(text.encode())
            print(f"wrote {path}")
    if args.check:
        print("manifests and ids.rs match the ABI description: PASS")
