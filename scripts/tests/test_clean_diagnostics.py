"""Clean-source failures explain Git paths without relaxing release admission."""
import contextlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import manual_gates


class CleanDiagnosticsTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="sdk-clean-diagnostics-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repo = self.repository(self.root / "repository")

    def git(self, repo, *args):
        return subprocess.check_output(["git", "-C", str(repo), *args])

    def repository(self, path):
        path.mkdir(parents=True)
        subprocess.run(["git", "init", "--quiet", str(path)], check=True)
        self.git(path, "config", "core.filemode", "true")
        for name in ("tracked.txt", "mode.sh", ".gitignore"):
            (path / name).write_text("ignored.log\n" if name == ".gitignore" else "committed\n")
        self.git(path, "add", ".")
        self.git(path, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "--quiet", "-m", "fixture")
        return path

    def rejection(self):
        with self.assertRaisesRegex(ValueError, "dirty source:") as failure:
            manual_gates.clean(self.repo)
        return str(failure.exception)

    def test_clean_and_ignored_files_pass_without_diagnostics(self):
        (self.repo / "ignored.log").write_text("ignored fixture\n")
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(manual_gates, "_failure_log_tail") as diagnostic, \
                contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            self.assertIsNone(manual_gates.clean(self.repo))
        diagnostic.assert_not_called()
        self.assertEqual(stdout.getvalue() + stderr.getvalue(), "")

    def test_tracked_untracked_and_mode_paths_retain_xy_status(self):
        contents = "FILE_CONTENT_MUST_NOT_APPEAR_IN_DIAGNOSTICS"
        (self.repo / "tracked.txt").write_text(contents)
        (self.repo / "unexpected.txt").write_text(contents)
        (self.repo / "mode.sh").chmod(0o755)
        before = self.git(self.repo, "status", "--porcelain", "--untracked-files=all")
        message = self.rejection()
        for status in (" M tracked.txt", "?? unexpected.txt", " M mode.sh"):
            self.assertIn(status, message)
        self.assertNotIn(contents, message)
        self.assertEqual(self.git(self.repo, "status", "--porcelain", "--untracked-files=all"), before)

    def test_path_secrets_are_redacted_without_dumping_environment(self):
        secret = "fixture-environment-secret-123456"
        token = "ghp_" + "A" * 36
        for name in (secret + ".txt", token + ".txt"):
            (self.repo / name).write_text("FILE_CONTENT_SECRET")
        with mock.patch.dict(os.environ, {"TEST_CLEAN_TOKEN": secret, "UNRELATED_SETTING": "ENVIRONMENT_MUST_NOT_BE_DUMPED"}):
            message = self.rejection()
        self.assertIn("[REDACTED]", message)
        for hidden in (secret, token, "FILE_CONTENT_SECRET", "ENVIRONMENT_MUST_NOT_BE_DUMPED"):
            self.assertNotIn(hidden, message)

    def test_large_dirty_inventory_is_bounded_and_still_rejected(self):
        for index in range(100):
            (self.repo / (f"entry-{index:03}-" + "x" * 180)).write_text("private contents")
        message = self.rejection()
        self.assertIn("?? entry-099-", message)
        self.assertLessEqual(len(message.splitlines()), 42)
        self.assertLessEqual(len(message.encode("utf-8")), 9000)
        self.assertNotIn("private contents", message)

    def test_every_nonempty_status_rejects_even_if_sanitized_away(self):
        for status in (b" ", b"\x00", b" M \x1b[31mcontrol.txt\x1b[0m\n"):
            with self.subTest(status=status), \
                    mock.patch.object(manual_gates.subprocess, "check_output", return_value=status):
                message = self.rejection()
                self.assertNotIn("\x1b", message)
                self.assertNotIn("\x00", message)

    def test_both_real_entrypoints_explain_dirty_source_and_sdk(self):
        scripts = Path(__file__).resolve().parents[1]
        cpus = sorted(os.sched_getaffinity(0))[-2:]
        self.assertEqual(len(cpus), 2, "entrypoint probes require two available CPUs")
        optimizer = self.root / "wasm-opt"
        optimizer.write_text("#!/bin/sh\nexit 0\n")
        optimizer.chmod(0o700)
        for script in ("manual_gates.py", "build-release.py"):
            for role in ("source", "sdk"):
                with self.subTest(script=script, role=role):
                    case = self.root / (script + "-" + role)
                    source, sdk = (self.repository(case / name) for name in ("source", "sdk"))
                    dirty = source if role == "source" else sdk
                    (dirty / "entrypoint-dirty.txt").write_text("PRIVATE_CONTENTS")
                    output = case / "output"
                    args = ["--source", str(source), "--sdk", str(sdk)]
                    if script == "manual_gates.py":
                        args += ["--evidence", str(output), "--optimizer", str(optimizer)]
                    else:
                        args = ["build", *args, "--gates", str(case / "absent-gates"), "--output", str(output)]
                    environment = {**os.environ, "AVM_SDK_QUALIFICATION": "1", "CARGO_BUILD_JOBS": "2",
                                   "TMPDIR": str(self.root), "TMP": str(self.root), "TEMP": str(self.root)}
                    process = subprocess.run(["taskset", "--cpu-list", ",".join(map(str, cpus)),
                                              sys.executable, "-B", str(scripts / script), *args],
                                             env=environment, capture_output=True, text=True)
                    self.assertNotEqual(process.returncode, 0)
                    self.assertIn("ValueError: dirty source:", process.stderr)
                    self.assertIn("?? entrypoint-dirty.txt", process.stderr)
                    self.assertNotIn("PRIVATE_CONTENTS", process.stderr)
                    self.assertFalse(output.exists(), "dirty input must stop before gates or builds")


    def test_git_quoted_paths_and_both_rename_names_redact_before_rendering(self):
        values = ('synthetic-"secret-123456', 'synthetic-secret-é-123456',
                  'synthetic-secret-\\-123456', 'synthetic-secret-\t-123456',
                  'synthetic-secret-\n-123456', 'synthetic-secret-\x1b-123456')
        for quote_path in ("true", "false"):
            self.git(self.repo, "config", "core.quotePath", quote_path)
            for index, secret in enumerate(values):
                prefix = f"quoted-{quote_path}-{index}-"
                (self.repo / (prefix + secret + ".txt")).write_text("PRIVATE_CONTENTS")
                before = self.git(self.repo, "status", "--porcelain", "--untracked-files=all")
                with self.subTest(quote_path=quote_path, case=index), \
                        mock.patch.dict(os.environ, {"TEST_CLEAN_TOKEN": secret}):
                    message = self.rejection()
                    line = next(line for line in message.splitlines() if prefix in line)
                    self.assertIn("[REDACTED]", line)
                    self.assertNotIn(secret, message)
                    self.assertNotIn("PRIVATE_CONTENTS", message)
                    self.assertNotIn("\x1b", message)
                    self.assertNotIn("\x00", message)
                    self.assertLessEqual(len(message.encode("utf-8")), 9000)
                self.assertEqual(self.git(self.repo, "status", "--porcelain", "--untracked-files=all"), before)
        old_secret, new_secret = 'old-"secret-é-123456', 'new-\\secret-\t-123456'
        old_name, new_name = old_secret + ".txt", new_secret + ".txt"
        (self.repo / old_name).write_text("PRIVATE_RENAME_CONTENTS")
        self.git(self.repo, "add", "--", old_name)
        self.git(self.repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
                 "commit", "--quiet", "-m", "rename fixture")
        self.git(self.repo, "mv", "--", old_name, new_name)
        before = self.git(self.repo, "status", "--porcelain", "--untracked-files=all")
        with mock.patch.dict(os.environ, {"TEST_OLD_TOKEN": old_secret, "TEST_NEW_TOKEN": new_secret}):
            message = self.rejection()
        self.assertNotIn(old_secret, message)
        self.assertNotIn(new_secret, message)
        self.assertGreaterEqual(message.count("[REDACTED]"), 2)
        self.assertNotIn("PRIVATE_RENAME_CONTENTS", message)
        self.assertEqual(self.git(self.repo, "status", "--porcelain", "--untracked-files=all"), before)

    def test_dirty_input_still_raises_value_error_when_raw_diagnostics_fail(self):
        with mock.patch.object(manual_gates.subprocess, "check_output",
                               side_effect=[b'?? "synthetic-\\\"secret.txt"\n',
                                            subprocess.CalledProcessError(1, ["git"])]):
            message = self.rejection()
        self.assertIn("dirty source:", message)
        self.assertNotIn("synthetic-", message)


if __name__ == "__main__":
    unittest.main()
