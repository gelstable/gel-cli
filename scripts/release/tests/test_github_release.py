import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import yaml

from gel_release import (
    candidate,
    cli,
    github_release,
    release_state,
    source_equivalence,
    verify_draft,
)
from gel_release.models import CandidateRecord

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


def _candidate_record(
    *,
    line: str = BASE_REF,
    pr_number: int = 101,
    phase: str | None = None,
    version: str = "7.1.0",
    base_sha: str = BASE_SHA,
    source_sha: str = SOURCE_SHA,
    build_sha: str | None = None,
    snapshot: str = SNAPSHOT,
    build_date: str = "2026-09-16T00:00:00+00:00",
    draft_release_id: int = 123456,
) -> CandidateRecord:
    return CandidateRecord.model_validate(
        {
            "schema_version": 2,
            "line": line,
            "pr_number": pr_number,
            "phase": phase,
            "version": version,
            "tag": f"v{version}",
            "draft_release_id": draft_release_id,
            "source_sha": source_sha,
            "source_snapshot": snapshot,
            "build_sha": build_sha if build_sha is not None else source_sha,
            "base_sha": base_sha,
            "build_date": build_date,
            "workflow_runs": [],
            "attestation": {
                "predicate_type": "https://slsa.dev/provenance/v1",
                "subject_count": 1,
            },
            "assets": [
                {
                    "id": 1,
                    "name": "candidate.tar.gz",
                    "size": 1,
                    "sha256": "0" * 64,
                    "blake2b512": "0" * 128,
                }
            ],
        }
    )


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

    def test_unpublished_tag_from_failed_patch_does_not_advance_suffix(self):
        identity = github_release.resolve_candidate(
            _release_pr(),
            _live_pr(labels=["prerelease:alpha"]),
            ["v7.1.0-alpha.1"],
            [
                {
                    "tag_name": "v7.1.0-alpha.1",
                    "draft": True,
                    "prerelease": True,
                }
            ],
        )
        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.version, "7.1.0-alpha.1")

    def test_tag_without_release_remains_occupied_with_unrelated_published_release(self):
        identity = github_release.resolve_candidate(
            _release_pr(),
            _live_pr(labels=["prerelease:alpha"]),
            ["v7.1.0-alpha.1", "v8.0.0-beta.1"],
            [_published_preview("8.0.0-beta.1", "e" * 64, phase="beta")],
        )

        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.version, "7.1.0-alpha.2")

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

    def _staged_repo(self) -> tuple[Path, str, str, str, str]:
        """Real repo with a prepared source and the staged record successor."""

        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        repo = Path(tmp.name)

        def git(*args: str) -> str:
            return subprocess.run(
                ["git", "-C", str(repo), *args],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

        git("init", "-q", "-b", "main", ".")
        git("config", "user.email", "test@example.com")
        git("config", "user.name", "Fixed Point Test")
        git("config", "commit.gpgsign", "false")
        (repo / "Cargo.toml").write_text('[package]\nname = "gel-cli"\nversion = "7.1.0"\n')
        (repo / "Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "gel-cli"\nversion = "7.1.0"\n'
        )
        (repo / "src").mkdir()
        (repo / "src" / "main.rs").write_text("fn main() {}\n")
        git("add", "-A")
        git("commit", "-qm", "release line base")
        base_sha = git("rev-parse", "HEAD")

        (repo / "README.md").write_text("prepared release\n")
        git("add", "-A")
        git("commit", "-qm", "chore: prepare release 7.1.0")
        source_sha = git("rev-parse", "HEAD")
        snapshot = source_equivalence.meaningful_tree(source_sha, repo)

        record = _candidate_record(base_sha=base_sha, source_sha=source_sha, snapshot=snapshot)
        (repo / "packaging").mkdir()
        (repo / "packaging" / "release-candidate.json").write_bytes(candidate.dump(record))
        git("add", "-A")
        git("commit", "-qm", "chore: stage release candidate v7.1.0")
        record_sha = git("rev-parse", "HEAD")
        return repo, base_sha, source_sha, record_sha, snapshot

    def test_staged_stable_record_is_the_resolution_fixed_point(self):
        repo, base, source, record_sha, snapshot = self._staged_repo()
        identity = github_release.resolve_candidate(
            _release_pr(base_sha=base, head_sha=record_sha),
            _live_pr(base_sha=base, head_sha=record_sha, snapshot=snapshot),
            [],
            [],
            checkout=repo,
        )
        self.assertIsNone(identity)

    def test_staged_record_for_a_different_identity_still_selects_a_candidate(self):
        repo, base, source, record_sha, snapshot = self._staged_repo()
        identity = github_release.resolve_candidate(
            _release_pr(base_sha=base, head_sha=record_sha),
            _live_pr(base_sha=base, head_sha=record_sha, version="7.2.0", snapshot=snapshot),
            [],
            [],
            checkout=repo,
        )
        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.version, "7.2.0")

    def test_head_without_the_record_is_not_the_fixed_point(self):
        repo, base, source, record_sha, snapshot = self._staged_repo()
        identity = github_release.resolve_candidate(
            _release_pr(base_sha=base, head_sha=source),
            _live_pr(base_sha=base, head_sha=source, snapshot=snapshot),
            [],
            [],
            checkout=repo,
        )
        self.assertIsNotNone(identity)

    def test_prepared_version_colliding_with_a_published_tag_fails_before_staging(self):
        releases = [
            {
                "id": 5,
                "tag_name": "v7.1.0",
                "name": "v7.1.0",
                "draft": False,
                "prerelease": False,
            }
        ]
        with self.assertRaisesRegex(ValueError, "already published|7.1.0"):
            github_release.resolve_candidate(_release_pr(), _live_pr(), ["v7.1.0"], releases)

    def test_unpublished_draft_tag_does_not_block_the_prepared_version(self):
        releases = [
            {
                "id": 5,
                "tag_name": "v7.1.0",
                "name": "v7.1.0",
                "draft": True,
                "prerelease": False,
            }
        ]
        identity = github_release.resolve_candidate(_release_pr(), _live_pr(), ["v7.1.0"], releases)
        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.version, "7.1.0")

    def test_unknown_head_sha_still_selects_a_candidate(self):
        repo, base, source, record_sha, snapshot = self._staged_repo()
        moved = "e" * 40
        identity = github_release.resolve_candidate(
            _release_pr(base_sha=base, head_sha=moved),
            _live_pr(base_sha=base, head_sha=moved, snapshot=snapshot),
            [],
            [],
            checkout=repo,
        )
        self.assertIsNotNone(identity)
        assert identity is not None
        self.assertEqual(identity.source_sha, moved)


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

    def test_older_drafts_and_published_releases_on_same_line_do_not_block_new_tag(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        older = {**identity.as_dict(), "version": "7.0.0", "tag": "v7.0.0"}
        releases = [
            {
                "id": 90,
                "tag_name": "v7.0.0",
                "name": "v7.0.0",
                "draft": False,
                "prerelease": False,
                "body": json.dumps({"candidate_identity": older}),
            },
            {
                "id": 91,
                "tag_name": "v7.0.0",
                "name": identity.tag,
                "draft": True,
                "prerelease": False,
                "body": json.dumps({"candidate_identity": older}),
            },
        ]

        self.assertIsNone(github_release.find_reusable_draft(releases, identity))

    def test_same_tag_source_refresh_is_replaceable_only_for_same_line_and_pr(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        stale = {**identity.as_dict(), "source_sha": "d" * 40, "build_sha": "d" * 40}
        release = {
            "id": 123,
            "tag_name": identity.tag,
            "name": identity.tag,
            "draft": True,
            "prerelease": False,
            "body": json.dumps({"candidate_identity": stale}),
        }

        self.assertEqual(
            github_release.find_replaceable_draft([release], identity),
            release,
        )
        for changed in ("line", "pr_number"):
            different = dict(stale)
            different[changed] = "release/v8.x" if changed == "line" else 102
            changed_release = {**release, "body": json.dumps({"candidate_identity": different})}
            with (
                self.subTest(changed=changed),
                self.assertRaisesRegex(ValueError, "identity|line|PR"),
            ):
                github_release.find_replaceable_draft([changed_release], identity)

    def test_same_tag_source_refresh_rejects_wrong_release_kind(self):
        identity = github_release.CandidateIdentity.from_dict(self.IDENTITY)
        stale = {**identity.as_dict(), "source_sha": "d" * 40, "build_sha": "d" * 40}
        release = {
            "id": 123,
            "tag_name": identity.tag,
            "name": identity.tag,
            "draft": True,
            "prerelease": True,
            "body": json.dumps({"candidate_identity": stale}),
        }
        with self.assertRaisesRegex(ValueError, "prerelease"):
            github_release.find_replaceable_draft([release], identity)

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


class StableMergeGateTests(unittest.TestCase):
    """The stable required check binds the PR, candidate, draft, and merge tree."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self._git("init", "-q", ".")
        self._git("config", "user.email", "test@example.com")
        self._git("config", "user.name", "Stable Gate Test")
        self._git("config", "commit.gpgsign", "false")
        (self.repo / "Cargo.toml").write_text('[package]\nname = "gel-cli"\nversion = "7.1.0"\n')
        (self.repo / "Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "gel-cli"\nversion = "7.1.0"\n'
        )
        (self.repo / "src").mkdir()
        (self.repo / "src" / "main.rs").write_text("fn main() {}\n")
        self._git("add", "-A")
        self._git("commit", "-qm", "release line base")
        self.base_sha = self._git("rev-parse", "HEAD")

        (self.repo / "README.md").write_text("prepared release\n")
        self._git("add", "-A")
        self._git("commit", "-qm", "prepared stable candidate")
        self.source_sha = self._git("rev-parse", "HEAD")
        self.snapshot = source_equivalence.meaningful_tree(self.source_sha, self.repo)
        self.record = self._record()
        self.staged_sha = self._stage_record()
        self.live = _live_pr(
            base_sha=self.base_sha,
            head_sha=self.staged_sha,
            snapshot=self.snapshot,
        )

    def _git(self, *argv: str) -> str:
        return subprocess.run(
            ["git", "-C", str(self.repo), *argv],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def _record(self, **overrides: object) -> CandidateRecord:
        value: dict[str, object] = {
            "schema_version": 2,
            "line": BASE_REF,
            "pr_number": 101,
            "phase": None,
            "version": "7.1.0",
            "tag": "v7.1.0",
            "draft_release_id": 123456,
            "source_sha": self.source_sha,
            "source_snapshot": self.snapshot,
            "build_sha": self.source_sha,
            "base_sha": self.base_sha,
            "build_date": "2026-09-16T00:00:00+00:00",
            "workflow_runs": [],
            "attestation": {
                "predicate_type": "https://slsa.dev/provenance/v1",
                "subject_count": 1,
            },
            "assets": [
                {
                    "id": 1,
                    "name": "candidate.tar.gz",
                    "size": 1,
                    "sha256": "0" * 64,
                    "blake2b512": "0" * 128,
                }
            ],
        }
        value.update(overrides)
        return CandidateRecord.model_validate(value)

    def _stage_record(self) -> str:
        target = self.repo / "packaging" / "release-candidate.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(candidate.dump(self.record))
        self._git("add", str(target.relative_to(self.repo)))
        self._git("commit", "-qm", "stage candidate record")
        return self._git("rev-parse", "HEAD")

    def _record_successor(self, record_bytes: bytes, *, source_change: bool = False) -> str:
        self._git("switch", "--detach", self.source_sha)
        target = self.repo / "packaging" / "release-candidate.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(record_bytes)
        if source_change:
            (self.repo / "README.md").write_text("source changed after staging\n")
        self._git("add", "-A")
        self._git("commit", "-qm", "stage candidate record")
        return self._git("rev-parse", "HEAD")

    def _merge_ref_sha(self) -> str:
        return subprocess.run(
            [
                "git",
                "-C",
                str(self.repo),
                "commit-tree",
                f"{self.staged_sha}^{{tree}}",
                "-p",
                self.staged_sha,
                "-p",
                self.base_sha,
            ],
            check=True,
            capture_output=True,
            text=True,
            input="prospective merge ref\n",
        ).stdout.strip()

    def _merge_sha(self, *generated: tuple[str, str]) -> str:
        for path, contents in generated:
            target = self.repo / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(contents)
        self._git("add", "-A")
        self._git("commit", "-qm", "merge generated metadata")
        return self._git("rev-parse", "HEAD")

    def _check(self, live: dict | None = None, merge_sha: str | None = None) -> None:
        with mock.patch("gel_release.verify_draft.verify"):
            github_release.check_stable_merge(
                self.record,
                live or self.live,
                merge_sha or self.staged_sha,
                self.repo,
            )

    def test_current_same_repository_generated_pr_with_verified_draft_is_accepted(self):
        self._check()

    def test_staged_record_successor_and_actual_merge_ref_are_accepted(self):
        self._check(merge_sha=self._merge_ref_sha())

    def test_staged_record_successor_requires_exact_record_bytes(self):
        value = self.record.model_dump(mode="json")
        value["build_date"] = "2026-09-17T00:00:00+00:00"
        successor = self._record_successor(candidate.dump(value))
        live = {**self.live, "head": {**self.live["head"], "sha": successor}}
        with self.assertRaisesRegex(ValueError, "record|bytes"):
            self._check(live=live, merge_sha=successor)

    def test_staged_record_successor_rejects_source_tree_drift(self):
        successor = self._record_successor(candidate.dump(self.record), source_change=True)
        live = {**self.live, "head": {**self.live["head"], "sha": successor}}
        with self.assertRaisesRegex(ValueError, "source|README|record"):
            self._check(live=live, merge_sha=successor)

    def test_pre_record_source_head_is_rejected_as_stale(self):
        live = {**self.live, "head": {**self.live["head"], "sha": self.source_sha}}
        with self.assertRaisesRegex(ValueError, "source|record"):
            self._check(live=live, merge_sha=self.source_sha)

    def test_generated_metadata_only_merge_tree_is_accepted(self):
        merge_sha = self._merge_sha(
            ("Formula/gel.rb", "class Gel < Formula\nend\n"),
            ("bucket/gel.json", "{}\n"),
            ("packaging/aur/PKGBUILD", "pkgname=gel-cli-bin\n"),
            ("packaging/release-candidate.json", "{}\n"),
        )
        self._check(merge_sha=merge_sha)

    def test_forked_head_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "fork|repository"):
            self._check(
                {**self.live, "head": {**self.live["head"], "repo": {"full_name": "fork/gel-cli"}}}
            )

    def test_wrong_head_or_base_names_are_rejected(self):
        for key, value, pattern in (
            ("head", {"ref": "feature/release-v7.x"}, "head|identity"),
            ("base", {"ref": "master"}, "base|line|identity"),
        ):
            with self.subTest(key=key):
                live = {**self.live, key: {**self.live[key], **value}}
                with self.assertRaisesRegex(ValueError, pattern):
                    self._check(live)

    def test_moved_release_line_base_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "base"):
            self._check({**self.live, "base": {**self.live["base"], "sha": "d" * 40}})

    def test_label_swap_cannot_keep_stale_stable_record_green(self):
        with self.assertRaisesRegex(ValueError, "phase"):
            self._check({**self.live, "labels": [{"name": "prerelease:beta"}]})

    def test_prepared_version_major_mismatch_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "version|major"):
            self._check({**self.live, "prepared_version": "8.1.0"})

    def test_missing_candidate_record_is_rejected(self):
        with mock.patch("gel_release.verify_draft.verify"):
            with self.assertRaisesRegex(ValueError, "record|candidate"):
                github_release.check_stable_merge(
                    None,  # type: ignore[arg-type]
                    self.live,
                    self.source_sha,
                    self.repo,
                )

    def test_changed_draft_assets_fail_the_gate(self):
        with mock.patch(
            "gel_release.verify_draft.verify",
            side_effect=verify_draft.DraftVerificationError("asset bytes changed"),
        ):
            with self.assertRaisesRegex(ValueError, "draft|asset|bytes"):
                github_release.check_stable_merge(
                    self.record,
                    self.live,
                    self.source_sha,
                    self.repo,
                )

    def test_prospective_merge_tree_with_source_change_is_rejected(self):
        (self.repo / "src" / "main.rs").write_text('fn main() { println!("changed"); }\n')
        self._git("add", "-A")
        self._git("commit", "-qm", "source change in merge")
        merge_sha = self._git("rev-parse", "HEAD")
        with self.assertRaisesRegex(ValueError, "source|drift|equivalent"):
            self._check(merge_sha=merge_sha)


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


class PublicationTests(unittest.TestCase):
    """Publication rechecks reviewed identity before changing GitHub state."""

    def setUp(self):
        # The publication fixtures use synthetic SHAs. Individual topology
        # tests stop this adapter and exercise the real first-parent check.
        self._topology = mock.patch.object(github_release, "_assert_line_push_base")
        self._topology.start()
        self.addCleanup(self._topology.stop)
        self._tag_lookup = mock.patch.object(verify_draft, "resolve_tag_commit", return_value=None)
        self._tag_lookup.start()
        self.addCleanup(self._tag_lookup.stop)

    def _preview_identity(self) -> dict[str, object]:
        return {
            "line": BASE_REF,
            "pr_number": 101,
            "base_sha": BASE_SHA,
            "source_sha": SOURCE_SHA,
            "build_sha": "d" * 40,
            "source_snapshot": SNAPSHOT,
            "phase": "alpha",
            "version": "7.1.0-alpha.1",
            "channel": "testing",
        }

    def _stable_identity(self) -> dict[str, object]:
        return {
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

    def _record(self, identity: dict[str, object]) -> CandidateRecord:
        return CandidateRecord.model_validate(
            {
                "schema_version": 2,
                "line": identity["line"],
                "pr_number": identity["pr_number"],
                "phase": identity["phase"],
                "version": identity["version"],
                "tag": f"v{identity['version']}",
                "draft_release_id": 123456,
                "source_sha": identity["source_sha"],
                "source_snapshot": identity["source_snapshot"],
                "build_sha": identity["build_sha"],
                "base_sha": identity["base_sha"],
                "build_date": "2026-09-16T00:00:00+00:00",
                "workflow_runs": [],
                "attestation": {
                    "predicate_type": "https://slsa.dev/provenance/v1",
                    "subject_count": 1,
                },
                "assets": [
                    {
                        "id": 1,
                        "name": "candidate.tar.gz",
                        "size": 1,
                        "sha256": "0" * 64,
                        "blake2b512": "0" * 128,
                    }
                ],
            }
        )

    def _preview_pr(self, **overrides: object) -> dict:
        values = _live_pr(
            labels=["prerelease:alpha"],
            version="7.1.0",
            snapshot=SNAPSHOT,
        )
        values["phase_authorized"] = True
        values.update(overrides)
        return values

    def _release(
        self,
        identity: dict[str, object],
        record: CandidateRecord,
        *,
        draft: bool = True,
        tag_target: str | None = None,
    ) -> dict:
        value = {
            "id": 123456,
            "tag_name": f"v{identity['version']}",
            "name": f"v{identity['version']}",
            "draft": draft,
            "prerelease": identity["phase"] is not None,
            "body": github_release.candidate_identity_body(identity),
            "tag_target": tag_target,
            "asset_bytes": {"gel-candidate.json": candidate.dump(record)},
        }
        return value

    def _stable_release(self, record: CandidateRecord, *, draft: bool = True) -> dict:
        identity = self._stable_identity()
        value = self._release(identity, record, draft=draft, tag_target="e" * 40)
        value["merged_pr"] = {
            "number": 101,
            "state": "closed",
            "merged": True,
            "merge_commit_sha": "e" * 40,
            "base": {
                "ref": BASE_REF,
                "sha": BASE_SHA,
                "repo": {"full_name": REPOSITORY},
            },
            "head": {
                "ref": HEAD_REF,
                "sha": SOURCE_SHA,
                "repo": {"full_name": REPOSITORY},
            },
        }
        value["record_introduced"] = True
        value["source_equivalent"] = True
        value["published_stable_versions"] = ["7.1.0"]
        return value

    def test_preview_rejects_removed_phase_before_mutation(self):
        identity = self._preview_identity()
        record = self._record(identity)
        release = self._release(identity, record)
        with mock.patch.object(github_release, "_gh_mutate") as mutate:
            with self.assertRaisesRegex(ValueError, "phase"):
                github_release.publish_preview(
                    identity, record, self._preview_pr(labels=[]), release
                )
            mutate.assert_not_called()

    def test_preview_rejects_changed_snapshot_and_line_or_pr(self):
        identity = self._preview_identity()
        record = self._record(identity)
        release = self._release(identity, record)
        for changed in (
            {"source_snapshot": "e" * 64},
            {"base": {**self._preview_pr()["base"], "ref": "release/v8.x"}},
            {"number": 102},
        ):
            with self.subTest(changed=changed):
                live = self._preview_pr(**changed)
                with self.assertRaisesRegex(ValueError, "snapshot|line|PR|identity"):
                    github_release.publish_preview(identity, record, live, release)

    def test_preview_rejects_stale_draft_identity(self):
        identity = self._preview_identity()
        record = self._record(identity)
        stale = self._release(identity, record)
        stale["body"] = github_release.candidate_identity_body({**identity, "source_sha": "e" * 40})
        with self.assertRaisesRegex(ValueError, "identity|draft"):
            github_release.publish_preview(identity, record, self._preview_pr(), stale)

    def test_preview_same_published_snapshot_retry_does_not_mutate(self):
        identity = self._preview_identity()
        record = self._record(identity)
        release = self._release(identity, record, draft=False, tag_target=identity["build_sha"])
        with mock.patch.object(
            verify_draft, "resolve_tag_commit", return_value=identity["build_sha"]
        ):
            with mock.patch.object(github_release, "_gh_mutate") as mutate:
                github_release.publish_preview(identity, record, self._preview_pr(), release)
            mutate.assert_not_called()

    def test_preview_rejects_immutable_mismatched_existing_tag(self):
        identity = self._preview_identity()
        record = self._record(identity)
        release = self._release(identity, record, tag_target="e" * 40)
        with mock.patch.object(verify_draft, "resolve_tag_commit", return_value="e" * 40):
            with self.assertRaisesRegex(ValueError, "tag|commit|target"):
                github_release.publish_preview(identity, record, self._preview_pr(), release)

    def test_preview_rejects_changed_inline_distribution_bytes(self):
        identity = self._preview_identity()
        record = self._record(identity)
        release = self._release(identity, record)
        release["asset_bytes"]["candidate.tar.gz"] = b"x"
        with self.assertRaisesRegex(ValueError, "asset|bytes|SHA"):
            github_release.publish_preview(identity, record, self._preview_pr(), release)

    def test_preview_creates_derived_commit_tag_and_publishes_existing_draft(self):
        identity = self._preview_identity()
        record = self._record(identity)
        release = self._release(identity, record, tag_target=None)
        with mock.patch.object(github_release, "_gh_mutate") as mutate:
            with mock.patch.object(verify_draft, "verify"):
                github_release.publish_preview(identity, record, self._preview_pr(), release)
        calls = [call.args for call in mutate.call_args_list]
        self.assertEqual(len(calls), 2)
        self.assertIn("refs/tags/v7.1.0-alpha.1", str(calls[0]))
        self.assertIn(identity["build_sha"], str(calls[0]))
        method, path, fields = calls[1]
        self.assertEqual(method, "PATCH")
        self.assertEqual(
            fields,
            {
                "tag_name": "v7.1.0-alpha.1",
                "draft": False,
                "prerelease": True,
                # GitHub documents make_latest as the string enum
                # "true"|"false"|"legacy"; previews never become latest.
                "make_latest": "false",
            },
        )

    def test_preview_patch_failure_can_retry_existing_unpublished_tag(self):
        identity = self._preview_identity()
        record = self._record(identity)
        release = self._release(identity, record, tag_target=identity["build_sha"])
        with mock.patch.object(
            verify_draft, "resolve_tag_commit", return_value=identity["build_sha"]
        ):
            with mock.patch.object(verify_draft, "verify"):
                with mock.patch.object(
                    github_release,
                    "_publish_release",
                    side_effect=[ValueError("PATCH failed"), None],
                ) as publish:
                    with self.assertRaisesRegex(ValueError, "PATCH"):
                        github_release.publish_preview(
                            identity, record, self._preview_pr(), release
                        )
                    github_release.publish_preview(identity, record, self._preview_pr(), release)
        self.assertEqual(publish.call_count, 2)

    def test_preview_rechecks_live_state_after_draft_verification(self):
        identity = self._preview_identity()
        record = self._record(identity)
        release = self._release(identity, record)
        changed = self._preview_pr()
        changed["head"] = {**changed["head"], "sha": "e" * 40}
        with mock.patch.object(verify_draft, "verify"):
            with self.assertRaisesRegex(ValueError, "source|head|identity"):
                github_release.publish_preview(
                    identity,
                    record,
                    self._preview_pr(),
                    release,
                    refresh_live_pr=lambda _identity, _initial: changed,
                )

    def test_stable_backport_without_merge_matching_record_is_a_noop(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record)
        release["merged_pr"]["number"] = 999
        with mock.patch.object(github_release, "_gh_mutate") as mutate:
            github_release.publish_stable(record, "e" * 40, release)
            mutate.assert_not_called()

    def test_stable_valid_merge_creates_tag_at_actual_merge_sha(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record)
        release["tag_target"] = None
        with mock.patch.object(github_release, "_gh_mutate") as mutate:
            with mock.patch.object(github_release.verify_draft, "verify"):
                github_release.publish_stable(record, "e" * 40, release)
        calls = [call.args for call in mutate.call_args_list]
        self.assertEqual(len(calls), 2)
        self.assertIn("refs/tags/v7.1.0", str(calls[0]))
        self.assertIn("e" * 40, str(calls[0]))
        _method, _path, fields = calls[1]
        self.assertEqual(
            fields,
            {
                "tag_name": "v7.1.0",
                "draft": False,
                "prerelease": False,
                # The newest stable across every line becomes latest, encoded
                # as the documented string enum.
                "make_latest": "true",
            },
        )

    def test_stable_older_line_publishes_without_the_latest_flag(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record)
        release["published_stable_versions"] = ["7.1.0", "8.0.0"]
        with mock.patch.object(github_release, "_gh_mutate") as mutate:
            with mock.patch.object(github_release.verify_draft, "verify"):
                github_release.publish_stable(record, "e" * 40, release)
        _method, _path, fields = mutate.call_args.args
        self.assertEqual(fields["make_latest"], "false")

    def test_stable_rejects_mismatched_tag_target(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record)
        release["tag_target"] = "f" * 40
        with mock.patch.object(verify_draft, "resolve_tag_commit", return_value="f" * 40):
            with self.assertRaisesRegex(ValueError, "tag|target|commit"):
                github_release.publish_stable(record, "e" * 40, release)

    def _release_repo(self) -> tuple[Path, str, str, str, dict[str, object], CandidateRecord]:
        """Build a real line history with a prepared and staged generated branch.

        Returns the repo path, base SHA, tested source SHA, generated head
        (the record successor), the stable identity, and the staged record.
        """

        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        repo = Path(tmp.name)

        def git(*args: str) -> str:
            return subprocess.run(
                ["git", "-C", str(repo), *args],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

        git("init", "-q", "-b", "main", ".")
        git("config", "user.email", "test@example.com")
        git("config", "user.name", "Publication Topology Test")
        git("config", "commit.gpgsign", "false")
        (repo / "Cargo.toml").write_text('[package]\nname = "gel-cli"\nversion = "7.1.0"\n')
        (repo / "Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "gel-cli"\nversion = "7.1.0"\n'
        )
        (repo / "src").mkdir()
        (repo / "src" / "main.rs").write_text("fn main() {}\n")
        git("add", "-A")
        git("commit", "-qm", "release line base")
        base_sha = git("rev-parse", "HEAD")

        git("switch", "-c", HEAD_REF, base_sha)
        (repo / "CHANGELOG.md").write_text("# 7.1.0\n")
        git("add", "-A")
        git("commit", "-qm", "chore: prepare release 7.1.0")
        source_sha = git("rev-parse", "HEAD")
        snapshot = source_equivalence.meaningful_tree(source_sha, repo)
        identity = {
            **self._stable_identity(),
            "base_sha": base_sha,
            "source_sha": source_sha,
            "build_sha": source_sha,
            "source_snapshot": snapshot,
        }
        record = self._record(identity)
        (repo / "packaging").mkdir()
        (repo / "packaging" / "release-candidate.json").write_bytes(candidate.dump(record))
        git("add", "-A")
        git("commit", "-qm", "chore: stage release candidate v7.1.0")
        record_sha = git("rev-parse", "HEAD")
        return repo, base_sha, source_sha, record_sha, identity, record

    def _merged_release(
        self,
        identity: dict[str, object],
        record: CandidateRecord,
        *,
        push_sha: str,
        record_sha: str,
    ) -> dict:
        value = self._release(identity, record, tag_target=None)
        value["merged_pr"] = {
            "number": 101,
            "state": "closed",
            "merged": True,
            "merge_commit_sha": push_sha,
            "base": {
                "ref": BASE_REF,
                "sha": identity["base_sha"],
                "repo": {"full_name": REPOSITORY},
            },
            "head": {
                "ref": HEAD_REF,
                "sha": record_sha,
                "repo": {"full_name": REPOSITORY},
            },
        }
        value["published_stable_versions"] = ["7.1.0"]
        return value

    def test_stable_publishes_through_merge_commit_squash_and_rebase_shapes(self):
        shapes = (
            (
                "merge commit",
                lambda git, base, head, _source, _record: git(
                    "merge", "--no-ff", "-m", "merge (#101)", head
                ),
            ),
            (
                "squash",
                lambda git, base, head, _source, _record: (
                    git("merge", "--squash", head),
                    git("commit", "-qm", "merge (#101)"),
                ),
            ),
            (
                "rebase",
                lambda git, base, head, source, record: git("cherry-pick", source, record),
            ),
        )
        for shape, prepare in shapes:
            with self.subTest(shape=shape):
                self._topology.stop()
                repo, base, source, record_sha, identity, record = self._release_repo()
                # _matching_merge_pr inspects the workflow checkout at Path("."),
                # so these topology tests run inside the fixture repository.
                working_dir = Path.cwd()
                self.addCleanup(os.chdir, working_dir)
                os.chdir(repo)

                def git(*args: str) -> str:
                    return subprocess.run(
                        ["git", "-C", str(repo), *args],
                        check=True,
                        capture_output=True,
                        text=True,
                    ).stdout.strip()

                git("switch", "-C", BASE_REF, base)
                prepare(git, base, HEAD_REF, source, record_sha)
                push_sha = git("rev-parse", BASE_REF)
                release = self._merged_release(
                    identity, record, push_sha=push_sha, record_sha=record_sha
                )
                with mock.patch.object(github_release, "_gh_mutate") as mutate:
                    with mock.patch.object(github_release.verify_draft, "verify"):
                        github_release.publish_stable(record, push_sha, release)
                self.assertTrue(
                    any("refs/tags/v7.1.0" in str(call.args) for call in mutate.call_args_list)
                )
                self.assertEqual(
                    mutate.call_args.args[2]["make_latest"],
                    "true",
                )

    def test_stable_draft_verification_accepts_actual_merge_tag_target(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record)
        with mock.patch.object(github_release, "_gh_mutate"):
            with mock.patch.object(verify_draft, "verify") as verify:
                github_release.publish_stable(record, "e" * 40, release)
        self.assertEqual(verify.call_args.kwargs.get("expected_tag_target"), "e" * 40)

    def test_stable_patch_failure_can_retry_existing_unpublished_tag(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record)
        with mock.patch.object(verify_draft, "resolve_tag_commit", return_value="e" * 40):
            with mock.patch.object(verify_draft, "verify"):
                with mock.patch.object(
                    github_release,
                    "_publish_release",
                    side_effect=[ValueError("PATCH failed"), None],
                ) as publish:
                    with self.assertRaisesRegex(ValueError, "PATCH"):
                        github_release.publish_stable(record, "e" * 40, release)
                    github_release.publish_stable(record, "e" * 40, release)
        self.assertEqual(publish.call_count, 2)

    def test_tag_target_resolves_the_actual_ref_when_release_has_sha_metadata(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record)
        release["target_commitish"] = "f" * 40
        release["tag"] = {"object": {"type": "commit", "sha": "f" * 40}}
        with mock.patch.object(verify_draft, "resolve_tag_commit", return_value=None) as resolve:
            self.assertIsNone(github_release._tag_target(record.tag, release))
        resolve.assert_called_once_with(record.tag, REPOSITORY)

    def test_stable_rejects_moved_merge_base(self):
        self._topology.stop()
        repo, base, _source, record_sha, identity, record = self._release_repo()
        working_dir = Path.cwd()
        self.addCleanup(os.chdir, working_dir)
        os.chdir(repo)

        def git(*args: str) -> str:
            return subprocess.run(
                ["git", "-C", str(repo), *args],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

        # A squash onto a line that moved past the recorded base: the chain
        # still contains the base several commits back only when the push was
        # rebased onto a descendant; craft a push rooted on unrelated history.
        git("switch", "-C", BASE_REF, base)
        unrelated = git("commit-tree", f"{record_sha}^{{tree}}", "-m", "unrelated root")
        push_sha = git("commit-tree", f"{record_sha}^{{tree}}", "-p", unrelated, "-m", "merge")
        release = self._merged_release(identity, record, push_sha=push_sha, record_sha=record_sha)
        with self.assertRaisesRegex(ValueError, "base|candidate"):
            github_release.publish_stable(record, push_sha, release)

    def test_stable_rejects_changed_draft_bytes(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record)
        with mock.patch.object(
            verify_draft,
            "verify",
            side_effect=verify_draft.DraftVerificationError("asset bytes changed"),
        ):
            with self.assertRaisesRegex(ValueError, "draft|asset|bytes"):
                github_release.publish_stable(record, "e" * 40, release)

    def test_stable_already_published_matching_retry_is_a_noop(self):
        identity = self._stable_identity()
        record = self._record(identity)
        release = self._stable_release(record, draft=False)
        with mock.patch.object(verify_draft, "resolve_tag_commit", return_value="e" * 40):
            with mock.patch.object(github_release, "_gh_mutate") as mutate:
                github_release.publish_stable(record, "e" * 40, release)
            mutate.assert_not_called()

    def test_latest_uses_numeric_semver_across_release_lines(self):
        self.assertFalse(github_release.should_make_latest("7.10.1", ["8.0.0"]))
        self.assertTrue(github_release.should_make_latest("8.0.1", ["7.10.1", "8.0.0"]))


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
