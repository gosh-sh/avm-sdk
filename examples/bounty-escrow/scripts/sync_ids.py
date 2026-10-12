#!/usr/bin/env python3
"""Extract IDs from the official builder's ACAB, never invent a hash algorithm.

ACAB v1 method records prefix names with u64 ID/u16 flags/u32 input ID/u16 name
length. Event records use u64 ID/u16 flags/u16 field count/u16 name length.
The script is intentionally pinned to this layout and fails on incompatible ABI.
"""
from pathlib import Path
import argparse

ROOT = Path(__file__).resolve().parents[1]
METHODS = {
    "FUND": "bounty.escrow.fund", "SUBMIT": "bounty.escrow.submit",
    "APPROVE": "bounty.escrow.approve", "PAY": "bounty.escrow.pay",
    "REFUND": "bounty.escrow.refund", "CALLBACK": "bounty.escrow.callback",
    "STATUS": "bounty.escrow.status", "TOPUP": "bounty.escrow.topup",
    "RECEIVE": "bounty.receiver.receive", "BIND": "bounty.receiver.bind",
    "REJECT": "bounty.receiver.reject",
}
EVENTS = ["created", "status", "fees_added", "funded", "submitted", "approved",
          "transfer_pending", "transfer_settled", "transfer_failed"]

def identifier(contract, name, prefix):
    data = (ROOT / "contracts" / contract / "target/avm/contract.acab").read_bytes()
    encoded = name.encode()
    if data[:4] != b"ACAB":
        raise ValueError(f"Unexpected canonical ABI: {name}")
    positions = [i for i in range(len(data)) if data[i:i+len(encoded)] == encoded and int.from_bytes(data[i-2:i], "little") == len(encoded)]
    if len(positions) != 1:
        raise ValueError(f"Ambiguous name record: {name}")
    pos = positions[0]
    if int.from_bytes(data[pos-2:pos], "little") != len(encoded):
        raise ValueError(f"Unexpected name record: {name}")
    value = int.from_bytes(data[pos-prefix:pos-prefix+8], "little")
    if value == 0:
        raise ValueError(f"Zero identifier: {name}")
    return value

def render():
    lines = ["// Generated from the official AVM builder's canonical ACAB files."]
    for key, name in METHODS.items():
        contract = "receiver" if name.startswith("bounty.receiver.") else "escrow"
        lines.append(f"pub const {key}: u64 = 0x{identifier(contract, name, 16):016x};")
    for name in EVENTS:
        lines.append(f"pub const EVENT_{name.upper()}: u64 = 0x{identifier('escrow', 'escrow.'+name, 14):016x};")
    return "\n".join(lines) + "\n"

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    path = ROOT / "contracts/escrow/src/ids.rs"
    expected = render()
    if args.check:
        if path.read_text() != expected:
            raise SystemExit("Canonical IDs differ; run scripts/sync_ids.py and rebuild both contracts.")
        print("Canonical method/event IDs: PASS")
    else:
        path.write_text(expected)
        print("Canonical IDs updated; rebuild both contracts.")
