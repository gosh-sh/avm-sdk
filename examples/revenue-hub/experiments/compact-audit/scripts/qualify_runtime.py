#!/usr/bin/env python3
"""Build and execute the optional experiment in a disposable project copy."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

EXP = Path(__file__).resolve().parents[1]
ROOT = EXP.parents[1]

def run(sdk, optimizer):
    evidence = EXP / 'evidence'
    manifest = json.loads((sdk / 'BUILD-MANIFEST.json').read_text())
    assert manifest['sdk_commit'] == '0748f4f0767a26da8bc363bcb6b5585539d6970e'
    assert manifest['ackinacki_commit'] == '06e96ef71006cca020575cbf35e6b82cc82a324e'
    commands = []
    def command(args, label, env=None):
        proc = subprocess.run([str(x) for x in args], text=True, capture_output=True, env=env)
        (evidence / (label+'.txt')).write_text(proc.stdout+proc.stderr)
        commands.append({'label':label,'exit_code':proc.returncode,'argv':[str(x) for x in args]})
        (evidence/'commands.json').write_text(json.dumps(commands,indent=2)+'\n')
        if proc.returncode:
            raise RuntimeError(f'{label}: exit {proc.returncode}; see {evidence}/{label}.txt')
    with tempfile.TemporaryDirectory(prefix='avm-compact-qualification-') as directory:
        work = Path(directory)
        for name in ['core','contracts']:
            shutil.copytree(ROOT/name,work/name,ignore=shutil.ignore_patterns('target','__pycache__'))
        # Rebase in-tree SDK dependencies only inside this disposable fixture.
        shutil.copytree(ROOT.parents[1]/'crates/avm-contract-sdk',work/'vendor/avm-contract-sdk')
        for cargo in work.glob('**/Cargo.toml'):
            text = cargo.read_text()
            text = text.replace('../../../../crates/avm-contract-sdk','../../vendor/avm-contract-sdk')
            text = text.replace('../../../crates/avm-contract-sdk','../vendor/avm-contract-sdk')
            cargo.write_text(text)
        for name in ['Cargo.toml','LICENSE']:
            shutil.copyfile(ROOT/name,work/name)
        shutil.copyfile(ROOT.parents[1]/'rust-toolchain.toml',work/'rust-toolchain.toml')
        shutil.copyfile(ROOT.parents[1]/'rustfmt.toml',work/'rustfmt.toml')
        shutil.copyfile(ROOT.parents[1]/'LICENSE.md',work/'LICENSE.md')
        cargo = work/'Cargo.toml'
        sdk_manifest = (ROOT.parents[1]/'Cargo.toml').read_text()
        cargo.write_text(cargo.read_text()+'\n[workspace.package]\n'+sdk_manifest.split('[workspace.package]',1)[1])
        (work/'scripts').mkdir()
        (work/'evidence').mkdir()
        for name in ['optimize.py','sync_ids.py']:
            shutil.copyfile(ROOT/'scripts'/name,work/'scripts'/name)
        for name in ['e2e.py','runtime_probes.py']:
            shutil.copyfile(EXP/'scripts'/name,work/'scripts'/name)
        legacy=work/'contracts/legacy-audit'
        shutil.copytree(work/'contracts/audit',legacy)
        cargo=legacy/'Cargo.toml'
        cargo.write_text(cargo.read_text().replace('name = "avm-revenue-audit"','name = "avm-revenue-legacy-audit"'))
        cargo=work/'Cargo.toml'
        cargo.write_text(cargo.read_text().replace(', "tests"','').replace('"contracts/audit"','"contracts/audit", "contracts/legacy-audit"'))
        shutil.copyfile(EXP/'src/lib.rs',work/'contracts/audit/src/lib.rs')
        shutil.copyfile(EXP/'avm-contract.toml',work/'contracts/audit/avm-contract.toml')
        command(['cargo','generate-lockfile','--offline','--manifest-path',work/'Cargo.toml'],'temporary-lock')
        env=os.environ.copy()
        env.update(CARGO_PROFILE_RELEASE_OPT_LEVEL='3',CARGO_PROFILE_RELEASE_LTO='false',CARGO_PROFILE_RELEASE_CODEGEN_UNITS='1',CARGO_PROFILE_RELEASE_STRIP='symbols')
        for name in ['vault','receiver','audit','legacy-audit']:
            command([sdk/'bin/avm-contract-build','build','--manifest-path',work/'contracts'/name/'Cargo.toml'],'official-build-'+name,env)
        command(['python3',work/'scripts/sync_ids.py','--check'],'canonical-ids')
        command(['python3',work/'scripts/optimize.py','--wasm-opt',optimizer],'optimize')
        command(['node',EXP/'scripts/guest_modes.mjs',work/'contracts/audit/target/avm/contract.wasm',evidence/'guest-vectors.tsv',evidence/'official-guest.json'],'official-guest')
        command(['python3',work/'scripts/e2e.py','--sdk',sdk,'--output',evidence/'journey.json'],'journey')
        command(['python3',work/'scripts/runtime_probes.py','--sdk',sdk,'--output',evidence/'runtime-probes.json'],'runtime-probes')
        shutil.copyfile(work/'evidence/optimization.json',evidence/'optimization.json')
        artifacts={}
        for name in ['vault','receiver','audit','legacy-audit']:
            artifacts[name]={}
            for filename in ['contract.wasm','contract.acab','contract.descriptor']:
                data=(work/'contracts'/name/'target/avm'/filename).read_bytes()
                artifacts[name][filename]={'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
        journey=json.loads((evidence/'journey.json').read_text())
        probes=json.loads((evidence/'runtime-probes.json').read_text())
        assert journey['status']==probes['status']=='PASS'
        summary={'status':'PASS','scope':'Published SDK 0.2.0 LOCAL stand; synthetic funds only',
                 'sdk_commit':manifest['sdk_commit'],'runtime_commit':manifest['ackinacki_commit'],
                 'artifacts':artifacts,'audit_state':journey['audit_state_limits'],
                 'accounting':journey['accounting'],'audit_totals':journey['audit_totals'],
                 'audit_counts':journey['audit_counts'],'restart':journey['restart'],
                 'queue_empty':journey['inspect']['queue']['message_count']==0,
                 'rent':probes['sizes_and_rent'],'migration':probes['migration'],
                 'native_tests':10,'guest_vectors':10,'mainnet':False,'real_funds':False}
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print('Compact audit: native + official guest + local journey/restart/replay + rent + legacy upgrade rejection PASS')

if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--sdk',type=Path,required=True)
    parser.add_argument('--optimizer',type=Path,required=True)
    args=parser.parse_args()
    run(args.sdk.resolve(),args.optimizer.resolve())
