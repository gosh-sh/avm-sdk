"""Cargo emits both Wasmtime's build script and its engine library."""
import copy
import importlib.util
from pathlib import Path
import sys
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location("engine_build_release", SCRIPTS / "build-release.py")
build_release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build_release)
ENGINE_ID = "registry+https://github.com/rust-lang/crates.io-index#wasmtime@36.0.12"


def artifact(name, kind, features):
    # Independent Cargo compiler-artifact shape, not a build or engine receipt.
    return {"reason": "compiler-artifact", "package_id": ENGINE_ID,
            "target": {"name": name, "kind": [kind]}, "features": features}


class EngineArtifacts(unittest.TestCase):
    def test_build_script_and_library_select_only_the_engine_library(self):
        script = artifact("build-script-build", "custom-build", ["runtime"])
        library = artifact("wasmtime", "lib", ["cranelift", "runtime", "std"])
        self.assertIs(build_release.engine_library_artifact([script, library], ENGINE_ID), library)
        self.assertIs(build_release.engine_library_artifact([library, script], ENGINE_ID), library)

    def test_absent_or_wrong_package_or_nonlibrary_engine_rejects(self):
        wrong_package = artifact("wasmtime", "lib", ["runtime"])
        wrong_package["package_id"] = ENGINE_ID.replace("36.0.12", "36.0.11")
        wrong_reason = artifact("wasmtime", "lib", ["runtime"])
        wrong_reason["reason"] = "build-script-executed"
        for records in ([], [artifact("build-script-build", "custom-build", ["runtime"])],
                        [artifact("wasmtime", "custom-build", ["runtime"])],
                        [artifact("wasmtime", "bin", ["runtime"])],
                        [wrong_package], [wrong_reason]):
            with self.subTest(records=records), self.assertRaisesRegex(ValueError, "engine compiler artifact"):
                build_release.engine_library_artifact(records, ENGINE_ID)

    def test_multiple_library_variants_reject_even_if_features_match(self):
        library = artifact("wasmtime", "lib", ["cranelift", "runtime", "std"])
        for features in (library["features"], ["runtime"]):
            other = copy.deepcopy(library)
            other["features"] = features
            with self.subTest(features=features), self.assertRaisesRegex(ValueError, "engine compiler artifact"):
                build_release.engine_library_artifact([library, other], ENGINE_ID)


if __name__ == "__main__":
    unittest.main()
