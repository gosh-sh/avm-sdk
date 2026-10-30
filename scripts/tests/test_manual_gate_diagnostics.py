"""A failed release gate must expose bounded context without changing its verdict."""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import manual_gates


class FailureLogDiagnosticsTests(unittest.TestCase):
    def test_failure_tail_preserves_final_error_and_redacts_credentials(self):
        secret = "unit-test-environment-secret"
        github_token = "ghp_" + "A" * 36
        text = "old context\n" * 80 + (
            "\x1b[31mTEST_GATE_TOKEN=" + secret + "\x1b[0m\n"
            "Authorization: Bearer unit-test-header-secret\n"
            + github_token + "\nValueError: application assertion failed\n"
        )
        tail = manual_gates._failure_log_tail(
            text, environment={"TEST_GATE_TOKEN": secret}, max_lines=4, max_bytes=1024
        )
        self.assertIn("ValueError: application assertion failed", tail)
        self.assertIn("[REDACTED]", tail)
        self.assertNotIn(secret, tail)
        self.assertNotIn("unit-test-header-secret", tail)
        self.assertNotIn(github_token, tail)
        self.assertNotIn("\x1b", tail)
        self.assertNotIn("old context", tail)
        self.assertLessEqual(len(tail.splitlines()), 4)
        self.assertLessEqual(len(tail.encode("utf-8")), 1024)

    def test_failure_tail_bounds_utf8_and_removes_private_key_blocks(self):
        text = (
            "-----BEGIN PRIVATE KEY-----\nunit-test-key-material\n-----END PRIVATE KEY-----\n"
            + "€" * 120 + "\nValueError: final failure\n"
        )
        tail = manual_gates._failure_log_tail(text, environment={}, max_lines=3, max_bytes=64)
        self.assertIn("ValueError: final failure", tail)
        self.assertNotIn("unit-test-key-material", tail)
        self.assertNotIn("BEGIN PRIVATE KEY", tail)
        self.assertLessEqual(len(tail.encode("utf-8")), 64)
        self.assertLessEqual(len(tail.splitlines()), 3)

    def test_real_failed_gate_keeps_exit_code_receipt_and_private_log(self):
        with tempfile.TemporaryDirectory(prefix="sdk-failure-diagnostics-") as directory:
            root = Path(directory)
            source, sdk, evidence = root / "source", root / "sdk", root / "evidence"
            (source / "scripts").mkdir(parents=True)
            sdk.mkdir()
            optimizer = root / "wasm-opt"
            optimizer.write_text("#!/bin/sh\nexit 0\n")
            optimizer.chmod(0o700)
            (source / "scripts/check-avm-ci-preflight.sh").write_text(
                "#!/bin/sh\nprintf '%s\\n' \"$TEST_GATE_TOKEN\"\n"
                "printf '%s\\n' 'ValueError: independent application fixture failed'\nexit 17\n"
            )
            secret = "unit-test-secret-kept-only-in-private-log"
            argv = ["manual_gates.py", "--source", str(source), "--sdk", str(sdk),
                    "--evidence", str(evidence), "--optimizer", str(optimizer)]
            stderr = io.StringIO()
            with mock.patch.dict(os.environ, {"AVM_SDK_QUALIFICATION": "1", "CARGO_BUILD_JOBS": "2", "TEST_GATE_TOKEN": secret}), \
                    mock.patch.object(manual_gates, "clean"), \
                    mock.patch.object(manual_gates, "head", return_value="a" * 40), \
                    mock.patch.object(manual_gates.os, "sched_getaffinity", return_value={0, 1}), \
                    mock.patch.object(sys, "argv", argv), contextlib.redirect_stderr(stderr):
                with self.assertRaisesRegex(ValueError, "gate failed: shared-avm-preflight"):
                    manual_gates.main()
            receipt = json.loads((evidence / "gates.json").read_text())
            gate = receipt["gates"][0]
            log = evidence / "shared-avm-preflight.log"
            self.assertEqual(gate["exit_code"], 17)
            self.assertFalse(gate["passed"])
            self.assertFalse(receipt["passed"])
            self.assertEqual(gate["log_sha256"], hashlib.sha256(log.read_bytes()).hexdigest())
            self.assertIn(secret, log.read_text())
            self.assertIn("shared-avm-preflight", stderr.getvalue())
            self.assertIn("exit_code=17", stderr.getvalue())
            self.assertIn("ValueError: independent application fixture failed", stderr.getvalue())
            self.assertNotIn(secret, stderr.getvalue())
            self.assertLessEqual(len(stderr.getvalue().encode("utf-8")), 9000)


if __name__ == "__main__":
    unittest.main()
