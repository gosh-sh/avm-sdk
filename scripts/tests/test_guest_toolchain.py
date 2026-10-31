"""Qualification keeps the host compiler separate from guest compilation."""
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import manual_gates


class GuestToolchainTests(unittest.TestCase):
    def test_guest_commands_do_not_inherit_the_native_host_compiler(self):
        native = {"RUSTUP_TOOLCHAIN": "1.93.0", "CARGO": "/host/cargo", "RUSTC": "/host/rustc",
                  "RUSTC_WRAPPER": "/wrapper-that-must-still-be-rejected"}
        with patch.dict(os.environ, native, clear=True), patch.object(
                manual_gates.subprocess, "check_output", side_effect=["/guest/cargo\n", "/guest/rustc\n"]) as which:
            result = manual_gates.guest_environment()
            self.assertEqual(result["RUSTUP_TOOLCHAIN"], "1.99.1")
            self.assertEqual(result["CARGO"], "/guest/cargo")
            self.assertEqual(result["RUSTC"], "/guest/rustc")
            self.assertEqual(result["RUSTC_WRAPPER"], native["RUSTC_WRAPPER"])
            self.assertEqual(dict(os.environ), native)
            self.assertEqual(which.call_args_list[0].args[0],
                             ["rustup", "which", "--toolchain", "1.99.1", "cargo"])
            self.assertEqual(which.call_args_list[1].args[0],
                             ["rustup", "which", "--toolchain", "1.99.1", "rustc"])


if __name__ == "__main__":
    unittest.main()
