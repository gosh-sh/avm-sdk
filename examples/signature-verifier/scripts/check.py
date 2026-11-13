from pathlib import Path
import argparse,json,os,subprocess
parser=argparse.ArgumentParser()
for name in ('builder','stand','profile','out'):
    parser.add_argument('--'+name,required=True,type=Path)
args=parser.parse_args()
project=Path(__file__).resolve().parents[1]
root=args.out.resolve();root.mkdir(parents=True,exist_ok=False)
state=root/'developer.state'
tool=str(args.stand.resolve());profile=args.profile.resolve()
label='signature-verifier'
environment=os.environ.copy()
environment['RUSTUP_TOOLCHAIN']='1.99.1'
for name,program in [('CARGO','cargo'),('RUSTC','rustc')]:
    environment[name]=subprocess.check_output(['rustup','which','--toolchain','1.99.1',program],text=True).strip()
with (root/'build.log').open('w') as log:
    subprocess.run([str(args.builder.resolve()),'build','--manifest-path',str(project/'Cargo.toml')],env=environment,stdout=log,stderr=subprocess.STDOUT,check=True)
common=['--state',str(state),'--wasmtime-profile',str(profile)]
counter=0
def run(command,*args,exit_code=0):
    global counter
    cmd=[tool,command,*common,*map(str,args)]
    r=subprocess.run(cmd,capture_output=True,text=True)
    (root/f'{counter:02d}-{command}.json').write_text(r.stdout)
    (root/f'{counter:02d}-{command}.stderr').write_text(r.stderr)
    counter+=1
    assert r.returncode==exit_code,(command,r.returncode,r.stdout[-1600:],r.stderr[-600:])
    return json.loads(r.stdout)
vectors=json.loads((project/'fixtures.json').read_text())
assert [len(case['payload'])//2 for case in vectors['cases']]==[0,14,4096], 'required payload fixtures are missing'
init=run('init','--dapp-id','11'*32)
owner=init['identities']['package_owner']
artifacts=project/'target/avm'
deploy=['--wasm',artifacts/'contract.wasm','--acab',artifacts/'contract.acab','--descriptor',artifacts/'contract.descriptor',
    '--salt-hex','63727970746f','--init-json',json.dumps('0x'+vectors['pubkey']),'--owner-id',owner,
    '--block-time',1800000000,'--created-at',1800000000,'--expire-at',1800003600,'--fuel-fee-shells',1000]
quote=run('deploy',*deploy,'--quote-address');address=quote['address']
run('fund-user','--address',address,'--shells',1000000)
run('deploy',*deploy)
outcomes=[]
for i,case in enumerate(vectors['cases']):
    payload=case['signature']+case['payload']
    args=['--address',address,'--method','crypto.verify','--args-json',json.dumps('0x'+payload),
        '--identity','package-owner','--extra-currencies','',
        '--block-time',1800000010+i*2,'--created-at',1800000009+i*2,
        '--expire-at',1800003609+i*2,'--fuel-fee-shells',1000]
    value=run('call',*args)
    assert value['raw_state']==owner+vectors['pubkey']+case['blake3'],value.keys()
    outcomes.append({'payload_bytes':len(case['payload'])//2,'raw_output':value['raw_executor_output'],'receipt':value['receipt']})
    bad=f'{int(payload[:2],16)^1:02x}'+payload[2:]
    badargs=args.copy();badargs[badargs.index('--args-json')+1]=json.dumps('0x'+bad)
    badargs[badargs.index('--block-time')+1]=1800000011+i*2
    badargs[badargs.index('--created-at')+1]=1800000010+i*2
    failed=run('call',*badargs)
    output=bytes.fromhex(failed['raw_executor_output'])
    assert output[:4]==b'ACMO' and int.from_bytes(output[4:8],'little')==1801,failed
    after=run('inspect','--account',address)
    assert after['raw_state']==owner+vectors['pubkey']+case['blake3']
before=run('inspect')
replay=run('replay')
after=run('inspect')
assert before==after,'replay changed final visible state'
summary={'passed':True,'label':label,'wasm_bytes':(artifacts/'contract.wasm').stat().st_size,
    'positive_cases':len(outcomes),'invalid_signatures':len(outcomes),'replay_verified':True,'outcomes':outcomes}
(root/'result.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({'passed':True,'label':label,'wasm_bytes':summary['wasm_bytes'],'positive_cases':len(outcomes),'invalid_signatures':len(outcomes),'replay_identical':True}))
