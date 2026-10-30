"""Portable child-process scratch selection for SDK release tooling."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from manual_gates import normalize_scratch_environment


class ScratchEnvironment(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="sdk-scratch-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.paths = [self.root / name for name in ("primary", "secondary", "third")]
        for path in self.paths:
            path.mkdir()

    def test_priority_and_empty_values_reach_child_processes(self):
        first, second, third = map(str, self.paths)
        for values, expected in [
            ((first, second, third), first),
            (("", second, third), second),
            (("", "", third), third),
        ]:
            with self.subTest(values=values), patch.dict(os.environ, dict(zip(("TMPDIR", "TMP", "TEMP"), values))):
                self.assertEqual(normalize_scratch_environment(), Path(expected))
                child = subprocess.check_output([sys.executable, "-c",
                    "import json,os; print(json.dumps([os.environ[k] for k in ('TMPDIR','TMP','TEMP')]))"], text=True)
                self.assertEqual(json.loads(child), [expected, expected, expected])

    def test_all_empty_uses_existing_platform_default(self):
        expected = Path(tempfile.gettempdir()).resolve()
        with patch.dict(os.environ, {key: "" for key in ("TMPDIR", "TMP", "TEMP")}):
            self.assertEqual(normalize_scratch_environment(), expected)
            self.assertEqual([os.environ[key] for key in ("TMPDIR", "TMP", "TEMP")], [str(expected)] * 3)

    def test_absent_values_use_existing_platform_default(self):
        expected = Path(tempfile.gettempdir()).resolve()
        with patch.dict(os.environ):
            for key in ("TMPDIR", "TMP", "TEMP"):
                os.environ.pop(key, None)
            self.assertEqual(normalize_scratch_environment(), expected)

    def test_invalid_explicit_directory_fails_without_fallback(self):
        regular_file = self.root / "not-a-directory"
        regular_file.write_text("fixture")
        for value in ("relative-scratch", str(self.root / "missing"), str(regular_file)):
            with self.subTest(value=value), patch.dict(os.environ, {"TMPDIR": value, "TMP": str(self.paths[1])}):
                with self.assertRaisesRegex(ValueError, "existing absolute writable directory"):
                    normalize_scratch_environment()
                self.assertEqual(os.environ["TMPDIR"], value)


if __name__ == "__main__":
    unittest.main()
