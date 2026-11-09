#!/usr/bin/env python3
"""Actual AVM vesting-stream journey. No network and no real funds.

Run after reproducible builds, canonical-ID checking, and deterministic optimization.
Every participant acts through its own keyed wallet (examples/wallet): the
participant signs an external to its wallet and the wallet sends the internal
call, with any value, to the application. The owner key is also kept by the
vesting and receiver accounts at owner_key_offset 0 for their owner's
no-value externals.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
WALLET = ROOT.parent / "wallet"
DAPP = "11" * 32
BASE_TIME = 1_800_000_000
# FUEL reserve of every wallet-to-application internal message.
INTERNAL_FUEL = 100_000

def method_ids():
    text = (ROOT / "core/src/ids.rs").read_text()
    return {name: int(value, 16) for name, value in re.findall(r"pub const (\w+): u64 = 0x([0-9a-f]+);", text)}
IDS = method_ids()
METHOD_IDS = {"stream.vesting." + name.lower(): IDS[name] for name in ["FUND", "CLAIM", "CANCEL", "REFUND", "STATUS", "TOPUP"]}

def address_bytes(value): return bytes.fromhex(value.replace(':',''))

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
        self.binary = os.environ.get("AVM_LOCAL_STAND", str(sdk / "bin/avm-local-stand"))
        self.profile = os.environ.get("AVM_WASMTIME_PROFILE", str(sdk / "profiles/wasmtime-release-profile.v1"))
        self.state = work / "journey.state"
        self.log = []
        self.time = BASE_TIME
        self.selections = {}
        self.wallets = {}
        self.keys = {}

    def run(self, operation, **options):
        cmd = [self.binary, operation, "--state", str(self.state), "--wasmtime-profile", self.profile]
        for key, value in options.items():
            cmd += ["--" + key.replace("_", "-")]
            if value is not True:
                cmd += [str(value)]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        try:
            result = json.loads(proc.stdout)
        except json.JSONDecodeError:
            raise RuntimeError(f"{operation}: {proc.stderr}\n{proc.stdout}") from None
        self.log.append({"operation": operation, "options": options, "exit_code": proc.returncode, "result": result})
        return cmd, proc, result

    def command(self, operation, **options):
        cmd, proc, result = self.run(operation, **options)
        assert proc.returncode == 0, {"command": cmd, "result": result, "stderr": proc.stderr}
        return result

    def rejected(self, error, operation, **options):
        """The node rejects this external at admission with `error`."""
        cmd, proc, result = self.run(operation, **options)
        assert proc.returncode != 0 and result.get("status") != "ok" and error in proc.stdout + proc.stderr, {"command": cmd, "result": result, "stderr": proc.stderr}
        return result

    def timestamps(self):
        self.time += 1
        return dict(block_time=self.time, created_at=self.time, expire_at=self.time+3600, fuel_fee_shells=10000)

    def deploy(self, contract, salt, init=None, owner_id=None, extra_currencies=None, artifacts=None, shells=10_000_000):
        artifacts = artifacts or ROOT / "contracts" / contract / "target/avm"
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab",
                       descriptor=artifacts / "contract.descriptor", salt_hex=salt.encode().hex(),
                       init_json=json.dumps(None if init is None else "0x"+init.hex()), **self.timestamps())
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

    def wallet(self, identity, key, extra_currencies=None):
        """Deploys the keyed wallet of `identity`; its data is the owner key."""
        self.keys[identity] = key
        self.wallets[identity] = self.deploy("wallet", "wallet-" + identity, owner_id=key,
                                             extra_currencies=extra_currencies, artifacts=WALLET / "target/avm", shells=100_000_000)
        return self.wallets[identity]

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

    def send(self, address, method, body=b"", identity="developer", value=0, extra=(), expected_status=0):
        """`identity` signs wallet.send on its own wallet; the wallet calls `address`.

        Returns the application account image after the queue drained and the
        events the call produced. The application's exact status comes back in
        the wallet's `wallet.callback` event.
        """
        # ECC0 is this example's explicit synthetic test principal; SHELL is separate.
        currencies = sorted(([(0, value)] if value else []) + list(extra))
        before = self.command("events")["count"]
        self.call(self.wallets[identity], "wallet.send", bytes.fromhex(send_args(address, METHOD_IDS[method], currencies, body)[2:]), identity=identity)
        events = [bytes.fromhex(e) for e in self.command("events")["events"][before:]]
        statuses = [s for s in map(wallet_callback_status, events) if s is not None]
        assert statuses == [expected_status], {"method": method, "contract_status": statuses, "expected": expected_status}
        account = self.command("inspect", account=address)
        account["events"] = [e.hex() for e in events]
        return account

def config(owner, owner_wallet, wallets, recipients):
    b=bytearray(1976);b[:32]=bytes.fromhex(owner);b[32:96]=address_bytes(owner_wallet);b[96:112]=(10000).to_bytes(16,'little');b[112]=3
    for i,(amount,cliff,end,step,revocable) in enumerate([(1000,200,500,0,1),(600,100,400,100,1),(300,200,400,0,0)]):
        o=120+i*232;b[o:o+64]=address_bytes(wallets[i]);b[o+64:o+128]=address_bytes(recipients[i][0]);b[o+128:o+192]=address_bytes(recipients[i][1]);b[o+192:o+208]=amount.to_bytes(16,'little')
        for off,value in [(208,BASE_TIME+100),(212,BASE_TIME+cliff),(216,BASE_TIME+end),(220,step)]:b[o+off:o+off+4]=value.to_bytes(4,'little')
        b[o+224]=revocable
    return bytes(b)
def decode(result):
    b=bytes.fromhex(result['raw_state']);assert len(b)==3592
    streams=[]
    for i in range(b[112]):
        o=1992+i*200
        streams.append(dict(cancelled=bool(b[o]),cutoff=int.from_bytes(b[o+4:o+8],'little'),cap=int.from_bytes(b[o+8:o+24],'little'),claimed=int.from_bytes(b[o+24:o+40],'little'),refunded=int.from_bytes(b[o+40:o+56],'little'),pending=[int.from_bytes(b[o+56+k*72:o+72+k*72],'little') for k in range(2)]))
    claimed=sum(s['claimed'] for s in streams);refunded=sum(s['refunded'] for s in streams);pending=sum(sum(s['pending']) for s in streams)
    return dict(funded=bool(b[1976]),time=int.from_bytes(b[1980:1984],'little'),attempts=int.from_bytes(b[1984:1992],'little'),claimed=claimed,refunded=refunded,pending=pending,held=1900-claimed-refunded-pending,streams=streams)
def run(sdk,output):
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='avm-stream-') as directory:
        stand=Stand(sdk,Path(directory))
        try:
            init=stand.command('init',dapp_id=DAPP);assert init['status']=='ok'
            identities=init['identities']
            # The owner is the Developer identity: the vesting and receiver
            # accounts keep its key at owner_key_offset 0 (the deploy owner_id).
            owner_key=identities['developer'];owner_role='developer'
            claim_roles=['developer','package-owner','user']
            # The owner wallet holds the 1,900 ECC0 principal it funds the grants with.
            wallets=[stand.wallet(role,identities[role.replace('-','_')],extra_currencies='0:1900' if role==owner_role else None) for role in claim_roles]
            recipients=[[stand.deploy('receiver',f'receiver-{i}-{k}',bytes([k,i]),owner_id=owner_key) for k in range(2)] for i in range(3)]
            receiver_owners=[]
            for pair in recipients:
                owners=[]
                for address in pair:
                    r=stand.call(address,'stream.receiver.status',identity=owner_role)
                    assert r['raw_state'][:64] == owner_key, 'receiver owner key must equal the deploy owner_id'
                    owners.append(owner_role)
                receiver_owners.append(owners)
            vault=stand.deploy('vesting','vesting',config(owner_key,stand.wallets[owner_role],wallets,recipients),owner_id=owner_key)
            for i,pair in enumerate(recipients):
                for k,address in enumerate(pair):stand.call(address,'stream.receiver.bind',address_bytes(vault),identity=receiver_owners[i][k])
            # The node admits an external only under the destination's owner key,
            # and no external carries value.
            stand.rejected('AccountKeyNotAllowed','call',address=vault,method='stream.vesting.cancel',args_json=json.dumps('0x00'),identity='user',extra_currencies='',**stand.timestamps())
            stand.rejected('AccountKeyNotAllowed','call',address=stand.wallets[owner_role],method='wallet.send',args_json=json.dumps(send_args(vault,METHOD_IDS['stream.vesting.status'],[],b'')),identity='user',extra_currencies='',**stand.timestamps())
            stand.rejected('ExternalValueNotAllowed','call',address=vault,method='stream.vesting.fund',args_json='null',identity=owner_role,extra_currencies='0:1900',**stand.timestamps())
            stand.send(vault,'stream.vesting.fund',value=1900,identity=owner_role)
            def at(offset,send,*args,**kwargs):
                target=BASE_TIME+offset;assert target>stand.time,(target,stand.time);stand.time=target-1
                return send(*args,**kwargs)
            # A call from a foreign wallet is rejected by the vesting account.
            at(50,stand.send,vault,'stream.vesting.claim',b'\x00',identity='user',expected_status=4001)
            at(150,stand.send,vault,'stream.vesting.claim',b'\x01',identity=claim_roles[1],expected_status=4005)
            at(180,stand.call,recipients[0][0],'stream.receiver.reject',b'\x01',identity=receiver_owners[0][0])
            at(181,stand.call,recipients[0][1],'stream.receiver.reject',b'\x01',identity=receiver_owners[0][1])
            at(199,stand.send,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0],expected_status=4005)
            r=at(200,stand.send,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0]);assert decode(r)['claimed']==0 and decode(r)['pending']==0
            assert any(b'stream.failed' in bytes.fromhex(e) for e in r['events'])
            at(202,stand.call,recipients[0][0],'stream.receiver.reject',b'\x00',identity=receiver_owners[0][0])
            r=at(220,stand.send,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0]);assert decode(r)['streams'][0]['claimed']==300
            r=at(221,stand.send,vault,'stream.vesting.claim',b'\x01',identity=claim_roles[1]);assert decode(r)['streams'][1]['claimed']==200
            r=at(260,stand.call,vault,'stream.vesting.cancel',b'\x00',identity=owner_role);assert decode(r)['streams'][0]['cap']==400
            r=at(261,stand.send,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0]);assert decode(r)['streams'][0]['claimed']==400
            r=at(262,stand.call,vault,'stream.vesting.refund',b'\x00',identity=owner_role);assert decode(r)['refunded']==0 and decode(r)['pending']==0
            assert any(b'stream.failed' in bytes.fromhex(e) for e in r['events'])
            at(263,stand.call,recipients[0][1],'stream.receiver.reject',b'\x00',identity=receiver_owners[0][1])
            r=at(264,stand.call,vault,'stream.vesting.refund',b'\x00',identity=owner_role);assert decode(r)['refunded']==600
            r=at(300,stand.send,vault,'stream.vesting.claim',b'\x01',identity=claim_roles[1]);assert decode(r)['streams'][1]['claimed']==400
            r=at(350,stand.call,vault,'stream.vesting.cancel',b'\x01',identity=owner_role);assert decode(r)['streams'][1]['cap']==400
            r=at(351,stand.call,vault,'stream.vesting.refund',b'\x01',identity=owner_role);assert decode(r)['refunded']==800
            at(352,stand.send,vault,'stream.vesting.claim',b'\x01',identity=claim_roles[1],expected_status=4005)
            at(353,stand.call,vault,'stream.vesting.cancel',b'\x02',identity=owner_role,expected_status=4010)
            r=at(354,stand.send,vault,'stream.vesting.claim',b'\x02',identity=claim_roles[2]);assert decode(r)['streams'][2]['claimed']==254
            r=at(400,stand.send,vault,'stream.vesting.claim',b'\x02',identity=claim_roles[2]);assert decode(r)['streams'][2]['claimed']==300
            at(401,stand.send,vault,'stream.vesting.claim',b'\x00',identity=claim_roles[0],expected_status=4005)
            at(402,stand.call,vault,'stream.vesting.refund',b'\x00',identity=owner_role,expected_status=4005)
            r=stand.call(vault,'stream.vesting.status',identity=owner_role);final=decode(r)
            assert (final['claimed'],final['refunded'],final['pending'],final['held'])==(1100,800,0,0),final
            actual=[]
            for i,pair in enumerate(recipients):
                actual.append([])
                for k,address in enumerate(pair):
                    r=stand.call(address,'stream.receiver.status',identity=receiver_owners[i][k]);b=bytes.fromhex(r['raw_state']);o=104+i*32;actual[i].append(int.from_bytes(b[o+8:o+24],'little'))
            assert actual==[[400,600],[400,200],[300,0]],actual
            inspect=stand.command('inspect');replay=stand.command('replay');events=stand.command('events')
            assert inspect['queue']['message_count']==0 and replay['status']=='ok'
            for key in ['account_state_root','registry_root','accepted_input_count','cumulative_fee_shells']:assert replay[key]==inspect[key],key
            payload=dict(status='PASS',accounting=final,receivers=actual,wallets=stand.wallets,inspect=inspect,replay=replay,events=events,journal=stand.log)
            output.write_text(json.dumps(payload,indent=2)+'\n');print('Actual AVM vesting/refund/retry journey: PASS')
        except Exception:
            output.write_text(json.dumps(dict(status='FAIL',journal=stand.log),indent=2)+'\n');raise
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--sdk',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();run(a.sdk.resolve(),a.output.resolve())
