import tempfile
import unittest
from pathlib import Path

from conftest import _git

from gel_release import source_equivalence


class EquivalenceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        _git(self.repo, "init", "-q", ".")
        _git(self.repo, "config", "user.email", "test@example.com")
        _git(self.repo, "config", "user.name", "Test")
        _git(self.repo, "config", "commit.gpgsign", "false")
        (self.repo / "src").mkdir()
        (self.repo / "src" / "main.rs").write_text("fn main() {}\n")
        (self.repo / "packaging").mkdir()
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-qm", "base")
        self.base = _git(self.repo, "rev-parse", "HEAD")

    def _commit(self, message: str) -> str:
        _git(self.repo, "add", "-A")
        _git(self.repo, "commit", "-qm", message)
        return _git(self.repo, "rev-parse", "HEAD")

    def test_identical_trees_are_equivalent(self):
        source_equivalence.assert_equivalent(self.base, self.base, self.repo)

    def test_allowlisted_additions_are_equivalent(self):
        (self.repo / "packaging" / "release-candidate.json").write_text("{}\n")
        head = self._commit("metadata only")

        source_equivalence.assert_equivalent(self.base, head, self.repo)

    def test_source_change_is_drift(self):
        (self.repo / "src" / "main.rs").write_text("fn main() { println!(); }\n")
        head = self._commit("source change")

        with self.assertRaises(source_equivalence.SourceDrift) as raised:
            source_equivalence.assert_equivalent(self.base, head, self.repo)
        self.assertIn("src/main.rs", str(raised.exception))

    def test_deleted_source_file_is_drift(self):
        (self.repo / "src" / "main.rs").unlink()
        head = self._commit("delete source")

        with self.assertRaises(source_equivalence.SourceDrift):
            source_equivalence.assert_equivalent(self.base, head, self.repo)

    def test_new_non_allowlisted_file_is_drift(self):
        (self.repo / "packaging" / "extra.json").write_text("{}\n")
        head = self._commit("stray packaging file")

        with self.assertRaises(source_equivalence.SourceDrift) as raised:
            source_equivalence.assert_equivalent(self.base, head, self.repo)
        self.assertIn("packaging/extra.json", str(raised.exception))

    def test_mode_change_is_drift(self):
        (self.repo / "src" / "main.rs").chmod(0o755)
        _git(self.repo, "update-index", "--chmod=+x", "src/main.rs")
        head = self._commit("chmod")

        with self.assertRaises(source_equivalence.SourceDrift):
            source_equivalence.assert_equivalent(self.base, head, self.repo)

    def test_allowlist_is_exactly_the_generated_record_path(self):
        self.assertEqual(
            sorted(source_equivalence.ALLOWLIST),
            ["packaging/release-candidate.json"],
        )

    def test_meaningful_tree_ignores_generated_candidate_and_distribution_files(self):
        baseline = source_equivalence.meaningful_tree(self.base, self.repo)

        (self.repo / "packaging" / "release-candidate.json").write_text("{}\n")
        generated = self._commit("generated candidate and distribution files")

        self.assertEqual(source_equivalence.meaningful_tree(generated, self.repo), baseline)

    def test_meaningful_tree_changes_for_source_and_prepared_metadata(self):
        baseline = source_equivalence.meaningful_tree(self.base, self.repo)

        (self.repo / "src" / "main.rs").write_text("fn main() { println!(); }\n")
        source_change = self._commit("source change")
        self.assertNotEqual(source_equivalence.meaningful_tree(source_change, self.repo), baseline)

        (self.repo / "Cargo.toml").write_text('[package]\nname = "gel"\nversion = "7.1.1"\n')
        (self.repo / "Cargo.lock").write_text("version = 4\n")
        prepared_change = self._commit("prepare version")
        self.assertNotEqual(
            source_equivalence.meaningful_tree(prepared_change, self.repo),
            source_equivalence.meaningful_tree(source_change, self.repo),
        )

    def test_snapshot_mismatch_is_rejected(self):
        with self.assertRaisesRegex(source_equivalence.SourceDrift, "snapshot"):
            source_equivalence.assert_snapshot("f" * 64, self.base, self.repo)

    def test_snapshot_match_is_accepted(self):
        expected = source_equivalence.meaningful_tree(self.base, self.repo)
        source_equivalence.assert_snapshot(expected, self.base, self.repo)


if __name__ == "__main__":
    unittest.main()
