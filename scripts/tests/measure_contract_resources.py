#!/usr/bin/env python3
"""Select contract resources from a complete captured contract test corpus."""
import argparse
from collections import defaultdict
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
import blake3
from contract_memory import memory_limits
from memory_test_driver import BUILD_INPUTS


def configure(manifest, stack, arena, maximum):
    lines, skipping = [], False
    for line in manifest.read_text().splitlines():
        if line.strip().startswith('['):
            skipping = line.strip() == '[build]'
        if not skipping:
            lines.append(line)
    lines += ['', '[build]', f'max_memory_bytes = {maximum}', f'stack_bytes = {stack}', f'arena_bytes = {arena}']
    manifest.write_text('\n'.join(lines) + '\n')


def read_corpus(root):
    records = [json.loads(line) for line in (root / 'builds.jsonl').read_text().splitlines()]
    recipes = {}
    for record in records:
        if record['code_hash'] not in recipes or record.get('kind') == 'builder':
            recipes[record['code_hash']] = record
    optimizations = {}
    path = root / 'optimizations.jsonl'
    if path.exists():
        optimizations = {row['output_hash']: row for row in map(json.loads, path.read_text().splitlines())}
    cases = defaultdict(list)
    for usage in sorted((root / 'test-vectors').glob('stand-*.memory.jsonl')):
        batches = [json.loads(line) for line in usage.read_text().splitlines()]
        if any(batch.get('complete') is not True for batch in batches):
            raise ValueError('incomplete memory observations')
        raw = usage.with_name(usage.name.removesuffix('.memory.jsonl') + '.tsv')
        if any(batch['runs'] for batch in batches) and not raw.is_file():
            raise ValueError('observed execution has no raw test capture')
    for path in sorted((root / 'test-vectors').glob('stand-*.tsv')):
        rows = []
        for line in path.read_text().splitlines():
            row = dict(field.split('=', 1) for field in line.split('\t'))
            if row.get('result') != 'ok':
                raise ValueError('test capture contains an incomplete execution')
            for name in ('vm_status', 'fuel_budget', 'output_cap'):
                row[name] = int(row[name])
            rows.append(row)
        usage = path.with_suffix('.memory.jsonl')
        batches = [json.loads(line) for line in usage.read_text().splitlines()]
        if any(batch.get('complete') is not True for batch in batches):
            raise ValueError('incomplete memory observations')
        observed = [run for batch in batches for run in batch['runs']]
        if len(rows) != len(observed) or any(a['code_hash'] != b['code_hash'] for a, b in zip(rows, observed)):
            raise ValueError('raw test capture and memory observations disagree')
        for row in rows:
            cases[row['code_hash']].append(row)
    for path in sorted((root / 'test-vectors').glob('direct-*.jsonl')):
        for line in path.read_text().splitlines():
            row = json.loads(line)
            cases[row['code_hash']].append(row)
    if not cases:
        raise ValueError('empty contract test corpus')
    return recipes, optimizations, cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('corpus', 'sdk', 'builder', 'stand', 'profile', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--optimizer', type=Path, help='The optimizer used by the captured test command, if any')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    recipes, optimizations, cases = read_corpus(args.corpus)
    groups = defaultdict(list)
    for code_hash in cases:
        if code_hash not in recipes:
            raise ValueError('a tested module has no owning contract recipe')
        groups[recipes[code_hash]['project']].append(code_hash)
    projects = json.loads((args.corpus / 'projects.json').read_text())
    if set(groups) != set(projects.values()):
        raise ValueError('the complete example project set was not observed')
    environment = os.environ.copy()
    for key in list(environment):
        if key.startswith(('AVM_MEMORY_TEST_', 'AVM_COMBINED_')):
            environment.pop(key)
    environment.pop('AVM_MEMORY_REPORT', None)
    for name in BUILD_INPUTS:
        environment.pop(name, None)
    probes, results = [], []

    def run(label, command, env):
        process = subprocess.run(list(map(str, command)), env=env, capture_output=True)
        (args.output / (label + '.stdout')).write_bytes(process.stdout)
        (args.output / (label + '.stderr')).write_bytes(process.stderr)
        return process

    for project in sorted(groups):
        hashes = groups[project]
        explicit = tomllib.loads((args.sdk / project / 'avm-contract.toml').read_text()).get('build', {})
        required = max((len(row['input']) + len(row['output'])) // 2 for h in hashes for row in cases[h]
                       if row['vm_status'] not in (23, 42, 46))
        arena = explicit.get('arena_bytes', required)
        maximum = explicit.get('max_memory_bytes', max(memory_limits((args.corpus / 'modules' / (h + '.wasm')).read_bytes())[1] for h in hashes))
        project_dir = args.output / project.removeprefix('examples/').replace('/', '_')
        project_dir.mkdir()

        def probe(stack, selected_arena=arena, selected_maximum=maximum):
            serial = len(probes)
            row = dict(project=project, stack_bytes=stack, arena_bytes=selected_arena,
                       max_memory_bytes=selected_maximum, passed=True)
            peak, initial, effective_arena = 0, 0, None
            built = {}
            tested_hashes = set()
            for h in hashes:
                optimization = optimizations.get(h)
                original = optimization['input_hash'] if optimization else h
                recipe = recipes[original]
                manifest = Path(recipe['manifest'])
                key = (str(manifest), json.dumps(recipe.get('environment', {}), sort_keys=True))
                env = environment.copy()
                env.update(recipe.get('environment', {}))
                if key not in built:
                    configuration = manifest.with_name('avm-contract.toml')
                    original_configuration = configuration.read_bytes()
                    label = f'{serial}-{len(built)}-build'
                    try:
                        configure(configuration, stack, selected_arena, selected_maximum)
                        process = run(label, [args.builder, 'build', '--for-testing', '--manifest-path', manifest], env)
                    finally:
                        configuration.write_bytes(original_configuration)
                    if process.returncode:
                        raise ValueError(f'contract recipe failed before stack qualification: {label}.stderr')
                    module = project_dir / f'{serial}-{len(built)}.wasm'
                    shutil.copyfile(manifest.parent / 'target/avm/contract.wasm', module)
                    provenance = (manifest.parent / 'target/avm-build-provenance.txt').read_text()
                    capacity = int(re.search(r'^arena_bytes: (\d+)$', provenance, re.MULTILINE)[1])
                    effective_arena = capacity if effective_arena is None else max(effective_arena, capacity)
                    built[key] = module
                module = built[key]
                if optimization:
                    if args.optimizer is None:
                        raise ValueError('the captured optimization recipe requires --optimizer')
                    optimized = project_dir / f'{serial}-{h[:12]}.wasm'
                    process = run(f'{serial}-{h[:12]}-optimize', [args.optimizer, module, *optimization['flags'], '-o', optimized], env)
                    if process.returncode:
                        raise ValueError('the recorded optimizer recipe failed')
                    module = optimized
                tested_hashes.add(blake3.blake3(module.read_bytes()).hexdigest())
                native = [case for case in cases[h] if not case.get('direct')]
                if native:
                    test_file = project_dir / f'{h}.native.jsonl'
                    test_file.write_text(''.join(json.dumps(case) + '\n' for case in native))
                    label = f'{serial}-{h[:12]}-native'
                    process = run(label, [args.stand, 'check-contract-tests', '--wasm', module,
                        '--manifest-path', manifest, '--tests', test_file, '--wasmtime-profile', args.profile], env)
                    if process.returncode:
                        row.update(passed=False, failure='native-test', log=label + '.stdout')
                        break
                    usage = json.loads(process.stdout)
                    peak, initial = max(peak, usage['peak_memory_bytes']), max(initial, usage['initial_memory_bytes'])
                direct = [case for case in cases[h] if case.get('direct')]
                if direct:
                    vector_file = project_dir / f'{h}.direct.tsv'
                    vector_file.write_text(''.join(f"case-{index}\t{case['entrypoint']}\t{case['input']}\t{case['output']}\n" for index, case in enumerate(direct)))
                    label = f'{serial}-{h[:12]}-direct'
                    env['AVM_MEMORY_VECTOR_STATS'] = '1'
                    process = run(label, [sys.executable, '-B', args.sdk / 'examples/revenue-hub/scripts/guest_modes.py',
                        module, vector_file, project_dir / f'{serial}-direct.json'], env)
                    if process.returncode:
                        row.update(passed=False, failure='direct-test', log=label + '.stdout')
                        break
                    usage = re.findall(rb'^AVM_MEMORY_USAGE \S+ (\d+) (\d+) (\d+) (\d+)$', process.stdout, re.MULTILINE)
                    if len(usage) != len(direct) or any(int(value[2]) for value in usage):
                        raise ValueError('incomplete direct resource observations')
                    initial = max(initial, max(int(value[0]) for value in usage))
                    peak = max(peak, max(int(value[1]) for value in usage))
            row.update(initial_memory_bytes=initial, peak_memory_bytes=peak, effective_arena_bytes=effective_arena,
                       tested_wasm_hashes=sorted(tested_hashes))
            probes.append(row)
            (args.output / 'probes.json').write_text(json.dumps(probes, indent=2) + '\n')
            (args.output / 'progress.json').write_text(json.dumps(row) + '\n')
            return row

        if 'stack_bytes' in explicit:
            stack = explicit['stack_bytes']
        else:
            high = 16384
            while not probe(high)['passed']:
                high *= 2
                if high > maximum:
                    raise ValueError('the full corpus does not fit the declared memory limit')
            low = 0
            while high - low > 16:
                middle = ((high + low) // 32) * 16
                if probe(middle)['passed']:
                    high = middle
                else:
                    low = middle
            stack = high
        qualified = probe(stack)
        if not qualified['passed']:
            raise ValueError('explicit or selected resources fail contract tests')
        lower_stack_failed = None
        if 'stack_bytes' not in explicit and stack > 16:
            lower_stack_failed = not probe(stack - 16)['passed']
            if not lower_stack_failed:
                raise ValueError('the selected stack lacks a failing lower-size witness')
        if qualified['effective_arena_bytes'] != arena:
            arena = qualified['effective_arena_bytes']
            arena_source = 'macro'
        else:
            arena_source = 'explicit' if 'arena_bytes' in explicit else 'tests'
        lower_arena_failed = None
        if arena_source == 'tests' and arena:
            smaller = probe(stack, arena - 1)
            if smaller['effective_arena_bytes'] == arena:
                arena_source = 'macro'
            else:
                lower_arena_failed = not smaller['passed']
            if lower_arena_failed is False:
                raise ValueError('the selected arena lacks a failing lower-size witness')
        declared = explicit.get('max_memory_bytes', max(qualified['initial_memory_bytes'], qualified['peak_memory_bytes']))
        declared = ((declared + 65535) // 65536) * 65536
        qualified = probe(stack, arena, declared)
        if not qualified['passed']:
            raise ValueError('page-rounded deployment maximum fails contract tests')
        record = dict(version=2, stack_bytes=stack, arena_bytes=arena,
            initial_memory_bytes=qualified['initial_memory_bytes'], peak_memory_bytes=qualified['peak_memory_bytes'],
            test_calls=sum(len(cases[h]) for h in hashes), test_wasm_hashes=qualified['tested_wasm_hashes'])
        results.append(dict(project=project, **record, lower_stack_failed=lower_stack_failed,
                            lower_arena_failed=lower_arena_failed, arena_source=arena_source))
        (project_dir / 'avm-memory.json').write_text(json.dumps(record, indent=2) + '\n')
        (args.output / 'result.json').write_text(json.dumps(dict(passed=True, examples=results), indent=2) + '\n')
    for row in results:
        record = {key: value for key, value in row.items() if key in ('version', 'stack_bytes', 'arena_bytes', 'initial_memory_bytes', 'peak_memory_bytes', 'test_calls', 'test_wasm_hashes')}
        (args.sdk / row['project'] / 'avm-memory.json').write_text(json.dumps(record, indent=2) + '\n')
    print(f'PASS: measured stack, live arena and page-rounded memory for {len(results)} contracts')


if __name__ == '__main__':
    main()
