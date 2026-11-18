#!/usr/bin/env python3
"""Independent arithmetic oracle for contracts/amm/src/math.rs.

Python integers are unbounded, so every expected value below is computed
without any of the contract's 256-bit machinery. The output is deterministic
(fixed seed); `--check` fails if the committed vectors differ.

Line format (tab separated): op arg... expected
  mul_div_floor / mul_div_ceil  a b c    -> value | none
  sqrt_product                  a b 0    -> value
  (the swap_* lines carry four arguments before the expected value)
  swap_out   reserve_in reserve_out amount_in  fee_bps -> value | invalid | overflow
  swap_in    reserve_in reserve_out amount_out fee_bps -> value | invalid | liquidity | overflow
"""
import argparse
import math
import pathlib
import random
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = ROOT / "tests/vectors/math.tsv"
U128 = (1 << 128) - 1
BPS = 10_000

EDGES = [0, 1, 2, 3, 7, 999, 1000, 1001, 10_000, (1 << 32) - 1, 1 << 32, (1 << 63),
         (1 << 64) - 1, 1 << 64, (1 << 64) + 1, (1 << 127) - 1, 1 << 127, (1 << 127) + 1,
         U128 - 1, U128]


def sample(rng: random.Random) -> int:
    kind = rng.random()
    if kind < 0.25:
        return rng.choice(EDGES)
    bits = rng.randint(1, 128)
    return rng.getrandbits(bits)


def ceil_div(n: int, d: int) -> int:
    return -(-n // d)


def mul_div(op: str, a: int, b: int, c: int) -> str:
    if c == 0:
        return "none"
    value = (a * b) // c if op == "floor" else ceil_div(a * b, c)
    return str(value) if value <= U128 else "none"


def swap_out(r_in: int, r_out: int, amount_in: int, fee: int) -> str:
    if amount_in == 0 or r_in == 0 or r_out == 0 or fee >= BPS:
        return "invalid"
    in_f = amount_in * (BPS - fee) // BPS
    denominator = r_in + in_f
    if denominator > U128:
        return "overflow"
    return str(r_out * in_f // denominator)


def swap_in(r_in: int, r_out: int, amount_out: int, fee: int) -> str:
    if amount_out == 0 or r_in == 0 or r_out == 0 or fee >= BPS:
        return "invalid"
    if amount_out >= r_out:
        return "liquidity"
    without_fee = ceil_div(r_in * amount_out, r_out - amount_out)
    if without_fee > U128:
        return "overflow"
    total = ceil_div(without_fee * BPS, BPS - fee)
    return str(total) if total <= U128 else "overflow"


def render() -> str:
    rng = random.Random(0xA3_1CE_5EED)
    lines = []
    per_op = 350
    for _ in range(per_op):
        a, b, c = sample(rng), sample(rng), sample(rng)
        lines.append(f"mul_div_floor\t{a}\t{b}\t{c}\t{mul_div('floor', a, b, c)}")
        lines.append(f"mul_div_ceil\t{a}\t{b}\t{c}\t{mul_div('ceil', a, b, c)}")
        a, b = sample(rng), sample(rng)
        lines.append(f"sqrt_product\t{a}\t{b}\t0\t{math.isqrt(a * b)}")
    fees = [0, 1, 5, 30, 100, 300, 999, 1000]
    for _ in range(per_op):
        r_in, r_out, amount = sample(rng), sample(rng), sample(rng)
        fee = rng.choice(fees)
        lines.append(f"swap_out\t{r_in}\t{r_out}\t{amount}\t{fee}\t{swap_out(r_in, r_out, amount, fee)}")
        lines.append(f"swap_in\t{r_in}\t{r_out}\t{amount}\t{fee}\t{swap_in(r_in, r_out, amount, fee)}")
    # Realistic pools: mid-size reserves, small trades, every fee tier.
    for _ in range(per_op):
        r_in = rng.randint(10**3, 10**18)
        r_out = rng.randint(10**3, 10**18)
        amount = rng.randint(1, max(1, r_in // rng.choice([1, 2, 10, 100, 1000])))
        fee = rng.choice(fees)
        lines.append(f"swap_out\t{r_in}\t{r_out}\t{amount}\t{fee}\t{swap_out(r_in, r_out, amount, fee)}")
        out = rng.randint(1, max(1, r_out - 1))
        lines.append(f"swap_in\t{r_in}\t{r_out}\t{out}\t{fee}\t{swap_in(r_in, r_out, out, fee)}")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    text = render()
    if args.check:
        if OUT.read_text() != text:
            sys.exit("vectors are stale; run scripts/gen_vectors.py")
        print(f"{text.count(chr(10))} oracle vectors match: PASS")
    else:
        OUT.write_text(text)
        print(f"wrote {text.count(chr(10))} vectors to {OUT.relative_to(ROOT)}")
