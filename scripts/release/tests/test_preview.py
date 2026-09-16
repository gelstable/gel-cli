import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from gel_release import preview, source_equivalence

REPO_ROOT = Path(__file__).resolve().parents[3]


def _git(repo: Path, *argv: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *argv],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


class StableVersionTests(unittest.TestCase):
    def test_plain_cargo_version_in_release_line_is_accepted(self):
        self.assertEqual(preview.stable_version("7.1.1", 7), "7.1.1")

    def test_cargo_version_from_another_major_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "major"):
            preview.stable_version("8.0.0", 7)

    def test_prerelease_cargo_version_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "unsupported|plain|prerelease"):
            preview.stable_version("7.1.1-alpha.1", 7)


class PreviewVersionTests(unittest.TestCase):
    def test_alpha_suffix_starts_at_one_and_advances_from_published_tags(self):
        self.assertEqual(
            preview.next_preview_version("7.1.1", "alpha", [], set(), "tree-1"),
            "7.1.1-alpha.1",
        )
        self.assertEqual(
            preview.next_preview_version("7.1.1", "alpha", ["v7.1.1-alpha.1"], set(), "tree-1"),
            "7.1.1-alpha.2",
        )

    def test_switching_phase_starts_at_one_for_the_same_source(self):
        self.assertEqual(
            preview.next_preview_version(
                "7.1.1",
                "beta",
                ["v7.1.1-alpha.1", "v7.1.1-alpha.2"],
                set(),
                "tree-1",
            ),
            "7.1.1-beta.1",
        )

    def test_changing_base_version_resets_phase_suffix(self):
        self.assertEqual(
            preview.next_preview_version(
                "7.1.2",
                "alpha",
                ["v7.1.1-alpha.1", "v7.1.1-alpha.2"],
                set(),
                "tree-2",
            ),
            "7.1.2-alpha.1",
        )

    def test_old_snapshot_allows_the_next_suffix(self):
        self.assertEqual(
            preview.next_preview_version(
                "7.1.1",
                "alpha",
                ["v7.1.1-alpha.1"],
                {("alpha", "old-tree")},
                "new-tree",
            ),
            "7.1.1-alpha.2",
        )

    def test_published_phase_and_current_snapshot_returns_no_work(self):
        self.assertIsNone(
            preview.next_preview_version(
                "7.1.1",
                "alpha",
                ["v7.1.1-alpha.1"],
                {("alpha", "snapshot-1")},
                "snapshot-1",
            )
        )

    def test_other_bases_phases_and_failed_drafts_do_not_advance_suffix(self):
        tags = [
            "v7.1.0-alpha.9",
            "v7.1.1-beta.4",
            "v8.1.1-alpha.7",
            "v7.1.1-alpha.1",
            "v7.1.1-alpha.2-draft",
            "v7.1.1-dev.8",
        ]
        self.assertEqual(
            preview.next_preview_version("7.1.1", "alpha", tags, set(), "tree-1"),
            "7.1.1-alpha.2",
        )

    def test_unsupported_base_and_phase_are_rejected(self):
        with self.assertRaises(ValueError):
            preview.next_preview_version("7.1.1-dev.1", "alpha", [], set(), "tree-1")
        with self.assertRaises(ValueError):
            preview.next_preview_version("7.1.1", "preview", [], set(), "tree-1")


class PreviewCliTests(unittest.TestCase):
    def _run(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-m", "gel_release.cli", *args],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
        )

    def test_snapshot_prints_meaningful_tree_for_revision(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            _git(repo, "init", "-q", ".")
            _git(repo, "config", "user.email", "test@example.com")
            _git(repo, "config", "user.name", "Test")
            (repo / "src").mkdir()
            (repo / "src" / "main.rs").write_text("fn main() {}\n")
            _git(repo, "add", "-A")
            _git(repo, "commit", "-qm", "base")
            rev = _git(repo, "rev-parse", "HEAD")
            expected = source_equivalence.meaningful_tree(rev, repo)

            completed = self._run("snapshot", "--rev", rev, "--repo", str(repo))

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout.strip(), expected)

    def test_preview_version_reads_tag_and_published_state_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            tags = Path(tmp) / "tags.json"
            published = Path(tmp) / "published.json"
            tags.write_text(json.dumps(["v7.1.1-alpha.1"]))
            published.write_text(json.dumps([]))

            completed = self._run(
                "preview-version",
                "--base",
                "7.1.1",
                "--phase",
                "alpha",
                "--snapshot",
                "new-tree",
                "--tags-json",
                str(tags),
                "--published-json",
                str(published),
            )

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout.strip(), "7.1.1-alpha.2")

    def test_preview_version_cli_returns_no_work_for_current_snapshot(self):
        with tempfile.TemporaryDirectory() as tmp:
            tags = Path(tmp) / "tags.json"
            published = Path(tmp) / "published.json"
            tags.write_text(json.dumps(["v7.1.1-alpha.1"]))
            published.write_text(json.dumps([["alpha", "current-tree"]]))

            completed = self._run(
                "preview-version",
                "--base",
                "7.1.1",
                "--phase",
                "alpha",
                "--snapshot",
                "current-tree",
                "--tags-json",
                str(tags),
                "--published-json",
                str(published),
            )

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stdout, "")


if __name__ == "__main__":
    unittest.main()
