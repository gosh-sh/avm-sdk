"""A system ld identity must not be mistaken for Rust's selected native linker."""
import hashlib
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location("linker_build_release", SCRIPTS / "build-release.py")
build_release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build_release)


class LinkerInputs(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.compiler = self.root / "sysroot/bin/rustc"
        self.lld = self.root / "sysroot/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
        self.shim = self.lld.parent / "gcc-ld/ld.lld"
        self.driver = self.root / "cc"
        for path in (self.compiler, self.lld, self.shim, self.driver):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(path.name.encode())

    def capture(self, argv, cwd):
        self.assertEqual(cwd, self.root)
        if argv[0] == str(self.driver) and argv[-1] == "-print-prog-name=ld.lld":
            self.assertEqual(argv[1:-1], ["-B" + str(self.shim.parent), "-fuse-ld=lld"])
            return str(self.shim)
        if argv == [str(self.driver), "--version"]:
            return "driver identity"
        self.assertEqual(argv, [str(self.lld), "-flavor", "gnu", "--version"])
        return "actual LLD identity"

    def test_receipt_hashes_the_forced_sysroot_linker_and_its_driver_shim(self):
        with patch.object(build_release, "capture", side_effect=self.capture):
            result = build_release.native_linker_inputs(self.compiler, self.driver, self.root)
        self.assertEqual(result["path"], str(self.lld))
        self.assertEqual(result["sha256"], hashlib.sha256(b"rust-lld").hexdigest())
        self.assertEqual(result["shim_sha256"], hashlib.sha256(b"ld.lld").hexdigest())
        self.assertEqual(result["version"], "actual LLD identity")
        self.assertEqual(result["rustflags"], ["-Clink-arg=-B" + str(self.shim.parent), "-Clink-arg=-fuse-ld=lld"])

    def test_driver_selecting_a_different_linker_rejects(self):
        with patch.object(build_release, "capture", return_value="/usr/bin/ld.bfd"):
            with self.assertRaises(ValueError):
                build_release.native_linker_inputs(self.compiler, self.driver, self.root)

    def test_missing_pinned_linker_rejects_without_a_system_fallback(self):
        self.lld.unlink()
        with patch.object(build_release, "capture") as capture:
            with self.assertRaises(ValueError):
                build_release.native_linker_inputs(self.compiler, self.driver, self.root)
            capture.assert_not_called()


if __name__ == "__main__":
    unittest.main()
