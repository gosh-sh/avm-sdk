#!/usr/bin/env python3
"""Reproducible, real AVM revenue-hub journey. No network and no real funds.

Run after the official builder produces all three artifact triples and the
SDK wallet example. Members and governance act through their own keyed wallets
(examples/wallet): each signs an external to its wallet and the wallet sends the
internal call, with any value, to the keyless vault. Receivers and the audit
ledger keep their owner key (the deploy owner_id) for their owner's externals.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
WALLET = ROOT.parent / "wallet"
DAPP = "11" * 32
BASE_TIME = 1_800_000_000
# FUEL reserve of every wallet-to-vault internal message.
INTERNAL_FUEL = 100_000
IDS = {name: int(value, 16) for name, value in
       re.findall(r"pub const (\w+): u64 = 0x([0-9a-f]+);", (ROOT / "core/src/ids.rs").read_text())}
VAULT_METHODS = {"hub.vault." + name.lower(): IDS[name] for name in
                 ["DEPOSIT", "CLAIM", "PROPOSE", "VOTE", "EXECUTE", "STATUS", "TOPUP"]}

def send_args(address, method_id, currencies, body):
    """`wallet.send` arguments: a callback is always requested."""
    args = address_bytes(address) + method_id.to_bytes(8, "little") + INTERNAL_FUEL.to_bytes(16, "little")
    args += bytes([1, len(currencies)]) + b"".join(c.to_bytes(4, "little") + n.to_bytes(16, "little") for c, n in currencies) + body
    # The stand adds the u32 length prefix of the ABI `bytes` argument itself.
    return "0x" + args.hex()

def wallet_callback_status(event):
    """Status code of a `wallet.callback` event, or None for another event."""
    if b"wallet.callback" not in event:
        return None
    # As examples/wallet/stand-e2e.sh reads it: 13 bytes after the field name,
    # trailing zero bytes of the u64 are not stored.
    at = event.index(b"status_code") + len(b"status_code") + 13
    return int.from_bytes((event[at:at+8] + bytes(8))[:8], "little")

class Stand:
    def __init__(self, sdk, work):
        self.binary = str(sdk / "bin/avm-local-stand")
        self.profile = str(sdk / "profiles/wasmtime-release-profile.v1")
        self.state = work / "journey.state"
        self.log = []
        self.time = BASE_TIME
        self.selections = {}
        self.wallets = {}
        self.identity = {}

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

    def timestamps(self):
        self.time += 1
        return dict(block_time=self.time, created_at=self.time, expire_at=self.time+3600, fuel_fee_shells=10000)

    def deploy(self, contract, salt, init=None, owner_id=None, extra_currencies=None, shells=10_000_000):
        artifacts = WALLET / "target/avm" if contract == "wallet" else ROOT / "contracts" / contract / "target/avm"
        init_json = None if contract == "wallet" else "0x"+(b"" if init is None else init).hex()
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
                       descriptor=artifacts / "contract.descriptor", salt_hex=salt.encode().hex(),
                       init_json=json.dumps(init_json), **self.timestamps())
        if owner_id is not None:
            options["owner_id"] = owner_id
        if contract in self.selections:
            for key in ["wasm", "acab", "descriptor"]:
                del options[key]
            options.update(self.selections[contract])
        # Convert paths to text for the evidence journal.
        options = {key: str(value) if isinstance(value, Path) else value for key,value in options.items()}
        precondition = self.command("deploy", quote_address=True, **options)
        assert precondition.get("quoted") is True, precondition
        address = precondition["address"]
        funding = dict(address=address, shells=shells)
        if extra_currencies:
            funding["extra_currencies"] = extra_currencies
        funding = self.command("fund-user", **funding)
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

    def wallet(self, name, identity, key, extra_currencies=None):
        """Deploys a keyed wallet whose data is the owner key `key` of `identity`."""
        self.identity[name] = identity
        self.wallets[name] = self.deploy("wallet", "wallet-" + name, owner_id=key,
                                         extra_currencies=extra_currencies, shells=100_000_000)
        return self.wallets[name]

    def rejected(self, error, address, method, body, identity, extra=""):
        """The node rejects this external at admission with `error`."""
        result = self.command("call", address=address, method=method, args_json=json.dumps("0x"+body.hex()),
                              identity=identity, extra_currencies=extra, **self.timestamps())
        assert result.get("status") != "ok" and error in json.dumps(result), result
        return result

    def call(self, address, method, body=b"", identity="developer", expected_status=0):
        """A no-value external signed by `identity`, which must own `address`."""
        result = self.command("call", address=address, method=method,
                            args_json=json.dumps("0x"+body.hex()) if body else "null",
                            identity=identity, extra_currencies="", **self.timestamps())
        assert result["status"] == "ok", result
        raw = bytes.fromhex(result["raw_executor_output"])
        actual_status = int.from_bytes(raw[4:8], "little")
        assert raw[:4] == b"ACMO" and actual_status == expected_status, {"method": method, "contract_status": actual_status, "expected": expected_status}
        return result

    def send(self, address, method, body=b"", wallet="package-owner", value=0, extra=(), expected_status=0):
        """The owner of `wallet` signs wallet.send; the wallet calls the vault.

        Returns the vault image after the queue drained, with the events of the
        call. The vault's exact status comes back in the `wallet.callback` event.
        """
        # ECC0 is this example's explicit synthetic test principal; SHELL is separate.
        currencies = sorted(([(0, value)] if value else []) + list(extra))
        before = self.command("events")["count"]
        self.call(self.wallets[wallet], "wallet.send", bytes.fromhex(send_args(address, VAULT_METHODS.get(method, method), currencies, body)[2:]),
                  identity=self.identity[wallet])
        events = [bytes.fromhex(e) for e in self.command("events")["events"][before:]]
        statuses = [s for s in map(wallet_callback_status, events) if s is not None]
        assert statuses == [expected_status], {"method": method, "contract_status": statuses, "expected": expected_status}
        account = self.command("inspect", account=address)
        account["events"] = [e.hex() for e in events]
        return account


def address_bytes(value):return bytes.fromhex(value.replace(":",""))
def receiver_config(audit,index):return address_bytes(audit)+bytes([index])+bytes(7)+(10000).to_bytes(16,"little")
def vault_config(signers,receivers):
    data=bytearray(2400)
    for i,wallet in enumerate(signers):data[i*64:i*64+64]=address_bytes(wallet)
    data[192:196]=(30).to_bytes(4,"little");data[196:200]=(300).to_bytes(4,"little")
    data[200:216]=(10000).to_bytes(16,"little");data[216]=3
    for i,(receiver,weight) in enumerate(zip(receivers,[5000,3000,2000])):
        o=224+i*136;data[o:o+64]=address_bytes(signers[i]);data[o+64:o+128]=address_bytes(receiver);data[o+128:o+130]=weight.to_bytes(2,"little")
    return bytes(data)
def decoded(result):
    b=bytes.fromhex(result["raw_state"]);assert len(b)==4168
    members=[]
    for i in range(b[216]):
        o=2504+i*104
        members.append({"claimable":int.from_bytes(b[o:o+16],"little"),"paid":int.from_bytes(b[o+16:o+32],"little"),
                        "pending":int.from_bytes(b[o+32:o+48],"little"),"last_callback_lt":int.from_bytes(b[o+64:o+72],"little")})
    received=int.from_bytes(b[2400:2416],"little");paid=int.from_bytes(b[2416:2432],"little")
    claimable=sum(m["claimable"] for m in members);pending=sum(m["pending"] for m in members)
    return {"received":received,"paid":paid,"claimable":claimable,"pending":pending,"dust":received-paid-claimable-pending,
            "paused":bool(b[2448]),"claim_attempts":int.from_bytes(b[2432:2440],"little"),"members":members}
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
            roles={value:key.replace("_","-") for key,value in identities.items()}
            # One wallet per stand identity: they are the governance signers and
            # the members. The package-owner wallet holds the deposited principal
            # (and ECC 7 for the foreign-currency deposit); a second Developer
            # wallet is a wallet outside the configuration.
            members=["developer","package-owner","user"]
            for role in members:
                stand.wallet(role,role,identities[role.replace("-","_")],extra_currencies="0:10010,7:10007" if role=="package-owner" else None)
            stand.wallet("outsider","developer",identities["developer"])
            # Receivers and the audit ledger keep the Developer key, their deploy
            # owner_id, at their owner_key_offset.
            owner_key=identities["developer"]
            audit=stand.deploy("audit","audit",b"\x03",owner_id=owner_key)
            # The host chooses the mode from the previous *whole* account image.
            audit_account = next(a for a in stand.command("inspect")["accounts"] if a["address"] == audit)
            assert audit_account["canonical_byte_length"] == 65_599, audit_account
            audit_limits = {"data_bytes": 65_540, "canonical_account_bytes": 65_599,
                            "first_post_init_mode": "DataPatchList"}
            receivers=[stand.deploy("receiver",f"member-{i}",receiver_config(audit,i),owner_id=owner_key) for i in range(3)]
            config=vault_config([stand.wallets[m] for m in members],receivers)
            vault=stand.deploy("vault","revenue",config)
            # Read actual fixture owner keys rather than assume them.
            r=stand.call(audit,"hub.audit.status");assert bytes.fromhex(r["raw_executor_output"])[9] == 1
            audit_owner=roles[r["raw_state"][8:72]]
            receiver_owners=[]
            for i,address in enumerate(receivers):
                r=stand.call(address,"hub.receiver.status");owner=roles[r["raw_state"][:64]];receiver_owners.append(owner)
                stand.call(address,"hub.receiver.bind",address_bytes(vault),identity=owner)
                binding = stand.call(audit,"hub.audit.bind",bytes([i])+address_bytes(address),identity=audit_owner)
                assert bytes.fromhex(binding["raw_executor_output"])[9] == 1
            # The node admits an external only under the destination's owner key
            # (the vault has none), and no external carries value.
            stand.rejected("AccountKeyNotAllowed",vault,"hub.vault.status",b"","package-owner")
            stand.rejected("AccountKeyNotAllowed",stand.wallets["developer"],"wallet.send",
                           bytes.fromhex(send_args(vault,VAULT_METHODS["hub.vault.status"],[],b"")[2:]),"user")
            stand.rejected("ExternalValueNotAllowed",audit,"hub.audit.status",b"",audit_owner,extra="0:1")
            stand.send(vault,"hub.vault.deposit",extra=[(7,10007)],expected_status=3004)
            r=stand.send(vault,"hub.vault.deposit",value=10007)
            expect(r,received=10007,paid=0,claimable=10006,dust=1)
            # Payout and governance calls from a wallet that is not the member's
            # or a signer's are rejected by the vault.
            stand.send(vault,"hub.vault.claim",b"\x00",wallet="user",expected_status=3001)
            stand.send(vault,"hub.vault.claim",b"\x00",wallet="outsider",expected_status=3001)
            stand.send(vault,"hub.vault.propose",proposal(1,1),wallet="outsider",expected_status=3001)
            stand.call(receivers[0],"hub.receiver.reject",b"\x01",identity=receiver_owners[0])
            r=stand.send(vault,"hub.vault.claim",b"\x00",wallet="developer")
            expect(r,paid=0,pending=0,claim_attempts=1)
            assert any(b"hub.claim_failed" in bytes.fromhex(e) for e in r["events"])
            stand.call(receivers[0],"hub.receiver.reject",b"\x00",identity=receiver_owners[0])
            r=stand.send(vault,"hub.vault.claim",b"\x00",wallet="developer")
            expect(r,paid=5003,pending=0,claim_attempts=2)
            stand.send(vault,"hub.vault.claim",b"\x00",wallet="developer",expected_status=3006)
            # Quorum, duplicate votes, immutable signed proposal, and timelock.
            p=proposal(1,1)
            r=stand.send(vault,"hub.vault.propose",p,wallet="developer")
            raw=bytes.fromhex(r["raw_state"]);ready=int.from_bytes(raw[2492:2496],"little")
            stand.send(vault,"hub.vault.vote",p,wallet="outsider",expected_status=3001)
            stand.send(vault,"hub.vault.execute",(1).to_bytes(8,"little"),expected_status=3014)
            stand.send(vault,"hub.vault.vote",p,wallet="developer",expected_status=3011)
            stand.send(vault,"hub.vault.vote",proposal(1,2),wallet="user",expected_status=3010)
            stand.send(vault,"hub.vault.vote",p,wallet="user")
            stand.send(vault,"hub.vault.execute",(1).to_bytes(8,"little"),expected_status=3012)
            stand.time=ready-1
            r=stand.send(vault,"hub.vault.execute",(1).to_bytes(8,"little"));expect(r,paused=True)
            stand.send(vault,"hub.vault.deposit",value=3,expected_status=3003)
            stand.send(vault,"hub.vault.claim",b"\x01",expected_status=3003)
            p=proposal(2,2)
            r=stand.send(vault,"hub.vault.propose",p);ready=int.from_bytes(bytes.fromhex(r["raw_state"])[2492:2496],"little")
            stand.send(vault,"hub.vault.vote",p,wallet="developer")
            stand.time=ready-1;r=stand.send(vault,"hub.vault.execute",(2).to_bytes(8,"little"));expect(r,paused=False)
            r=stand.send(vault,"hub.vault.deposit",value=3);expect(r,received=10010,dust=0)
            stand.call(receivers[1],"hub.receiver.duplicate_audit",b"\x01",identity=receiver_owners[1])
            for i,wallet in enumerate(members):
                r=stand.send(vault,"hub.vault.claim",bytes([i]),wallet=wallet)
                assert decoded(r)["members"][i]["claimable"]==0
            r=stand.send(vault,"hub.vault.status");final=expect(r,received=10010,paid=10010,claimable=0,pending=0,dust=0,claim_attempts=5)
            # The configuration, governance wallets included, never changed after init.
            assert bytes.fromhex(r["raw_state"])[:2400]==config
            actual_receipts=[]
            for i,address in enumerate(receivers):
                r=stand.call(address,"hub.receiver.status",identity=receiver_owners[i]);b=bytes.fromhex(r["raw_state"])
                amount=int.from_bytes(b[176:192],"little");assert amount==[5005,3003,2002][i]
                actual_receipts.append(amount)
            r=stand.call(audit,"hub.audit.status",identity=audit_owner);b=bytes.fromhex(r["raw_state"]);assert len(b)==65540
            audit_totals=[int.from_bytes(b[2052+i*32+8:2052+i*32+24],"little") for i in range(3)]
            audit_counts=[int.from_bytes(b[2052+i*32+24:2052+i*32+32],"little") for i in range(3)]
            assert audit_totals==actual_receipts,audit_totals
            assert audit_counts==[2,1,1],audit_counts
            inspect=stand.command("inspect");events=stand.command("events");replay=stand.command("replay")
            assert replay["status"]=="ok" and inspect["queue"]["message_count"]==0
            for key in ["account_state_root","registry_root","cumulative_fee_shells","accepted_input_count"]:assert replay[key]==inspect[key],key
            payload={"status":"PASS","audit_state_limits":audit_limits,"accounting":final,"receiver_totals":actual_receipts,"audit_totals":audit_totals,"audit_counts":audit_counts,"semantic_duplicate_audit_did_not_double_credit":True,
                     "addresses":{"vault":vault,"audit":audit,"receivers":receivers,"wallets":stand.wallets},"inspect":inspect,"events":events,"replay":replay,"journal":stand.log}
            output.write_text(json.dumps(payload,indent=2)+"\n");print("Actual AVM revenue hub journey: PASS")
        except Exception:
            output.write_text(json.dumps({"status":"FAIL","journal":stand.log},indent=2)+"\n");raise

if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("--sdk",type=Path,required=True);p.add_argument("--output",type=Path,default=ROOT/"evidence/avm-e2e.json")
    a=p.parse_args();run(a.sdk.resolve(),a.output.resolve())
