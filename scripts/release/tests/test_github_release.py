import contextlib
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml

from gel_release import cli, github_release, release_state

REPO_ROOT = Path(__file__).resolve().parents[3]
REPOSITORY = "gelstable/gel-cli"
BASE_REF = "release/v7.x"
HEAD_REF = "knope/release-v7.x"
BASE_SHA = "b" * 40
SOURCE_SHA = "a" * 40
SNAPSHOT = "c" * 64


def _release_pr(
    *,
    number: int = 101,
    base_sha: str = BASE_SHA,
    head_sha: str = SOURCE_SHA,
) -> release_state.ReleasePr:
    return release_state.ReleasePr(
        number=number,
        base_ref=BASE_REF,
        base_sha=base_sha,
        head_ref=HEAD_REF,
        head_sha=head_sha,
        repository=REPOSITORY,
        major=7,
    )


def _live_pr(
    *,
    labels: list[str] | None = None,
    number: int = 101,
    base_ref: str = BASE_REF,
    head_ref: str = HEAD_REF,
    base_sha: str = BASE_SHA,
    head_sha: str = SOURCE_SHA,
    base_repository: str = REPOSITORY,
    head_repository: str = REPOSITORY,
    version: str = "7.1.0",
    snapshot: str = SNAPSHOT,
) -> dict:
    return {
        "number": number,
        "state": "open",
        "base": {
            "ref": base_ref,
            "sha": base_sha,
            "repo": {"full_name": base_repository},
        },
        "head": {
            "ref": head_ref,
            "sha": head_sha,
            "repo": {"full_name": head_repository},
        },
        "labels": [{"name": label} for label in labels or []],
        "prepared_version": version,
        "source_snapshot": snapshot,
    }


def _published_preview(
    version: str, snapshot: str, *, phase: str = "alpha", draft: bool = False
) -> dict:
    return {
        "tag_name": f"v{version}",
        "name": f"v{version}",
        "draft": draft,
        "prerelease": True,
        "candidate": {
            "phase": phase,
            "source_snapshot": snapshot,
            "version": version,
        },
    }


class ResolveCandidateTests(unittest.TestCase):
    def test_unlabelled_pr_selects_stable_candidate_from_live_prepared_version(self):
        identity = github_release.resolve_candidate(_release_pr(), _live_pr(), [], [])

        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.line, BASE_REF)
        self.assertEqual(identity.pr_number, 101)
        self.assertEqual(identity.base_sha, BASE_SHA)
        self.assertEqual(identity.source_sha, SOURCE_SHA)
        self.assertEqual(identity.source_snapshot, SNAPSHOT)
        self.assertIsNone(identity.phase)
        self.assertEqual(identity.version, "7.1.0")
        self.assertEqual(identity.channel, "stable")
        self.assertEqual(identity.build_sha, SOURCE_SHA)

    def test_active_phase_label_selects_next_published_tag_suffix(self):
        identity = github_release.resolve_candidate(
            _release_pr(),
            _live_pr(labels=["prerelease:alpha"]),
            ["v7.1.0-alpha.1"],
            [],
        )

        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.phase, "alpha")
        self.assertEqual(identity.version, "7.1.0-alpha.2")
        self.assertEqual(identity.channel, "testing")
        self.assertEqual(identity.source_sha, SOURCE_SHA)

    def test_phase_switch_starts_new_phase_at_one_for_unchanged_source(self):
        identity = github_release.resolve_candidate(
            _release_pr(),
            _live_pr(labels=["prerelease:beta"]),
            ["v7.1.0-alpha.1"],
            [_published_preview("7.1.0-alpha.1", SNAPSHOT)],
        )

        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.version, "7.1.0-beta.1")
        self.assertEqual(identity.phase, "beta")

    def test_source_update_advances_phase_suffix(self):
        newer_snapshot = "d" * 64
        identity = github_release.resolve_candidate(
            _release_pr(head_sha="d" * 40),
            _live_pr(labels=["prerelease:alpha"], head_sha="d" * 40, snapshot=newer_snapshot),
            ["v7.1.0-alpha.1"],
            [_published_preview("7.1.0-alpha.1", SNAPSHOT)],
        )

        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.version, "7.1.0-alpha.2")
        self.assertEqual(identity.source_snapshot, newer_snapshot)

    def test_generated_record_only_update_does_not_trigger_preview(self):
        identity = github_release.resolve_candidate(
            _release_pr(head_sha="d" * 40),
            _live_pr(labels=["prerelease:alpha"], head_sha="d" * 40),
            ["v7.1.0-alpha.1"],
            [_published_preview("7.1.0-alpha.1", SNAPSHOT)],
        )

        self.assertIsNone(identity)

    def test_published_same_phase_and_snapshot_does_not_trigger_retry(self):
        identity = github_release.resolve_candidate(
            _release_pr(),
            _live_pr(labels=["prerelease:alpha"]),
            ["v7.1.0-alpha.1"],
            [_published_preview("7.1.0-alpha.1", SNAPSHOT)],
        )

        self.assertIsNone(identity)

    def test_changed_prepared_base_starts_phase_sequence_for_new_base(self):
        identity = github_release.resolve_candidate(
            _release_pr(),
            _live_pr(labels=["prerelease:alpha"], version="7.2.0"),
            ["v7.1.0-alpha.1", "v7.2.0-alpha.9"],
            [_published_preview("7.2.0-alpha.9", "e" * 64)],
        )

        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.version, "7.2.0-alpha.10")

    def test_live_pr_labels_win_over_stale_event_payload(self):
        live = _live_pr(labels=["prerelease:beta"])
        live["event_labels"] = [{"name": "prerelease:alpha"}]

        identity = github_release.resolve_candidate(_release_pr(), live, [], [])

        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.phase, "beta")
        self.assertEqual(identity.version, "7.1.0-beta.1")

    def test_conflicting_phase_labels_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "phase labels"):
            github_release.resolve_candidate(
                _release_pr(),
                _live_pr(labels=["prerelease:alpha", "prerelease:rc"]),
                [],
                [],
            )

    def test_unsupported_dev_version_fails_before_stage_dispatch(self):
        with self.assertRaisesRegex(ValueError, "unsupported|dev"):
            github_release.resolve_candidate(
                _release_pr(),
                _live_pr(labels=["prerelease:alpha"], version="7.1.0-dev.12"),
                [],
                [],
            )

    def test_forked_head_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "fork|repository"):
            github_release.resolve_candidate(
                _release_pr(),
                _live_pr(head_repository="attacker/gel-cli"),
                [],
                [],
            )

    def test_mismatched_head_or_base_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "identity|base|head"):
            github_release.resolve_candidate(
                _release_pr(),
                _live_pr(head_ref="knope/release-v8.x"),
                [],
                [],
            )

    def test_phase_authorization_uses_latest_label_transition_and_permission(self):
        timeline = [
            {
                "event": "labeled",
                "label": {"name": "prerelease:alpha"},
                "actor": {"login": "maintainer"},
            },
            {
                "event": "synchronize",
                "actor": {"login": "gelstable-release[bot]"},
            },
        ]

        self.assertTrue(
            github_release.phase_authorized(timeline, "alpha", {"maintainer": "maintain"})
        )
        self.assertFalse(
            github_release.phase_authorized(timeline, "alpha", {"gelstable-release[bot]": "admin"})
        )

    def test_phase_authorization_rejects_transition_before_latest_removal(self):
        timeline = [
            {
                "event": "labeled",
                "label": {"name": "prerelease:alpha"},
                "actor": {"login": "old-maintainer"},
            },
            {
                "event": "unlabeled",
                "label": {"name": "prerelease:alpha"},
                "actor": {"login": "maintainer"},
            },
        ]

        self.assertFalse(
            github_release.phase_authorized(
                timeline, "alpha", {"old-maintainer": "admin", "maintainer": "write"}
            )
        )


class CandidateIdentityBoundaryTests(unittest.TestCase):
    IDENTITY = {
        "line": BASE_REF,
        "pr_number": 101,
        "base_sha": BASE_SHA,
        "source_sha": SOURCE_SHA,
        "build_sha": SOURCE_SHA,
        "source_snapshot": SNAPSHOT,
        "phase": None,
        "version": "7.1.0",
        "channel": "stable",
    }

    def test_live_pr_matching_identity_is_accepted(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        github_release.assert_live_identity(identity, _live_pr())

    def test_live_pr_head_change_is_rejected_before_record_push(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        with self.assertRaisesRegex(ValueError, "source|head"):
            github_release.assert_live_identity(identity, _live_pr(head_sha="d" * 40))

    def test_live_pr_phase_change_is_rejected_before_record_push(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        with self.assertRaisesRegex(ValueError, "phase"):
            github_release.assert_live_identity(
                identity,
                _live_pr(labels=["prerelease:alpha"]),
            )

    def test_live_build_sha_change_is_rejected_when_present(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        live_pr = _live_pr()
        live_pr["build_sha"] = "d" * 40
        with self.assertRaisesRegex(ValueError, "build SHA"):
            github_release.assert_live_identity(identity, live_pr)

    def test_matching_draft_can_be_reused_only_when_body_identity_matches(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        release = {
            "id": 123,
            "tag_name": identity.tag,
            "name": identity.tag,
            "draft": True,
            "prerelease": False,
            "body": json.dumps({"candidate_identity": identity.as_dict()}),
        }
        self.assertEqual(
            github_release.find_reusable_draft([release], identity),
            release,
        )

    def test_draft_with_same_tag_but_different_line_fails_closed(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        other = {**identity.as_dict(), "line": "release/v8.x"}
        release = {
            "id": 123,
            "tag_name": identity.tag,
            "name": identity.tag,
            "draft": True,
            "prerelease": False,
            "body": json.dumps({"candidate_identity": other}),
        }
        with self.assertRaisesRegex(ValueError, "identity|line"):
            github_release.find_reusable_draft([release], identity)

    def test_published_matching_tag_fails_closed(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        release = {
            "id": 123,
            "tag_name": identity.tag,
            "name": identity.tag,
            "draft": False,
            "prerelease": False,
            "body": json.dumps({"candidate_identity": identity.as_dict()}),
        }
        with self.assertRaisesRegex(ValueError, "published|draft"):
            github_release.find_reusable_draft([release], identity)


class PreviewCommitTests(unittest.TestCase):
    def _repo(self) -> tuple[tempfile.TemporaryDirectory[str], Path, str]:
        directory = tempfile.TemporaryDirectory()
        repo = Path(directory.name)
        subprocess.run(["git", "-C", str(repo), "init", "-q", "."], check=True)
        subprocess.run(
            ["git", "-C", str(repo), "config", "user.email", "test@example.com"], check=True
        )
        subprocess.run(["git", "-C", str(repo), "config", "user.name", "Preview Test"], check=True)
        (repo / "Cargo.toml").write_text('[package]\nname = "gel-cli"\nversion = "7.1.0"\n')
        (repo / "Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "gel-cli"\nversion = "7.1.0"\n'
        )
        (repo / "src").mkdir()
        (repo / "src" / "main.rs").write_text("fn main() {}\n")
        (repo / "README.md").write_text("source\n")
        subprocess.run(["git", "-C", str(repo), "add", "-A"], check=True)
        subprocess.run(
            ["git", "-C", str(repo), "commit", "-qm", "prepared stable candidate"], check=True
        )
        source_sha = subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        return directory, repo, source_sha

    def test_derived_commit_changes_only_cargo_versions_and_keeps_source_parent(self):
        directory, repo, source_sha = self._repo()
        try:
            before = subprocess.run(
                ["git", "-C", str(repo), "status", "--porcelain"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            derived = github_release.derive_preview_commit(source_sha, "7.1.0-alpha.1", repo)
            changed = subprocess.run(
                [
                    "git",
                    "-C",
                    str(repo),
                    "diff-tree",
                    "--no-commit-id",
                    "--name-only",
                    "-r",
                    derived,
                ],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.splitlines()
            parent = subprocess.run(
                ["git", "-C", str(repo), "rev-parse", f"{derived}^"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            cargo = subprocess.run(
                ["git", "-C", str(repo), "show", f"{derived}:Cargo.toml"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            lock = subprocess.run(
                ["git", "-C", str(repo), "show", f"{derived}:Cargo.lock"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout

            self.assertEqual(parent, source_sha)
            self.assertEqual(changed, ["Cargo.lock", "Cargo.toml"])
            self.assertIn('version = "7.1.0-alpha.1"', cargo)
            self.assertIn('version = "7.1.0-alpha.1"', lock)
            self.assertEqual(before, "")
            self.assertEqual(
                subprocess.run(
                    ["git", "-C", str(repo), "status", "--porcelain"],
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout,
                "",
            )
            self.assertEqual(
                derived, github_release.derive_preview_commit(source_sha, "7.1.0-alpha.1", repo)
            )
        finally:
            directory.cleanup()

    def test_derived_commit_rejects_dev_version(self):
        directory, repo, source_sha = self._repo()
        try:
            with self.assertRaisesRegex(ValueError, "unsupported|dev"):
                github_release.derive_preview_commit(source_sha, "7.1.0-dev.12", repo)
        finally:
            directory.cleanup()


class GithubReleaseCliTests(unittest.TestCase):
    def test_resolve_candidate_cli_prints_all_immutable_identity_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            pr = root / "pr.json"
            live = root / "live.json"
            tags = root / "tags.json"
            releases = root / "releases.json"
            pr.write_text(json.dumps(_live_pr()))
            live.write_text(json.dumps(_live_pr(labels=["prerelease:alpha"])))
            tags.write_text(json.dumps(["v7.1.0-alpha.1"]))
            releases.write_text(json.dumps([]))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                status = cli.main(
                    [
                        "resolve-candidate",
                        "--pr-json",
                        str(pr),
                        "--live-pr-json",
                        str(live),
                        "--tags-json",
                        str(tags),
                        "--releases-json",
                        str(releases),
                        "--repo",
                        REPOSITORY,
                    ]
                )

        self.assertEqual(status, 0)
        payload = json.loads(output.getvalue())
        self.assertEqual(
            payload,
            {
                "base_sha": BASE_SHA,
                "build_sha": SOURCE_SHA,
                "channel": "testing",
                "line": BASE_REF,
                "phase": "alpha",
                "pr_number": 101,
                "source_sha": SOURCE_SHA,
                "source_snapshot": SNAPSHOT,
                "version": "7.1.0-alpha.2",
            },
        )

    def test_release_controller_listens_to_state_changes_and_keeps_runs(self):
        workflow = yaml.safe_load(
            (REPO_ROOT / ".github" / "workflows" / "release-controller.yml").read_text()
        )
        triggers = workflow["on"]
        self.assertEqual(
            set(triggers["pull_request"]["types"]),
            {"labeled", "unlabeled", "synchronize", "reopened"},
        )
        self.assertIn("workflow_dispatch", triggers)
        self.assertEqual(triggers["repository_dispatch"]["types"], ["release-line"])
        self.assertFalse(workflow["concurrency"]["cancel-in-progress"])
        self.assertIn("release-controller", workflow["concurrency"]["group"])

    def test_release_controller_rechecks_live_pr_before_stage_and_does_not_build(self):
        run = yaml.safe_load(
            (REPO_ROOT / ".github" / "workflows" / "release-controller.yml").read_text()
        )["jobs"]["resolve"]["steps"]
        rendered = "\n".join(step.get("run", "") for step in run)
        self.assertIn("pulls/$PR_NUMBER", rendered)
        self.assertIn("resolve-candidate", rendered)
        self.assertIn("release-candidate.yml", rendered)
        self.assertIn("build_sha", rendered)
        self.assertIn("gh workflow run", rendered)
        self.assertNotIn("cargo build", rendered)

    def test_release_controller_normalizes_all_paginated_release_pages(self):
        steps = yaml.safe_load(
            (REPO_ROOT / ".github" / "workflows" / "release-controller.yml").read_text()
        )["jobs"]["resolve"]["steps"]
        release_step = next(
            step for step in steps if step.get("name") == "Read published tags and release records"
        )
        run = release_step["run"]
        self.assertIn("gh api --paginate --slurp", run)
        self.assertIn("| jq 'add' > \"$RUNNER_TEMP/releases.json\"", run)

    def test_release_controller_keys_preview_ref_by_build_sha_without_force_push(self):
        steps = yaml.safe_load(
            (REPO_ROOT / ".github" / "workflows" / "release-controller.yml").read_text()
        )["jobs"]["resolve"]["steps"]
        preview_step = next(
            step
            for step in steps
            if step.get("name") == "Derive and preserve the preview build commit"
        )
        run = preview_step["run"]
        self.assertIn(
            'preview_ref="refs/heads/gel-preview-build/${preview_line}/${TAG}/${build_sha}"',
            run,
        )
        self.assertIn('git push origin "$build_sha:$preview_ref"', run)
        self.assertNotIn("git push --force", run)


if __name__ == "__main__":
    unittest.main()
