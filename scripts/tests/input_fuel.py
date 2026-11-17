#!/usr/bin/env python3
"""Check that the FUEL of an SDK entrypoint call does not grow with its input.

The AVM host copies the whole input into guest memory outside FUEL: it calls
`avm_guest_alloc(input.len)`, writes the input, then calls the entrypoint
(AVM executor call flow, `docs/unsafe.md`). The SDK's input buffer must
therefore cost only its allocation. The fixture's probe runs the same
length-independent code at every data size, so between two probes with input
lengths L1 < L2 the guest may pay only for the extra linear-memory pages:

    allowed = memory_grow_per_page * (ceil(L2 / 65536) - floor(L1 / 65536) + 2)
              + ALLOCATOR_BRANCHES

`memory_grow_per_page` is read from the profile's `variable_operator_costs`.
The two extra pages cover the allocator's chunk header and page rounding of
each call. ALLOCATOR_BRANCHES covers the allocator's length-independent paths
for two different request sizes. Any per-byte work on the buffer, even one
`memory.fill` at `memory_fill_per_byte`, exceeds the bound by orders of
magnitude.
"""

import argparse
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

PAGE = 65_536
ALLOCATOR_BRANCHES = 5_000
PROBE = 0x8000_0000
# The data sizes of the two probes: the smallest DataPatchList account and
# about 1 MiB, within the 1048576-byte input and patch-size limits.
GROWS = [(65_504,), (262_144, 262_144, 262_144, 188_032)]
SIZES = [65_536, 1_040_000]


def profile_cost(profile, name):
    for line in profile.read_text().splitlines():
        if line.startswith("variable_operator_costs="):
            costs = dict(pair.split(":") for pair in line.split("=", 1)[1].split(","))
            return int(costs[name])
    raise SystemExit("the profile has no variable_operator_costs")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("sdk", "builder", "stand", "profile", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    grow_page = profile_cost(args.profile, "memory_grow_per_page")
    commands = []
    now = 1_800_000_000
    with tempfile.TemporaryDirectory(prefix="avm-input-fuel-") as temporary:
        consumer = Path(temporary) / "consumer"
        shutil.copytree(args.sdk / "crates/avm-contract-sdk", consumer / "crates/avm-contract-sdk", ignore=shutil.ignore_patterns("target"))
        for name in ("Cargo.toml", "Cargo.lock", "LICENSE.md"):
            shutil.copy2(args.sdk / name, consumer / name)
        fixture = consumer / "scripts/tests/fixtures/input-fuel"
        shutil.copytree(args.sdk / "scripts/tests/fixtures/input-fuel", fixture, ignore=shutil.ignore_patterns("target"))
        executions = Path(temporary) / "executions.tsv"
        environment = os.environ.copy()
        environment["AVM_COMBINED_EXECUTION_PATH"] = str(executions)
        state = Path(temporary) / "developer.state"

        def run(argv, label, expected=0):
            process = subprocess.run([str(value) for value in argv], capture_output=True, env=environment)
            (args.output / (label + ".stdout")).write_bytes(process.stdout)
            (args.output / (label + ".stderr")).write_bytes(process.stderr)
            commands.append({"argv": [str(value) for value in argv], "exit": process.returncode})
            (args.output / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
            assert process.returncode == expected, process.stderr.decode() + process.stdout.decode()
            return process

        def stand(name, label, expected=0, **options):
            argv = [args.stand.resolve(), name, "--state", state, "--wasmtime-profile", args.profile.resolve()]
            for key, value in options.items():
                argv.append("--" + key.replace("_", "-"))
                if value is not True:
                    argv.append(str(value))
            process = run(argv, label, expected)
            return json.loads(process.stdout)

        def timing():
            nonlocal now
            now += 1
            return dict(block_time=now, created_at=now, expire_at=now + 3600, fuel_fee_shells=100)

        def call(label, argument):
            stand("call", label, address=address, method="input.fuel", args_json=str(argument), identity="user", **timing())
            fields = dict(item.split("=", 1) for item in executions.read_text().splitlines()[-1].split("\t"))
            assert fields["entrypoint"] == "process_message" and fields["result"] == "ok", label
            assert fields["vm_status"] == "0", (label, fields["vm_status"], fields["output"][:64])
            return len(fields["input"]) // 2, int(fields["fuel_used"])

        run([args.builder.resolve(), "build", "--manifest-path", fixture / "Cargo.toml", "--for-testing"], "build")
        artifacts = fixture / "target/avm"
        stand("init", "init", dapp_id="11" * 32)
        options = dict(wasm=artifacts / "contract.wasm", acab=artifacts / "contract.acab", descriptor=artifacts / "contract.descriptor",
                       salt_hex="00", init_json="null", **timing())
        address = stand("deploy", "deploy-precondition", expected=2, **options)["address"]
        stand("fund-user", "fund", address=address, shells=1_000_000_000)
        stand("deploy", "deploy", **options)
        probes = []
        for index, (grows, size) in enumerate(zip(GROWS, SIZES)):
            for step, length in enumerate(grows):
                call(f"grow-{index}-{step}", length)
            input_length, fuel = call(f"probe-{index}", PROBE | size)
            probes.append({"data_bytes": size, "input_bytes": input_length, "fuel_used": fuel})
        (small, large) = probes
        pages = math.ceil(large["input_bytes"] / PAGE) - small["input_bytes"] // PAGE + 2
        allowed = grow_page * pages + ALLOCATOR_BRANCHES
        delta = large["fuel_used"] - small["fuel_used"]
        result = {"probes": probes, "memory_grow_per_page": grow_page, "allowed_pages": pages,
                  "allocator_branches": ALLOCATOR_BRANCHES, "allowed_delta": allowed,
                  "fuel_delta": delta, "passed": delta <= allowed}
        (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result))
        assert result["passed"], f"input FUEL grows with the input: delta {delta} > allowed {allowed}"


if __name__ == "__main__":
    main()
