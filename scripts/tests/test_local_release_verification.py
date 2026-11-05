"""Verify prepared archives against committed local sources without GitHub."""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_release_integrity import payload, write_archive, digest

SPEC = importlib.util.spec_from_file_location(
    "verify_public_release", Path(__file__).resolve().parents[1] / "verify-public-release.py")
verify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verify)


class LocalVerificationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sdk = self.root / "sdk"
        self.sdk.mkdir()
        self.files = payload()
        for name in ("profiles/wasmtime-release-profile.v1",):
            path = self.sdk / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(self.files[name])
        for name in ("crates/avm-contract-sdk/src/lib.rs", "examples/hello/src/lib.rs"):
            path = self.sdk / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"// source parity fixture\n")
            self.files[name] = path.read_bytes()
        self.git("init", "-q")
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "commit", "-qm", "Source fixture")
        self.commit = self.git("rev-parse", "HEAD").strip()
        self.source = "1" * 40
        self.tag = f"avm-dev-{self.source[:12]}-sdk-{self.commit[:12]}"
        receipt = json.loads(self.files["provenance/build.json"])
        receipt["sdk_commit"] = self.commit
        self.files["provenance/build.json"] = (json.dumps(receipt) + "\n").encode()
        manifest = json.loads(self.files["BUILD-MANIFEST.json"])
        manifest.update(sdk_commit=self.commit, release_tag=self.tag,
                        build_receipt_sha256=digest(self.files["provenance/build.json"]))
        self.files["BUILD-MANIFEST.json"] = (json.dumps(manifest) + "\n").encode()

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.sdk), *args],
                                       text=True, stderr=subprocess.DEVNULL)

    def verify_files(self, expected_sdk=None):
        archive, checksum = write_archive(self.root, self.files)
        with patch.object(verify, "gh_json", side_effect=AssertionError("network forbidden")):
            return verify.verify_local(self.tag, self.root / "result", expected_sdk or self.commit,
                                       self.source, digest(archive.read_bytes()),
                                       archive, checksum, self.sdk)

    def test_local_verification_checks_archive_and_committed_sources_without_network(self):
        # A dirty working file is not the reviewed source identity.
        (self.sdk / "examples/hello/src/lib.rs").write_bytes(b"uncommitted change\n")
        self.verify_files()
        result = json.loads((self.root / "result/local-verification.json").read_text())
        self.assertTrue(result["verified"])
        self.assertEqual(result["mode"], "local")
        self.assertEqual(result["sdk_commit"], self.commit)
        self.assertEqual(result["source_files_verified"], 3)
        self.assertNotIn("release_url", result)

    def test_validly_rehashed_archive_with_different_source_is_rejected(self):
        self.files["examples/hello/src/lib.rs"] += b"changed\n"
        with self.assertRaisesRegex(ValueError, "source/package differs"):
            self.verify_files()

    def test_missing_packaged_source_is_rejected(self):
        del self.files["crates/avm-contract-sdk/src/lib.rs"]
        with self.assertRaisesRegex(ValueError, "source missing from package"):
            self.verify_files()

    def test_wrong_expected_full_head_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "commits differ"):
            self.verify_files("3" * 40)

    def test_tampered_archive_is_rejected_before_source_check(self):
        archive, checksum = write_archive(self.root, self.files)
        expected = digest(archive.read_bytes())
        archive.write_bytes(archive.read_bytes() + b"tamper")
        with patch.object(verify, "gh_json", side_effect=AssertionError("network forbidden")):
            with self.assertRaisesRegex(ValueError, "qualified archive"):
                verify.verify_local(self.tag, self.root / "result", self.commit,
                                    self.source, expected, archive, checksum, self.sdk)


if __name__ == "__main__":
    unittest.main()
