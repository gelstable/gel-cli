import contextlib
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml

from gel_release import cli, release_state

REPO_ROOT = Path(__file__).resolve().parents[3]


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


class ReleasePrWorkflowTests(unittest.TestCase):
    def test_live_line_is_refetched_after_knope_before_prepared_validation(self):
        workflow = yaml.safe_load(
            (REPO_ROOT / ".github" / "workflows" / "release-pr.yml").read_text()
        )
        steps = workflow["jobs"]["prepare"]["steps"]
        preparation = next(
            step for step in steps if step.get("name") == "Prepare the line-specific release branch"
        )
        run = preparation["run"]
        base_fetch = (
            'git fetch --no-tags origin "refs/heads/${BASE_REF}:refs/remotes/origin/${BASE_REF}"'
        )
        fetches = [index for index in range(len(run)) if run.startswith(base_fetch, index)]
        self.assertGreaterEqual(
            len(fetches),
            2,
            "the line must be fetched again after preparation to avoid a stale push",
        )
        post_fetch = fetches[-1]
        knope = run.index("knope prepare-release")
        post_validation = run.index("uv run --frozen gel-release prepare-line")
        self.assertLess(knope, post_fetch)
        self.assertLess(post_fetch, post_validation)

        push = next(
            step for step in steps if step.get("name") == "Push the prepared release branch"
        )
        self.assertIn("git fetch --no-tags origin", push["run"])
        self.assertIn("live_base_sha", push["run"])
        self.assertIn('live_base_sha" != "$BASE_SHA"', push["run"])

    def test_pr_operation_creates_after_previous_pr_is_merged(self):
        create = cli.release_pr_operation(
            [],
            base_ref="release/v7.x",
            head_ref="knope/release-v7.x",
        )
        self.assertEqual(create.operation, "create")
        self.assertIsNone(create.number)

        refresh = cli.release_pr_operation(
            [
                {
                    "number": 101,
                    "baseRefName": "release/v7.x",
                    "headRefName": "knope/release-v7.x",
                }
            ],
            base_ref="release/v7.x",
            head_ref="knope/release-v7.x",
        )
        self.assertEqual(refresh.operation, "refresh")
        self.assertEqual(refresh.number, 101)

        # A merged PR is no longer returned by `gh pr list --state open`, so
        # the next release must select create again for the same line/head.
        next_release = cli.release_pr_operation(
            [],
            base_ref="release/v7.x",
            head_ref="knope/release-v7.x",
        )
        self.assertEqual(next_release.operation, "create")

    def test_pr_operation_rejects_ambiguous_open_prs(self):
        with self.assertRaisesRegex(ValueError, "more than one"):
            cli.release_pr_operation(
                [
                    {"number": 101},
                    {"number": 102},
                ],
                base_ref="release/v7.x",
                head_ref="knope/release-v7.x",
            )

    def test_pr_operation_cli_is_the_workflow_create_refresh_boundary(self):
        with tempfile.TemporaryDirectory() as tmp:
            prs = Path(tmp) / "open-prs.json"
            prs.write_text(
                json.dumps(
                    [
                        {
                            "number": 101,
                            "baseRefName": "release/v7.x",
                            "headRefName": "knope/release-v7.x",
                        }
                    ]
                )
            )
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                status = cli.main(
                    [
                        "pr-operation",
                        "--pr-json",
                        str(prs),
                        "--base-ref",
                        "release/v7.x",
                        "--head-ref",
                        "knope/release-v7.x",
                    ]
                )

        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output.getvalue()), {"operation": "refresh", "number": 101})

    def test_workflow_uses_live_pr_list_for_create_or_refresh(self):
        workflow = yaml.safe_load(
            (REPO_ROOT / ".github" / "workflows" / "release-pr.yml").read_text()
        )
        steps = workflow["jobs"]["prepare"]["steps"]
        pr_step = next(
            step
            for step in steps
            if step.get("name") == "Create or refresh the release pull request"
        )
        run = pr_step["run"]
        self.assertIn("gh pr list", run)
        self.assertIn("--state open", run)
        self.assertIn("gel-release pr-operation", run)
        self.assertIn("gh pr create", run)
        self.assertIn("gh pr edit", run)


if __name__ == "__main__":
    unittest.main()
