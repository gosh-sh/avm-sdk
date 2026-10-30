#!/usr/bin/env python3
"""Independent unbounded-integer reference for checked Rust arithmetic."""
import argparse
from pathlib import Path
import random

ROOT = Path(__file__).resolve().parents[1]
def render():
    rng = random.Random(0x765714)
    lines = []
    for i in range(1200):
        start = rng.randint(1, 2**32 - 20000)
        end = start + rng.randint(1, 19999)
        cliff = rng.randint(start, end)
        step = rng.choice([0, 1, end-start, rng.randint(1, end-start)])
        total = rng.choice([1, 2, 10, 2**128-1, rng.randint(1, 2**128-1)])
        now = [start-1, start, cliff-1, cliff, end-1, end, end+1,
               rng.randint(start, end)][i % 8]
        if now < cliff or now <= start:
            expected = 0
        elif now >= end:
            expected = total
        else:
            elapsed = now-start
            if step:
                elapsed = elapsed // step * step
            expected = total * elapsed // (end-start)
        lines.append(','.join(map(str, [total,start,cliff,end,step,now,expected])))
    return '\n'.join(lines)+'\n'
if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    path = ROOT/'tests/fixtures/python-bigint-vesting.csv'
    if args.check:
        assert path.read_text() == render(), 'Oracle fixture differs from seeded generator'
        print('Independent Python bigint fixture reproducibility: PASS')
    else:
        path.write_text(render())
