#!/usr/bin/env python3
"""Run accumulated SDK release gates on the bounded qualification pod."""

import argparse
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
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
    require(not subprocess.check_output(["git", "-C", str(root), "status", "--porcelain", "--untracked-files=all"]), f"dirty source: {root}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    args = parser.parse_args()
    source, sdk, evidence = args.source.resolve(), args.sdk.resolve(), args.evidence.resolve()
    require(os.environ.get("AVM_SDK_POD_QUALIFICATION") == "1", "use the bounded pod launcher")
    require(os.sched_getaffinity(0) <= {0, 1}, "qualification requires CPUs 0-1")
    require(os.environ.get("CARGO_BUILD_JOBS") == "2", "CARGO_BUILD_JOBS must be 2")
    for key in ("TMPDIR", "TMP", "TEMP"):
        require(bool(os.environ.get(key)), f"missing {key}")
        scratch = Path(os.environ.get(key, "")).resolve()
        require(scratch.is_relative_to(Path("/media/futurizt/AVM/tmp")), f"invalid {key}")
    clean(source)
    clean(sdk)
    evidence.mkdir(parents=True, exist_ok=False)
    result = {"schema": "gosh.avm-sdk.gates.v1", "source_commit": head(source),
              "sdk_commit": head(sdk), "passed": False, "gates": []}

    def run(label, argv, cwd=source, tests=False, exact=None, python_tests=False, env=None):
        log = evidence / (label + ".log")
        (evidence / "current.json").write_text(json.dumps({"gate": label, "pid": os.getpid(), "completed": len(result["gates"]), "source_commit": result["source_commit"], "sdk_commit": result["sdk_commit"]}) + "\n")
        with log.open("wb") as stream:
            process = subprocess.run(argv, cwd=cwd, stdout=stream, stderr=subprocess.STDOUT, env=env)
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
        (evidence / "gates.json").write_text(json.dumps(result, indent=2) + "\n")
        require(passed, f"gate failed: {label}; see {log}")
        return text

    # Preserve every shared preflight check while keeping its launcher scratch
    # in this task's allocated directory. Do not edit the runtime's shared script.
    preflight_source = source / "scripts/check-avm-ci-preflight.sh"
    preflight = preflight_source.read_text()
    replacements = {
        'cd "$(dirname "$0")/.."': "cd -- " + shlex.quote(str(source)),
        "export TMPDIR=/media/futurizt/AVM/tmp TMP=/media/futurizt/AVM/tmp TEMP=/media/futurizt/AVM/tmp":
            "export " + " ".join(key + "=" + shlex.quote(os.environ[key]) for key in ("TMPDIR", "TMP", "TEMP")),
    }
    for old, new in replacements.items():
        require(preflight.count(old) == 1, "shared preflight launcher changed; review scratch adaptation")
        preflight = preflight.replace(old, new)
    scoped_preflight = evidence / "shared-preflight.sh"
    scoped_preflight.write_text(preflight)
    result["preflight_source_sha256"] = file_digest(preflight_source)
    result["preflight_scoped_launcher_sha256"] = file_digest(scoped_preflight)
    run("shared-avm-preflight", ["bash", str(scoped_preflight)])
    run("engine-format", ["cargo", "fmt", "-p", "avm-wasm-code", "-p", "avm-wasm-executor", "--", "--check"])
    run("guest-source-parity", ["diff", "-ru", str(sdk / "crates/avm-contract-sdk"), str(source / "node/libs/avm-contract-sdk")])
    run("profile-parity", ["cmp", str(sdk / "profiles/wasmtime-release-profile.v1"), str(source / "specs/avm-vm/wasmtime-release-profile.v1")])
    run("installer-regressions", [sys.executable, "-B", "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*.py", "-v"], sdk, python_tests=True)
    run("sdk-format", ["cargo", "fmt", "--", "--check"], sdk)
    run("sdk-clippy", ["cargo", "clippy", "--locked", "--offline", "--all-targets", "--", "-D", "warnings"], sdk)
    run("sdk-tests", ["cargo", "test", "--locked", "--offline", "--", "--test-threads=1"], sdk, tests=True)
    run("candidate-tools", ["cargo", "build", "--locked", "--offline", "-p", "avm-contract-build", "-p", "avm-test-harness", "--bin", "avm-contract-build", "--bin", "avm-local-stand"])
    target = Path(os.environ.get("CARGO_TARGET_DIR", source / "target"))
    builder = target / "debug/avm-contract-build"
    stand = target / "debug/avm-local-stand"
    for manifest in sorted((sdk / "examples").glob("*/Cargo.toml")):
        run(manifest.parent.name + "-format", ["cargo", "fmt", "--manifest-path", str(manifest), "--", "--check"], sdk)
        env = os.environ.copy()
        env.pop("AVM_CONTRACT_BINDINGS", None)
        bindings = tomllib.loads((manifest.parent / "avm-contract.toml").read_text()).get("bindings", {})
        if bindings.get("enabled"):
            argv = [str(builder), "bindings", "--manifest-path", str(manifest)]
            if bindings.get("dispatch"):
                argv.append("--dispatch")
            generated = evidence / (manifest.parent.name + "-bindings.rs")
            generated.write_text(run(manifest.parent.name + "-bindings", argv, sdk))
            env["AVM_CONTRACT_BINDINGS"] = str(generated)
        run(manifest.parent.name + "-clippy", ["cargo", "clippy", "--locked", "--offline", "--manifest-path", str(manifest), "--target", "wasm32-unknown-unknown", "--", "-D", "warnings"], sdk, env=env)
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
    test("avm-contract-sdk", ["--test", "event_read_outbound"])
    test("avm-execution-abi", [])
    run("sdk-public-issue-paths", [sys.executable, "-B", str(sdk / "scripts/tests/sdk_issue_public_paths.py"), "--sdk", str(sdk), "--builder", str(builder), "--stand", str(stand), "--profile", str(source / "specs/avm-vm/wasmtime-release-profile.v1"), "--out", str(evidence / "public-issue-paths")])
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
