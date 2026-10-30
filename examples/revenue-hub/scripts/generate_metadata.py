#!/usr/bin/env python3
"""Generate explicit AVM metadata; source control retains the generated TOML."""
from pathlib import Path
import hashlib
ROOT=Path(__file__).resolve().parents[1]
METHODS={
    "vault":[("hub.vault.deploy",2),("hub.vault.deposit",0),("hub.vault.claim",3),
             ("hub.vault.callback",4),("hub.vault.propose",4),("hub.vault.vote",4),
             ("hub.vault.execute",5),("hub.vault.status",0),("hub.vault.topup",0)],
    "receiver":[("hub.receiver.deploy",2),("hub.receiver.receive",4),("hub.receiver.bind",6),
                ("hub.receiver.reject",3),("hub.receiver.duplicate_audit",3),("hub.receiver.status",0)],
    "audit":[("hub.audit.deploy",3),("hub.audit.bind",7),("hub.audit.record",4),("hub.audit.status",0)],
}
EVENT_FIELDS={
    "vault":[("amount",8),("dust",8),("held",8),("member",9),("nonce",9),
             ("paid",8),("paused",9),("pending",8),("proposal",9),("received",8)],
    "audit":[("amount",8),("member",9),("nonce",9),("receipts",9),("total",8)],
}
EVENTS={"vault":["created","deposited","claim_pending","claim_settled","claim_failed",
                 "proposed","voted","executed","status","fees_added"],"audit":["audited"],"receiver":[]}
ERRORS={"vault":list(enumerate(["invalid_args","unauthorized","invalid_state","paused","invalid_value",
    "claim_pending","nothing_to_claim","invalid_callback","overflow","insufficient_balance",
    "invalid_proposal","duplicate_vote","timelock","proposal_expired","quorum_missing"],3000)),
    "receiver":[(3100,"unauthorized"),(3101,"invalid_receipt"),(3102,"delivery_rejected")],
    "audit":[(3200,"invalid_args"),(3201,"unauthorized"),(3202,"receipt_replay"),(3203,"overflow")]}

def generate():
    for contract,size,init_bytes in [("vault",3560,1792),("receiver",224,88),("audit",65540,1)]:
        schema=hashlib.sha256(f"avm.revenue.{contract}.v1".encode()).hexdigest()
        mode="account_batch" if contract=="audit" else "message_isolated"
        text=f'[descriptor]\nexecution_mode = "{mode}"\nstate_schema_id = "{schema}"\n\n[abi]\nstate_root_type_id = 1\n'
        if contract=="audit":
            text=text.replace('\n[abi]\n','\n[descriptor.batch]\nmax_batch_messages = 256\nmax_batch_state_bytes = 131072\nmax_batch_input_bytes = 262144\nmax_batch_output_bytes = 262144\nbatch_min_fuel_fee_shells = 1\n\n[abi]\n')
        for i,name,kind,length in [(1,"state","bytes" if contract=="audit" else "fixed_bytes",None if contract=="audit" else size),(2,"config","fixed_bytes",init_bytes),
            (3,"index","fixed_bytes",1),(4,"payload","fixed_bytes",25),(5,"nonce","fixed_bytes",8),
            (6,"address","fixed_bytes",64),(7,"index_address","fixed_bytes",65),(8,"amount","u128",None),(9,"counter","u64",None)]:
            text+=f'\n[[abi.types]]\nid = {i}\nkind = "{kind}"\nname = "{name}"\n'
            if length is not None:text+=f'len = {length}\n'
        for name,input_type in METHODS[contract]:
            text+=f'\n[[abi.methods]]\nname = "{name}"\n'
            if input_type:text+=f'input_type_id = {input_type}\n'
        for event in EVENTS[contract]:
            text+=f'\n[[abi.events]]\nname = "hub.{event}"\n'
            for i,(name,type_id) in enumerate(EVENT_FIELDS[contract],1):
                text+=f'\n[[abi.events.fields]]\nid = {i}\ntype_id = {type_id}\nname = "{name}"\n'
        for code,name in ERRORS[contract]:
            text+=f'\n[[abi.errors]]\nstatus_code = {code}\nname = "{name}"\nmessage = "{name.replace("_"," ")}"\n'
        (ROOT/"contracts"/contract/"avm-contract.toml").write_text(text)
if __name__=="__main__":generate()
