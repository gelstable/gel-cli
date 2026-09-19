import contextlib
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from gel_release import cli, release_state

REPO_ROOT = Path(__file__).resolve().parents[3]
REPOSITORY = "gelstable/gel-cli"


def _pr(
    *,
    number: int = 42,
    base_ref: str = "release/v7.x",
    head_ref: str = "knope/release-v7.x",
    state: str = "open",
    base_repo: str = REPOSITORY,
    head_repo: str = REPOSITORY,
    base_sha: str = "b" * 40,
    head_sha: str = "a" * 40,
    labels: list[dict[str, str]] | None = None,
) -> dict:
    return {
        "number": number,
        "state": state,
        "base": {
            "ref": base_ref,
            "sha": base_sha,
            "repo": {"full_name": base_repo},
        },
        "head": {
            "ref": head_ref,
            "sha": head_sha,
            "repo": {"full_name": head_repo},
        },
        "labels": labels or [],
    }


class LineIdentityTests(unittest.TestCase):
    def test_release_line_maps_to_major_and_generated_head(self):
        self.assertEqual(release_state.parse_line("release/v7.x"), 7)
        self.assertEqual(release_state.expected_head(7), "knope/release-v7.x")
        self.assertEqual(
            release_state.expected_head_for_line("release/v7.x"),
            "knope/release-v7.x",
        )

    def test_master_is_not_a_release_line(self):
        with self.assertRaisesRegex(ValueError, "master"):
            release_state.parse_line("master")

    def test_malformed_release_line_is_rejected(self):
        for value in (
            "release/v.x",
            "release/v0.x",
            "release/v07.x",
            "release/7.x",
            "release/v7",
            "release/v7.x/extra",
        ):
            with self.subTest(value=value):
                with self.assertRaisesRegex(ValueError, value):
                    release_state.parse_line(value)


class PullRequestIdentityTests(unittest.TestCase):
    def test_same_repository_open_pr_with_matching_line_is_valid(self):
        identity = release_state.validate_pr(_pr(), REPOSITORY)

        self.assertEqual(
            identity,
            release_state.ReleasePr(
                number=42,
                base_ref="release/v7.x",
                base_sha="b" * 40,
                head_ref="knope/release-v7.x",
                head_sha="a" * 40,
                repository=REPOSITORY,
                major=7,
            ),
        )
        with self.assertRaises((AttributeError, TypeError)):
            identity.number = 7

    def test_forked_head_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "PR #42.*forked|head.*repository"):
            release_state.validate_pr(_pr(head_repo="someone/gel-cli"), REPOSITORY)

    def test_mismatched_major_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "release/v7.x|release-v8.x"):
            release_state.validate_pr(
                _pr(head_ref="knope/release-v8.x"),
                REPOSITORY,
            )

    def test_closed_pr_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "PR #42.*closed|state"):
            release_state.validate_pr(_pr(state="closed"), REPOSITORY)

    def test_conflicting_phase_labels_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "prerelease:alpha"):
            release_state.validate_pr(
                _pr(
                    labels=[
                        {"name": "prerelease:alpha"},
                        {"name": "prerelease:rc"},
                    ]
                ),
                REPOSITORY,
            )

    def test_mismatched_base_repository_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "PR #42.*repository"):
            release_state.validate_pr(_pr(base_repo="someone/gel-cli"), REPOSITORY)


class PhaseLabelTests(unittest.TestCase):
    def test_no_active_phase_is_none(self):
        self.assertIsNone(release_state.phase_from_labels([]))

    def test_each_allowed_phase_label_maps_to_phase(self):
        for label, phase in (
            ("prerelease:alpha", "alpha"),
            ("prerelease:beta", "beta"),
            ("prerelease:rc", "rc"),
        ):
            with self.subTest(label=label):
                self.assertEqual(release_state.phase_from_labels([label]), phase)

    def test_two_active_phase_labels_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "prerelease:alpha"):
            release_state.phase_from_labels(["prerelease:alpha", "prerelease:rc"])


class CliIdentityTests(unittest.TestCase):
    def _pr_identity(self, pr: dict) -> tuple[int, str]:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pr.json"
            path.write_text(json.dumps(pr))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                status = cli.main(["pr-identity", "--pr-json", str(path), "--repo", REPOSITORY])
        return status, output.getvalue()

    def test_pr_identity_prints_validated_machine_readable_identity(self):
        status, output = self._pr_identity(_pr())

        self.assertEqual(status, 0)
        self.assertEqual(
            json.loads(output),
            {
                "number": 42,
                "base_ref": "release/v7.x",
                "base_sha": "b" * 40,
                "head_ref": "knope/release-v7.x",
                "head_sha": "a" * 40,
                "repository": REPOSITORY,
                "major": 7,
            },
        )

    def test_pr_identity_module_entry_point_exits_two_without_a_traceback(self):
        """The only test that runs the CLI as a real process.

        It proves the ``python -m gel_release.cli`` entry point maps a
        validation failure onto exit status 2 with a plain stderr message,
        which an in-process ``cli.main`` call cannot observe.
        """

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pr.json"
            path.write_text(
                json.dumps(_pr(labels=[{"name": "prerelease:alpha"}, {"name": "prerelease:rc"}]))
            )
            completed = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "gel_release.cli",
                    "pr-identity",
                    "--pr-json",
                    str(path),
                    "--repo",
                    REPOSITORY,
                ],
                cwd=REPO_ROOT,
                text=True,
                capture_output=True,
            )

        self.assertEqual(completed.returncode, 2)
        self.assertIn("prerelease:alpha", completed.stderr)
        self.assertIn("prerelease:rc", completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)


class ControllerClassificationTests(unittest.TestCase):
    """The controller runs for generated release PRs and nobody else."""

    def test_generated_head_runs(self):
        for event in ("pull_request", "workflow_dispatch", "repository_dispatch"):
            with self.subTest(event=event):
                self.assertTrue(
                    release_state.controller_should_run("knope/release-v7.x", "release/v7.x", event)
                )

    def test_ordinary_backport_head_is_skipped_neutrally_on_pull_request_events(self):
        # Branch protection does not require the controller, so a backport PR
        # targeting a release line must not get a red check from it.
        self.assertFalse(
            release_state.controller_should_run("backport/fix", "release/v7.x", "pull_request")
        )

    def test_dispatching_an_ordinary_backport_fails_loudly(self):
        # An operator explicitly named that pull request, so silence is wrong.
        for event in ("workflow_dispatch", "repository_dispatch"):
            with self.subTest(event=event):
                with self.assertRaisesRegex(ValueError, "generated head"):
                    release_state.controller_should_run("backport/fix", "release/v7.x", event)

    def test_master_head_against_a_release_line_is_not_generated(self):
        self.assertFalse(
            release_state.controller_should_run("master", "release/v7.x", "pull_request")
        )

    def test_invalid_inputs_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "release line"):
            release_state.controller_should_run("knope/release-v7.x", "master", "pull_request")
        with self.assertRaisesRegex(ValueError, "head ref"):
            release_state.controller_should_run(None, "release/v7.x", "pull_request")

    def test_classify_pr_prints_the_controller_decision(self):
        for head_ref, event, expected in (
            ("knope/release-v7.x", "pull_request", "true"),
            ("backport/fix", "pull_request", "false"),
        ):
            with self.subTest(head_ref=head_ref, event=event):
                with tempfile.TemporaryDirectory() as tmp:
                    path = Path(tmp) / "pr.json"
                    path.write_text(json.dumps({"head": {"ref": head_ref}}))
                    output = io.StringIO()
                    with contextlib.redirect_stdout(output):
                        status = cli.main(
                            [
                                "classify-pr",
                                "--pr-json",
                                str(path),
                                "--line",
                                "release/v7.x",
                                "--event",
                                event,
                            ]
                        )
                self.assertEqual(status, 0)
                self.assertEqual(output.getvalue().strip(), expected)

    def test_classify_pr_rejects_a_dispatched_backport_with_status_two(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pr.json"
            path.write_text(json.dumps({"head": {"ref": "backport/fix"}}))
            output = io.StringIO()
            errors = io.StringIO()
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
                status = cli.main(
                    [
                        "classify-pr",
                        "--pr-json",
                        str(path),
                        "--line",
                        "release/v7.x",
                        "--event",
                        "workflow_dispatch",
                    ]
                )
        self.assertEqual(status, 2)
        self.assertIn("generated head", errors.getvalue())


if __name__ == "__main__":
    unittest.main()
