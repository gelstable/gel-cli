import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from gel_release import release_state

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
        with self.assertRaisesRegex(ValueError, "phase labels"):
            release_state.phase_from_labels(["prerelease:alpha", "prerelease:rc"])


class CliIdentityTests(unittest.TestCase):
    def test_pr_identity_prints_validated_machine_readable_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pr.json"
            path.write_text(json.dumps(_pr()))
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

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            json.loads(completed.stdout),
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


if __name__ == "__main__":
    unittest.main()
