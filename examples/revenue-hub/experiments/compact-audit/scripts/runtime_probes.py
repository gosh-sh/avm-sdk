#!/usr/bin/env python3
"""Synthetic, file-backed local stand only. No network transactions."""
from pathlib import Path
import argparse,json,tempfile,shutil,hashlib
from e2e import Stand,DAPP,ROOT,COMPACT
class ProbeStand(Stand):
    def command(self,operation,**options):
        if operation=='fund-user': options['shells']=10**12
        return super().command(operation,**options)
def account(s,address):return next(a for a in s.command('inspect')['accounts'] if a['address']==address)
def check_replay(s):
    i=s.command('inspect');r=s.command('replay');assert r['status']=='ok',r
    for k in ['account_state_root','registry_root','cumulative_fee_shells','accepted_input_count']:assert i[k]==r[k],k
    return True

def run(sdk,out):
 result={'scope':'Explicit-tool local stand; synthetic funds only','sizes_and_rent':[],'journal':[]}
 try:
  with tempfile.TemporaryDirectory(prefix='compact-runtime-probes-') as d:
   work=Path(d)
   for contract,n,size in [('audit',1,199),('audit',3,391),('audit',8,871),('audit',16,1639),('legacy-audit',3,65599)]:
    folder=work/f'{contract}-{n}';folder.mkdir();s=ProbeStand(sdk,folder)
    assert s.command('init',dapp_id=DAPP)['status']=='ok'
    address=s.deploy(contract,f'{contract}-{n}',bytes([n]));a=account(s,address);assert a['canonical_byte_length']==size,a
    row={'contract':contract,'members':n,'canonical_bytes':size,'probes':[]}
    for elapsed in [1,3601,86401]:
     sub=folder/str(elapsed);sub.mkdir();b=ProbeStand(sdk,sub);shutil.copyfile(s.state,b.state)
     b.time=s.time+elapsed-1;b.selections=s.selections.copy()
     before=int(b.command('inspect')['cumulative_fee_shells'])
     call=b.call(address,'hub.audit.status');after=b.command('inspect')
     aa=next(a for a in after['accounts'] if a['address']==address)
     assert aa['storage_due_payment']=='0' and aa['storage_unpaid_tx_count']==0
     assert aa['storage_last_paid']==s.time+elapsed
     mode=bytes.fromhex(call['raw_executor_output'])[9];assert mode==(1 if contract=='legacy-audit' else 0)
     row['probes'].append({'elapsed_seconds':elapsed,'total_charged_shells':int(after['cumulative_fee_shells'])-before,'mode':mode,'replay_matches':check_replay(b),'storage':aa})
     result['journal']+=b.log
    base=row['probes'][0]['total_charged_shells']
    for v in row['probes'][1:]:
     actual=v['total_charged_shells']-base;expected=size*(v['elapsed_seconds']-1)
     assert actual==expected,(actual,expected,row)
     v['matched_elapsed_time_fee_difference']=actual;v['expected_canonical_byte_seconds']=expected
    result['sizes_and_rent'].append(row);result['journal']+=s.log
   # Existing legacy code must authorize its own upgrade. Test, don't assume.
   folder=work/'upgrade';folder.mkdir();s=ProbeStand(sdk,folder);s.command('init',dapp_id=DAPP)
   address=s.deploy('legacy-audit','legacy-upgrade',bytes([3]))
   old_selection=s.selections['legacy-audit'];old=s.call(address,'hub.audit.status')['raw_state']
   artifacts=COMPACT/'target/avm'
   publish_options=dict(wasm=str(artifacts/'contract.wasm'),acab=str(artifacts/'contract.acab'),descriptor=str(artifacts/'contract.descriptor'),package_id=old_selection['package_id'],salt_hex='1234',init_json=json.dumps('0x03'),**s.timestamps())
   pub=s.command('publish-version',**publish_options)
   if pub.get('error')=='funding_required':
    assert s.command('fund-user',address=pub['address'],shells=10**12)['status']=='ok'
    pub=s.command('publish-version',**publish_options)
   result['migration']={'publication_result':pub}
   assert pub['status']=='ok',pub
   i=s.command('inspect');entries=[e for e in i['registry_entries'] if e['key'].startswith('02') and e['value'][:64]!=old_selection['selected_code_id']]
   assert len(entries)==1;target=entries[0]['value'][:64]
   attempts=[]
   for identity in ['developer','package-owner','user']:
    up=s.command('upgrade',address=address,selected_code_id=target,migration_payload_hex='',identity=identity,**s.timestamps())
    after=s.call(address,'hub.audit.status')['raw_state'];assert after==old
    assert account(s,address)['canonical_byte_length']==65599
    # Require explicit rejection, not merely an unchanged state assertion.
    raw=bytes.fromhex(up.get('raw_executor_output',''))
    code=int.from_bytes(raw[4:8],'little') if raw[:4]==b'ACMO' else None
    assert up.get('status')=='error' or code not in (None,0),up
    attempts.append({'identity':identity,'result':up,'application_status':code,'legacy_data_unchanged':True})
   result['migration'].update(attempts=attempts,replay_matches=check_replay(s),conclusion='Existing legacy contract has no authorized upgrade handler; direct upgrade is rejected. No migration performed.')
   result['journal']+=s.log
  result['status']='PASS';out.write_text(json.dumps(result,indent=2)+'\n');print('Runtime sizes, rent deltas, replay and legacy upgrade rejection: PASS')
 except Exception:
  result['status']='FAIL'
  if 's' in locals():result['last_journal']=s.log
  out.write_text(json.dumps(result,indent=2)+'\n');raise
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--sdk',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();run(a.sdk.resolve(),a.output.resolve())
