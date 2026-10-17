#!/usr/bin/env python3
"""Reproducible, real AVM revenue-hub journey. No network and no real funds.

Run after the official builder produces all three artifact triples.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
DAPP = "11" * 32
BASE_TIME = 1_800_000_000

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
            cmd += ["--" + key.replace("_", "-"), str(value)]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        try:
            result = json.loads(proc.stdout)
        except json.JSONDecodeError:
            raise RuntimeError(f"{operation}: {proc.stderr}\n{proc.stdout}") from None
        self.log.append({"operation": operation, "options": options, "exit_code": proc.returncode, "result": result})
        return result

    def timestamps(self):
        self.time += 1
        return dict(block_time=self.time, created_at=self.time, expire_at=self.time+3600, fuel_fee_shells=10000)

    def deploy(self, contract, salt, init=None):
        artifacts = ROOT / "contracts" / contract / "target/avm"
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
                       descriptor=artifacts / "contract.descriptor", salt_hex=salt.encode().hex(),
                       init_json=json.dumps(None if init is None else "0x"+init.hex()), **self.timestamps())
        if contract in self.selections:
            for key in ["wasm", "acab", "descriptor"]:
                del options[key]
            options.update(self.selections[contract])
        # Convert paths to text for the evidence journal.
        options = {key: str(value) if isinstance(value, Path) else value for key,value in options.items()}
        precondition = self.command("deploy", **options)
        assert precondition.get("error") == "funding_required", precondition
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

    def call(self, address, method, body=b"", identity="package-owner", value=0, extra="", expected_status=0):
        result = self.command("call", address=address, method=method,
                            args_json=json.dumps("0x"+body.hex()) if body else "null",
                            identity=identity, value_main=value, extra_currencies=extra, **self.timestamps())
        assert result["status"] == "ok", result
        raw = bytes.fromhex(result["raw_executor_output"])
        actual_status = int.from_bytes(raw[4:8], "little")
        assert raw[:4] == b"ACMO" and actual_status == expected_status, {"method": method, "contract_status": actual_status, "expected": expected_status}
        return result


def address_bytes(value):return bytes.fromhex(value.replace(":",""))
def receiver_config(audit,index):return address_bytes(audit)+bytes([index])+bytes(7)+(10000).to_bytes(16,"little")
def vault_config(identities,receivers):
    data=bytearray(1792)
    keys=[identities[k] for k in ["developer","package_owner","user"]]
    for i,key in enumerate(keys):data[i*32:i*32+32]=bytes.fromhex(key)
    data[96:100]=(30).to_bytes(4,"little");data[100:104]=(300).to_bytes(4,"little")
    data[104:120]=(10000).to_bytes(16,"little");data[120]=3
    for i,(receiver,weight) in enumerate(zip(receivers,[5000,3000,2000])):
        o=128+i*104;data[o:o+32]=bytes.fromhex(keys[i]);data[o+32:o+96]=address_bytes(receiver);data[o+96:o+98]=weight.to_bytes(2,"little")
    return bytes(data)
def decoded(result):
    b=bytes.fromhex(result["raw_state"]);assert len(b)==3560
    members=[]
    for i in range(b[120]):
        o=1896+i*104
        members.append({"claimable":int.from_bytes(b[o:o+16],"little"),"paid":int.from_bytes(b[o+16:o+32],"little"),
                        "pending":int.from_bytes(b[o+32:o+48],"little"),"last_callback_lt":int.from_bytes(b[o+64:o+72],"little")})
    received=int.from_bytes(b[1792:1808],"little");paid=int.from_bytes(b[1808:1824],"little")
    claimable=sum(m["claimable"] for m in members);pending=sum(m["pending"] for m in members)
    return {"received":received,"paid":paid,"claimable":claimable,"pending":pending,"dust":received-paid-claimable-pending,
            "paused":bool(b[1840]),"claim_attempts":int.from_bytes(b[1824:1832],"little"),"members":members}
def expect(result,**values):
    d=decoded(result)
    for key,value in values.items():assert d[key]==value,(key,d[key],value)
    return d
def proposal(nonce,action,value=0):return nonce.to_bytes(8,"little")+bytes([action])+value.to_bytes(16,"little")

def run(sdk,output):
    with tempfile.TemporaryDirectory(prefix="avm-revenue-") as directory:
        stand=Stand(sdk,Path(directory))
        try:
            init=stand.command("init",dapp_id=DAPP);assert init["status"]=="ok"
            identities=init["identities"]
            audit=stand.deploy("audit","audit",b"\x03")
            # The host chooses the mode from the previous *whole* account image.
            audit_account = next(a for a in stand.command("inspect")["accounts"] if a["address"] == audit)
            assert audit_account["canonical_byte_length"] == 65_615, audit_account
            audit_limits = {"data_bytes": 65_540, "canonical_account_bytes": 65_615,
                            "first_post_init_mode": "DataPatchList"}
            receivers=[stand.deploy("receiver",f"member-{i}",receiver_config(audit,i)) for i in range(3)]
            vault=stand.deploy("vault","revenue",vault_config(identities,receivers))
            # Read actual fixture init authorities rather than assume that the
            # publisher/account owner's identities are interchangeable.
            roles={value:key.replace("_","-") for key,value in identities.items()}
            r=stand.call(audit,"hub.audit.status");assert bytes.fromhex(r["raw_executor_output"])[9] == 1
            audit_owner=roles[r["raw_state"][8:72]]
            receiver_owners=[]
            for i,address in enumerate(receivers):
                r=stand.call(address,"hub.receiver.status");owner=roles[r["raw_state"][:64]];receiver_owners.append(owner)
                stand.call(address,"hub.receiver.bind",address_bytes(vault),identity=owner)
                binding = stand.call(audit,"hub.audit.bind",bytes([i])+address_bytes(address),identity=audit_owner)
                assert bytes.fromhex(binding["raw_executor_output"])[9] == 1
            r=stand.call(vault,"hub.vault.deposit",value=10007)
            expect(r,received=10007,paid=0,claimable=10006,dust=1)
            stand.call(vault,"hub.vault.claim",b"\x00",identity="user",expected_status=3001)
            stand.call(receivers[0],"hub.receiver.reject",b"\x01",identity=receiver_owners[0])
            r=stand.call(vault,"hub.vault.claim",b"\x00",identity="developer")
            expect(r,paid=0,pending=0,claim_attempts=1)
            assert any(b"hub.claim_failed" in bytes.fromhex(e) for e in r["events"])
            stand.call(receivers[0],"hub.receiver.reject",b"\x00",identity=receiver_owners[0])
            r=stand.call(vault,"hub.vault.claim",b"\x00",identity="developer")
            expect(r,paid=5003,pending=0,claim_attempts=2)
            stand.call(vault,"hub.vault.claim",b"\x00",identity="developer",expected_status=3006)
            # Quorum, duplicate votes, immutable signed proposal, and timelock.
            p=proposal(1,1)
            r=stand.call(vault,"hub.vault.propose",p,identity="developer")
            raw=bytes.fromhex(r["raw_state"]);ready=int.from_bytes(raw[1884:1888],"little")
            stand.call(vault,"hub.vault.execute",(1).to_bytes(8,"little"),expected_status=3014)
            stand.call(vault,"hub.vault.vote",p,identity="developer",expected_status=3011)
            stand.call(vault,"hub.vault.vote",proposal(1,2),identity="user",expected_status=3010)
            stand.call(vault,"hub.vault.vote",p,identity="user")
            stand.call(vault,"hub.vault.execute",(1).to_bytes(8,"little"),expected_status=3012)
            stand.time=ready-1
            r=stand.call(vault,"hub.vault.execute",(1).to_bytes(8,"little"));expect(r,paused=True)
            stand.call(vault,"hub.vault.deposit",value=3,expected_status=3003)
            stand.call(vault,"hub.vault.claim",b"\x01",expected_status=3003)
            p=proposal(2,2)
            r=stand.call(vault,"hub.vault.propose",p);ready=int.from_bytes(bytes.fromhex(r["raw_state"])[1884:1888],"little")
            stand.call(vault,"hub.vault.vote",p,identity="developer")
            stand.time=ready-1;r=stand.call(vault,"hub.vault.execute",(2).to_bytes(8,"little"));expect(r,paused=False)
            r=stand.call(vault,"hub.vault.deposit",value=3);expect(r,received=10010,dust=0)
            stand.call(receivers[1],"hub.receiver.duplicate_audit",b"\x01",identity=receiver_owners[1])
            for i,identity in enumerate(["developer","package-owner","user"]):
                r=stand.call(vault,"hub.vault.claim",bytes([i]),identity=identity)
                assert decoded(r)["members"][i]["claimable"]==0
            r=stand.call(vault,"hub.vault.status");final=expect(r,received=10010,paid=10010,claimable=0,pending=0,dust=0,claim_attempts=5)
            actual_receipts=[]
            for i,address in enumerate(receivers):
                r=stand.call(address,"hub.receiver.status");b=bytes.fromhex(r["raw_state"])
                amount=int.from_bytes(b[176:192],"little");assert amount==[5005,3003,2002][i]
                actual_receipts.append(amount)
            r=stand.call(audit,"hub.audit.status");b=bytes.fromhex(r["raw_state"]);assert len(b)==65540
            audit_totals=[int.from_bytes(b[2052+i*32+8:2052+i*32+24],"little") for i in range(3)]
            audit_counts=[int.from_bytes(b[2052+i*32+24:2052+i*32+32],"little") for i in range(3)]
            assert audit_totals==actual_receipts,audit_totals
            assert audit_counts==[2,1,1],audit_counts
            inspect=stand.command("inspect");events=stand.command("events");replay=stand.command("replay")
            assert replay["status"]=="ok" and inspect["queue"]["message_count"]==0
            for key in ["account_state_root","registry_root","cumulative_fee_shells","accepted_input_count"]:assert replay[key]==inspect[key],key
            payload={"status":"PASS","audit_state_limits":audit_limits,"accounting":final,"receiver_totals":actual_receipts,"audit_totals":audit_totals,"audit_counts":audit_counts,"semantic_duplicate_audit_did_not_double_credit":True,
                     "addresses":{"vault":vault,"audit":audit,"receivers":receivers},"inspect":inspect,"events":events,"replay":replay,"journal":stand.log}
            output.write_text(json.dumps(payload,indent=2)+"\n");print("Actual AVM revenue hub journey: PASS")
        except Exception:
            output.write_text(json.dumps({"status":"FAIL","journal":stand.log},indent=2)+"\n");raise

if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("--sdk",type=Path,required=True);p.add_argument("--output",type=Path,default=ROOT/"evidence/avm-e2e.json")
    a=p.parse_args();run(a.sdk.resolve(),a.output.resolve())
