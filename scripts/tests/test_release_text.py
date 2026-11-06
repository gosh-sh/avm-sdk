"""Release text checks use isolated repositories and exact revision ranges."""

from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "check-release-text.py"


class ReleaseTextTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.git("init", "-q")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        (self.root / "README.md").write_text("Release fixture\n")
        self.git("add", ".")
        self.git("commit", "-qm", "Initial fixture")
        self.base = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args], text=True,
                                       stderr=subprocess.DEVNULL)

    def check(self):
        return subprocess.run(["python3", "-B", str(SCRIPT), "--repo", str(self.root),
                               "--base", self.base], capture_output=True, text=True)

    def reject(self):
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("release text:", result.stderr)

    def test_clean_tree_and_commit_pass(self):
        self.git("commit", "--allow-empty", "-qm", "Prepare release")
        self.assertEqual(self.check().returncode, 0)

    def test_commit_footer_is_rejected(self):
        footer = "Co\u002dAuthored\u002dBy: Fixture <fixture@example.invalid>"
        self.git("commit", "--allow-empty", "-qm", "Prepare release\n\n" + footer)
        self.reject()

    def test_commit_generation_line_is_rejected(self):
        self.git("commit", "--allow-empty", "-qm", "Prepare release\n\nGenerat\u0065d with a tool")
        self.reject()

    def test_earlier_commit_in_range_is_not_hidden_by_clean_tip(self):
        self.git("commit", "--allow-empty", "-qm", "Prepare release\n\nCo\u002dAuthored\u002dBy: Fixture")
        self.git("commit", "--allow-empty", "-qm", "Finish release")
        self.reject()

    def test_restricted_words_in_tracked_text_are_rejected(self):
        cases = ("Cl\u0061ude", "Anthr\u006fpic", "Cod\u0065x", "Open\u0041I",
                 "GP\u0054-6", "Astr\u0061", "Cop\u0069lot", "L\u004cM", "A\u0049 ag\u0065nt",
                 "ChatG\u0050T", "Gem\u0069ni", "DeepS\u0065ek", "Generat\u0065d with a tool")
        for index, value in enumerate(cases):
            with self.subTest(case=index):
                (self.root / "README.md").write_text(value + "\n")
                self.reject()

    def test_restricted_identifiers_are_rejected(self):
        cases = ("import open\u0061i_client", "cl\u0061ude_code", "l\u006cm_backend", "a\u0069_ag\u0065nt")
        for index, value in enumerate(cases):
            with self.subTest(case=index):
                (self.root / "README.md").write_text(value + "\n")
                self.reject()

    def test_cyrillic_in_tracked_text_and_commit_is_rejected(self):
        value = "\u041f\u0440\u0438\u043c\u0435\u0440"
        (self.root / "README.md").write_text(value + "\n")
        self.reject()
        (self.root / "README.md").write_text("Release fixture\n")
        self.git("commit", "--allow-empty", "-qm", value)
        self.reject()

    def test_untracked_scratch_is_outside_the_tree(self):
        (self.root / "scratch.txt").write_text("Cod\u0065x\n")
        self.assertEqual(self.check().returncode, 0)

    def test_invalid_base_fails_closed(self):
        self.base = "missing-revision"
        self.reject()

    def test_each_provisional_profile_value_needs_its_own_note(self):
        fields = ("memory_page_price", "memory_class_maxima_bytes", "signals_based_traps",
                  "pooling_linear_memory_keep_resident")
        profile = self.root / "profiles/wasmtime-release-profile.v1"
        profile.parent.mkdir()
        profile.write_text("consume_fuel=true\n" + "".join(field + "=1\n" for field in fields))
        (self.root / "docs").mkdir()
        notes, guide = self.root / "docs/release-notes.md", self.root / "docs/guide.md"
        general = "These are development settings, not mainnet promises; all values are provisional.\n"
        own = "".join(f"- The bundled `{field}` is a provisional\n  development value.\n" for field in fields)
        named = "".join(f"- The bundled `{field}` changed.\n" for field in fields)
        split = "".join(f"- The bundled `{field}` changed.\n- Some values are provisional.\n" for field in fields)
        cases = {
            "general caveat only": ("# AVM SDK 1.0.0\n\n" + general, general),
            "named but not provisional": ("# AVM SDK 1.0.0\n\n" + named, named),
            "split across list items": ("# AVM SDK 1.0.0\n\n" + split, split),
            "older release only": ("# AVM SDK 1.0.0\n\n- Changes.\n\n# AVM SDK 0.9.0\n\n" + own, own),
            "release notes only": ("# AVM SDK 1.0.0\n\n" + own, general),
            "user documentation only": ("# AVM SDK 1.0.0\n\n" + general, own),
        }
        for label, (release_notes, documentation) in cases.items():
            with self.subTest(case=label):
                notes.write_text(release_notes)
                guide.write_text(documentation)
                self.git("add", ".")
                self.reject()
        notes.write_text("# AVM SDK 1.0.0\n\n" + own + "\n# AVM SDK 0.9.0\n\n- Earlier.\n")
        guide.write_text("# Settings\n\n" + own)
        self.assertEqual(self.check().returncode, 0)
        # A value that is no longer bundled needs no note.
        profile.write_text("consume_fuel=true\n")
        notes.write_text("# AVM SDK 1.0.0\n\n- Changes.\n")
        guide.write_text("# Settings\n")
        self.assertEqual(self.check().returncode, 0)


if __name__ == "__main__":
    unittest.main()
