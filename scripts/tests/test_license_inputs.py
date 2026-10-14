"""Notice collection preserves vendor scopes and rejects paths outside source inputs."""

import importlib.util
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location("build_release", SCRIPTS / "build-release.py")
build_release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build_release)


class LicenseInputs(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        self.crate = self.source / "libs/example"
        self.crate.mkdir(parents=True)
        (self.crate / "Cargo.toml").write_text("fixture")
        self.package = {"id": "example", "source": None,
                        "manifest_path": str(self.crate / "Cargo.toml"), "license_file": None}

    def collect(self):
        return build_release.license_paths(self.package, self.source)

    def test_nested_vendor_grant_is_retained_with_the_crate_grant(self):
        own = self.crate / "LICENSE-MIT"
        own.write_text("wrapper grant")
        vendor = self.crate / "vendor/native/COPYING"
        vendor.parent.mkdir(parents=True)
        vendor.write_text("different native grant")
        self.assertEqual(self.collect(), {own, vendor})

    def test_workspace_inherited_license_keeps_its_real_scope(self):
        owner = self.source / "LICENSE.md"
        owner.write_text("actual owner grant")
        self.package["license_file"] = "../../LICENSE.md"
        self.assertEqual(self.collect(), {owner})

    def test_declared_license_cannot_escape_the_build_source(self):
        outside = self.root / "private"
        outside.write_text("not a license")
        self.package["license_file"] = str(outside)
        with self.assertRaises(ValueError):
            self.collect()

    def test_license_symlink_cannot_escape_the_build_source(self):
        outside = self.root / "private"
        outside.write_text("not a license")
        (self.crate / "LICENSE").symlink_to(outside)
        with self.assertRaises(ValueError):
            self.collect()

    def test_expression_alone_is_not_a_bundled_notice(self):
        self.package["license"] = "MIT"
        with self.assertRaises(ValueError):
            self.collect()

    def test_reviewed_upstream_grant_requires_exact_crate_vcs_scope_and_hash(self):
        notices = self.root / "reviewed"
        notices.mkdir()
        license_path = notices / "LICENSE"
        license_path.write_bytes(b"actual upstream grant")
        vcs = {"git": {"sha1": "a" * 40}, "path_in_vcs": "crates/example"}
        (self.crate / ".cargo_vcs_info.json").write_text(json.dumps(vcs))
        self.package.update(source="registry+https://github.com/rust-lang/crates.io-index", license="MIT", repository="https://example.invalid/upstream")
        entry = {"vcs": vcs, "repository_metadata": self.package["repository"],
                 "license_expression": "MIT", "files": [{"file": "LICENSE", "sha256": hashlib.sha256(license_path.read_bytes()).hexdigest()}]}
        (notices / "index.json").write_text(json.dumps({"packages": {"example": entry}}))
        self.assertEqual(build_release.license_paths(self.package, self.source, notices), {license_path})
        changed = dict(vcs, path_in_vcs="different/scope")
        (self.crate / ".cargo_vcs_info.json").write_text(json.dumps(changed))
        with self.assertRaises(ValueError):
            build_release.license_paths(self.package, self.source, notices)
        (self.crate / ".cargo_vcs_info.json").write_text(json.dumps(vcs))
        license_path.write_bytes(b"invented grant")
        with self.assertRaises(ValueError):
            build_release.license_paths(self.package, self.source, notices)

    def test_registry_source_bytes_and_package_checksum_are_independently_bound(self):
        self.package.update(name="example", version="1.2.3", source="registry+https://github.com/rust-lang/crates.io-index")
        cache = self.root / "cache"
        (cache / "libs").mkdir(parents=True)
        archive = cache / "libs/example-1.2.3.crate"
        with tarfile.open(archive, "w:gz") as output:
            member = tarfile.TarInfo("example-1.2.3/Cargo.toml")
            member.size = 7
            output.addfile(member, io.BytesIO(b"fixture"))
        lock = [{"name": "example", "version": "1.2.3", "source": self.package["source"], "checksum": hashlib.sha256(archive.read_bytes()).hexdigest()}]
        build_release.verify_registry_source(self.package, lock, cache)
        (self.crate / "Cargo.toml").write_text("modified source")
        with self.assertRaises(ValueError):
            build_release.verify_registry_source(self.package, lock, cache)
        (self.crate / "Cargo.toml").write_text("fixture")
        archive.write_bytes(archive.read_bytes() + b"altered archive")
        with self.assertRaises(ValueError):
            build_release.verify_registry_source(self.package, lock, cache)

    def test_standard_library_notices_preserve_text_and_reject_missing_or_changed_inputs(self):
        sysroot = self.root / "sysroot"
        notice = sysroot / "share/doc/rust/COPYRIGHT-library.html"
        notice.parent.mkdir(parents=True)
        original = b"<html>Rust grant; distinct embedded vendor grants</html>"
        notice.write_bytes(original)
        pinned = {"COPYRIGHT-library.html": hashlib.sha256(original).hexdigest()}
        with patch.object(build_release, "RUST_NOTICE_SHA256", pinned):
            inventory, files = build_release.toolchain_notices(sysroot, "exact rustc identity")
            name = "licenses/toolchain/COPYRIGHT-library.html"
            self.assertEqual(files, {name: original})
            self.assertEqual(inventory["rustc_verbose"], "exact rustc identity")
            self.assertEqual(inventory["texts"], [{"file": name, "sha256": pinned[notice.name], "sysroot_path": "share/doc/rust/COPYRIGHT-library.html"}])
            notice.write_bytes(original + b"changed")
            with self.assertRaises(ValueError):
                build_release.toolchain_notices(sysroot, "exact rustc identity")
            notice.unlink()
            with self.assertRaises(ValueError):
                build_release.toolchain_notices(sysroot, "exact rustc identity")


if __name__ == "__main__":
    unittest.main()
