#!/usr/bin/env python3
"""Capture an existing contract test command for deterministic resource selection."""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('contract', 'builder', 'stand', 'profile', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--optimizer', type=Path, help='Capture wasm-opt calls made through PATH')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('provide the complete contract test command after --')
    contract = args.contract.resolve(strict=True)
    args.output = args.output.resolve()
    if args.output.is_relative_to(contract):
        parser.error('capture output must be outside the contract project')
    args.output.mkdir(parents=True, exist_ok=False)
    package = tomllib.loads((contract / 'Cargo.toml').read_text())['package']['name']
    (args.output / 'projects.json').write_text(json.dumps({package: contract.name}) + '\n')
    for name in ('builds.jsonl', 'memory-runs.jsonl'):
        (args.output / name).write_text('')
    tools = args.output / 'bin'
    tools.mkdir()
    driver = Path(__file__).with_name('memory_test_driver.py').resolve()
    wrappers = [('builder', 'avm-contract-build'), ('stand', 'avm-local-stand')]
    if args.optimizer:
        wrappers.append(('optimizer', 'wasm-opt'))
    for mode, name in wrappers:
        wrapper = tools / name
        wrapper.write_text('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' -B ' + shlex.quote(str(driver)) + ' ' + mode + ' "$@"\n')
        wrapper.chmod(0o755)
    environment = os.environ.copy()
    environment.update(AVM_MEMORY_VECTOR_STATS='1', AVM_GUEST_ARENA_BYTES='8388624',
        AVM_MEMORY_REPORT=str(args.output / 'memory-runs.jsonl'),
        AVM_MEMORY_TEST_PROJECTS=str(args.output / 'projects.json'),
        AVM_MEMORY_TEST_BUILDS=str(args.output / 'builds.jsonl'),
        AVM_MEMORY_TEST_VECTORS=str(args.output / 'test-vectors'),
        AVM_MEMORY_TEST_BUILDER=str(args.builder.resolve(strict=True)),
        AVM_MEMORY_TEST_STAND=str(args.stand.resolve(strict=True)),
        AVM_CONTRACT_BUILD=str(tools / 'avm-contract-build'), AVM_LOCAL_STAND=str(tools / 'avm-local-stand'),
        AVM_WASMTIME_PROFILE=str(args.profile.resolve(strict=True)),
        PATH=str(tools) + os.pathsep + environment['PATH'])
    environment.pop('AVM_MEMORY_TEST_DEPLOYMENT', None)
    if args.optimizer:
        environment['AVM_MEMORY_TEST_OPTIMIZER'] = str(args.optimizer.resolve(strict=True))
    for label, argv in (('build', [tools / 'avm-contract-build', 'build', '--manifest-path', contract / 'Cargo.toml']), ('tests', command)):
        result = subprocess.run(list(map(str, argv)), cwd=contract, env=environment, capture_output=True)
        (args.output / (label + '.stdout')).write_bytes(result.stdout)
        (args.output / (label + '.stderr')).write_bytes(result.stderr)
        if result.returncode:
            raise SystemExit(f'{label} failed; inspect the capture output')
    print('PASS: test command captured; select resources and rerun the complete command on deployment artifacts')


if __name__ == '__main__':
    main()
