import importlib.util
from pathlib import Path
import json
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("measure_examples", Path(__file__).with_name("measure_example_memory.py"))
measure = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(measure)


class MemorySourceCopyTests(unittest.TestCase):
    def test_fixture_limits_preserve_earlier_bundles_and_marked_test_builds(self):
        from packaged_lifecycle import fixture_memory_maximum
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "avm-contract.toml").write_text('[contract]\nname = "fixture"\n')
            self.assertEqual(fixture_memory_maximum(root, False), 33554432)
            (root / "avm-memory.json").write_text(json.dumps({"version": 1, "initial_memory_bytes": 65536,
                "peak_memory_bytes": 65537, "test_calls": 1}))
            self.assertEqual(fixture_memory_maximum(root, False), 131072)
            self.assertEqual(fixture_memory_maximum(root, True), 33554432)
            (root / "avm-contract.toml").write_text('[build]\nmax_memory_bytes = 262144\n')
            self.assertEqual(fixture_memory_maximum(root, False), 262144)

    def test_copy_keeps_source_bin_targets_and_excludes_installed_tools(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, destination = root / "source", root / "copy"
            for name in ("bin/avm-contract-build", "examples/app/src/bin/probe.rs", "examples/app/target/cache"):
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("source\n")
            measure.copy_sdk_source(source, destination)
            self.assertTrue((destination / "examples/app/src/bin/probe.rs").is_file())
            self.assertFalse((destination / "bin").exists())
            self.assertFalse((destination / "examples/app/target").exists())


if __name__ == "__main__":
    unittest.main()
