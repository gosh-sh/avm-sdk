"""Independent hostile-package fixtures; never use the user's home or commands."""

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "release", Path(__file__).resolve().parents[1] / "release.py"
)
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)
ROOT = "avm-sdk-linux-x86_64"
SOURCE = "1" * 40
SDK = "2" * 40
TAG = f"avm-dev-{SOURCE[:12]}-sdk-{SDK[:12]}"
PROFILE = Path(__file__).resolve().parents[2] / "profiles/wasmtime-release-profile.v1"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def json_bytes(value):
    return (json.dumps(value, sort_keys=True) + "\n").encode()


def replace_artifacts(files, records):
    """Renew every surrounding digest so rejection tests the artifact identity."""
    files["provenance/cargo-artifacts.jsonl"] = b"".join(json_bytes(row) for row in records)
    receipt = json.loads(files["provenance/build.json"])
    receipt["compiler_artifacts_sha256"] = digest(files["provenance/cargo-artifacts.jsonl"])
    files["provenance/build.json"] = json_bytes(receipt)
    manifest = json.loads(files["BUILD-MANIFEST.json"])
    manifest["build_receipt_sha256"] = digest(files["provenance/build.json"])
    files["BUILD-MANIFEST.json"] = json_bytes(manifest)


def payload():
    lock = b"lock fixture\n"
    profile = b"".join(
        b"cargo_lock_sha256=" + digest(lock).encode() + b"\n"
        if line.startswith(b"cargo_lock_sha256=") else line
        for line in PROFILE.read_bytes().splitlines(keepends=True)
    )
    files = {
        "bin/avm-contract-build": b"builder fixture\n",
        "bin/avm-local-stand": b"stand fixture\n",
        "profiles/wasmtime-release-profile.v1": profile,
        "provenance/runtime.Cargo.lock": lock,
        "LICENSE.md": b"SDK MIT license fixture\n",
        "licenses/Acki-Nacki-Node-LICENSE.md": b"Owner grant fixture\n",
        "licenses/toolchain/COPYRIGHT-library.html": b"Rust and standard-library vendor grants\n",
    }
    files["THIRD-PARTY-NOTICES.json"] = json_bytes({"packages": [{"name": "fixture", "texts": [{
        "file": "licenses/Acki-Nacki-Node-LICENSE.md",
        "sha256": digest(files["licenses/Acki-Nacki-Node-LICENSE.md"]),
    }]}], "rust_toolchain": {"rustc_verbose": "rustc exact fixture", "texts": [{
        "file": "licenses/toolchain/COPYRIGHT-library.html",
        "sha256": digest(files["licenses/toolchain/COPYRIGHT-library.html"]),
    }]}})
    features = next(line.split(b"=", 1)[1].decode().split(",") for line in profile.splitlines()
                    if line.startswith(b"wasmtime_required_features="))
    engine_id = "registry+https://github.com/rust-lang/crates.io-index#wasmtime@36.0.12"
    engine = {"package_id": engine_id, "features": features,
              "scope": "Inert independent regression fixture; no compiler executed"}
    files["provenance/cargo-artifacts.jsonl"] = json_bytes({
        "reason": "compiler-artifact", "target": {"name": "wasmtime", "kind": ["lib"]},
        "package_id": engine_id, "features": features,
    }) + b"".join(json_bytes({
        "reason": "compiler-artifact", "target": {"name": name, "kind": ["bin"]},
        "executable": f"/qualification-target/release/{name}",
    }) for name in ("avm-contract-build", "avm-local-stand")) + json_bytes({"reason": "build-finished", "success": True})
    receipt = {
        "schema": "gosh.avm-sdk.build.v1",
        "source_commit": SOURCE,
        "sdk_commit": SDK,
        "profile_sha256": digest(files["profiles/wasmtime-release-profile.v1"]),
        "lock_sha256": digest(files["provenance/runtime.Cargo.lock"]),
        "compiler_artifacts_sha256": digest(files["provenance/cargo-artifacts.jsonl"]),
        "rustc_verbose": "rustc exact fixture",
        "engine_builds": {name: engine.copy() for name in ("avm-contract-build", "avm-local-stand")},
        "binaries": {name: digest(data) for name, data in files.items() if name.startswith("bin/")},
    }
    files["provenance/build.json"] = json_bytes(receipt)
    files["BUILD-MANIFEST.json"] = json_bytes({
        "schema": "gosh.avm-sdk.release.v2",
        "release_tag": TAG,
        "ackinacki_commit": SOURCE,
        "sdk_commit": SDK,
        "wasmtime_profile_sha256": receipt["profile_sha256"],
        "runtime_lock_sha256": receipt["lock_sha256"],
        "build_receipt_sha256": digest(files["provenance/build.json"]),
        "third_party_notices_sha256": digest(files["THIRD-PARTY-NOTICES.json"]),
    })
    return files


def write_archive(directory, files=None, extra=(), corrupt_inner=False):
    files = payload() if files is None else files.copy()
    sums = "".join(f"{digest(data)}  {name}\n" for name, data in sorted(files.items()))
    files["SHA256SUMS"] = sums.encode()
    if corrupt_inner:
        files["bin/avm-local-stand"] += b"tampered"
    archive = directory / f"{ROOT}.tar.gz"
    with tarfile.open(archive, "w:gz") as output:
        for name, data in files.items():
            member = tarfile.TarInfo(f"{ROOT}/{name}")
            member.size = len(data)
            member.mode = 0o755 if name.startswith("bin/") else 0o644
            output.addfile(member, io.BytesIO(data))
        for member, data in extra:
            output.addfile(member, io.BytesIO(data))
    checksum = archive.with_suffix(archive.suffix + ".sha256")
    checksum.write_text(f"{digest(archive.read_bytes())}  {archive.name}\n")
    return archive, checksum


class IntegrityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sdk_home = self.root / "sdk"
        self.bin_dir = self.root / "bin"
        self.sdk_home.mkdir()
        self.previous = self.sdk_home / "previous"
        self.previous.mkdir()
        (self.sdk_home / "current").symlink_to(self.previous, target_is_directory=True)

    def install(self, archive, checksum, tag=TAG):
        return release.install_archive(archive, checksum, tag, self.sdk_home, self.bin_dir)

    def reject(self, archive, checksum, tag=TAG):
        with self.assertRaises((ValueError, OSError)):
            self.install(archive, checksum, tag)
        self.assertEqual((self.sdk_home / "current").resolve(), self.previous)
        self.assertFalse((self.sdk_home / "releases" / tag).exists())
        self.assertFalse(self.bin_dir.exists() and any(self.bin_dir.iterdir()))

    def test_valid_install_is_verified_and_idempotent(self):
        archive, checksum = write_archive(self.root)
        installed = self.install(archive, checksum)
        self.assertEqual((self.sdk_home / "current").resolve(), installed.resolve())
        self.assertEqual((self.bin_dir / "avm-local-stand").read_bytes(), b"stand fixture\n")
        before = {path.relative_to(installed): (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
                  for path in installed.rglob("*") if path.is_file()}
        for name in release.BINARIES:
            self.assertEqual(before[Path("bin") / name][1], 0o755)
        self.assertEqual(before[Path("LICENSE.md")][1], 0o644)
        for _ in range(2):
            self.assertEqual(self.install(archive, checksum), installed)
            self.assertEqual({path.relative_to(installed): (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
                              for path in installed.rglob("*") if path.is_file()}, before)

    def test_reinstall_reports_lost_execute_permissions_without_activation(self):
        files = payload()
        files["scripts/example.sh"] = b"#!/bin/sh\nexit 0\n"
        archive, checksum = write_archive(self.root, files)
        installed = self.install(archive, checksum)
        current = self.sdk_home / "current"
        current.unlink()
        current.symlink_to(self.previous, target_is_directory=True)
        links = {path: (path.readlink(), path.lstat().st_ino)
                 for path in [current, *(self.bin_dir / name for name in release.BINARIES)]}
        for name in [*("bin/" + name for name in release.BINARIES), "scripts/example.sh"]:
            for mode in (0o644, 0o654, 0o745, 0o754):
                with self.subTest(name=name, mode=oct(mode)):
                    command = installed / name
                    command.chmod(mode)
                    result = subprocess.run([
                        sys.executable, str(SPEC.origin), "install", "--archive", str(archive),
                        "--checksum", str(checksum), "--tag", TAG,
                        "--sdk-home", str(self.sdk_home), "--bin-dir", str(self.bin_dir),
                    ], capture_output=True, text=True, check=False)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("missing executable permissions", result.stderr)
                    self.assertIn(str(command), result.stderr)
                    self.assertIn("0755", result.stderr)
                    self.assertIn("rerun", result.stderr)
                    self.assertNotIn("installed verified", result.stdout)
                    self.assertEqual(stat.S_IMODE(command.stat().st_mode), mode)
                    self.assertEqual(command.read_bytes(), files[name])
                    self.assertEqual({path: (path.readlink(), path.lstat().st_ino) for path in links}, links)
                    self.assertEqual(current.resolve(), self.previous)
                    command.chmod(0o755)
        # Following the diagnostic recovers the same verified release.
        self.assertEqual(self.install(archive, checksum), installed)
        self.assertEqual(current.resolve(), installed.resolve())

    def test_modified_existing_files_reject_before_permission_validation(self):
        archive, checksum = write_archive(self.root)
        installed = self.install(archive, checksum)
        command = installed / "bin/avm-contract-build"
        command.chmod(0o644)
        current = self.sdk_home / "current"
        current.unlink()
        current.symlink_to(self.previous, target_is_directory=True)
        for name in ("bin/avm-local-stand", "LICENSE.md"):
            with self.subTest(name=name):
                modified = installed / name
                before = modified.read_bytes()
                modified.write_bytes(before + b"changed")
                with self.assertRaisesRegex(ValueError, "different contents"):
                    self.install(archive, checksum)
                self.assertEqual(modified.read_bytes(), before + b"changed")
                self.assertEqual(stat.S_IMODE(command.stat().st_mode), 0o644)
                self.assertEqual(current.resolve(), self.previous)
                modified.write_bytes(before)

    def test_existing_release_symlinks_cannot_bypass_permission_validation(self):
        archive, checksum = write_archive(self.root)
        installed = self.install(archive, checksum)
        command = installed / "bin/avm-contract-build"
        command.chmod(0o644)
        current = self.sdk_home / "current"
        current.unlink()
        current.symlink_to(self.previous, target_is_directory=True)
        outside = self.root / "outside"
        for path in (command, command.parent, installed, installed.parent):
            with self.subTest(path=str(path)):
                path.rename(outside)
                path.symlink_to(outside, target_is_directory=outside.is_dir())
                files = list(outside.rglob("*")) if outside.is_dir() else [outside]
                before = {entry: (entry.read_bytes(), stat.S_IMODE(entry.stat().st_mode))
                          for entry in files if entry.is_file()}
                with self.assertRaisesRegex(ValueError, "link|real directory"):
                    self.install(archive, checksum)
                self.assertEqual({entry: (entry.read_bytes(), stat.S_IMODE(entry.stat().st_mode))
                                  for entry in before}, before)
                self.assertEqual(current.resolve(), self.previous)
                path.unlink()
                outside.rename(path)

    def test_engine_build_script_does_not_replace_or_duplicate_its_library(self):
        files = payload()
        records = [json.loads(line) for line in files["provenance/cargo-artifacts.jsonl"].splitlines()]
        records.insert(0, {"reason": "compiler-artifact", "package_id": records[0]["package_id"],
                           "target": {"name": "build-script-build", "kind": ["custom-build"]},
                           "features": []})
        replace_artifacts(files, records)
        installed = self.install(*write_archive(self.root, files))
        self.assertEqual((self.sdk_home / "current").resolve(), installed.resolve())

    def test_rehashed_missing_ambiguous_or_nonlibrary_engine_rejects(self):
        for mutation in ("missing", "duplicate", "build-script", "binary"):
            with self.subTest(mutation=mutation):
                files = payload()
                records = [json.loads(line) for line in files["provenance/cargo-artifacts.jsonl"].splitlines()]
                if mutation == "missing":
                    records.pop(0)
                elif mutation == "duplicate":
                    records.insert(0, records[0].copy())
                else:
                    records[0]["target"]["kind"] = ["custom-build" if mutation == "build-script" else "bin"]
                replace_artifacts(files, records)
                self.reject(*write_archive(self.root, files))

    def test_missing_outer_checksum_rejects_before_activation(self):
        archive, checksum = write_archive(self.root)
        checksum.unlink()
        self.reject(archive, checksum)

    def test_outer_tampering_rejects_before_activation(self):
        archive, checksum = write_archive(self.root)
        archive.write_bytes(archive.read_bytes() + b"changed")
        self.reject(archive, checksum)

    def test_inner_tampering_with_valid_outer_checksum_rejects(self):
        self.reject(*write_archive(self.root, corrupt_inner=True))

    def test_decompressed_archive_limit_applies_before_tar_metadata_parsing(self):
        archive, checksum = write_archive(self.root)
        # The compressed fixture fits; its tar headers and data exceed this cap.
        limit = archive.stat().st_size + 512
        with patch.object(release, "MAX_BYTES", limit), patch.object(release.tarfile, "open") as parser:
            self.reject(archive, checksum)
            parser.assert_not_called()

    def test_checksum_filename_cannot_select_another_file(self):
        archive, checksum = write_archive(self.root)
        checksum.write_text(f"{digest(archive.read_bytes())}  ../escape\n")
        self.reject(archive, checksum)

    def test_manifest_tag_must_bind_both_full_commits(self):
        files = payload()
        manifest = json.loads(files["BUILD-MANIFEST.json"])
        manifest["ackinacki_commit"] = "3" * 40
        files["BUILD-MANIFEST.json"] = json_bytes(manifest)
        self.reject(*write_archive(self.root, files))

    def test_receipt_must_bind_actual_binary_despite_valid_inner_checksums(self):
        files = payload()
        files["bin/avm-local-stand"] = b"unrelated but checksummed binary"
        self.reject(*write_archive(self.root, files))

    def test_per_binary_engine_features_must_match_the_actual_cargo_artifact(self):
        for missing in (False, True):
            with self.subTest(missing=missing):
                files = payload()
                receipt = json.loads(files["provenance/build.json"])
                if missing:
                    del receipt["engine_builds"]["avm-local-stand"]
                else:
                    receipt["engine_builds"]["avm-local-stand"]["features"].remove("threads")
                files["provenance/build.json"] = json_bytes(receipt)
                manifest = json.loads(files["BUILD-MANIFEST.json"])
                manifest["build_receipt_sha256"] = digest(files["provenance/build.json"])
                files["BUILD-MANIFEST.json"] = json_bytes(manifest)
                self.reject(*write_archive(self.root, files))

    def test_toolchain_notices_cannot_be_omitted_or_bound_to_another_compiler(self):
        for mismatch in (False, True):
            with self.subTest(mismatch=mismatch):
                files = payload()
                notices = json.loads(files["THIRD-PARTY-NOTICES.json"])
                if mismatch:
                    notices["rust_toolchain"]["rustc_verbose"] = "different compiler"
                else:
                    del notices["rust_toolchain"]
                files["THIRD-PARTY-NOTICES.json"] = json_bytes(notices)
                manifest = json.loads(files["BUILD-MANIFEST.json"])
                manifest["third_party_notices_sha256"] = digest(files["THIRD-PARTY-NOTICES.json"])
                files["BUILD-MANIFEST.json"] = json_bytes(manifest)
                self.reject(*write_archive(self.root, files))

    def test_receipt_source_and_lock_are_bound_independently(self):
        for field, value in [("source_commit", "3" * 40), ("lock_sha256", "0" * 64)]:
            with self.subTest(field=field):
                files = payload()
                receipt = json.loads(files["provenance/build.json"])
                receipt[field] = value
                files["provenance/build.json"] = json_bytes(receipt)
                manifest = json.loads(files["BUILD-MANIFEST.json"])
                manifest["build_receipt_sha256"] = digest(files["provenance/build.json"])
                files["BUILD-MANIFEST.json"] = json_bytes(manifest)
                self.reject(*write_archive(self.root, files))

    def test_fully_rehashed_contradictory_or_malformed_profile_preserves_installation(self):
        valid = payload()["profiles/wasmtime-release-profile.v1"]
        lock_line = next(line for line in valid.splitlines(keepends=True)
                         if line.startswith(b"cargo_lock_sha256="))
        cases = {
            # All archive, manifest and receipt hashes are independently renewed.
            # Only the profile's own claim contradicts the packaged lock.
            "contradictory-lock": valid.replace(lock_line, b"cargo_lock_sha256=" + b"0" * 64 + b"\n"),
            "missing-lock": valid.replace(lock_line, b""),
            "duplicate-lock": valid + lock_line,
            "duplicate-setting": valid + b"consume_fuel=true\n",
            "missing-setting": valid.replace(b"consume_fuel=true\n", b""),
            "malformed-line": valid + b"not-an-assignment\n",
            "unknown-field": valid + b"unapproved=true\n",
            "malformed-hash": valid.replace(lock_line, b"cargo_lock_sha256=xyz\n"),
            "malformed-boolean": valid.replace(b"consume_fuel=true\n", b"consume_fuel=yes\n"),
            "whitespace": valid.replace(b"consume_fuel=true\n", b"consume_fuel= true\n"),
            "crlf": valid.replace(b"\n", b"\r\n"),
            "vertical-tab": valid.replace(b"\n", b"\v", 1),
            "form-feed": valid.replace(b"\n", b"\f", 1),
            "unterminated": valid[:-1],
            "invalid-encoding": valid + b"\xff\n",
        }
        (self.previous / "retained-contract").write_bytes(b"historical state must survive")
        self.bin_dir.mkdir()
        unrelated = self.bin_dir / "other-command"
        unrelated.write_bytes(b"unrelated command")
        for label, profile in cases.items():
            with self.subTest(label=label):
                files = payload()
                files["profiles/wasmtime-release-profile.v1"] = profile
                receipt = json.loads(files["provenance/build.json"])
                receipt["profile_sha256"] = digest(profile)
                files["provenance/build.json"] = json_bytes(receipt)
                manifest = json.loads(files["BUILD-MANIFEST.json"])
                manifest["wasmtime_profile_sha256"] = digest(profile)
                manifest["build_receipt_sha256"] = digest(files["provenance/build.json"])
                files["BUILD-MANIFEST.json"] = json_bytes(manifest)
                archive, checksum = write_archive(self.root, files)
                with self.assertRaisesRegex(ValueError, "profile"):
                    self.install(archive, checksum)
                self.assertEqual((self.sdk_home / "current").resolve(), self.previous)
                self.assertEqual((self.previous / "retained-contract").read_bytes(),
                                 b"historical state must survive")
                self.assertFalse((self.sdk_home / "releases" / TAG).exists())
                self.assertEqual(list(self.bin_dir.iterdir()), [unrelated])
                self.assertEqual(unrelated.read_bytes(), b"unrelated command")

    def test_receipt_cannot_reuse_another_cargo_artifact_log(self):
        files = payload()
        files["provenance/cargo-artifacts.jsonl"] = b"{}\n"
        self.reject(*write_archive(self.root, files))

    def test_empty_artifact_log_is_not_build_provenance_even_when_rehashed(self):
        files = payload()
        files["provenance/cargo-artifacts.jsonl"] = b""
        receipt = json.loads(files["provenance/build.json"])
        receipt["compiler_artifacts_sha256"] = digest(b"")
        files["provenance/build.json"] = json_bytes(receipt)
        manifest = json.loads(files["BUILD-MANIFEST.json"])
        manifest["build_receipt_sha256"] = digest(files["provenance/build.json"])
        files["BUILD-MANIFEST.json"] = json_bytes(manifest)
        self.reject(*write_archive(self.root, files))

    def test_unsafe_paths_types_and_duplicates_reject_before_extraction(self):
        cases = [
            ("../escape", tarfile.REGTYPE, ""),
            ("/absolute", tarfile.REGTYPE, ""),
            (f"{ROOT}/../escape", tarfile.REGTYPE, ""),
            (f"{ROOT}/bin/link", tarfile.SYMTYPE, "../../../escape"),
            (f"{ROOT}/bin/link", tarfile.LNKTYPE, f"{ROOT}/LICENSE.md"),
            (f"{ROOT}/device", tarfile.CHRTYPE, ""),
            (f"{ROOT}/LICENSE.md", tarfile.REGTYPE, ""),
            (f"{ROOT}/unlisted", tarfile.REGTYPE, ""),
        ]
        for name, kind, link in cases:
            with self.subTest(name=name, kind=kind):
                member = tarfile.TarInfo(name)
                member.type = kind
                member.linkname = link
                self.reject(*write_archive(self.root, extra=[(member, b"")]))
                self.assertFalse((self.root / "escape").exists())

    def test_tag_cannot_escape_release_directory(self):
        archive, checksum = write_archive(self.root)
        self.reject(archive, checksum, "../../escape")

    def test_existing_release_is_immutable(self):
        archive, checksum = write_archive(self.root)
        installed = self.install(archive, checksum)
        before = (installed / "bin/avm-local-stand").read_bytes()
        files = payload()
        files["LICENSE.md"] += b"changed"
        archive, checksum = write_archive(self.root, files)
        with self.assertRaises(ValueError):
            self.install(archive, checksum)
        self.assertEqual((installed / "bin/avm-local-stand").read_bytes(), before)

    def test_existing_user_command_is_preserved(self):
        self.bin_dir.mkdir()
        command = self.bin_dir / "avm-local-stand"
        command.write_bytes(b"user-owned")
        with self.assertRaises(ValueError):
            self.install(*write_archive(self.root))
        self.assertEqual(command.read_bytes(), b"user-owned")
        self.assertEqual((self.sdk_home / "current").resolve(), self.previous)


if __name__ == "__main__":
    unittest.main()
