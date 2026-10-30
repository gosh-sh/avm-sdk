"""Build real contracts from independent roots and compare exact artifacts."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

from blake3 import blake3


def source_inventory(root):
    return {path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(root.rglob("*"))
            if path.is_file() and "target" not in path.relative_to(root).parts}


def add_path_dependency(root, case):
    if case == "sdk":
        return
    if case in ("root-package", "standalone-package"):
        dependency = root / "vendor/root-package"
        dependency.mkdir(parents=True)
        (dependency / "Cargo.toml").write_text(
            '[workspace]\nresolver="2"\n[workspace.package]\n'
            'edition="2021"\nversion="0.1.0"\nlicense="MIT"\n'
            '[package]\nname="relocation-input"\nversion.workspace=true\n'
            'edition.workspace=true\nlicense.workspace=true\n')
        if case == "standalone-package":
            (dependency / "Cargo.toml").write_text(
                '[package]\nname="relocation-input"\nversion="0.1.0"\n'
                'edition="2021"\nlicense="MIT"\n')
            workspace_manifest = root / "Cargo.toml"
            workspace_manifest.write_text(workspace_manifest.read_text().replace(
                '[workspace]\n', '[workspace]\nexclude=["vendor/root-package"]\n', 1))
        value = "pub const EXPECTED: u128 = 37;\n"
    else:
        repository = root / "vendor/schema-repository"
        workspace = repository / "rust"
        dependency = workspace / "crates/schema"
        dependency.mkdir(parents=True)
        (repository / "schemas").mkdir()
        (repository / "schemas/value.txt").write_text("37\n")
        (workspace / "Cargo.toml").write_text(
            '[workspace]\nresolver="2"\nmembers=["crates/schema"]\n'
            '[workspace.package]\nedition="2021"\n')
        (dependency / "Cargo.toml").write_text(
            '[package]\nname="relocation-input"\nversion="0.1.0"\n'
            'edition.workspace=true\nbuild="build.rs"\n')
        input_expression = ('root.ancestors().nth(3).unwrap().join("schemas/value.txt")'
                            if case == "build-script-parents" else
                            'root.join("../../../schemas/value.txt")')
        if case == "source-root":
            shared = root.parent / "shared"
            shared.mkdir()
            (shared / "value.txt").write_text("37\n")
            input_expression = 'root.ancestors().nth(6).unwrap().join("shared/value.txt")'
        helper = ""
        if case in ("build-script-helper", "source-root"):
            (repository / "generate.py").write_text(
                'import subprocess, sys\n'
                'subprocess.run([sys.executable, "-c", '
                '"from pathlib import Path; import sys; print(Path(sys.argv[1]).read_text().strip())", '
                'sys.argv[1]], check=True)\n')
            helper = (
                ' let output = std::process::Command::new("python3")\n'
                '  .arg(root.join("../../../generate.py")).arg(&input).output().unwrap();\n'
                ' assert!(output.status.success(), "helper failed: {}", String::from_utf8_lossy(&output.stderr));\n'
                ' let generated: u128 = String::from_utf8(output.stdout).unwrap().trim().parse().unwrap();\n'
                ' assert_eq!(generated, value);\n'
                ' let value = generated;\n')
        (dependency / "build.rs").write_text(
            'fn main() {\n'
            ' let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());\n'
            f' let input = {input_expression};\n'
            ' let value: u128 = std::fs::read_to_string(&input).unwrap().trim().parse().unwrap();\n'
            ' assert_eq!(value, 37, "the original shared schema must be read");\n' + helper +
            ' println!("cargo:rerun-if-changed={}", input.display());\n'
            ' let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());\n'
            ' std::fs::write(out.join("schema.rs"), format!("pub const EXPECTED: u128 = {value};\\n")).unwrap();\n'
            '}\n')
        value = 'include!(concat!(env!("OUT_DIR"), "/schema.rs"));\n'
    (dependency / "src").mkdir()
    (dependency / "src/lib.rs").write_text(
        '#![no_std]\n' + value + '\n#[inline(never)]\n'
        'pub fn matches(value: u128) -> bool { value == EXPECTED }\n')
    manifest = root / "examples/contract-b/Cargo.toml"
    relative = os.path.relpath(dependency, manifest.parent)
    with manifest.open("a") as stream:
        stream.write(f'relocation-input = {{ path = "{relative}" }}\n')
    guest = root / "examples/contract-b/src/lib.rs"
    text = guest.read_text()
    assert "ecc0.amount != VALUE_ECC0" in text
    guest.write_text(text.replace("ecc0.amount != VALUE_ECC0",
                                  "!relocation_input::matches(ecc0.amount)"))


def check_case(sdk, builder, out, case):
    out.mkdir(parents=True, exist_ok=False)
    roots = [out / "a", out / "different length with spaces # ü" / "b"]
    source_roots = [root / "project" if case == "source-root" else root for root in roots]
    for root in source_roots:
        root.mkdir(parents=True)
        for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rustfmt.toml", "LICENSE.md"):
            shutil.copy2(sdk / name, root / name)
        shutil.copytree(sdk / "crates", root / "crates", ignore=shutil.ignore_patterns("target"))
        shutil.copytree(sdk / "examples/contract-b", root / "examples/contract-b",
                        ignore=shutil.ignore_patterns("target"))
        add_path_dependency(root, case)
        if case != "sdk":
            command = ["cargo", "generate-lockfile", "--offline", "--manifest-path",
                       str(root / "examples/contract-b/Cargo.toml")]
            result = subprocess.run(command, capture_output=True, text=True)
            (out / f"lock-{source_roots.index(root)}.log").write_text(result.stdout + result.stderr)
            assert result.returncode == 0, "fixture lock generation failed"
    before = [source_inventory(root) for root in roots]
    assert before[0] == before[1], "independent roots must contain byte-identical sources and locks"
    results = []
    for index, root in enumerate(source_roots):
        command = [str(builder), "build", "--manifest-path", str(root / "examples/contract-b/Cargo.toml")]
        if case == "source-root":
            command += ["--source-root", str(roots[index])]
        if case == "sdk" and index == 0:
            # A stable source identity must not depend on inherited descriptors.
            # The public builder is still invoked directly, never a compiler wrapper.
            command = ["bash", "--noprofile", "--norc", "-p", "-c",
                       'exec 16<"$1"; shift; "$@"; status=$?; '
                       '[[ -r /dev/fd/16 ]] || exit 99; exit "$status"',
                       "--", str(root / "LICENSE.md"), *command]
        environment = os.environ.copy()
        # Native host LTO is not a guest build input. Leave the builder's pinned
        # compiler validation and wrapper rejection intact.
        environment.pop("CARGO_PROFILE_RELEASE_LTO", None)
        result = subprocess.run(command, capture_output=True, text=True, env=environment)
        (out / f"build-{index}.log").write_text(result.stdout + result.stderr)
        row = {"command": command, "exit": result.returncode}
        results.append(row)
        (out / "commands.json").write_text(json.dumps(results, indent=2) + "\n")
        assert result.returncode == 0, f"build in root{index} failed; see its retained log"
        reported = dict(line.split() for line in result.stdout.splitlines())
        row["artifacts"] = {}
        for name in ("contract.wasm", "contract.acab", "contract.descriptor"):
            data = (root / "examples/contract-b/target/avm" / name).read_bytes()
            # WASM code_hash and descriptor_hash are raw BLAKE3; ABI hash uses
            # its protocol domain and is independently covered by the ABI gate.
            if name != "contract.acab":
                assert reported[name] == blake3(data).hexdigest()
            row["artifacts"][name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
                                       "reported_hash": reported[name]}
        assert source_inventory(roots[index]) == before[index], "builder changed a source or dependency lock"
        (out / "commands.json").write_text(json.dumps(results, indent=2) + "\n")
        if case == "sdk" and index == 0:
            excluded = [str(builder), "build", "--manifest-path", str(root / "examples/contract-b/Cargo.toml"),
                        "--source-root", str(root / "examples/contract-b")]
            rejected = subprocess.run(excluded, capture_output=True, text=True, env=environment)
            (out / "excluded-source-root.log").write_text(rejected.stdout + rejected.stderr)
            assert rejected.returncode != 0 and "--source-root must contain" in rejected.stderr
            assert source_inventory(root) == before[index]
            for name, artifact in row["artifacts"].items():
                assert hashlib.sha256((root / "examples/contract-b/target/avm" / name).read_bytes()).hexdigest() == artifact["sha256"]
            (out / "excluded-source-root.json").write_text(json.dumps(
                {"command": excluded, "exit": rejected.returncode, "expected_rejection": True}, indent=2) + "\n")
    for name in ("contract.wasm", "contract.acab", "contract.descriptor"):
        left, right = [(root / "examples/contract-b/target/avm" / name).read_bytes() for root in source_roots]
        assert left == right, f"{name} changes with the source root"
        assert results[0]["artifacts"][name] == results[1]["artifacts"][name]
    wasm = (source_roots[0] / "examples/contract-b/target/avm/contract.wasm").read_bytes()
    assert all(str(root).encode() not in wasm for root in roots), "absolute source root leaked into WASM"
    summary = {"passed": True, "case": case, "source_files": len(before[0]), "source_inventory": before[0],
               "builder_sha256": hashlib.sha256(builder.read_bytes()).hexdigest(), "builds": results}
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return {"passed": True, "case": case, "artifacts": results[0]["artifacts"]}


def check_source_history(sdk, builder, out):
    # Seed the same real SDK contract and dependency in two independent roots.
    check_case(sdk, builder, out, "root-package")
    roots = [out / "a", out / "different length with spaces # ü" / "b"]
    results = []

    def artifacts(root):
        directory = root / "examples/contract-b/target/avm"
        return {name: (directory / name).read_bytes()
                for name in ("contract.wasm", "contract.acab", "contract.descriptor")}

    def build(root, label, stale=False):
        command = [str(builder), "build", "--manifest-path",
                   str(root / "examples/contract-b/Cargo.toml")]
        environment = os.environ.copy()
        environment.pop("CARGO_PROFILE_RELEASE_LTO", None)
        before = source_inventory(root)
        previous = artifacts(root)
        result = subprocess.run(command, capture_output=True, text=True, env=environment)
        (out / (label + ".log")).write_text(result.stdout + result.stderr)
        results.append({"label": label, "command": command, "exit": result.returncode,
                        "expected_stale_rejection": stale})
        (out / "history-commands.json").write_text(json.dumps(results, indent=2) + "\n")
        assert source_inventory(root) == before, "builder changed current sources"
        if stale:
            assert result.returncode != 0 and "stale generated source tree" in result.stderr, \
                "a deleted or renamed input must reject before compilation"
            assert artifacts(root) == previous, "rejected build replaced published artifacts"
        else:
            assert result.returncode == 0, f"{label} failed; see retained log"

    for action in ("delete", "rename"):
        for root in roots:
            (root / "vendor/root-package/history.txt").write_text("source inventory input\n")
        build(roots[0], action + "-seed")
        for root in roots:
            original = root / "vendor/root-package/history.txt"
            if action == "delete":
                original.unlink()
            else:
                original.rename(original.with_name("renamed.txt"))
        current = source_inventory(roots[0])
        assert source_inventory(roots[1]) == current
        build(roots[0], action + "-stale", stale=True)
        # Follow the error's repair for this test-owned generated namespace.
        # Keep the lock and every original source; never delete a foreign tree.
        base = roots[0] / "examples/contract-b/target/avm-source-trees"
        aliases = [p for p in base.iterdir() if p.is_dir()]
        assert len(aliases) == 1 and not aliases[0].is_symlink()
        assert len(aliases[0].name) == 64 and all(c in "0123456789abcdef" for c in aliases[0].name)
        shutil.rmtree(aliases[0])
        build(roots[0], action + "-repaired")
        build(roots[1], action + "-independent")
        assert artifacts(roots[0]) == artifacts(roots[1]), "repair changed relocated artifacts"
        assert source_inventory(roots[0]) == source_inventory(roots[1]) == current
    summary = {"passed": True, "case": "source-history", "builds": results,
               "builder_sha256": hashlib.sha256(builder.read_bytes()).hexdigest()}
    (out / "history-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return {"passed": True, "case": "source-history", "mutations": ["delete", "rename"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--builder", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    cases = ("sdk", "root-package", "standalone-package", "build-script-inputs",
             "build-script-parents", "build-script-helper", "source-root", "source-history")
    parser.add_argument("--case", choices=cases)
    args = parser.parse_args()
    sdk, builder, out = args.sdk.resolve(), args.builder.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    results = []
    for case in [args.case] if args.case else cases:
        if case == "source-history":
            results.append(check_source_history(sdk, builder, out / case))
        else:
            results.append(check_case(sdk, builder, out / case, case))
    summary = {"passed": True, "cases": results}
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
