"""The real qualification entrypoint accepts exactly two available CPUs."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import manual_gates


class ManualGateAffinityTests(unittest.TestCase):
    def run_entrypoint(self, cpus):
        sdk = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory(prefix="sdk-affinity-") as directory:
            root = Path(directory)
            evidence = root / "evidence"
            environment = {
                **os.environ,
                "AVM_SDK_QUALIFICATION": "1",
                "CARGO_BUILD_JOBS": "2",
                "RUST_TEST_THREADS": "2",
                "TMPDIR": str(root), "TMP": str(root), "TEMP": str(root),
            }
            result = subprocess.run(
                ["taskset", "--cpu-list", ",".join(map(str, cpus)), sys.executable,
                 "-B", str(sdk / "scripts/manual_gates.py"),
                 "--source", str(sdk), "--sdk", str(sdk),
                 "--evidence", str(evidence), "--optimizer", str(root / "absent-optimizer")],
                env=environment, capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(evidence.exists(), "affinity probes must not start release gates")
            return result

    def assert_reaches_optimizer(self, cpus):
        result = self.run_entrypoint(cpus)
        self.assertIn("pass --optimizer with checksum-pinned Binaryen 133 wasm-opt", result.stderr)
        self.assertNotIn("qualification requires", result.stderr)

    def test_two_available_cpus_reach_optimizer(self):
        allowed = os.sched_getaffinity(0)
        self.assertGreaterEqual(len(allowed), 2, "qualification tests require two available CPUs")
        nonlegacy = sorted(allowed - {0, 1})
        pairs = [nonlegacy[-2:] if len(nonlegacy) >= 2 else sorted(allowed)[-2:]]
        if {0, 1} <= allowed and pairs[0] != [0, 1]:
            pairs.append([0, 1])
        for cpus in pairs:
            with self.subTest(cpus=cpus):
                self.assert_reaches_optimizer(cpus)

    def test_one_available_cpu_is_rejected_before_optimizer(self):
        cpus = sorted(os.sched_getaffinity(0))
        result = self.run_entrypoint([0 if 0 in cpus else cpus[0]])
        self.assertIn("qualification requires exactly 2 allowed CPUs", result.stderr)
        self.assertNotIn("pass --optimizer", result.stderr)

    def test_more_than_two_cpus_are_rejected_before_optimizer(self):
        with tempfile.TemporaryDirectory(prefix="sdk-affinity-unit-") as directory:
            root = Path(directory)
            evidence = root / "evidence"
            argv = ["manual_gates.py", "--source", str(root), "--sdk", str(root),
                    "--evidence", str(evidence), "--optimizer", str(root / "absent-optimizer")]
            # This negative unit input also runs on CI workers exposing exactly two CPUs.
            with mock.patch.dict(os.environ, {"AVM_SDK_QUALIFICATION": "1", "CARGO_BUILD_JOBS": "2"}), \
                    mock.patch.object(os, "sched_getaffinity", return_value={3, 8, 19}), \
                    mock.patch.object(sys, "argv", argv):
                with self.assertRaisesRegex(ValueError, "qualification requires exactly 2 allowed CPUs"):
                    manual_gates.main()
            self.assertFalse(evidence.exists(), "negative unit probe must not start release gates")


class BuildReleaseAffinityTests(unittest.TestCase):
    def run_entrypoint(self, cpus):
        sdk = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory(prefix="sdk-build-affinity-") as directory:
            root = Path(directory)
            output = root / "output"
            environment = {
                **os.environ,
                "AVM_SDK_QUALIFICATION": "1", "CARGO_BUILD_JOBS": "2",
                "RUST_TEST_THREADS": "2",
                "TMPDIR": str(root), "TMP": str(root), "TEMP": str(root),
            }
            result = subprocess.run(
                ["taskset", "--cpu-list", ",".join(map(str, cpus)), sys.executable,
                 "-B", str(sdk / "scripts/build-release.py"), "build",
                 "--source", str(root / "absent-source-sentinel"), "--sdk", str(sdk),
                 "--gates", str(root / "absent-gates"), "--output", str(output)],
                env=environment, capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists(), "affinity probes must not build release tools")
            return result

    def test_two_available_cpus_reach_source_identity(self):
        allowed = os.sched_getaffinity(0)
        self.assertGreaterEqual(len(allowed), 2, "qualification tests require two available CPUs")
        nonlegacy = sorted(allowed - {0, 1})
        cpus = nonlegacy[-2:] if len(nonlegacy) >= 2 else sorted(allowed)[-2:]
        result = self.run_entrypoint(cpus)
        self.assertIn("absent-source-sentinel", result.stderr)
        self.assertNotIn("build requires", result.stderr)

    def test_one_available_cpu_is_rejected_before_source_identity(self):
        cpus = sorted(os.sched_getaffinity(0))
        result = self.run_entrypoint([0 if 0 in cpus else cpus[0]])
        self.assertIn("build requires exactly 2 allowed CPUs and jobs=2", result.stderr)
        self.assertNotIn("absent-source-sentinel", result.stderr)

    def test_more_than_two_cpus_are_rejected_before_source_identity(self):
        script = Path(__file__).resolve().parents[1] / "build-release.py"
        spec = importlib.util.spec_from_file_location("affinity_build_release", script)
        builder = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(builder)
        with tempfile.TemporaryDirectory(prefix="sdk-build-affinity-unit-") as directory:
            root = Path(directory)
            with mock.patch.dict(os.environ, {"AVM_SDK_QUALIFICATION": "1", "CARGO_BUILD_JOBS": "2"}), \
                    mock.patch.object(os, "sched_getaffinity", return_value={3, 8, 19}):
                with self.assertRaisesRegex(ValueError, "build requires exactly 2 allowed CPUs and jobs=2"):
                    builder.build(root / "absent-source-sentinel", root, root / "output", root / "absent-gates")
            self.assertFalse((root / "output").exists())


if __name__ == "__main__":
    unittest.main()
