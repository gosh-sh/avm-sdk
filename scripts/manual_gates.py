#!/usr/bin/env python3
"""Run accumulated SDK release gates on a bounded qualification host."""

import argparse
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib

from release import file_digest, parse_profile, require


def tree_engine_features(text):
    entries = [line.split("|", 1)[1].removesuffix(" (*)").split(",")
               for line in text.splitlines() if line.startswith("wasmtime v36.0.12|")]
    require(entries and all(set(item) == set(entries[0]) for item in entries),
            "missing or ambiguous resolved Wasmtime graph")
    return sorted(entries[0])


def compiled_engine_features(text):
    records = [json.loads(line) for line in text.splitlines() if line.startswith('{"reason":')]
    require(any(row.get("reason") == "build-finished" and row.get("success") for row in records),
            "engine graph compilation did not complete")
    entries = [row for row in records if row.get("reason") == "compiler-artifact"
               and row.get("target", {}).get("name") == "wasmtime"]
    require(len(entries) == 1 and "wasmtime@36.0.12" in entries[0]["package_id"],
            "missing or ambiguous compiled Wasmtime artifact")
    return sorted(entries[0]["features"])


def head(root):
    return subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()


def clean(root):
    status = subprocess.check_output(["git", "-C", str(root), "status", "--porcelain", "--untracked-files=all"])
    if status:
        try:
            diagnostic = subprocess.check_output(
                ["git", "-C", str(root), "status", "--porcelain", "-z", "--untracked-files=all"],
                stderr=subprocess.DEVNULL).decode("utf-8", errors="surrogateescape")
        except (OSError, subprocess.CalledProcessError):
            diagnostic = "[raw Git path diagnostics unavailable]"
        # Mask recognized values in raw path records before escaping control
        # bytes or Unicode. Git's C-quoted spellings are not redaction-safe.
        for name, value in os.environ.items():
            if len(value) >= 8 and re.search(r"TOKEN|SECRET|PASSWORD|CREDENTIAL|AUTHORIZATION|COOKIE|PRIVATE_KEY", name, re.I):
                diagnostic = diagnostic.replace(value, "[REDACTED]")
        rendered = "\n".join(json.dumps(record, ensure_ascii=True)[1:-1]
                             for record in diagnostic.split("\x00") if record)
        require(not status, f"dirty source: {root}\n"
                "git status --porcelain -z --untracked-files=all (bounded):\n"
                + _failure_log_tail(rendered))


def normalize_scratch_environment():
    selected = next((os.environ[key] for key in ("TMPDIR", "TMP", "TEMP") if os.environ.get(key)), None)
    scratch = Path(selected if selected is not None else tempfile.gettempdir())
    require(scratch.is_absolute() and scratch.is_dir() and os.access(scratch, os.W_OK),
            "temporary directory must be an existing absolute writable directory")
    scratch = scratch.resolve()
    for key in ("TMPDIR", "TMP", "TEMP"):
        os.environ[key] = str(scratch)
    return scratch


def preflight_environment(scratch):
    environment = os.environ.copy()
    if not environment.get("AVM_TMPDIR"):
        environment["AVM_TMPDIR"] = str(scratch)
    return environment


def example_environment(source, manifest, evidence, run):
    environment = os.environ.copy()
    environment.pop("AVM_CONTRACT_BINDINGS", None)
    metadata = manifest.parent / "avm-contract.toml"
    bindings_config = tomllib.loads(metadata.read_text()).get("bindings", {}) if metadata.is_file() else {}
    if bindings_config.get("enabled"):
        bindings = evidence / (manifest.parent.name + "-bindings.rs")
        command = ["cargo", "run", "--quiet", "--locked", "--offline",
            "-p", "avm-contract-build", "--bin", "avm-contract-build", "--", "bindings",
            "--manifest-path", str(manifest)]
        if bindings_config.get("dispatch"):
            command.append("--dispatch")
        run(manifest.parent.name + "-bindings", command, source, stdout_file=bindings)
        environment["AVM_CONTRACT_BINDINGS"] = str(bindings)
    return environment


def _failure_log_tail(text, *, environment=None, max_lines=40, max_bytes=8192):
    """Bound diagnostic output and redact known secrets and common token formats."""
    text = re.sub(r"\x1b\][^\x07]*(?:\x07|\x1b\\)", "", text)
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    text = re.sub(r"[\x00-\x08\x0b-\x1f\x7f]", "", text)
    for name, value in (os.environ if environment is None else environment).items():
        if len(value) >= 8 and re.search(r"TOKEN|SECRET|PASSWORD|CREDENTIAL|AUTHORIZATION|COOKIE|PRIVATE_KEY", name, re.I):
            text = text.replace(value, "[REDACTED]")
    text = re.sub(r"-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----.*?-----END \1-----",
                  "[REDACTED_PRIVATE_KEY]", text, flags=re.S)
    text = re.sub(r"(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|"
                  r"eyJ[A-Za-z0-9_-]{15,}\.[A-Za-z0-9_-]{15,}\.[A-Za-z0-9_-]+)", "[REDACTED]", text)
    text = re.sub(r"(?im)^([^\n]*\b(?:authorization|cookie)\s*[:=]\s*)[^\r\n]*", r"\1[REDACTED]", text)
    text = re.sub(r"(?i)\b((?:[A-Z][A-Z0-9_]*_)?(?:TOKEN|PASSWORD|SECRET|CREDENTIAL|PRIVATE_KEY)\s*[:=]\s*)"
                  r"(?:\"[^\"]*\"|'[^']*'|[^\s]+)", r"\1[REDACTED]", text)
    tail = "\n".join(text.splitlines()[-max_lines:])
    return tail.encode("utf-8")[-max_bytes:].decode("utf-8", errors="ignore")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--optimizer", type=Path, help="checksum-pinned Binaryen 133 wasm-opt")
    args = parser.parse_args()
    source, sdk, evidence = args.source.resolve(), args.sdk.resolve(), args.evidence.resolve()
    require(os.environ.get("AVM_SDK_QUALIFICATION") == "1"
            or os.environ.get("AVM_SDK_POD_QUALIFICATION") == "1", "use a bounded qualification launcher")
    require(len(os.sched_getaffinity(0)) == 2, "qualification requires exactly 2 allowed CPUs")
    require(os.environ.get("CARGO_BUILD_JOBS") == "2", "CARGO_BUILD_JOBS must be 2")
    require(args.optimizer is not None and args.optimizer.is_file()
            and os.access(args.optimizer, os.X_OK), "pass --optimizer with checksum-pinned Binaryen 133 wasm-opt")
    optimizer = args.optimizer.resolve()
    scratch = normalize_scratch_environment()
    clean(source)
    clean(sdk)
    evidence.mkdir(parents=True, exist_ok=False)
    result = {"schema": "gosh.avm-sdk.gates.v1", "source_commit": head(source),
              "sdk_commit": head(sdk), "passed": False, "gates": []}

    def run(label, argv, cwd=source, tests=False, exact=None, python_tests=False,
            environment=None, stdout_file=None):
        log = evidence / (label + ".log")
        (evidence / "current.json").write_text(json.dumps({"gate": label, "pid": os.getpid(), "completed": len(result["gates"]), "source_commit": result["source_commit"], "sdk_commit": result["sdk_commit"]}) + "\n")
        with log.open("wb") as stream:
            if stdout_file is None:
                process = subprocess.run(argv, cwd=cwd, env=environment, stdout=stream, stderr=subprocess.STDOUT)
            else:
                with stdout_file.open("wb") as generated:
                    process = subprocess.run(argv, cwd=cwd, env=environment, stdout=generated, stderr=stream)
        text = log.read_text(errors="replace")
        counts = [int(n) for n in re.findall(r"test result: ok\. (\d+) passed", text)]
        matched = sum(counts)
        passed = process.returncode == 0 and (not tests or matched > 0)
        if python_tests:
            count = re.search(r"Ran (\d+) tests? in", text)
            passed = passed and count is not None and int(count[1]) > 0
        if exact:
            passed = passed and re.search(r"test " + re.escape(exact) + r" \.{3} ok", text) is not None
        result["gates"].append({"name": label, "argv": argv, "cwd": str(cwd),
                                "exit_code": process.returncode, "rust_tests_passed": matched,
                                "exact_selector": exact, "passed": passed,
                                "log_sha256": file_digest(log)})
        if stdout_file is not None:
            result["gates"][-1]["stdout_file"] = stdout_file.name
            result["gates"][-1]["stdout_sha256"] = file_digest(stdout_file)
        (evidence / "gates.json").write_text(json.dumps(result, indent=2) + "\n")
        if not passed:
            print(f"gate={label} exit_code={process.returncode} failure log tail:", file=sys.stderr)
            print(_failure_log_tail(text), file=sys.stderr)
        require(passed, f"gate failed: {label}; see {log}")
        return text

    run("shared-avm-preflight", ["bash", "scripts/check-avm-ci-preflight.sh"],
        environment=preflight_environment(scratch))
    run("engine-format", ["cargo", "fmt", "-p", "avm-wasm-code", "-p", "avm-wasm-executor", "--", "--check"])
    run("guest-source-parity", ["diff", "-ru", str(sdk / "crates/avm-contract-sdk"), str(source / "node/libs/avm-contract-sdk")])
    run("profile-parity", ["cmp", str(sdk / "profiles/wasmtime-release-profile.v1"), str(source / "specs/avm-vm/wasmtime-release-profile.v1")])
    run("installer-regressions", [sys.executable, "-B", "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*.py", "-v"], sdk, python_tests=True)
    run("sdk-format", ["cargo", "fmt", "--", "--check"], sdk)
    run("sdk-clippy", ["cargo", "clippy", "--locked", "--offline", "--all-targets", "--", "-D", "warnings"], sdk)
    run("sdk-tests", ["cargo", "test", "--locked", "--offline", "--", "--test-threads=1"], sdk, tests=True)
    for manifest in sorted((sdk / "examples").glob("*/Cargo.toml")):
        # These workspaces own full native, guest, optimizer and application
        # gates below; compiling a virtual root as one WASM package is invalid.
        if manifest.parent.name in {"bounty-escrow", "revenue-hub", "vesting-streams"}:
            continue
        run(manifest.parent.name + "-format", ["cargo", "fmt", "--manifest-path", str(manifest), "--", "--check"], sdk)
        environment = example_environment(source, manifest, evidence, run)
        run(manifest.parent.name + "-clippy", ["cargo", "clippy", "--locked", "--offline", "--manifest-path", str(manifest), "--target", "wasm32-unknown-unknown", "--", "-D", "warnings"], sdk, environment=environment)
    packages = ["avm-config", "avm-contract-sdk", "avm-contract-build", "avm-code-registry", "avm-deploy-upgrade", "avm-economics", "avm-effect-materializer", "avm-test-harness", "avm-wasm-code", "avm-wasm-executor"]
    selection = [arg for package in packages for arg in ("-p", package)]
    run("runtime-check", ["cargo", "check", "--locked", "--offline", "--all-targets", *selection])
    run("runtime-clippy", ["cargo", "clippy", "--locked", "--offline", "--all-targets", *selection, "--", "-D", "warnings"])
    for fixture in ("contract-b", "contract-batch-patch", "contract-a-v1"):
        manifest = source / "node/libs/avm-test-harness/tests/fixtures/e2e-standard-contract-stand" / fixture / "Cargo.toml"
        run("runtime-" + fixture + "-clippy", ["cargo", "clippy", "--locked", "--offline", "--manifest-path", str(manifest), "--target", "wasm32-unknown-unknown", "--", "-D", "warnings"])

    def test(package, target, selectors=(), release=False):
        common = ["cargo", "test", "--locked", "--offline", "--message-format=json", "-p", package, *target]
        if release:
            common.append("--release")
        if not selectors:
            run(package + "-" + "-".join(target).replace("--", ""), common + ["--", "--test-threads=1"], tests=True)
        for selector in selectors:
            run(package + "-" + selector.replace("::", "-"), common + [selector, "--", "--exact", "--test-threads=1"], tests=True, exact=selector)

    # Preserve every pre-existing release gate, then add the audited owner cases.
    for package in ("avm-test-harness", "avm-contract-sdk", "avm-contract-build", "avm-wasm-code"):
        test(package, ["--lib"])
    # Library tests and --all-targets check/clippy do not execute these owner
    # admission/preparation boundaries (SDK-R167-005).
    for target in ("validate_wasm_module", "prepare_code", "get_prepared",
                   "invalid_forbidden_sections", "invalid_mvp_function_body",
                   "disabled_feature_encodings"):
        test("avm-wasm-code", ["--test", target])
    for package in ("avm-code-registry", "avm-deploy-upgrade", "avm-economics", "avm-config"):
        test(package, [])
    test("avm-effect-materializer", ["--test", "zero_intent_checkpoint"])
    test("avm-contract-sdk", ["--test", "canonical_io"])
    test("avm-contract-sdk", ["--lib"], ["runtime::tests::" + case for case in "input_copy_preserves_bytes_and_live_bounds_at_chunk_boundaries dispatch_preserves_live_input_and_lifo_output_ownership dispatch_failure_capacity_and_overflow_leave_input_owned direct_dispatch_writes_once_without_the_vec_growth_path batch_dispatch_preserves_count_mode_and_failed_index_checks bounded_writer_failures_release_only_the_output_reservation direct_message_sections_and_batch_match_the_public_serializer".split()])
    test("avm-contract-build", ["--test", "reproducible_build"])
    for target_name in ("sdk_issue_abi", "sdk_issue_bindings", "sdk_issue_diagnostics", "sdk_issue_dispatch_wasm"):
        test("avm-contract-build", ["--test", target_name])
    test("avm-contract-sdk", ["--test", "deploy_accessors"])
    run("candidate-tools", ["cargo", "build", "--locked", "--offline", "-p", "avm-contract-build",
        "--bin", "avm-contract-build", "-p", "avm-test-harness", "--bin", "avm-local-stand"])
    target = Path(os.environ.get("CARGO_TARGET_DIR") or source / "target").resolve()
    builder, stand = target / "debug/avm-contract-build", target / "debug/avm-local-stand"
    profile = source / "specs/avm-vm/wasmtime-release-profile.v1"
    consumer_sdk = evidence / "consumer-sdk"
    consumer_sdk.mkdir()
    archive = subprocess.check_output(["git", "-C", str(sdk), "archive", "HEAD"])
    with tarfile.open(fileobj=io.BytesIO(archive)) as snapshot:
        snapshot.extractall(consumer_sdk, filter="data")
    (consumer_sdk / "bin").mkdir()
    for binary in (builder, stand):
        (consumer_sdk / "bin" / binary.name).symlink_to(binary)
    run("sdk-public-issue-paths", [sys.executable, "-B", str(sdk / "scripts/tests/sdk_issue_public_paths.py"),
        "--sdk", str(consumer_sdk), "--builder", str(builder), "--stand", str(stand),
        "--profile", str(profile), "--out", str(evidence / "public-issue-paths")])
    run("sdk-deploy-callback-delivery", ["bash", str(sdk / "examples/deploy-callback-repro/repro.sh"),
        "--builder", str(builder), "--stand", str(stand), "--profile", str(profile),
        "--out", str(evidence / "deploy-callback-repro")], sdk)
    for name in ("bounty-escrow", "revenue-hub"):
        command = ["bash", str(consumer_sdk / "examples" / name / "scripts/check.sh"),
            "--builder", str(builder), "--stand", str(stand), "--profile", str(profile),
            "--out", str(evidence / name)]
        if name == "revenue-hub":
            command.extend(("--optimizer", str(optimizer)))
        run(name + "-application-checks", command, consumer_sdk)
    application_environment = os.environ.copy()
    application_environment.update(AVM_CONTRACT_BUILD=str(builder), AVM_LOCAL_STAND=str(stand),
                                   AVM_WASMTIME_PROFILE=str(profile),
                                   AVM_EVIDENCE_DIR=str(evidence / "vesting-streams"))
    run("vesting-streams-application-checks", ["bash",
        str(consumer_sdk / "examples/vesting-streams/scripts/check.sh"), str(consumer_sdk), str(optimizer)],
        consumer_sdk, environment=application_environment)
    application_environment.update(AVM_TOKEN_WORK=str(evidence / "fungible-token"),
                                   PROVENANCE_OUT=str(evidence / "factory-child-provenance"), PROVENANCE_KEEP="1")
    run("fungible-token-application-checks", ["bash",
        str(consumer_sdk / "examples/fungible-token/stand-e2e.sh")], consumer_sdk,
        environment=application_environment)
    run("factory-child-provenance-application-checks", ["bash",
        str(consumer_sdk / "examples/factory-child-provenance/prove.sh")], consumer_sdk,
        environment=application_environment)
    cases = "deploy_init_success_executes_real_wasm_with_exact_output_and_fuel process_message_selects_real_export_with_exact_output_and_fuel process_batch_uses_the_batch_export_and_output_at_effective_cap_succeeds allocator_fuel_exhaustion_precedes_the_generic_trap_status invalid_non_null_allocator_ranges_do_not_copy_call_init_or_free output_cap_precedes_pointer_validation invalid_output_pointer_range_and_input_overlap_are_rejected_before_read_or_free output_free_non_fuel_trap_skips_input_free_and_discards_copied_output fuel_exhaustion_at_output_free_and_input_free_has_fuel_precedence successful_cleanup_copies_output_then_frees_output_before_input same_input_uses_fresh_stores_and_is_deterministic failed_attempts_also_discard_their_store_state".split()
    test("avm-wasm-executor", ["--lib"], ["tests::" + case for case in cases])
    test("avm-wasm-executor", ["--lib"], ["tests::deadline::" + case for case in "completed_controlled_exports_preserve_exact_output_status_and_fuel expired_control_starts_no_guest_or_alarm cutoff_interrupts_allocator_entrypoint_and_each_free cancelled_store_is_discarded_and_next_call_matches_fresh_execution".split()])
    test("avm-canonical", ["--test", "compact_maps"])
    test("avm-canonical", ["--test", "compact_account"], ["profile_dispatch_preserves_legacy_roots_errors_and_load_trace", "source_rejects_wrong_hashes_missing_nodes_and_mixed_profiles"])
    test("avm-event-capnp-codec", ["--test", "build_event_log"], "avm_event_capnp_codec_build_event_log_fixed_raw_vectors avm_event_capnp_codec_parse_event_log_unit_determinism_and_no_payload_copy avm_event_capnp_codec_parse_event_log_negative avm_event_capnp_codec_parse_event_log_does_not_replace_inner_validation verified_log_preserves_encoding_and_rejects_invalid_inputs avm_event_capnp_codec_build_event_log_competing_fault_precedence".split())
    test("avm-scheduler", ["--test", "isolated_fuel_admission"])
    test("avm-test-harness", ["--lib"], ["local_stand::tests::" + case for case in "owner_event_reads_use_block_start_and_advance_only_in_the_next_block owner_block_records_failed_batch_and_rejected_input_fuel_burns_once account_batch_exhausted_pool_settles_zero_fuel_fallback_without_state_effects".split()])
    # SDK-R167-001: inspect and compile the actual node graph. Separately run
    # the owner assertions with the exact same compiled Wasmtime feature set.
    tree = ["cargo", "tree", "--locked", "--offline", "--edges", "normal,build",
            "--prefix", "none", "--format", "{p}|{f}"]
    sdk_features = tree_engine_features(run("sdk-engine-graph", tree + ["-p", "avm-contract-build", "-p", "avm-test-harness"]))
    node_features = tree_engine_features(run("node-engine-graph", tree + ["-p", "node"]))
    profile = parse_profile((source / "specs/avm-vm/wasmtime-release-profile.v1").read_bytes())
    required_features = set(profile["wasmtime_required_features"].split(","))
    for features in (sdk_features, node_features):
        require(required_features <= set(features) and "component-model-async" not in features,
                "unqualified Wasmtime build capability")
    require(compiled_engine_features((evidence / "avm-wasm-code-lib.log").read_text()) == sdk_features,
            "SDK graph differs from its executed engine assertions")
    node_build = run("node-release-compiler-artifacts", ["cargo", "build", "--locked", "--offline",
                     "--release", "--message-format=json", "-vv", "-p", "node", "--bin", "node"])
    require(compiled_engine_features(node_build) == node_features,
            "actual node build differs from its resolved graph")
    for package in ("avm-wasm-code", "avm-wasm-executor"):
        text = run(package + "-node-engine-features", ["cargo", "test", "--locked", "--offline",
                   "--release", "--message-format=json", "-p", package, "--features", "wasmtime/default",
                   "--lib", "--", "--test-threads=1"], tests=True)
        require(compiled_engine_features(text) == node_features,
                "node feature assertions ran under a different engine build graph")
    result["engine_qualification"] = {"sdk_wasmtime_features": sdk_features,
                                      "node_wasmtime_features": node_features,
                                      "profile_sha256": file_digest(source / "specs/avm-vm/wasmtime-release-profile.v1")}
    test("avm-test-harness", ["--test", "e2e_standard_contract_stand"], ["e2e_standard_contract_stand"], release=True)
    test("avm-test-harness", ["--test", "e2e_deploy_upgrade_and_code_lifecycle"], release=True)
    clean(source)
    clean(sdk)
    require(head(source) == result["source_commit"] and head(sdk) == result["sdk_commit"], "source changed during gates")
    result["passed"] = True
    (evidence / "gates.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": True, "gates": len(result["gates"]), "evidence": str(evidence)}))


if __name__ == "__main__":
    main()
