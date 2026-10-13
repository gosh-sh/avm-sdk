#!/usr/bin/env python3
"""Actual AVM vesting-stream journey. No network and no real funds.

Run after reproducible builds, canonical-ID checking, and deterministic optimization.
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

def address_bytes(value): return bytes.fromhex(value.replace(':',''))
def config(owner, keys, recipients):
    b=bytearray(1656);b[:32]=bytes.fromhex(owner);b[32:48]=(10000).to_bytes(16,'little');b[48]=3
    for i,(amount,cliff,end,step,revocable) in enumerate([(1000,200,500,0,1),(600,100,400,100,1),(300,200,400,0,0)]):
        o=56+i*200;b[o:o+32]=bytes.fromhex(keys[i]);b[o+32:o+96]=address_bytes(recipients[i][0]);b[o+96:o+160]=address_bytes(recipients[i][1]);b[o+160:o+176]=amount.to_bytes(16,'little')
        for off,value in [(176,BASE_TIME+100),(180,BASE_TIME+cliff),(184,BASE_TIME+end),(188,step)]:b[o+off:o+off+4]=value.to_bytes(4,'little')
        b[o+192]=revocable
    return bytes(b)
def decode(result):
    b=bytes.fromhex(result['raw_state']);assert len(b)==3272
    streams=[]
    for i in range(b[48]):
        o=1672+i*200
        streams.append(dict(cancelled=bool(b[o]),cutoff=int.from_bytes(b[o+4:o+8],'little'),cap=int.from_bytes(b[o+8:o+24],'little'),claimed=int.from_bytes(b[o+24:o+40],'little'),refunded=int.from_bytes(b[o+40:o+56],'little'),pending=[int.from_bytes(b[o+56+k*72:o+72+k*72],'little') for k in range(2)]))
    claimed=sum(s['claimed'] for s in streams);refunded=sum(s['refunded'] for s in streams);pending=sum(sum(s['pending']) for s in streams)
    return dict(funded=bool(b[1656]),time=int.from_bytes(b[1660:1664],'little'),attempts=int.from_bytes(b[1664:1672],'little'),claimed=claimed,refunded=refunded,pending=pending,held=1900-claimed-refunded-pending,streams=streams)
def run(sdk,output):
    with tempfile.TemporaryDirectory(prefix='avm-stream-') as directory:
        stand=Stand(sdk,Path(directory))
        try:
            init=stand.command('init',dapp_id=DAPP);assert init['status']=='ok'
            identities=init['identities'];roles={value:key.replace('_','-') for key,value in identities.items()}
            recipients=[[stand.deploy('receiver',f'receiver-{i}-{k}',bytes([k,i])) for k in range(2)] for i in range(3)]
            receiver_owners=[]
            for pair in recipients:
                owners=[]
                for address in pair:
                    r=stand.call(address,'stream.receiver.status');owners.append(roles[r['raw_state'][:64]])
                receiver_owners.append(owners)
            owner_key=identities['package_owner'];owner_role=roles[owner_key]
            keys=[identities['developer'],identities['package_owner'],identities['user']]
            vault=stand.deploy('vesting','vesting',config(owner_key,keys,recipients))
            for i,pair in enumerate(recipients):
                for k,address in enumerate(pair):stand.call(address,'stream.receiver.bind',address_bytes(vault),identity=receiver_owners[i][k])
            stand.call(vault,'stream.vesting.fund',value=1900,identity=owner_role)
            def at(offset,address,method,body=b'',**kwargs):
                target=BASE_TIME+offset;assert target>stand.time,(target,stand.time);stand.time=target-1
                return stand.call(address,method,body,**kwargs)
            claim_roles=['developer','package-owner','user']
            at(50,vault,'stream.vesting.claim',b'\x00',identity='user',expected_status=4001)
            at(150,vault,'stream.vesting.claim',b'\x01',identity=claim_roles[1],expected_status=4005)
            at(180,recipients[0][0],'stream.receiver.reject',b'\x01',identity=receiver_owners[0][0])
            at(181,recipients[0][1],'stream.receiver.reject',b'\x01',identity=receiver_owners[0][1])
            at(199,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0],expected_status=4005)
            r=at(200,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0]);assert decode(r)['claimed']==0 and decode(r)['pending']==0
            assert any(b'stream.failed' in bytes.fromhex(e) for e in r['events'])
            at(202,recipients[0][0],'stream.receiver.reject',b'\x00',identity=receiver_owners[0][0])
            r=at(220,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0]);assert decode(r)['streams'][0]['claimed']==300
            r=at(221,vault,'stream.vesting.claim',b'\x01',identity=claim_roles[1]);assert decode(r)['streams'][1]['claimed']==200
            r=at(260,vault,'stream.vesting.cancel',b'\x00',identity=owner_role);assert decode(r)['streams'][0]['cap']==400
            r=at(261,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0]);assert decode(r)['streams'][0]['claimed']==400
            r=at(262,vault,'stream.vesting.refund',b'\x00',identity=owner_role);assert decode(r)['refunded']==0 and decode(r)['pending']==0
            assert any(b'stream.failed' in bytes.fromhex(e) for e in r['events'])
            at(263,recipients[0][1],'stream.receiver.reject',b'\x00',identity=receiver_owners[0][1])
            r=at(264,vault,'stream.vesting.refund',b'\x00',identity=owner_role);assert decode(r)['refunded']==600
            r=at(300,vault,'stream.vesting.claim',b'\x01',identity=claim_roles[1]);assert decode(r)['streams'][1]['claimed']==400
            r=at(350,vault,'stream.vesting.cancel',b'\x01',identity=owner_role);assert decode(r)['streams'][1]['cap']==400
            r=at(351,vault,'stream.vesting.refund',b'\x01',identity=owner_role);assert decode(r)['refunded']==800
            at(352,vault,'stream.vesting.claim',b'\x01',identity=claim_roles[1],expected_status=4005)
            at(353,vault,'stream.vesting.cancel',b'\x02',identity=owner_role,expected_status=4010)
            r=at(354,vault,'stream.vesting.claim',b'\x02',identity=claim_roles[2]);assert decode(r)['streams'][2]['claimed']==254
            r=at(400,vault,'stream.vesting.claim',b'\x02',identity=claim_roles[2]);assert decode(r)['streams'][2]['claimed']==300
            at(401,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0],expected_status=4005)
            at(402,vault,'stream.vesting.refund',b'\x00',identity=owner_role,expected_status=4005)
            r=stand.call(vault,'stream.vesting.status');final=decode(r)
            assert (final['claimed'],final['refunded'],final['pending'],final['held'])==(1100,800,0,0),final
            actual=[]
            for i,pair in enumerate(recipients):
                actual.append([])
                for k,address in enumerate(pair):
                    r=stand.call(address,'stream.receiver.status');b=bytes.fromhex(r['raw_state']);o=104+i*32;actual[i].append(int.from_bytes(b[o+8:o+24],'little'))
            assert actual==[[400,600],[400,200],[300,0]],actual
            inspect=stand.command('inspect');replay=stand.command('replay');events=stand.command('events')
            assert inspect['queue']['message_count']==0 and replay['status']=='ok'
            for key in ['account_state_root','registry_root','accepted_input_count','cumulative_fee_shells']:assert replay[key]==inspect[key],key
            payload=dict(status='PASS',accounting=final,receivers=actual,inspect=inspect,replay=replay,events=events,journal=stand.log)
            output.write_text(json.dumps(payload,indent=2)+'\n');print('Actual AVM vesting/refund/retry journey: PASS')
        except Exception:
            output.write_text(json.dumps(dict(status='FAIL',journal=stand.log),indent=2)+'\n');raise
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--sdk',type=Path,required=True);p.add_argument('--output',type=Path,default=ROOT/'evidence/avm-e2e.json');a=p.parse_args();run(a.sdk.resolve(),a.output.resolve())

