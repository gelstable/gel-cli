import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import yaml
from conftest import _git, line_repo

from gel_release import cli, release_state

REPO_ROOT = Path(__file__).resolve().parents[3]


class PrepareLineTests(unittest.TestCase):
    def test_no_pending_change_files_returns_no_release_work(self):
        with line_repo() as repo:
            result = cli.prepare_line("release/v7.x", repo)

        self.assertFalse(result.pending)
        self.assertEqual(result.base_ref, "release/v7.x")
        self.assertEqual(result.major, 7)
        self.assertEqual(result.generated_head, "knope/release-v7.x")

    def test_pending_patch_change_is_ready_for_line_preparation(self):
        with line_repo(pending=True) as repo:
            result = cli.prepare_line("release/v7.x", repo)

        self.assertTrue(result.pending)
        self.assertEqual(result.prepared_version, "7.1.0")
        self.assertEqual(result.base_ref, "release/v7.x")

    def test_pending_minor_change_is_ready_for_line_preparation(self):
        with line_repo(pending=True, change_type="minor") as repo:
            result = cli.prepare_line("release/v7.x", repo)

        self.assertTrue(result.pending)
        self.assertEqual(result.pending_files, (".changeset/release.md",))

    def test_first_release_line_uses_its_deliberate_starting_major(self):
        with line_repo(major=8, version="8.0.0", pending=True) as repo:
            result = cli.prepare_line("release/v8.x", repo)

        self.assertEqual(result.major, 8)
        self.assertEqual(result.prepared_version, "8.0.0")
        self.assertEqual(result.generated_head, "knope/release-v8.x")

    def test_prepared_version_from_another_major_is_rejected(self):
        with line_repo(version="8.0.0", pending=True) as repo:
            with self.assertRaisesRegex(ValueError, "8.0.0"):
                cli.prepare_line("release/v7.x", repo)

    def test_stale_local_line_against_remote_line_is_rejected(self):
        with line_repo(pending=True) as repo:
            old_sha = _git(repo, "rev-parse", "HEAD")
            (repo / "line.txt").write_text("new line tip\n")
            _git(repo, "add", "line.txt")
            _git(repo, "commit", "-qm", "advance line")
            new_sha = _git(repo, "rev-parse", "HEAD")
            _git(repo, "update-ref", "refs/remotes/origin/release/v7.x", new_sha)
            _git(repo, "reset", "--keep", old_sha)

            with self.assertRaisesRegex(ValueError, "stale|base"):
                cli.prepare_line("release/v7.x", repo)

    def test_independent_lines_have_independent_generated_heads(self):
        with (
            line_repo(major=7, pending=True) as first,
            line_repo(major=8, version="8.0.0", pending=True) as second,
        ):
            seven = cli.prepare_line("release/v7.x", first)
            eight = cli.prepare_line("release/v8.x", second)

        self.assertEqual(seven.generated_head, "knope/release-v7.x")
        self.assertEqual(eight.generated_head, "knope/release-v8.x")
        self.assertNotEqual(seven.generated_head, eight.generated_head)

    def test_prepared_generated_branch_can_be_validated_after_knope_consumes_changesets(self):
        with line_repo(pending=True) as repo:
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

        self.assertFalse(prepared.pending)
        self.assertEqual(prepared.prepared_version, "7.1.0")

    def test_next_line_change_is_ready_after_the_previous_release_merges(self):
        with line_repo(pending=True) as repo:
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

        self.assertTrue(second.pending)
        self.assertNotEqual(first.base_sha, second.base_sha)
        self.assertEqual(second.generated_head, "knope/release-v7.x")


class PrepareLineCliTests(unittest.TestCase):
    def test_prepare_line_prints_machine_readable_result(self):
        output = io.StringIO()
        with line_repo(pending=True) as repo:
            with contextlib.redirect_stdout(output):
                status = cli.main(
                    ["prepare-line", "--base-ref", "release/v7.x", "--repo-root", str(repo)]
                )

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

    def test_pr_operation_selects_create_or_refresh_for_open_prs(self):
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
        self.assertIn("gel-release pr-sync", run)
        self.assertIn('--repo "$GITHUB_REPOSITORY"', run)
        self.assertIn('--base-ref "$BASE_REF"', run)
        self.assertIn('--head-ref "$HEAD_REF"', run)
        self.assertIn('--body-file "$body_file"', run)

    def test_pr_sync_invokes_create_after_open_pr_is_marked_merged(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            state_path = root / "gh-state.json"
            log_path = root / "gh-log.jsonl"
            state_path.write_text(json.dumps({"open": [101], "next": 102}))
            fake_gh = root / "gh"
            fake_gh.write_text(
                "#!/usr/bin/env python3\n"
                "import json, os, sys\n"
                "from pathlib import Path\n"
                "state_path = Path(os.environ['FAKE_GH_STATE'])\n"
                "log_path = Path(os.environ['FAKE_GH_LOG'])\n"
                "state = json.loads(state_path.read_text())\n"
                "args = sys.argv[1:]\n"
                "with log_path.open('a') as log:\n"
                "    log.write(json.dumps(args) + '\\n')\n"
                "if args[:2] == ['pr', 'list']:\n"
                "    print(json.dumps([{'number': n, 'baseRefName': 'release/v7.x', "
                "'headRefName': 'knope/release-v7.x'} for n in state['open']]))\n"
                "elif args[:2] == ['pr', 'edit']:\n"
                "    print('edited')\n"
                "elif args[:2] == ['pr', 'create']:\n"
                "    number = state['next']\n"
                "    state['next'] = number + 1\n"
                "    state['open'].append(number)\n"
                "    state_path.write_text(json.dumps(state))\n"
                "    print(f'https://github.com/gelstable/gel-cli/pull/{number}')\n"
                "else:\n"
                "    raise SystemExit(f'unexpected fake gh command: {args!r}')\n"
            )
            fake_gh.chmod(0o755)
            body = root / "body.md"
            body.write_text("release body\n")
            environment = {
                "PATH": f"{root}{os.pathsep}{os.environ['PATH']}",
                "FAKE_GH_STATE": str(state_path),
                "FAKE_GH_LOG": str(log_path),
            }
            arguments = [
                "pr-sync",
                "--repo",
                "gelstable/gel-cli",
                "--base-ref",
                "release/v7.x",
                "--head-ref",
                "knope/release-v7.x",
                "--version",
                "7.1.0",
                "--body-file",
                str(body),
            ]
            with mock.patch.dict(os.environ, environment, clear=False):
                first_output = io.StringIO()
                with contextlib.redirect_stdout(first_output):
                    first_status = cli.main(arguments)

                state_path.write_text(json.dumps({"open": [], "next": 102}))
                second_output = io.StringIO()
                with contextlib.redirect_stdout(second_output):
                    second_status = cli.main(arguments)

            calls = [json.loads(line) for line in log_path.read_text().splitlines()]

        self.assertEqual(first_status, 0)
        self.assertEqual(second_status, 0)
        self.assertEqual(first_output.getvalue().strip(), "101")
        self.assertEqual(second_output.getvalue().strip(), "102")
        self.assertEqual(
            [call[:2] for call in calls],
            [["pr", "list"], ["pr", "edit"], ["pr", "list"], ["pr", "create"]],
        )


if __name__ == "__main__":
    unittest.main()
