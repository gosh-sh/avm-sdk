#!/usr/bin/env python3
from pathlib import Path
import argparse
ROOT=Path(__file__).resolve().parents[1]
METHODS={'vesting':[('FUND','stream.vesting.fund'),('CLAIM','stream.vesting.claim'),('CANCEL','stream.vesting.cancel'),('REFUND','stream.vesting.refund'),('CALLBACK','stream.vesting.callback'),('STATUS','stream.vesting.status'),('TOPUP','stream.vesting.topup')], 'receiver':[('RECEIVE','stream.receiver.receive'),('BIND','stream.receiver.bind'),('REJECT','stream.receiver.reject'),('RECEIVER_STATUS','stream.receiver.status')]}
EVENTS=['created','funded','cancelled','pending','settled','failed','status','fees']
def identifier(contract,name,prefix):
    data=(ROOT/'contracts'/contract/'target/avm/contract.acab').read_bytes();encoded=name.encode()
    assert data[:4]==b'ACAB'
    positions=[i for i in range(len(data)) if data[i:i+len(encoded)]==encoded and int.from_bytes(data[i-2:i],'little')==len(encoded)]
    assert len(positions)==1,name
    i=positions[0];value=int.from_bytes(data[i-prefix:i-prefix+8],'little');assert value!=0
    return value
def render():
    lines=['// Generated from the official AVM builder\'s canonical ACAB files.']
    for c,rows in METHODS.items():
        for key,name in rows:lines.append(f'pub const {key}: u64 = 0x{identifier(c,name,16):016x};')
    for e in EVENTS:lines.append(f'pub const EVENT_{e.upper()}: u64 = 0x{identifier("vesting","stream."+e,14):016x};')
    return '\n'.join(lines)+'\n'
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--check',action='store_true');a=p.parse_args()
    path=ROOT/'core/src/ids.rs';expected=render()
    if a.check:assert path.read_text()==expected,'Canonical IDs differ';print('Canonical IDs: PASS')
    else:path.write_text(expected)
