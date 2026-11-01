#!/usr/bin/env python3
"""Single source of truth for both ABI manifests and `ids.rs`.

Generates (or with --check verifies):
  contracts/treasury/avm-contract.toml
  contracts/payee/avm-contract.toml
  contracts/treasury/src/ids.rs

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
STATE_BYTES = 3136  # keep equal to state::STATE_BYTES (checked by a test)

# ----------------------------------------------------------------- the ABI ---

TREASURY_TYPES = [
    ("state", "fixed_bytes", STATE_BYTES),
    ("propose_args", "fixed_bytes", 89),
    ("proposal_id", "u64", None),
    ("payout_receipt", "fixed_bytes", 32),
    ("amount", "u128", None),
    ("counter", "u64", None),
    ("owner_key", "fixed_bytes", 32),
]
T = {name: index + 1 for index, (name, _, _) in enumerate(TREASURY_TYPES)}

TREASURY_METHODS = [
    ("treasury.deposit", None),
    ("treasury.propose", "propose_args"),
    ("treasury.approve", "proposal_id"),
    ("treasury.cancel", "proposal_id"),
    ("treasury.execute", "proposal_id"),
    ("treasury.freeze", None),
    ("treasury.callback", "payout_receipt"),
]

U, A, K = "counter", "amount", "owner_key"  # field type shorthands
EVENTS = [
    ("msig.created", [("delay", U), ("fuel_reserve", A), ("grace", U), ("owners", U),
                      ("threshold", U), ("ttl", U)]),
    ("msig.deposited", [("amount", A), ("currency", U), ("from", K)]),
    ("msig.proposed", [("amount", A), ("currency", U), ("id", U), ("kind", U), ("proposer", K)]),
    ("msig.approved", [("approvals", U), ("id", U), ("owner", K)]),
    ("msig.queued", [("eta", U), ("expires", U), ("id", U)]),
    ("msig.cancelled", [("by", K), ("id", U)]),
    ("msig.frozen", [("by", K)]),
    ("msig.unfrozen", [("id", U)]),
    ("msig.owner_added", [("id", U), ("owner", K)]),
    ("msig.owner_removed", [("id", U), ("owner", K)]),
    ("msig.policy_set", [("currency", U), ("fast_limit", A), ("id", U)]),
    ("msig.transfer_pending", [("amount", A), ("currency", U), ("id", U)]),
    ("msig.transfer_settled", [("amount", A), ("currency", U), ("id", U)]),
    ("msig.transfer_failed", [("amount", A), ("currency", U), ("id", U)]),
]

TREASURY_ERRORS = [
    (4000, "bad_args", "bad arguments"),
    (4001, "unauthorized", "not an owner"),
    (4002, "bad_state", "bad state"),
    (4003, "frozen", "frozen"),
    (4004, "bad_value", "bad value"),
    (4005, "not_found", "not found"),
    (4006, "expired", "expired"),
    (4007, "timelock", "timelock active"),
    (4009, "low_balance", "balance too low"),
    (4010, "capacity", "table full"),
    (4011, "insolvent", "low reserve"),
    (4012, "bad_callback", "bad callback"),
    (4013, "duplicate", "duplicate"),
    (4014, "bad_threshold", "below threshold"),
    (4015, "overflow", "arithmetic overflow"),
    (4016, "busy", "payout in flight"),
    (4017, "not_ready", "wrong state"),
]

PAYEE_TYPES = [
    ("state", "fixed_bytes", 144),
    ("address_bytes", "fixed_bytes", 64),
    ("reject_flag", "fixed_bytes", 1),
    ("payout_receipt", "fixed_bytes", 32),
]
PAYEE_METHODS = [
    ("treasury.payee.bind", "address_bytes"),
    ("treasury.payee.reject", "reject_flag"),
    ("treasury.payee.receive", "payout_receipt"),
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

    for name, _ in TREASURY_METHODS:
        add(const_name(name, "treasury."), name)
    for name, _ in PAYEE_METHODS:
        add(const_name(name, "treasury."), name)
    for name, _ in EVENTS:
        add("EVENT_" + const_name(name, "msig."), name)
    lines += ["", "/// Event name to class lookup used by the emitter.",
              "pub const EVENTS: &[(&str, u64)] = &["]
    for name, _ in EVENTS:
        lines.append(f'    ("{name}", EVENT_{const_name(name, "msig.")}),')
    lines.append("];")
    return "\n".join(lines) + "\n"


OUTPUTS = {
    "contracts/treasury/avm-contract.toml": lambda: toml_text(
        "multisig-treasury.state.v1", TREASURY_TYPES, None, TREASURY_METHODS, EVENTS,
        TREASURY_ERRORS),
    "contracts/payee/avm-contract.toml": lambda: toml_text(
        "treasury-payee.state.v1", PAYEE_TYPES, None, PAYEE_METHODS, [], PAYEE_ERRORS),
    "contracts/treasury/src/ids.rs": ids_rs,
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
