#!/usr/bin/env python3
"""Run each bundled contract's real test scenarios and record its memory maximum."""

import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tomllib


def copy_sdk_source(source, destination):
    def ignored(directory, names):
        excluded = {name for name in names if name in (".git", "target", "__pycache__")}
        if Path(directory).resolve() == source.resolve() and "bin" in names:
            excluded.add("bin")
        return excluded
    shutil.copytree(source, destination, ignore=ignored)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("sdk", "builder", "stand", "profile", "optimizer", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--collect-only", action="store_true", help="Retain the complete corpus without replacing project measurements")
    parser.add_argument("--deployment", action="store_true", help="Exercise deployment builds with committed resource measurements")
    args = parser.parse_args()
    if args.deployment and not args.collect_only:
        parser.error("use --collect-only with --deployment to preserve the selected resource measurements")
    source, output = args.sdk.resolve(), args.output.resolve()
    if output.is_relative_to(source):
        raise ValueError("measurement output must be outside the SDK source tree")
    output.mkdir(parents=True, exist_ok=False)
    sdk = output / "consumer"
    copy_sdk_source(source, sdk)
    projects = {}
    for path in sorted((sdk / "examples").rglob("avm-contract.toml")):
        if "target" in path.relative_to(sdk).parts:
            continue
        name = tomllib.loads(path.with_name("Cargo.toml").read_text())["package"]["name"]
        if name in projects:
            raise ValueError("ambiguous contract package name")
        projects[name] = path.parent.relative_to(sdk).as_posix()
    if len(projects) != 17:
        raise ValueError("expected all 17 example contracts")
    (output / "projects.json").write_text(json.dumps(projects, indent=2) + "\n")
    report, builds = output / "memory-runs.jsonl", output / "builds.jsonl"
    report.write_text("")
    builds.write_text("")
    environment = os.environ.copy()
    if args.collect_only and not args.deployment:
        # Type-checking guest-only tests needs a capacity as well. The builder
        # owns real artifact capacities and overrides this test environment.
        environment["AVM_GUEST_ARENA_BYTES"] = "8388624"
    if args.deployment:
        environment["AVM_MEMORY_TEST_DEPLOYMENT"] = "1"
    environment.update(AVM_MEMORY_REPORT=str(report), AVM_MEMORY_VECTOR_STATS="1",
        AVM_MEMORY_TEST_PROJECTS=str(output / "projects.json"), AVM_MEMORY_TEST_BUILDS=str(builds),
        AVM_MEMORY_TEST_BUILDER=str(args.builder.resolve()), AVM_MEMORY_TEST_STAND=str(args.stand.resolve()),
        AVM_MEMORY_TEST_OPTIMIZER=str(args.optimizer.resolve()), AVM_MEMORY_TEST_VECTORS=str(output / "test-vectors"))
    driver = sdk / "scripts/tests/memory_test_driver.py"
    (sdk / "bin").mkdir(exist_ok=True)
    for mode, name in (("builder", "avm-contract-build"), ("stand", "avm-local-stand"), ("optimizer", "wasm-opt")):
        path = sdk / "bin" / name
        if path.is_symlink():
            path.unlink()
        path.write_text("#!/bin/sh\nexec " + shlex.quote(sys.executable) + " -B " + shlex.quote(str(driver)) + " " + mode + ' "$@"\n')
        path.chmod(0o755)
    environment.update(AVM_CONTRACT_BUILD=str(sdk / "bin/avm-contract-build"),
        AVM_LOCAL_STAND=str(sdk / "bin/avm-local-stand"), AVM_WASMTIME_PROFILE=str(args.profile.resolve()),
        PATH=str(sdk / "bin") + os.pathsep + environment["PATH"])
    commands = []

    def run(label, command, cwd=sdk, extra=None):
        env = environment.copy()
        if extra:
            env.update({key: str(value) for key, value in extra.items()})
        command = [str(value) for value in command]
        process = subprocess.run(command, cwd=cwd, env=env, capture_output=True)
        (output / (label + ".stdout")).write_bytes(process.stdout)
        (output / (label + ".stderr")).write_bytes(process.stderr)
        commands.append({"label": label, "argv": command, "exit": process.returncode})
        (output / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        if process.returncode:
            raise ValueError(f"{label} failed; see retained stdout/stderr")

    for relative in projects.values():
        run("initial-build-" + relative.replace("/", "_"), [sdk / "bin/avm-contract-build", "build", "--manifest-path", sdk / relative / "Cargo.toml"])
    run("source-contract-scenarios", [sys.executable, "-B", sdk / "scripts/tests/packaged_lifecycle.py", "--bundle", sdk, "--evidence", output / "source-contract-scenarios", "--source-deployment" if args.deployment else "--source"])
    run("contract-tools", [sys.executable, "-B", sdk / "scripts/tests/sdk_issue_public_paths.py", "--sdk", sdk,
        "--builder", sdk / "bin/avm-contract-build", "--stand", sdk / "bin/avm-local-stand", "--profile", args.profile.resolve(), "--out", output / "contract-tools"])
    run("deploy-callback", ["bash", sdk / "examples/deploy-callback-repro/repro.sh", "--builder", sdk / "bin/avm-contract-build",
        "--stand", sdk / "bin/avm-local-stand", "--profile", args.profile.resolve(), "--out", output / "deploy-callback"])
    for name in ("bounty-escrow", "revenue-hub"):
        command = ["bash", sdk / "examples" / name / "scripts/check.sh", "--sdk", sdk, "--out", output / name]
        if name == "revenue-hub":
            command += ["--optimizer", sdk / "bin/wasm-opt"]
        run(name, command)
    run("vesting-streams", ["bash", sdk / "examples/vesting-streams/scripts/check.sh", sdk, sdk / "bin/wasm-opt"],
        extra={"AVM_EVIDENCE_DIR": output / "vesting-streams"})
    run("fungible-token", ["bash", sdk / "examples/fungible-token/stand-e2e.sh"], extra={"AVM_TOKEN_WORK": output / "fungible-token"})
    run("factory-child", ["bash", sdk / "examples/factory-child-provenance/prove.sh"],
        extra={"PROVENANCE_OUT": output / "factory-child", "PROVENANCE_KEEP": "1"})
    ownership = {}
    for line in builds.read_text().splitlines():
        event = json.loads(line)
        ownership.setdefault(event["code_hash"], set()).add(event["project"])
    observed = {project: set() for project in projects.values()}
    for line in report.read_text().splitlines():
        batch = json.loads(line)
        if batch.get("complete") is not True:
            raise ValueError("incomplete memory capture")
        for event in batch["runs"]:
            owners = ownership.get(event["code_hash"], set())
            if len(owners) != 1:
                raise ValueError(f"unknown or ambiguous tested CodeHash: {event['code_hash']}")
            observed[next(iter(owners))].add(event["code_hash"])
    results = []
    if args.collect_only:
        (output / "result.json").write_text(json.dumps({"passed": True, "mode": "complete-test-corpus", "observed": {key: sorted(value) for key, value in observed.items()}}, indent=2) + "\n")
        return
    for project, hashes in observed.items():
        if not hashes:
            raise ValueError(f"no real test invocation for {project}")
        command = [args.builder.resolve(), "record-memory", "--manifest-path", sdk / project / "Cargo.toml", "--report", report]
        for code_hash in sorted(hashes):
            command += ["--code-hash", code_hash]
        run("record-" + project.replace("/", "_"), command)
        record = json.loads((sdk / project / "avm-memory.json").read_text())
        results.append({"project": project, **record})
    (output / "result.json").write_text(json.dumps({"passed": True, "examples": results}, indent=2) + "\n")
    for project in projects.values():
        shutil.copy2(sdk / project / "avm-memory.json", source / project / "avm-memory.json")
    print("PASS: all 17 examples measured through their own complete test scenarios")


if __name__ == "__main__":
    main()
