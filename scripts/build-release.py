#!/usr/bin/env python3
"""Build and package only clean, qualified exact heads; never publish automatically."""

import argparse
import gzip
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tomllib

from manual_gates import clean, head
from release import ARCHIVE, BINARIES, ROOT, artifact_records, digest, engine_library_artifact, file_digest, parse_profile, require, verify_archive

RUST_NOTICE_SHA256 = {
    "COPYRIGHT.html": "ed776cb483639823551326501ea4a7d0e63a040a5589d25d68234c223bd9d9f9",
    "COPYRIGHT-library.html": "d93d65853736bde4e8649691233a98848ba709673873aeb39487bc1ca6ee292a",
}


def capture(argv, cwd):
    return subprocess.check_output(argv, cwd=cwd, text=True).strip()


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def tracked_files(root, prefix):
    names = subprocess.check_output(["git", "-C", str(root), "ls-files", "-z", "--", prefix]).decode().split("\0")
    return {name: (root / name).read_bytes() for name in names if name}


def native_linker_inputs(compiler, driver, source):
    directory = compiler.parent.parent / "lib/rustlib/x86_64-unknown-linux-gnu/bin"
    linker, shim = directory / "rust-lld", directory / "gcc-ld/ld.lld"
    require(linker.is_file() and shim.is_file(), "pinned native rust-lld or driver shim is missing")
    # Rust's cc driver normally uses bundled LLD and may retry without it. Supply
    # the selection again as user link arguments, which survive that retry and
    # fail closed if this driver cannot use the pinned linker.
    args = ["-B" + str(shim.parent), "-fuse-ld=lld"]
    selected = capture([str(driver), *args, "-print-prog-name=ld.lld"], source)
    require(Path(selected).resolve() == shim.resolve(), "native driver did not select the pinned LLD shim")
    return {"driver": str(driver), "driver_sha256": file_digest(driver),
            "driver_version": capture([str(driver), "--version"], source),
            "path": str(linker), "sha256": file_digest(linker),
            "version": capture([str(linker), "-flavor", "gnu", "--version"], source),
            "shim": str(shim), "shim_sha256": file_digest(shim),
            "rustflags": ["-Clink-arg=" + arg for arg in args]}


def toolchain_notices(sysroot, rustc_verbose):
    """Preserve Rust 1.93.0's own notices, including non-Cargo library vendors."""
    sysroot = sysroot.resolve()
    texts, files = [], {}
    for name, expected in sorted(RUST_NOTICE_SHA256.items()):
        relative = "share/doc/rust/" + name
        path = sysroot / relative
        require(path.is_file() and path.resolve().is_relative_to(sysroot), "missing pinned Rust toolchain notice")
        data = path.read_bytes()
        require(digest(data) == expected, "Rust 1.93.0 notice differs from its reviewed input")
        destination = "licenses/toolchain/" + name
        files[destination] = data
        texts.append({"file": destination, "sha256": expected, "sysroot_path": relative})
    return {"rustc_verbose": rustc_verbose,
            "scope": "COPYRIGHT-library.html covers the Rust standard library and its vendors, whose code can be linked into these executables. COPYRIGHT.html preserves the broader build-toolchain notices; the compiler and every listed component are not thereby claimed to be distributed. Original per-file and dependency scopes apply.",
            "texts": texts}, files


def reviewed_notice(package, notices):
    index_path = notices / "index.json"
    entries = json.loads(index_path.read_bytes())["packages"] if index_path.is_file() else {}
    entry = entries.get(package["id"])
    require(entry is not None, f"missing third-party license text: {package['id']}")
    directory = Path(package["manifest_path"]).parent
    vcs = json.loads((directory / ".cargo_vcs_info.json").read_bytes())
    require(vcs == entry["vcs"], "upstream notice belongs to a different crate commit or scope")
    require(package["license"] == entry["license_expression"] and package["repository"] == entry["repository_metadata"], "upstream notice grant or repository mismatch")
    paths = set()
    for text in entry["files"]:
        path = (notices / text["file"]).resolve()
        require(path.is_relative_to(notices.resolve()) and path.is_file(), "upstream notice path is invalid")
        require(file_digest(path) == text["sha256"], "upstream notice text changed")
        paths.add(path)
    require(paths, "empty upstream license grant")
    return paths


def license_paths(package, source, notices=None):
    directory = Path(package["manifest_path"]).parent.resolve()
    allowed = directory
    if package["source"] is None:
        allowed = source.resolve()
        require(directory.is_relative_to(allowed), "unrecorded path dependency outside node source")
    elif package["source"].startswith("git+"):
        for parent in (directory, *directory.parents):
            if (parent / ".git").exists():
                allowed = parent
                break
    paths = set()
    if package.get("license_file"):
        path = Path(package["license_file"])
        paths.add(path if path.is_absolute() else directory / path)
    roots = ("license", "licence", "notice", "copying", "copyright")
    for path in directory.rglob("*"):
        relative = path.relative_to(directory)
        if any(part in (".git", "target", "__pycache__") for part in relative.parts):
            continue
        name = path.name.lower()
        notice_name = any(name == root or any(name.startswith(root + suffix) for suffix in (".", "-", "_")) for root in roots)
        notice_directory = any(part.lower() in ("license", "licence", "licenses", "licences") for part in relative.parts[:-1])
        if path.is_file() and (notice_name or notice_directory):
            paths.add(path)
    if not paths and allowed != directory and package["source"] is not None:
        # A Git workspace can keep a shared grant at its checkout root.
        paths.update(path for path in allowed.iterdir() if path.is_file() and any(path.name.lower().startswith(root) for root in roots))
    if not paths and (package["source"] or "").startswith("registry+"):
        notices = notices or Path(__file__).resolve().parents[1] / "licenses/upstream"
        return reviewed_notice(package, notices)
    require(paths, f"missing third-party license text: {package['id']}")
    resolved = {path.resolve() for path in paths}
    require(all(path.is_relative_to(allowed) and path.is_file() for path in resolved), "license path escapes recorded source inputs or is absent")
    return resolved


def verify_registry_source(package, locked, cache=None):
    if not (package["source"] or "").startswith("registry+"):
        return
    matches = [p for p in locked if (p["name"], p["version"], p.get("source")) ==
               (package["name"], package["version"], package["source"])]
    require(len(matches) == 1, "registry package is absent or ambiguous in the pinned lock")
    directory = Path(package["manifest_path"]).parent.resolve()
    cache = cache or Path(os.environ["CARGO_HOME"]) / "registry/cache"
    prefix = package["name"] + "-" + package["version"]
    archive = cache / directory.parent.name / (prefix + ".crate")
    require(file_digest(archive) == matches[0]["checksum"], "cached registry archive differs from the lock checksum")
    expected = set()
    with tarfile.open(archive, "r:gz") as contents:
        for member in contents:
            require(member.isdir() or member.isfile(), "registry source contains a link or special file")
            if not member.isfile():
                continue
            path = Path(member.name)
            require(path.parts[0] == prefix and not path.is_absolute() and ".." not in path.parts, "invalid registry source path")
            relative = path.relative_to(prefix)
            require(relative not in expected, "duplicate registry source file")
            expected.add(relative)
            actual = directory / relative
            require(actual.resolve().is_relative_to(directory) and not actual.is_symlink(), "registry source file escaped its archive")
            with contents.extractfile(member) as stream:
                require(file_digest(actual) == digest(stream.read()), "registry source differs from the locked archive: " + str(relative))
    members = list(directory.rglob("*"))
    require(not any(p.is_symlink() for p in members), "registry source contains an added symlink")
    actual = {p.relative_to(directory) for p in members if p.is_file()}
    require(actual - {Path(".cargo-ok"), Path(".cargo-checksum.json")} == expected, "registry source inventory differs from its locked archive")


def identity(source, sdk):
    clean(source)
    clean(sdk)
    profile = source / "specs/avm-vm/wasmtime-release-profile.v1"
    require(profile.read_bytes() == (sdk / "profiles/wasmtime-release-profile.v1").read_bytes(), "source/SDK profile mismatch")
    lock = source / "Cargo.lock"
    require(capture(["git", "ls-files", "--error-unmatch", "Cargo.lock"], source) == "Cargo.lock", "runtime lock must be committed")
    fields = parse_profile(profile.read_bytes())
    require(fields["cargo_lock_sha256"] == file_digest(lock), "profile does not bind the actual runtime lock")
    locked = tomllib.loads(lock.read_text())["package"]
    for name, key in [("wasmtime", "wasmtime"), ("cranelift-codegen", "cranelift_codegen"), ("wasmparser", "wasmparser")]:
        matches = [p for p in locked if p["name"] == name and p["version"] == fields[key + "_version"]]
        require(len(matches) == 1 and matches[0].get("checksum") == fields[key + "_sha256"], f"profile crate checksum mismatch: {name}")
    guest = tracked_files(source, "node/libs/avm-contract-sdk")
    public = tracked_files(sdk, "crates/avm-contract-sdk")
    require({name.removeprefix("node/libs/avm-contract-sdk/"): data for name, data in guest.items()} ==
            {name.removeprefix("crates/avm-contract-sdk/"): data for name, data in public.items()}, "guest source parity failed")
    return dict(source_commit=head(source), sdk_commit=head(sdk), profile_sha256=file_digest(profile), lock_sha256=file_digest(lock))


def build(source, sdk, output, gates):
    require(os.environ.get("AVM_SDK_POD_QUALIFICATION") == "1", "use the bounded pod launcher")
    require(os.sched_getaffinity(0) <= {0, 1} and os.environ.get("CARGO_BUILD_JOBS") == "2", "build requires CPUs 0-1 and jobs=2")
    ids = identity(source, sdk)
    qualification = json.loads(gates.read_bytes())
    require(qualification.get("passed") is True and qualification.get("gates"), "combined source gates have not passed")
    for field in ("source_commit", "sdk_commit"):
        require(qualification[field] == ids[field], "gates do not belong to these exact heads")
    for gate in qualification["gates"]:
        require(gate["passed"] and gate["exit_code"] == 0, "known-red source gate")
        require(file_digest(gates.parent / (gate["name"] + ".log")) == gate["log_sha256"], "gate log missing or changed")
    output.mkdir(parents=True, exist_ok=False)
    target = Path(os.environ["CARGO_TARGET_DIR"]).resolve()
    require(not target.is_relative_to(source) and not target.is_relative_to(sdk), "target directory must be outside source")
    profile = parse_profile((source / "specs/avm-vm/wasmtime-release-profile.v1").read_bytes())
    rustc = capture(["rustc", "-vV"], source)
    cargo = capture(["cargo", "-vV"], source)
    compiler = Path(capture(["rustc", "--print", "sysroot"], source)) / "bin/rustc"
    require(capture([str(compiler), "-vV"], source) == rustc, "effective compiler/sysroot mismatch")
    require(f"release: {profile['rustc_version']}" in rustc.splitlines() and f"commit-hash: {profile['rustc_commit']}" in rustc.splitlines(), "rustc identity mismatch")
    require(f"release: {profile['cargo_version']}" in cargo.splitlines() and f"commit-hash: {profile['cargo_commit']}" in cargo.splitlines(), "Cargo identity mismatch")
    linker = Path(shutil.which("cc")).resolve()
    native_linker = native_linker_inputs(compiler, linker, source)
    # Keep host feature detection dynamic. Compiling the host itself for v3 can
    # constant-fold is_x86_feature_detected! and bypass the engine's CPU rejection.
    # The accepted profile separately fixes Cranelift's guest target to v3.
    flags = f"--cfg tokio_unstable -Ctarget-cpu=x86-64 --remap-path-prefix={source}=/avm/source --remap-path-prefix={target}=/avm/target"
    flags += " " + " ".join(native_linker["rustflags"])
    environment = os.environ.copy()
    environment.update(RUSTFLAGS=flags, CARGO_INCREMENTAL="0", SOURCE_DATE_EPOCH="0",
                       RUSTC=str(compiler),
                       CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=str(linker),
                       RUSTC_WRAPPER="", RUSTC_WORKSPACE_WRAPPER="")
    environment.pop("CARGO_ENCODED_RUSTFLAGS", None)
    environment.pop("CARGO_BUILD_RUSTFLAGS", None)
    environment.pop("RUSTC_BOOTSTRAP", None)
    argv = ["cargo", "build", "--locked", "--offline", "--release", "--message-format=json", "-p", "avm-contract-build", "--bin", "avm-contract-build", "-p", "avm-test-harness", "--bin", "avm-local-stand"]
    with (output / "cargo-artifacts.jsonl").open("wb") as stdout, (output / "build.log").open("wb") as stderr:
        process = subprocess.run(argv, cwd=source, env=environment, stdout=stdout, stderr=stderr)
    require(process.returncode == 0, "release binary build failed; retained build.log")
    ids_after = identity(source, sdk)
    require(ids_after == ids, "build inputs changed")
    messages = [json.loads(line) for line in (output / "cargo-artifacts.jsonl").read_text().splitlines() if line.startswith("{")]
    artifacts = [entry for entry in messages if entry.get("reason") == "compiler-artifact"]
    executed = {}
    (output / "bin").mkdir()
    platform = {}
    for name in BINARIES:
        entries = [item for item in artifacts if item.get("target", {}).get("name") == name and item.get("executable")]
        require(len(entries) == 1, "missing or ambiguous Cargo executable receipt")
        binary = Path(entries[0]["executable"])
        require(binary.is_relative_to(target), "Cargo output outside selected target")
        shutil.copyfile(binary, output / "bin" / name)
        (output / "bin" / name).chmod(0o755)
        executed["bin/" + name] = file_digest(binary)
        elf = capture(["readelf", "--version-info", str(binary)], source)
        glibc = sorted(set(re.findall(r"GLIBC_([0-9]+\.[0-9]+)", elf)), key=lambda v: tuple(map(int, v.split("."))))
        dynamic = capture(["readelf", "-d", str(binary)], source)
        platform[name] = {"minimum_glibc": glibc[-1] if glibc else None,
                          "needed": re.findall(r"Shared library: \[([^]]+)\]", dynamic)}
    metadata = json.loads(capture(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], source))
    actual_ids = {item["package_id"] for item in artifacts}
    packages = [package for package in metadata["packages"] if package["id"] in actual_ids]
    require({p["id"] for p in packages} == actual_ids, "artifact package absent from locked metadata")
    locked = tomllib.loads((source / "Cargo.lock").read_text())["package"]
    for package in packages:
        verify_registry_source(package, locked)
    wasmtime_ids = {p["id"] for p in packages if p["name"] == "wasmtime"}
    require(len(wasmtime_ids) == 1, "ambiguous or absent built Wasmtime")
    wasmtime_build = engine_library_artifact(artifacts, next(iter(wasmtime_ids)))
    engine_features = sorted(wasmtime_build["features"])
    require(set(profile["wasmtime_required_features"].split(",")) <= set(engine_features),
            "built Wasmtime lacks a required explicit-config feature")
    require("component-model-async" not in engine_features, "unqualified component async build feature")
    require(engine_features == qualification.get("engine_qualification", {}).get("sdk_wasmtime_features"),
            "packaged Wasmtime features differ from the qualified SDK graph")
    rust_notices, license_files = toolchain_notices(compiler.parent.parent, rustc)
    notices = []
    for package in sorted(packages, key=lambda p: p["id"]):
        directory = Path(package["manifest_path"]).parent
        paths = license_paths(package, source)
        texts = []
        for index, path in enumerate(sorted(paths)):
            data = path.read_bytes()
            name = f"licenses/third-party/{digest(package['id'].encode())[:16]}-{index}.txt"
            license_files[name] = data
            texts.append({"file": name, "sha256": digest(data), "upstream_path": os.path.relpath(path, directory)})
        notices.append({"name": package["name"], "version": package["version"], "package_id": package["id"],
                        "source": package["source"], "license_expression": package["license"], "texts": texts})
    for name, data in license_files.items():
        path = output / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    (output / "THIRD-PARTY-NOTICES.json").write_bytes(json_bytes({"scope": "Packages reported by Cargo compiler-artifact for these two host executables, including build dependencies; each retains its own grant. Rust standard-library and toolchain notices are recorded separately.", "packages": notices, "rust_toolchain": rust_notices}))
    receipt = {"schema": "gosh.avm-sdk.build.v1", **ids, "binaries": executed,
               "target": "x86_64-unknown-linux-gnu", "cpu": "x86-64-v3", "native_target_cpu": "x86-64", "platform": platform,
               "rustc_verbose": rustc, "cargo_verbose": cargo,
               "rustc_binary_sha256": file_digest(compiler),
               "native_linker": native_linker,
               "argv": argv, "rustflags": flags, "source_date_epoch": 0,
               "compiler_artifacts_sha256": file_digest(output / "cargo-artifacts.jsonl"),
               "resolved_features": {item["package_id"]: sorted(item["features"]) for item in artifacts},
               "engine_builds": {name: {"package_id": wasmtime_build["package_id"],
                                         "features": engine_features,
                                         "scope": "Actual shared Wasmtime artifact linked by this combined Cargo build"}
                                 for name in BINARIES},
               "source_gates_sha256": file_digest(gates)}
    (output / "build.json").write_bytes(json_bytes(receipt))
    print(json.dumps({"built": ids, "binaries": executed}))


def package(source, sdk, build_root, output):
    ids = identity(source, sdk)
    receipt_bytes = (build_root / "build.json").read_bytes()
    receipt = json.loads(receipt_bytes)
    require(all(receipt.get(key) == value for key, value in ids.items()), "build receipt belongs to different inputs")
    require(file_digest(build_root / "cargo-artifacts.jsonl") == receipt["compiler_artifacts_sha256"], "Cargo artifact evidence changed")
    built = artifact_records((build_root / "cargo-artifacts.jsonl").read_bytes())
    files = {}
    for prefix in ("crates/avm-contract-sdk", "examples", "docs", "scripts", "licenses/upstream", "README.md", "LICENSE.md", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rustfmt.toml", "profiles"):
        files.update(tracked_files(sdk, prefix))
    for name in BINARIES:
        data = (build_root / "bin" / name).read_bytes()
        require(digest(data) == receipt["binaries"]["bin/" + name], "binary changed since actual Cargo build")
        require(file_digest(Path(built[name]["executable"])) == digest(data), "package binary differs from Cargo's actual executable")
        files["bin/" + name] = data
    for path in (build_root / "licenses").rglob("*"):
        if path.is_file():
            require(not path.is_symlink(), "packaged license text cannot be a symlink")
            files[path.relative_to(build_root).as_posix()] = path.read_bytes()
    files["THIRD-PARTY-NOTICES.json"] = (build_root / "THIRD-PARTY-NOTICES.json").read_bytes()
    files["licenses/Acki-Nacki-Node-LICENSE.md"] = (source / "LICENSE.md").read_bytes()
    files["provenance/build.json"] = receipt_bytes
    files["provenance/runtime.Cargo.lock"] = (source / "Cargo.lock").read_bytes()
    files["provenance/cargo-artifacts.jsonl"] = (build_root / "cargo-artifacts.jsonl").read_bytes()
    tag = f"avm-dev-{ids['source_commit'][:12]}-sdk-{ids['sdk_commit'][:12]}"
    files["BUILD-MANIFEST.json"] = json_bytes({
        "schema": "gosh.avm-sdk.release.v2", "bundle_version": "0.2.0", "guest_version": "0.1.1", "release_tag": tag,
        "ackinacki_repository": "gosh-sh/acki-nacki", "ackinacki_commit": ids["source_commit"],
        "sdk_repository": "gosh-sh/avm-sdk", "sdk_commit": ids["sdk_commit"],
        "wasmtime_profile_sha256": ids["profile_sha256"], "runtime_lock_sha256": ids["lock_sha256"],
        "build_receipt_sha256": digest(receipt_bytes), "third_party_notices_sha256": digest(files["THIRD-PARTY-NOTICES.json"]),
        "rust_toolchain": "1.93.0", "target": receipt["target"], "cpu": receipt["cpu"], "platform": receipt["platform"],
        "license_scope": {"guest_and_sdk": "LICENSE.md", "host_tools": "licenses/Acki-Nacki-Node-LICENSE.md", "dependencies": "THIRD-PARTY-NOTICES.json"},
    })
    files["SHA256SUMS"] = "".join(f"{digest(data)}  {name}\n" for name, data in sorted(files.items())).encode()
    output.mkdir(parents=True, exist_ok=False)
    archive = output / ARCHIVE
    with archive.open("xb") as destination, gzip.GzipFile(filename="", fileobj=destination, mode="wb", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as bundle:
            for name, data in sorted(files.items()):
                item = tarfile.TarInfo(ROOT + "/" + name)
                item.size = len(data)
                item.mode = 0o755 if name.startswith("bin/") or name.endswith(".sh") else 0o644
                item.uid = item.gid = item.mtime = 0
                item.uname = item.gname = ""
                bundle.addfile(item, io.BytesIO(data))
    checksum = output / (ARCHIVE + ".sha256")
    checksum.write_text(f"{file_digest(archive)}  {ARCHIVE}\n")
    verify_archive(archive, checksum, tag)
    print(json.dumps({"packaged": tag, "sha256": file_digest(archive)}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("build", "package"))
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--gates", type=Path)
    parser.add_argument("--build-root", type=Path)
    args = parser.parse_args()
    if args.command == "build":
        require(args.gates, "--gates is required")
        build(args.source.resolve(), args.sdk.resolve(), args.output.resolve(), args.gates.resolve())
    else:
        require(args.build_root, "--build-root is required")
        package(args.source.resolve(), args.sdk.resolve(), args.build_root.resolve(), args.output.resolve())


if __name__ == "__main__":
    main()
