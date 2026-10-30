#!/usr/bin/env python3
"""Read canonical IDs from ACAB v1 produced by the official AVM builder."""
from pathlib import Path
import argparse
ROOT=Path(__file__).resolve().parents[1]
METHODS={"DEPOSIT":("vault","hub.vault.deposit"),"CLAIM":("vault","hub.vault.claim"),
"CALLBACK":("vault","hub.vault.callback"),"PROPOSE":("vault","hub.vault.propose"),
"VOTE":("vault","hub.vault.vote"),"EXECUTE":("vault","hub.vault.execute"),
"STATUS":("vault","hub.vault.status"),"TOPUP":("vault","hub.vault.topup"),
"RECEIVE":("receiver","hub.receiver.receive"),"BIND":("receiver","hub.receiver.bind"),
"REJECT":("receiver","hub.receiver.reject"),"RECEIVER_STATUS":("receiver","hub.receiver.status"),
"AUDIT_BIND":("audit","hub.audit.bind"),"AUDIT_RECORD":("audit","hub.audit.record"),
"AUDIT_STATUS":("audit","hub.audit.status"),"DUPLICATE_AUDIT":("receiver","hub.receiver.duplicate_audit")}
EVENTS=["created","deposited","claim_pending","claim_settled","claim_failed","proposed",
        "voted","executed","status","fees_added","audited"]
def identifier(contract,name,prefix):
    data=(ROOT/"contracts"/contract/"target/avm/contract.acab").read_bytes();encoded=name.encode()
    assert data[:4]==b"ACAB"
    positions=[i for i in range(len(data)) if data[i:i+len(encoded)]==encoded and int.from_bytes(data[i-2:i],"little")==len(encoded)]
    assert len(positions)==1,name
    i=positions[0];value=int.from_bytes(data[i-prefix:i-prefix+8],"little");assert value!=0
    return value
def render():
    lines=["// Generated from the official AVM builder's canonical ACAB files."]
    for key,(contract,name) in METHODS.items():lines.append(f"pub const {key}: u64 = 0x{identifier(contract,name,16):016x};")
    for name in EVENTS:
        contract="audit" if name=="audited" else "vault"
        lines.append(f"pub const EVENT_{name.upper()}: u64 = 0x{identifier(contract,'hub.'+name,14):016x};")
    return "\n".join(lines)+"\n"
if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("--check",action="store_true");args=p.parse_args()
    file=ROOT/"core/src/ids.rs";expected=render()
    if args.check:
        assert file.read_text()==expected,"Canonical IDs changed; sync and rebuild."
        print("Canonical IDs: PASS")
    else:file.write_text(expected);print("Canonical IDs regenerated; rebuild all contracts.")
