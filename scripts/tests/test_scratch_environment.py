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
from manual_gates import normalize_scratch_environment, preflight_environment


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

    def preflight_child(self, environment):
        fallback = self.root / "source-fallback"
        fallback.mkdir(exist_ok=True)
        program = (
            "import json,os,tempfile; from pathlib import Path; "
            "directory=tempfile.TemporaryDirectory(prefix='sdk-preflight-child-'); "
            "print(json.dumps({'parent':str(Path(directory.name).parent),"
            "'environment':{key:os.environ.get(key) for key in "
            "('TMPDIR','TMP','TEMP','AVM_TMPDIR','AVM_SCRATCH_ROOT','CARGO_TARGET_DIR')}})); "
            "directory.cleanup()"
        )
        output = subprocess.check_output([
            "bash", "-c",
            'export TMPDIR="${AVM_TMPDIR:-$1}" TMP="${AVM_TMPDIR:-$1}" TEMP="${AVM_TMPDIR:-$1}"; '
            'exec "$2" -c "$3"',
            "sdk-preflight-child", str(fallback), sys.executable, program,
        ], env=environment, text=True)
        return json.loads(output)

    def test_preflight_child_receives_selected_scratch_when_bridge_is_absent_or_empty(self):
        first, second, third = map(str, self.paths)
        default = str(Path(tempfile.gettempdir()).resolve())
        for values, expected in [
            ((first, second, third), first),
            (("", second, third), second),
            (("", "", third), third),
            (("", "", ""), default),
            ((None, None, None), default),
        ]:
            for bridge in (None, ""):
                with self.subTest(values=values, bridge=bridge), patch.dict(os.environ):
                    for key in ("TMPDIR", "TMP", "TEMP", "AVM_TMPDIR"):
                        os.environ.pop(key, None)
                    for key, value in zip(("TMPDIR", "TMP", "TEMP"), values):
                        if value is not None:
                            os.environ[key] = value
                    if bridge is not None:
                        os.environ["AVM_TMPDIR"] = bridge
                    selected = normalize_scratch_environment()
                    before = dict(os.environ)
                    actual = self.preflight_child(preflight_environment(selected))
                    self.assertEqual(dict(os.environ), before)
                    self.assertEqual(actual["parent"], expected)
                    self.assertEqual(
                        [actual["environment"][key] for key in ("TMPDIR", "TMP", "TEMP", "AVM_TMPDIR")],
                        [expected] * 4,
                    )

    def test_preflight_child_preserves_explicit_overrides_without_mutating_parent(self):
        first, second, third = map(str, self.paths)
        values = {"TMPDIR": first, "TMP": "", "TEMP": "", "AVM_TMPDIR": second,
                  "AVM_SCRATCH_ROOT": third, "CARGO_TARGET_DIR": str(self.root / "caller target")}
        with patch.dict(os.environ, values):
            selected = normalize_scratch_environment()
            before = dict(os.environ)
            actual = self.preflight_child(preflight_environment(selected))
            self.assertEqual(dict(os.environ), before)
            self.assertEqual(os.environ["TMPDIR"], first)
            self.assertEqual(actual["parent"], second)
            self.assertEqual(
                [actual["environment"][key] for key in ("TMPDIR", "TMP", "TEMP", "AVM_TMPDIR")],
                [second] * 4,
            )
            for key in ("AVM_SCRATCH_ROOT", "CARGO_TARGET_DIR"):
                self.assertEqual(actual["environment"][key], values[key])


if __name__ == "__main__":
    unittest.main()
