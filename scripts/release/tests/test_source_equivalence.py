import subprocess
import tempfile
import unittest
from pathlib import Path

from gel_release import source_equivalence


def _git(repo: Path, *argv: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *argv],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


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
        (self.repo / "Formula").mkdir()
        (self.repo / "Formula" / "gel.rb").write_text("class Gel < Formula\nend\n")
        (self.repo / "bucket").mkdir()
        (self.repo / "bucket" / "gel.json").write_text("{}\n")
        (self.repo / "packaging" / "aur").mkdir()
        (self.repo / "packaging" / "aur" / "PKGBUILD").write_text("pkgname=gel-cli-bin\n")
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

    def test_allowlist_is_exactly_the_four_generated_paths(self):
        self.assertEqual(
            sorted(source_equivalence.ALLOWLIST),
            [
                "Formula/gel.rb",
                "bucket/gel.json",
                "packaging/aur/PKGBUILD",
                "packaging/release-candidate.json",
            ],
        )


if __name__ == "__main__":
    unittest.main()
