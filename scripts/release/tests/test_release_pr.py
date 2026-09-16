import contextlib
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from gel_release import cli, release_state


def _git(repo: Path, *argv: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *argv],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def _line_repo(
    *,
    major: int = 7,
    version: str | None = None,
    pending: bool = False,
    change_type: str = "patch",
) -> tuple[tempfile.TemporaryDirectory[str], Path]:
    directory = tempfile.TemporaryDirectory()
    repo = Path(directory.name)
    _git(repo, "init", "-q", ".")
    _git(repo, "config", "user.email", "test@example.com")
    _git(repo, "config", "user.name", "Release Test")
    version = version or f"{major}.1.0"
    (repo / "Cargo.toml").write_text(f'[package]\nname = "gel-cli"\nversion = "{version}"\n')
    (repo / "Cargo.lock").write_text(
        f'version = 4\n\n[[package]]\nname = "gel-cli"\nversion = "{version}"\n'
    )
    (repo / "CHANGELOG.md").write_text("# Changelog\n")
    if pending:
        changeset = repo / ".changeset"
        changeset.mkdir()
        (changeset / "release.md").write_text(
            f"---\ngel-cli: {change_type}\n---\n\nA release line change.\n"
        )
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "base")
    _git(repo, "switch", "-c", f"release/v{major}.x")
    return directory, repo


class PrepareLineTests(unittest.TestCase):
    def test_no_pending_change_files_returns_no_release_work(self):
        directory, repo = _line_repo()
        try:
            result = cli.prepare_line("release/v7.x", repo)
        finally:
            directory.cleanup()

        self.assertFalse(result.pending)
        self.assertEqual(result.base_ref, "release/v7.x")
        self.assertEqual(result.major, 7)
        self.assertEqual(result.generated_head, "knope/release-v7.x")

    def test_pending_patch_change_is_ready_for_line_preparation(self):
        directory, repo = _line_repo(pending=True)
        try:
            result = cli.prepare_line("release/v7.x", repo)
        finally:
            directory.cleanup()

        self.assertTrue(result.pending)
        self.assertEqual(result.prepared_version, "7.1.0")
        self.assertEqual(result.base_ref, "release/v7.x")

    def test_pending_minor_change_is_ready_for_line_preparation(self):
        directory, repo = _line_repo(pending=True, change_type="minor")
        try:
            result = cli.prepare_line("release/v7.x", repo)
        finally:
            directory.cleanup()

        self.assertTrue(result.pending)
        self.assertEqual(result.pending_files, (".changeset/release.md",))

    def test_first_release_line_uses_its_deliberate_starting_major(self):
        directory, repo = _line_repo(major=8, version="8.0.0", pending=True)
        try:
            result = cli.prepare_line("release/v8.x", repo)
        finally:
            directory.cleanup()

        self.assertEqual(result.major, 8)
        self.assertEqual(result.prepared_version, "8.0.0")
        self.assertEqual(result.generated_head, "knope/release-v8.x")

    def test_prepared_version_from_another_major_is_rejected(self):
        directory, repo = _line_repo(version="8.0.0", pending=True)
        try:
            with self.assertRaisesRegex(ValueError, "major"):
                cli.prepare_line("release/v7.x", repo)
        finally:
            directory.cleanup()

    def test_stale_local_line_against_remote_line_is_rejected(self):
        directory, repo = _line_repo(pending=True)
        try:
            old_sha = _git(repo, "rev-parse", "HEAD")
            (repo / "line.txt").write_text("new line tip\n")
            _git(repo, "add", "line.txt")
            _git(repo, "commit", "-qm", "advance line")
            new_sha = _git(repo, "rev-parse", "HEAD")
            _git(repo, "update-ref", "refs/remotes/origin/release/v7.x", new_sha)
            _git(repo, "reset", "--keep", old_sha)

            with self.assertRaisesRegex(ValueError, "stale|base"):
                cli.prepare_line("release/v7.x", repo)
        finally:
            directory.cleanup()

    def test_independent_lines_have_independent_generated_heads(self):
        first_dir, first = _line_repo(major=7, pending=True)
        second_dir, second = _line_repo(major=8, version="8.0.0", pending=True)
        try:
            seven = cli.prepare_line("release/v7.x", first)
            eight = cli.prepare_line("release/v8.x", second)
        finally:
            first_dir.cleanup()
            second_dir.cleanup()

        self.assertEqual(seven.generated_head, "knope/release-v7.x")
        self.assertEqual(eight.generated_head, "knope/release-v8.x")
        self.assertNotEqual(seven.generated_head, eight.generated_head)

    def test_prepared_generated_branch_can_be_validated_after_knope_consumes_changesets(self):
        directory, repo = _line_repo(pending=True)
        try:
            base = cli.prepare_line("release/v7.x", repo)
            _git(repo, "switch", "-c", base.generated_head)
            (repo / ".changeset" / "release.md").unlink()
            _git(repo, "add", "-A")
            _git(repo, "commit", "-qm", "chore: prepare release")
            prepared = cli.prepare_line(
                "release/v7.x",
                repo,
                prepared=True,
                expected_base_sha=base.base_sha,
                head_ref=base.generated_head,
            )
        finally:
            directory.cleanup()

        self.assertFalse(prepared.pending)
        self.assertEqual(prepared.prepared_version, "7.1.0")

    def test_next_line_change_is_ready_after_the_previous_release_merges(self):
        directory, repo = _line_repo(pending=True)
        try:
            first = cli.prepare_line("release/v7.x", repo)
            _git(repo, "switch", "-c", first.generated_head)
            (repo / ".changeset" / "release.md").unlink()
            _git(repo, "add", "-A")
            _git(repo, "commit", "-qm", "chore: prepare release")
            _git(repo, "switch", "release/v7.x")
            _git(repo, "merge", "--no-ff", "-m", "Merge release PR", first.generated_head)
            (repo / ".changeset").mkdir()
            (repo / ".changeset" / "next.md").write_text(
                "---\ngel-cli: patch\n---\n\nA follow-up line change.\n"
            )
            _git(repo, "add", ".changeset/next.md")
            _git(repo, "commit", "-qm", "fix: prepare next line release")
            second = cli.prepare_line("release/v7.x", repo)
        finally:
            directory.cleanup()

        self.assertTrue(second.pending)
        self.assertNotEqual(first.base_sha, second.base_sha)
        self.assertEqual(second.generated_head, "knope/release-v7.x")


class PrepareLineCliTests(unittest.TestCase):
    def test_prepare_line_prints_machine_readable_result(self):
        directory, repo = _line_repo(pending=True)
        output = io.StringIO()
        try:
            with contextlib.redirect_stdout(output):
                status = cli.main(
                    [
                        "prepare-line",
                        "--base-ref",
                        "release/v7.x",
                        "--repo-root",
                        str(repo),
                    ]
                )
        finally:
            directory.cleanup()

        self.assertEqual(status, 0)
        payload = json.loads(output.getvalue())
        self.assertTrue(payload["pending"])
        self.assertEqual(payload["base_ref"], "release/v7.x")
        self.assertEqual(payload["head_ref"], "knope/release-v7.x")

    def test_line_parser_still_rejects_master_before_preparation(self):
        with self.assertRaisesRegex(ValueError, "master"):
            release_state.parse_line("master")


if __name__ == "__main__":
    unittest.main()
