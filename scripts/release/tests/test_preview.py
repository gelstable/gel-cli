import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from conftest import _git, git_repo

from gel_release import cli, preview, source_equivalence


class StableVersionTests(unittest.TestCase):
    def test_plain_cargo_version_in_release_line_is_accepted(self):
        self.assertEqual(preview.stable_version("7.1.1", 7), "7.1.1")

    def test_cargo_version_from_another_major_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "8.0.0"):
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
    """The CLI surface the release workflows shell out to."""

    def _run(self, *args: str) -> tuple[int, str]:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = cli.main(list(args))
        return status, output.getvalue()

    def _preview_version(self, published: list, snapshot: str) -> tuple[int, str]:
        with tempfile.TemporaryDirectory() as tmp:
            tags = Path(tmp) / "tags.json"
            published_path = Path(tmp) / "published.json"
            tags.write_text(json.dumps(["v7.1.1-alpha.1"]))
            published_path.write_text(json.dumps(published))
            return self._run(
                "preview-version",
                "--base",
                "7.1.1",
                "--phase",
                "alpha",
                "--snapshot",
                snapshot,
                "--tags-json",
                str(tags),
                "--published-json",
                str(published_path),
            )

    def test_snapshot_prints_meaningful_tree_for_revision(self):
        with git_repo() as repo:
            rev = _git(repo, "rev-parse", "HEAD")
            expected = source_equivalence.meaningful_tree(rev, repo)
            status, output = self._run("snapshot", "--rev", rev, "--repo", str(repo))

        self.assertEqual(status, 0)
        self.assertEqual(output.strip(), expected)

    def test_preview_version_reads_tag_and_published_state_json(self):
        status, output = self._preview_version([], "new-tree")

        self.assertEqual(status, 0)
        self.assertEqual(output.strip(), "7.1.1-alpha.2")

    def test_preview_version_cli_returns_no_work_for_current_snapshot(self):
        status, output = self._preview_version([["alpha", "current-tree"]], "current-tree")

        self.assertEqual(status, 0)
        self.assertEqual(output, "")


if __name__ == "__main__":
    unittest.main()
