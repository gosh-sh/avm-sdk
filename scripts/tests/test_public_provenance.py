"""Packaging publishes stable public build paths and keeps the private build records."""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location("provenance_build_release", SCRIPTS / "build-release.py")
build_release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build_release)
import release  # noqa: E402  the verifier that build-release.py itself uses

REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
ENGINE = REGISTRY + "#wasmtime@48.0.8"
INDEX = "registry/src/index.crates.io-1949cf8c6b5b557f"
SOURCE, SDK = "1" * 40, "2" * 40
TAG = f"avm-dev-{SOURCE[:12]}-sdk-{SDK[:12]}"
LOCK = b"runtime lock fixture\n"
PUBLIC_ROOTS = {"cargo": "/avm/cargo", "sdk": "/avm/sdk", "source": "/avm/source",
                "target": "/avm/target", "toolchain": "/avm/toolchain"}
# Two build hosts with the same inputs under different private directories.
LAYOUTS = {
    "a": {"source": "checkout-private-a/acki-nacki", "sdk": "checkout-private-a/avm-sdk",
          "target": "target-private-a", "cargo": "cargo-private-a",
          "toolchain": "rustup-private-a/toolchains/1.98.0-x86_64-unknown-linux-gnu"},
    "b": {"source": "work/private-b/src", "sdk": "work/private-b/sdk/checkout",
          "target": "scratch/private-b/target", "cargo": "home/private-b/.cargo",
          "toolchain": "home/private-b/.rustup/toolchains/1.98.0-x86_64-unknown-linux-gnu"},
}
LICENSES = {"avm-contract-build": b"Node license fixture\n", "avm-test-harness": b"Node license fixture\n",
            "avm-types": b"Node license fixture\n", "wasmtime": b"Wasmtime crate grant\n",
            "cranelift-assembler-x64-meta": b"Reviewed upstream grant\n"}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


def profile():
    original = (SCRIPTS.parent / "profiles/wasmtime-release-profile.v1").read_bytes()
    return b"".join(b"cargo_lock_sha256=" + digest(LOCK).encode() + b"\n"
                    if line.startswith(b"cargo_lock_sha256=") else line
                    for line in original.splitlines(keepends=True))


def engine_features():
    line = next(line for line in profile().splitlines() if line.startswith(b"wasmtime_required_features="))
    return line.split(b"=", 1)[1].decode().split(",")


def private_build(base, layout):
    """Write the build root that build() leaves on one build host."""
    roots = {name: base / relative for name, relative in LAYOUTS[layout].items()}
    source, target, cargo, toolchain = roots["source"], roots["target"], roots["cargo"], roots["toolchain"]
    build = base / f"build-private-{layout}"
    write(source / "LICENSE.md", LICENSES["avm-types"])
    write(source / "Cargo.lock", LOCK)
    reviewed = write(roots["sdk"] / "licenses/upstream/wasmtime-0123456789abcdef/LICENSE",
                     LICENSES["cranelift-assembler-x64-meta"])
    wasmtime = cargo / INDEX / "wasmtime-48.0.8"
    crates = [("avm-contract-build", "0.1.0", source / "node/libs/avm-contract-build", source / "LICENSE.md"),
              ("avm-test-harness", "0.1.0", source / "node/libs/avm-test-harness", source / "LICENSE.md"),
              ("avm-types", "0.1.0", source / "node/libs/avm-types", source / "LICENSE.md"),
              ("wasmtime", "48.0.8", wasmtime, write(wasmtime / "LICENSE", LICENSES["wasmtime"])),
              ("cranelift-assembler-x64-meta", "0.135.8", cargo / INDEX / "cranelift-assembler-x64-meta-0.135.8", reviewed)]
    executables = {"avm-contract-build": "avm-contract-build", "avm-test-harness": "avm-local-stand"}
    profile_record = {"opt_level": "3", "debuginfo": 0, "debug_assertions": False, "overflow_checks": False, "test": False}
    records, packages = [], []
    for name, version, directory, license_path in crates:
        registry = directory.is_relative_to(cargo)
        package_id = f"{REGISTRY}#{name}@{version}" if registry else f"path+file://{directory}#{version}"
        kind = "bin" if name in executables else "lib"
        target_name = executables.get(name, name.replace("-", "_"))
        executable = str(target / "release" / target_name) if kind == "bin" else None
        records.append({"reason": "compiler-artifact", "package_id": package_id,
                        "manifest_path": str(directory / "Cargo.toml"),
                        "target": {"kind": [kind], "crate_types": [kind], "name": target_name,
                                   "src_path": str(directory / ("src/main.rs" if kind == "bin" else "src/lib.rs")),
                                   "edition": "2024", "doc": True, "doctest": False, "test": True},
                        "profile": profile_record, "features": engine_features() if name == "wasmtime" else [],
                        "filenames": [executable or str(target / f"release/deps/lib{target_name}-0123456789abcdef.rlib")],
                        "executable": executable, "fresh": False})
        packages.append({"id": package_id, "name": name, "version": version, "source": REGISTRY if registry else None,
                         "manifest_path": str(directory / "Cargo.toml"), "license": "Apache-2.0", "path": license_path})
    out = target / "release/build/wasmtime-0a1b2c3d4e5f6a7b/out"
    records += [{"reason": "compiler-artifact", "package_id": ENGINE, "manifest_path": str(wasmtime / "Cargo.toml"),
                 "target": {"kind": ["custom-build"], "crate_types": ["bin"], "name": "build-script-build",
                            "src_path": str(wasmtime / "build.rs"), "edition": "2024", "doc": False,
                            "doctest": False, "test": False},
                 "profile": profile_record, "features": engine_features(),
                 "filenames": [str(target / "release/build/wasmtime-9f8e7d6c5b4a3921/build-script-build")],
                 "executable": None, "fresh": False},
                {"reason": "build-script-executed", "package_id": ENGINE, "linked_libs": [],
                 "linked_paths": [f"native={out}"], "cfgs": [], "env": [["WASMTIME_OUT", str(out)]], "out_dir": str(out)}]
    binaries = {}
    for name in release.BINARIES:
        data = f"{name} fixture executable\n".encode()
        write(target / "release" / name, data)
        write(build / "bin" / name, data)
        binaries["bin/" + name] = digest(data)
    # Cargo's record stream, canonically ordered by build(), build-finished last.
    artifacts = b"".join(sorted((json.dumps(row) + "\n").encode() for row in records))
    artifacts += (json.dumps({"reason": "build-finished", "success": True}) + "\n").encode()
    write(build / "cargo-artifacts.jsonl", artifacts)
    # Dependency notices as build() records them.
    notices = []
    for crate in sorted(packages, key=lambda p: p["id"]):
        data = crate["path"].read_bytes()
        name = f"licenses/third-party/{digest(crate['id'].encode())[:16]}-0.txt"
        write(build / name, data)
        notices.append({"name": crate["name"], "version": crate["version"], "package_id": crate["id"],
                        "source": crate["source"], "license_expression": crate["license"],
                        "texts": [{"file": name, "sha256": digest(data),
                                   "upstream_path": os.path.relpath(crate["path"], Path(crate["manifest_path"]).parent)}]})
    rustc = "rustc 1.98.0 fixture\nrelease: 1.98.0"
    library = write(build / "licenses/toolchain/COPYRIGHT-library.html", b"Rust standard library grants\n")
    write(build / "THIRD-PARTY-NOTICES.json", build_release.json_bytes({
        "scope": "fixture notices", "packages": notices,
        "rust_toolchain": {"rustc_verbose": rustc, "scope": "fixture toolchain notices", "texts": [{
            "file": "licenses/toolchain/COPYRIGHT-library.html", "sha256": digest(library.read_bytes()),
            "sysroot_path": "share/doc/rust/COPYRIGHT-library.html"}]}}))
    linker = toolchain / "lib/rustlib/x86_64-unknown-linux-gnu/bin"
    receipt = {
        "schema": "gosh.avm-sdk.build.v1", "source_commit": SOURCE, "sdk_commit": SDK,
        "profile_sha256": digest(profile()), "lock_sha256": digest(LOCK), "binaries": binaries,
        "target": "x86_64-unknown-linux-gnu", "cpu": "x86-64-v3", "native_target_cpu": "x86-64",
        "platform": {name: {"minimum_glibc": "2.39", "needed": ["libc.so.6"]} for name in release.BINARIES},
        "rustc_verbose": rustc, "cargo_verbose": "cargo 1.98.0 fixture", "rustc_binary_sha256": "4" * 64,
        "native_linker": {"driver": "/usr/bin/x86_64-linux-gnu-gcc-13", "driver_sha256": "5" * 64,
                          "driver_version": "gcc fixture", "path": str(linker / "rust-lld"), "sha256": "6" * 64,
                          "version": "LLD fixture", "shim": str(linker / "gcc-ld/ld.lld"), "shim_sha256": "7" * 64,
                          "rustflags": ["-Clink-arg=-B" + str(linker / "gcc-ld"), "-Clink-arg=-fuse-ld=lld"]},
        "argv": ["cargo", "build", "--locked", "--offline", "--release", "--message-format=json"],
        "rustflags": f"--cfg tokio_unstable -Ctarget-cpu=x86-64 --remap-path-prefix={source}=/avm/source "
                     f"--remap-path-prefix={target}=/avm/target -Clink-arg=-B{linker / 'gcc-ld'} -Clink-arg=-fuse-ld=lld",
        "source_date_epoch": 0, "compiler_artifacts_sha256": digest(artifacts),
        "resolved_features": {row["package_id"]: sorted(row["features"]) for row in records
                              if row["reason"] == "compiler-artifact"},
        "engine_builds": {name: {"package_id": ENGINE, "features": engine_features(), "scope": "fixture engine"}
                          for name in release.BINARIES},
        "source_gates_sha256": "8" * 64,
        "path_roots": {name: str(path) for name, path in roots.items()},
    }
    write(build / "build.json", build_release.json_bytes(receipt))
    return types.SimpleNamespace(roots=roots, source=source, sdk=roots["sdk"], build=build)


def package(fixture, output, extra=None):
    ids = {"source_commit": SOURCE, "sdk_commit": SDK, "profile_sha256": digest(profile()), "lock_sha256": digest(LOCK)}
    tracked = {"profiles/wasmtime-release-profile.v1": profile(), "LICENSE.md": b"SDK license fixture\n",
               "docs/releases.md": b"# Releases fixture\n", **(extra or {})}

    def tracked_files(root, prefix):
        return {name: data for name, data in tracked.items() if name == prefix or name.startswith(prefix + "/")}

    printed = io.StringIO()
    with patch.object(build_release, "identity", return_value=ids), \
            patch.object(build_release, "tracked_files", side_effect=tracked_files), \
            contextlib.redirect_stdout(printed):
        build_release.package(fixture.source, fixture.sdk, fixture.build, output)
    archive = output / release.ARCHIVE
    files = release.verify_archive(archive, output / (release.ARCHIVE + ".sha256"), TAG)
    return files, archive, printed.getvalue()


def records(data):
    return [json.loads(line) for line in data.splitlines() if line.strip()]


class PublishedProvenance(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)

    def assert_public(self, files):
        for name, data in files.items():
            if not name.startswith("bin/"):
                for needle in (str(self.base).encode(), b"private-"):
                    with self.subTest(published=name, needle=needle):
                        self.assertFalse(needle in data, f"{needle!r} is published in {name}")

    def test_package_publishes_public_paths_and_keeps_every_binding(self):
        fixture = private_build(self.base, "a")
        private = {path: path.read_bytes() for path in fixture.build.rglob("*") if path.is_file()}
        files, _, printed = package(fixture, self.base / "package-a")
        self.assert_public(files)
        # The private build records stay unchanged outside the package and are linked to it.
        self.assertEqual({path: path.read_bytes() for path in fixture.build.rglob("*") if path.is_file()}, private)
        link = json.loads(printed.splitlines()[-1])
        self.assertEqual(link.get("private_build_receipt_sha256"), digest(private[fixture.build / "build.json"]))
        self.assertEqual(link.get("published_build_receipt_sha256"), digest(files["provenance/build.json"]))
        receipt = json.loads(files["provenance/build.json"])
        manifest = json.loads(files["BUILD-MANIFEST.json"])
        for name in release.BINARIES:
            executable = (fixture.roots["target"] / "release" / name).read_bytes()
            self.assertEqual(files["bin/" + name], executable)
            self.assertEqual(receipt["binaries"]["bin/" + name], digest(executable))
        self.assertEqual(files["profiles/wasmtime-release-profile.v1"], profile())
        self.assertEqual(manifest["wasmtime_profile_sha256"], digest(profile()))
        self.assertEqual(files["provenance/runtime.Cargo.lock"], LOCK)
        self.assertEqual(manifest["runtime_lock_sha256"], digest(LOCK))
        self.assertEqual(manifest["build_receipt_sha256"], digest(files["provenance/build.json"]))
        self.assertEqual(receipt["compiler_artifacts_sha256"], digest(files["provenance/cargo-artifacts.jsonl"]))
        self.assertEqual(receipt["path_roots"], PUBLIC_ROOTS)
        # Every build record and feature selection survives under its public name.
        original = json.loads(private[fixture.build / "build.json"])
        source = str(fixture.roots["source"])
        self.assertEqual(receipt["resolved_features"],
                         {key.replace(source, "/avm/source"): value for key, value in original["resolved_features"].items()})
        published = records(files["provenance/cargo-artifacts.jsonl"])
        self.assertEqual(sorted(row["reason"] for row in published),
                         sorted(row["reason"] for row in records(private[fixture.build / "cargo-artifacts.jsonl"])))
        engine = release.engine_library_artifact(published, ENGINE)
        self.assertEqual(engine["features"], engine_features())
        self.assertEqual(engine["manifest_path"], f"/avm/cargo/{INDEX}/wasmtime-48.0.8/Cargo.toml")
        for name in release.BINARIES:
            self.assertEqual(receipt["engine_builds"][name]["features"], engine_features())
            row = next(row for row in published if row.get("executable") and row["target"]["name"] == name)
            self.assertEqual(row["executable"], "/avm/target/release/" + name)
        linker = "/avm/toolchain/lib/rustlib/x86_64-unknown-linux-gnu/bin"
        self.assertEqual(receipt["native_linker"]["path"], linker + "/rust-lld")
        self.assertIn("-Clink-arg=-B" + linker + "/gcc-ld", receipt["rustflags"])
        # License texts keep their bytes; names and origins follow the public package IDs.
        notices = json.loads(files["THIRD-PARTY-NOTICES.json"])
        self.assertEqual(manifest["third_party_notices_sha256"], digest(files["THIRD-PARTY-NOTICES.json"]))
        self.assertEqual({entry["name"] for entry in notices["packages"]}, set(LICENSES))
        reviewed = os.path.relpath("/avm/sdk/licenses/upstream/wasmtime-0123456789abcdef/LICENSE",
                                   f"/avm/cargo/{INDEX}/cranelift-assembler-x64-meta-0.135.8")
        origins = {"wasmtime": "LICENSE", "cranelift-assembler-x64-meta": reviewed}
        for entry in notices["packages"]:
            with self.subTest(package=entry["name"]):
                (text,) = entry["texts"]
                self.assertEqual(text["file"], f"licenses/third-party/{digest(entry['package_id'].encode())[:16]}-0.txt")
                self.assertEqual(files[text["file"]], LICENSES[entry["name"]])
                self.assertEqual(text["sha256"], digest(LICENSES[entry["name"]]))
                self.assertEqual(text["upstream_path"], origins.get(entry["name"], "../../../LICENSE.md"))
        (toolchain,) = notices["rust_toolchain"]["texts"]
        self.assertEqual(files[toolchain["file"]], b"Rust standard library grants\n")

    def test_equivalent_build_roots_publish_identical_bytes(self):
        first, second = private_build(self.base, "a"), private_build(self.base, "b")
        self.assertNotEqual((first.build / "build.json").read_bytes(), (second.build / "build.json").read_bytes())
        files, archive, _ = package(first, self.base / "package-a")
        other, other_archive, _ = package(second, self.base / "package-b")
        self.assertEqual(sorted(name for name in files.keys() | other.keys() if files.get(name) != other.get(name)), [])
        self.assertEqual(archive.read_bytes(), other_archive.read_bytes())

    def test_binary_and_feature_graph_changes_still_reject(self):
        def drop_engine_feature(fixture, rehash):
            path = fixture.build / "cargo-artifacts.jsonl"
            rows = []
            for line in path.read_bytes().splitlines(keepends=True):
                row = json.loads(line)
                if row.get("target", {}).get("name") == "wasmtime" and row["target"]["kind"] == ["lib"]:
                    row["features"] = row["features"][1:]
                    line = (json.dumps(row) + "\n").encode()
                rows.append(line)
            path.write_bytes(b"".join(rows))
            if rehash:
                receipt = json.loads((fixture.build / "build.json").read_bytes())
                receipt["compiler_artifacts_sha256"] = digest(path.read_bytes())
                write(fixture.build / "build.json", build_release.json_bytes(receipt))

        mutations = {
            "packaged-binary": lambda fixture: write(fixture.build / "bin/avm-local-stand", b"other\n"),
            "cargo-executable": lambda fixture: write(fixture.roots["target"] / "release/avm-local-stand", b"other\n"),
            "feature-graph": lambda fixture: drop_engine_feature(fixture, rehash=False),
            "rehashed-feature-graph": lambda fixture: drop_engine_feature(fixture, rehash=True),
        }
        for label, mutate in mutations.items():
            with self.subTest(mutation=label):
                fixture = private_build(self.base / label, "a")
                mutate(fixture)
                with self.assertRaises(ValueError):
                    package(fixture, self.base / label / "package")

    def test_private_path_left_in_published_text_fails_closed(self):
        fixture = private_build(self.base, "a")
        leak = {"docs/leak.md": f"Built in {fixture.roots['source']}\n".encode()}
        with self.assertRaisesRegex(ValueError, "private build path"):
            package(fixture, self.base / "package-leak", leak)
        self.assertFalse((self.base / "package-leak").exists())
        receipt = json.loads((fixture.build / "build.json").read_bytes())
        del receipt["path_roots"]
        write(fixture.build / "build.json", build_release.json_bytes(receipt))
        with self.assertRaisesRegex(ValueError, "path roots"):
            package(fixture, self.base / "package-unrecorded")


if __name__ == "__main__":
    unittest.main()
