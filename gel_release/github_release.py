"""GitHub release-line identity and preview commit mechanics.

The controller deliberately keeps GitHub API concerns at the edge.  It passes
the freshly fetched pull request and the published release inventory into
``resolve_candidate``; that function returns one immutable candidate identity
or ``None`` when the current phase and source snapshot have already been
published.  Preview builds are derived from the original pull-request head by
Git plumbing, leaving the prepared stable branch unchanged.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import re
import subprocess
import tarfile
import tempfile
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from . import assets, candidate, preview, release_state, source_equivalence, verify_draft
from .models import CandidateRecord

_GIT_SHA = re.compile(r"^[0-9a-f]{40}$")
_SNAPSHOT = re.compile(r"^[0-9a-f]{64}$")
_PREVIEW_VERSION = re.compile(
    r"^(?P<base>(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))"
    r"-(?P<phase>alpha|beta|rc)\.(?P<number>[1-9][0-9]*)$"
)
_STABLE_VERSION = re.compile(r"^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)$")
_PHASE_LABELS = {"alpha": "prerelease:alpha", "beta": "prerelease:beta", "rc": "prerelease:rc"}
_AUTHORIZED_PERMISSIONS = frozenset({"write", "maintain", "admin"})


@dataclass(frozen=True, slots=True)
class CandidateIdentity:
    """Immutable input accepted by the candidate staging workflow."""

    line: str
    pr_number: int
    base_sha: str
    source_sha: str
    source_snapshot: str
    phase: str | None
    version: str
    channel: str
    build_sha: str | None = None

    def __post_init__(self) -> None:
        major = release_state.parse_line(self.line)
        if (
            isinstance(self.pr_number, bool)
            or not isinstance(self.pr_number, int)
            or self.pr_number < 1
        ):
            raise ValueError(f"invalid release PR number {self.pr_number!r}")
        for name, value in (
            ("base", self.base_sha),
            ("source", self.source_sha),
        ):
            if not isinstance(value, str) or _GIT_SHA.fullmatch(value) is None:
                raise ValueError(f"invalid {name} SHA {value!r}")
        if self.build_sha is None:
            object.__setattr__(self, "build_sha", self.source_sha)
        elif _GIT_SHA.fullmatch(self.build_sha) is None:
            raise ValueError(f"invalid build SHA {self.build_sha!r}")
        if (
            not isinstance(self.source_snapshot, str)
            or _SNAPSHOT.fullmatch(self.source_snapshot) is None
        ):
            raise ValueError(f"invalid source snapshot {self.source_snapshot!r}")
        if self.phase is None:
            preview.stable_version(self.version, major)
            if self.channel != "stable":
                raise ValueError("stable candidates must use the stable channel")
            if self.build_sha != self.source_sha:
                raise ValueError("stable candidate build SHA must equal source SHA")
        else:
            if self.phase not in _PHASE_LABELS:
                raise ValueError(f"unsupported candidate phase {self.phase!r}")
            match = _PREVIEW_VERSION.fullmatch(self.version)
            if match is None:
                raise ValueError(f"unsupported preview version {self.version!r}")
            if int(match.group("base").split(".", 1)[0]) != major:
                raise ValueError(
                    f"line {self.line} major {major} does not match version {self.version}"
                )
            if match.group("phase") != self.phase:
                raise ValueError("candidate phase must match the version")
            if self.channel != "testing":
                raise ValueError("preview candidates must use the testing channel")

    @property
    def tag(self) -> str:
        """Return the Git tag name for this candidate."""

        return f"v{self.version}"

    @property
    def pr(self) -> int:
        """Compatibility alias for callers that refer to the PR as ``pr``."""

        return self.pr_number

    @property
    def meaningful_snapshot(self) -> str:
        """Return the source snapshot under the design document's name."""

        return self.source_snapshot

    def as_dict(self) -> dict[str, object]:
        """Serialize only the fields needed to reproduce the candidate."""

        return {
            "line": self.line,
            "pr_number": self.pr_number,
            "base_sha": self.base_sha,
            "source_sha": self.source_sha,
            "source_snapshot": self.source_snapshot,
            "phase": self.phase,
            "version": self.version,
            "channel": self.channel,
            "build_sha": self.build_sha,
        }

    @classmethod
    def from_dict(cls, value: Mapping[str, object]) -> CandidateIdentity:
        """Validate an identity read from workflow JSON."""

        if not isinstance(value, Mapping):
            raise ValueError("candidate identity must be a JSON object")
        required = (
            "line",
            "pr_number",
            "base_sha",
            "source_sha",
            "source_snapshot",
            "phase",
            "version",
            "channel",
        )
        missing = [name for name in required if name not in value]
        if missing:
            raise ValueError(f"candidate identity is missing fields: {', '.join(missing)}")
        return cls(
            line=value["line"],  # type: ignore[arg-type]
            pr_number=value["pr_number"],  # type: ignore[arg-type]
            base_sha=value["base_sha"],  # type: ignore[arg-type]
            source_sha=value["source_sha"],  # type: ignore[arg-type]
            source_snapshot=value["source_snapshot"],  # type: ignore[arg-type]
            phase=value["phase"],  # type: ignore[arg-type]
            version=value["version"],  # type: ignore[arg-type]
            channel=value["channel"],  # type: ignore[arg-type]
            build_sha=value.get("build_sha"),  # type: ignore[arg-type]
        )


def _coerce_identity(identity: CandidateIdentity | Mapping[str, object]) -> CandidateIdentity:
    if isinstance(identity, CandidateIdentity):
        return identity
    if isinstance(identity, Mapping):
        return CandidateIdentity.from_dict(identity)
    raise ValueError("candidate identity must be a CandidateIdentity or JSON object")


def assert_live_identity(
    identity: CandidateIdentity | Mapping[str, object], live_pr: Mapping[str, object]
) -> None:
    """Require a live pull request to still describe an immutable candidate.

    A staging run may take long enough for the PR head, base, labels, or
    prepared metadata to move.  Stable record publication calls this check
    immediately before pushing the record to the generated PR branch.
    """

    expected = _coerce_identity(identity)
    if not isinstance(live_pr, Mapping):
        raise ValueError("live release PR must be a JSON object")
    base = live_pr.get("base")
    if not isinstance(base, Mapping):
        raise ValueError("live release PR has no base identity")
    base_repo = base.get("repo")
    repository = base_repo.get("full_name") if isinstance(base_repo, Mapping) else None
    if not isinstance(repository, str) or not repository:
        raise ValueError("live release PR has no base repository")
    live = release_state.validate_pr(dict(live_pr), repository)

    # A preview identity carries the selected prerelease version while the
    # generated PR keeps its planned plain stable version.  Compare the
    # prepared PR metadata to that plain base; stable candidates compare the
    # version unchanged.
    prepared_version = expected.version
    if expected.phase is not None:
        prepared_version = expected.version.split("-", 1)[0]
    checks: tuple[tuple[str, object, object], ...] = (
        ("PR number", live.number, expected.pr_number),
        ("release line", live.base_ref, expected.line),
        ("base SHA", live.base_sha, expected.base_sha),
        ("source SHA", live.head_sha, expected.source_sha),
        ("prepared version", _prepared_version(live_pr), prepared_version),
        ("source snapshot", _source_snapshot(live_pr), expected.source_snapshot),
        ("phase", release_state.phase_from_labels(_labels(live_pr)), expected.phase),
    )
    live_build_sha = _field(live_pr, "build_sha")
    if live_build_sha is not None:
        checks += (("build SHA", live_build_sha, expected.build_sha),)
    live_channel = _field(live_pr, "channel")
    if live_channel is not None:
        checks += (("channel", live_channel, expected.channel),)
    for name, actual, wanted in checks:
        if actual != wanted:
            raise ValueError(f"live PR {name} {actual!r} does not match candidate {wanted!r}")
    expected_channel = "testing" if expected.phase is not None else "stable"
    if expected.channel != expected_channel:
        raise ValueError(
            f"candidate channel {expected.channel!r} does not match its phase {expected.phase!r}"
        )


def check_stable_merge(
    record: CandidateRecord,
    live_pr: dict,
    merge_sha: str,
    repo: Path,
) -> None:
    """Validate every identity boundary required before a stable merge.

    The candidate record was produced from a reviewed draft on the generated
    PR head. This check re-reads the live PR, the prospective merge tree, both
    Cargo version files, and the draft release before allowing the PR to merge.
    Generated distribution metadata is ignored only by ``source_equivalence``'s
    explicit four-path allowlist.
    """

    validated = candidate.validate_record(record)
    if validated.phase is not None:
        raise ValueError(
            f"stable merge candidate must have no active phase; record has {validated.phase!r}"
        )
    if not isinstance(repo, Path):
        raise ValueError(f"candidate source repository must be a Path, got {repo!r}")
    if not isinstance(merge_sha, str) or _GIT_SHA.fullmatch(merge_sha) is None:
        raise ValueError(f"prospective merge revision has an invalid SHA {merge_sha!r}")

    live = release_state.validate_pr(live_pr, assets.REPOSITORY)
    active_phase = release_state.phase_from_labels(_labels(live_pr))
    if active_phase is not None:
        raise ValueError(f"stable merge candidate cannot have active phase label {active_phase!r}")

    checks: tuple[tuple[str, object, object], ...] = (
        ("PR number", live.number, validated.pr_number),
        ("release line", live.base_ref, validated.line),
        ("base SHA", live.base_sha, validated.base_sha),
    )
    for name, actual, expected in checks:
        if actual != expected:
            raise ValueError(f"live PR {name} {actual!r} does not match candidate {expected!r}")

    try:
        expected_record = candidate.dump(validated)
        if live.head_sha == validated.source_sha:
            # A retry can find the record already committed by an earlier
            # staging attempt. It is still required to contain the exact
            # record bytes before this equality is accepted.
            source_equivalence.assert_record_present(
                validated.source_sha,
                expected_record,
                repo,
            )
        else:
            # The staging workflow commits the record after recording the
            # tested source SHA. Require that one record-only successor so a
            # newer source or an unrelated generated commit cannot pass.
            source_equivalence.assert_record_successor(
                validated.source_sha,
                live.head_sha,
                expected_record,
                repo,
            )
    except source_equivalence.SourceDrift as error:
        raise ValueError(f"live PR source SHA check failed: {error}") from error

    version_fields = ("prepared_version", "cargo_version", "version")
    head = live_pr.get("head")
    has_version = any(name in live_pr for name in version_fields) or (
        isinstance(head, Mapping) and any(name in head for name in version_fields)
    )
    live_version = _prepared_version(live_pr) if has_version else None
    if live_version is not None and live_version != validated.version:
        raise ValueError(
            f"live PR prepared version {live_version!r} does not match candidate "
            f"{validated.version!r}"
        )

    snapshot_fields = ("source_snapshot", "meaningful_tree", "snapshot")
    has_snapshot = any(name in live_pr for name in snapshot_fields) or (
        isinstance(head, Mapping) and any(name in head for name in snapshot_fields)
    )
    live_snapshot = _source_snapshot(live_pr) if has_snapshot else None
    if live_snapshot is not None and live_snapshot != validated.source_snapshot:
        raise ValueError(
            f"live PR source snapshot {live_snapshot!r} does not match candidate "
            f"{validated.source_snapshot!r}"
        )

    try:
        source_equivalence.assert_snapshot(validated.source_snapshot, validated.source_sha, repo)
        source_equivalence.assert_merge_equivalent(validated.source_sha, merge_sha, repo)
    except source_equivalence.SourceDrift as error:
        raise ValueError(f"stable merge source check failed: {error}") from error

    # The source-equivalence check includes Cargo files, but checking both
    # revisions explicitly also proves that the files resolve to the plain
    # stable version expected by the candidate record.
    verify_draft.check_cargo_version(validated.source_sha, validated.version, repo)
    verify_draft.check_cargo_version(merge_sha, validated.version, repo)

    expected_identity = {
        "line": validated.line,
        "pr_number": validated.pr_number,
        "base_sha": validated.base_sha,
        "source_sha": validated.source_sha,
        "build_sha": validated.build_sha,
        "source_snapshot": validated.source_snapshot,
        "phase": None,
        "version": validated.version,
        "channel": "stable",
    }
    with tempfile.TemporaryDirectory(prefix="stable-merge-gate-") as directory:
        try:
            verify_draft.verify(
                validated,
                Path(directory),
                repo=assets.REPOSITORY,
                verify_attestations_flag=True,
                expected_identity=expected_identity,
            )
        except verify_draft.DraftVerificationError as error:
            raise ValueError(f"stable candidate draft verification failed: {error}") from error


def _body_identity(release: Mapping[str, object]) -> Mapping[str, object] | None:
    """Extract a candidate identity from a release body or API wrapper."""

    value: object = release.get("candidate_identity")
    if not isinstance(value, str) and not isinstance(value, Mapping):
        value = release.get("body")
    if isinstance(value, Mapping):
        nested = value.get("candidate_identity")
        return nested if isinstance(nested, Mapping) else value
    if not isinstance(value, str):
        return None
    try:
        parsed = json.loads(value)
    except json.JSONDecodeError:
        start, end = value.find("{"), value.rfind("}")
        if start < 0 or end <= start:
            return None
        try:
            parsed = json.loads(value[start : end + 1])
        except json.JSONDecodeError:
            return None
    if not isinstance(parsed, Mapping):
        return None
    nested = parsed.get("candidate_identity")
    return nested if isinstance(nested, Mapping) else parsed


def find_reusable_draft(
    releases: list[Mapping[str, object]],
    identity: CandidateIdentity | Mapping[str, object],
) -> Mapping[str, object] | None:
    """Find the one unpublished draft whose tag and identity exactly match.

    A release with the same tag but a different line or candidate identity is
    an unsafe collision.  Published releases and malformed old drafts fail
    closed so a retry cannot replace immutable release bytes.
    """

    expected = _coerce_identity(identity)
    if not isinstance(releases, list):
        raise ValueError("GitHub releases must be a list")
    expected_tag = expected.tag
    reusable: Mapping[str, object] | None = None
    for index, release in enumerate(releases):
        if not isinstance(release, Mapping):
            raise ValueError(f"release entry {index} is not an object")
        payload = _body_identity(release)
        identity_line = payload.get("line") if payload is not None else None
        tag_matches = release.get("tag_name") == expected_tag or release.get("name") == expected_tag
        line_matches = identity_line == expected.line
        if not tag_matches and not line_matches:
            continue
        if release.get("tag_name") != expected_tag or release.get("name") != expected_tag:
            raise ValueError(f"release for candidate line {expected.line} has a tag/name mismatch")
        if release.get("draft") is not True:
            raise ValueError(f"release {expected_tag} is published; refusing to reuse it")
        if payload is None:
            raise ValueError(f"draft {expected_tag} has no candidate identity")
        try:
            actual = CandidateIdentity.from_dict(payload)
        except (TypeError, ValueError) as error:
            raise ValueError(f"draft {expected_tag} has an invalid candidate identity") from error
        if actual.as_dict() != expected.as_dict():
            raise ValueError(
                f"draft {expected_tag} candidate identity does not match selected identity"
            )
        expected_prerelease = expected.phase is not None
        if release.get("prerelease") is not expected_prerelease:
            raise ValueError(f"draft {expected_tag} prerelease flag does not match candidate phase")
        if reusable is not None:
            raise ValueError(f"more than one reusable draft exists for {expected_tag}")
        reusable = release
    return reusable


def candidate_identity_body(identity: CandidateIdentity | Mapping[str, object]) -> str:
    """Serialize an identity for the release body used by retry checks."""

    return json.dumps({"candidate_identity": _coerce_identity(identity).as_dict()}, sort_keys=True)


def _labels(pr: Mapping[str, object]) -> list[str]:
    raw = pr.get("labels", [])
    if not isinstance(raw, list):
        raise ValueError("live PR labels must be a list")
    labels: list[str] = []
    for entry in raw:
        if isinstance(entry, str):
            labels.append(entry)
        elif isinstance(entry, Mapping) and isinstance(entry.get("name"), str):
            labels.append(entry["name"])
        else:
            raise ValueError("live PR labels must contain names")
    return labels


def _field(value: Mapping[str, object], *names: str) -> object | None:
    for name in names:
        if name in value:
            return value[name]
    return None


def _prepared_version(pr: Mapping[str, object]) -> str:
    value = _field(pr, "prepared_version", "cargo_version", "version")
    head = pr.get("head")
    if value is None and isinstance(head, Mapping):
        value = _field(head, "prepared_version", "cargo_version", "version")
    if not isinstance(value, str) or not value:
        raise ValueError("live PR does not include its prepared Cargo version")
    return value


def _source_snapshot(pr: Mapping[str, object]) -> str:
    value = _field(pr, "source_snapshot", "meaningful_tree", "snapshot")
    head = pr.get("head")
    if value is None and isinstance(head, Mapping):
        value = _field(head, "source_snapshot", "meaningful_tree", "snapshot")
    if not isinstance(value, str) or _SNAPSHOT.fullmatch(value) is None:
        raise ValueError("live PR does not include a valid meaningful source snapshot")
    return value


def _record_candidates(release: Mapping[str, object]) -> list[Mapping[str, object]]:
    """Extract candidate-shaped objects from common GitHub release payloads."""

    values: list[Mapping[str, object]] = []
    for key in ("candidate", "record", "metadata"):
        value = release.get(key)
        if isinstance(value, Mapping):
            values.append(value)
    assets = release.get("assets")
    if isinstance(assets, list):
        for asset in assets:
            if not isinstance(asset, Mapping):
                continue
            for key in ("candidate", "record", "metadata"):
                value = asset.get(key)
                if isinstance(value, Mapping):
                    values.append(value)
                elif isinstance(value, str):
                    try:
                        parsed = json.loads(value)
                    except json.JSONDecodeError:
                        parsed = None
                    if isinstance(parsed, Mapping):
                        values.append(parsed)
            for key in ("content", "data"):
                value = asset.get(key)
                if isinstance(value, str):
                    try:
                        parsed = json.loads(value)
                    except json.JSONDecodeError:
                        parsed = None
                    if isinstance(parsed, Mapping):
                        values.append(parsed)
    for value in list(values):
        # A caller may pass ``{"record": {"record": {...}}}`` from an API
        # wrapper.  Follow one more level without accepting arbitrary data.
        nested = value.get("record")
        if isinstance(nested, Mapping):
            values.append(nested)

    body = release.get("body")
    if isinstance(body, str):
        try:
            parsed = json.loads(body)
        except json.JSONDecodeError:
            # Draft/release bodies often wrap the record in explanatory text.
            parsed = None
            start, end = body.find("{"), body.rfind("}")
            if start >= 0 and end > start:
                try:
                    parsed = json.loads(body[start : end + 1])
                except json.JSONDecodeError:
                    parsed = None
        if isinstance(parsed, Mapping):
            values.append(parsed)
    return values


def published_snapshots(releases: list[dict]) -> set[tuple[str, str]]:
    """Return phase/source pairs from already published preview releases."""

    if not isinstance(releases, list):
        raise ValueError("published releases must be a list")
    result: set[tuple[str, str]] = set()
    for index, release in enumerate(releases):
        if not isinstance(release, Mapping):
            raise ValueError(f"release entry {index} is not an object")
        if release.get("draft") is True or release.get("prerelease") is False:
            continue
        records = _record_candidates(release)
        # API wrappers often put these fields directly on the release fixture.
        if not records:
            records = [release]
        for record in records:
            phase = record.get("phase")
            snapshot = record.get("source_snapshot", record.get("meaningful_tree"))
            if snapshot is None:
                snapshot = record.get("snapshot")
            if not isinstance(phase, str) or phase not in _PHASE_LABELS:
                continue
            if not isinstance(snapshot, str) or _SNAPSHOT.fullmatch(snapshot) is None:
                continue
            result.add((phase, snapshot))
    return result


def resolve_candidate(
    pr: release_state.ReleasePr,
    live_pr: dict,
    tags: list[str],
    releases: list[dict],
) -> CandidateIdentity | None:
    """Resolve the current live PR into one immutable candidate identity.

    ``live_pr`` is validated again so stale event payload labels cannot affect
    the result.  The original ``pr`` supplies the trusted PR number and line;
    a moved release-line base is rejected because a prepared candidate tied to
    the old base cannot be reused.  A newer head is allowed and becomes the
    candidate's source SHA.
    """

    if not isinstance(pr, release_state.ReleasePr):
        raise ValueError("release PR identity must be a ReleasePr")
    live = release_state.validate_pr(live_pr, pr.repository)
    if live.number != pr.number:
        raise ValueError(f"live PR #{live.number} does not match requested PR #{pr.number}")
    for field in ("base_ref", "head_ref", "repository", "major"):
        if getattr(live, field) != getattr(pr, field):
            raise ValueError(
                f"live PR {field} {getattr(live, field)!r} does not match requested identity"
            )
    if live.base_sha != pr.base_sha:
        raise ValueError(
            f"release line base moved from {pr.base_sha} to {live.base_sha}; refresh the PR"
        )
    if not isinstance(tags, list) or not all(isinstance(tag, str) for tag in tags):
        raise ValueError("published tags must be a list of strings")

    version = _prepared_version(live_pr)
    snapshot = _source_snapshot(live_pr)
    phase = release_state.phase_from_labels(_labels(live_pr))
    stable = preview.stable_version(version, live.major)
    published = published_snapshots(releases)
    if phase is None:
        return CandidateIdentity(
            line=live.base_ref,
            pr_number=live.number,
            base_sha=live.base_sha,
            source_sha=live.head_sha,
            source_snapshot=snapshot,
            phase=None,
            version=stable,
            channel="stable",
            build_sha=live.head_sha,
        )

    selected = preview.next_preview_version(
        stable,
        phase,
        tags,
        published,
        snapshot,
    )
    if selected is None:
        return None
    return CandidateIdentity(
        line=live.base_ref,
        pr_number=live.number,
        base_sha=live.base_sha,
        source_sha=live.head_sha,
        source_snapshot=snapshot,
        phase=phase,
        version=selected,
        channel="testing",
        build_sha=live.head_sha,
    )


def _timeline_sort(timeline: list[dict]) -> list[dict]:
    if not timeline:
        return []
    parsed: list[tuple[datetime, int, dict]] = []
    for index, entry in enumerate(timeline):
        created = entry.get("created_at")
        if not isinstance(created, str):
            return timeline
        try:
            parsed.append((datetime.fromisoformat(created.replace("Z", "+00:00")), index, entry))
        except ValueError:
            return timeline
    parsed.sort(key=lambda item: (item[0], item[1]))
    return [entry for _, _, entry in parsed]


def _timeline_label(entry: Mapping[str, object]) -> str | None:
    label = entry.get("label")
    if isinstance(label, Mapping):
        label = label.get("name")
    return label if isinstance(label, str) else None


def _timeline_actor(entry: Mapping[str, object]) -> tuple[str | None, str | None]:
    actor = entry.get("actor", entry.get("user"))
    if not isinstance(actor, Mapping):
        return None, None
    login = actor.get("login")
    actor_type = actor.get("type")
    return (
        login if isinstance(login, str) else None,
        actor_type if isinstance(actor_type, str) else None,
    )


def _permission(value: object) -> str | None:
    if isinstance(value, Mapping):
        value = value.get("permission")
    return value.lower() if isinstance(value, str) else None


def phase_authorized(timeline: list[dict], phase: str, permissions: Mapping[str, object]) -> bool:
    """Check the latest active phase label was added by an authorized user.

    The event actor that triggered a controller run is intentionally absent
    from this API.  Authorization comes only from the most recent label
    transition in the PR timeline and a fresh repository permission lookup.
    """

    if phase not in _PHASE_LABELS:
        raise ValueError(f"unsupported preview phase {phase!r}")
    if not isinstance(timeline, list):
        raise ValueError("PR timeline must be a list")
    if not isinstance(permissions, Mapping):
        raise ValueError("phase permissions must be an object")
    label_name = _PHASE_LABELS[phase]
    latest_actor: tuple[str | None, str | None] | None = None
    for entry in _timeline_sort(timeline):
        if not isinstance(entry, Mapping):
            raise ValueError("PR timeline entries must be objects")
        if _timeline_label(entry) != label_name:
            continue
        event = entry.get("event", entry.get("type"))
        if event == "unlabeled":
            latest_actor = None
        elif event == "labeled":
            latest_actor = _timeline_actor(entry)
    if latest_actor is None:
        return False
    login, actor_type = latest_actor
    if not login or actor_type and actor_type.lower() == "bot" or login.endswith("[bot]"):
        return False
    return _permission(permissions.get(login)) in _AUTHORIZED_PERMISSIONS


# Keep the verb used by workflow callers discoverable without making a second
# implementation that could drift from ``phase_authorized``.
authorize_phase = phase_authorized


def _gh_json(*args: str) -> object:
    """Run ``gh`` and decode its JSON output.

    Publication is deliberately kept at the GitHub API edge.  Keeping this
    small wrapper in the module also gives the publication tests a single
    mutation boundary to replace, while the workflow uses the exact same
    code path against GitHub.
    """

    try:
        completed = subprocess.run(
            ["gh", *args],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = (
            error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) else str(error)
        )
        raise ValueError(f"gh {' '.join(args)} failed: {detail}") from error
    output = completed.stdout.strip()
    if not output:
        return None
    decoder = json.JSONDecoder()
    values: list[object] = []
    position = 0
    try:
        while position < len(output):
            while position < len(output) and output[position].isspace():
                position += 1
            if position >= len(output):
                break
            value, position = decoder.raw_decode(output, position)
            values.append(value)
    except json.JSONDecodeError as error:
        raise ValueError(f"gh {' '.join(args)} returned invalid JSON: {error}") from error
    return values[0] if len(values) == 1 else values


def _gh_mutate(
    method: str,
    path: str,
    fields: Mapping[str, object] | None = None,
) -> object:
    """Apply one explicit GitHub API mutation.

    The publication functions call this only for creating an immutable tag or
    changing an existing draft's publication state.  In particular, there is
    no upload, delete, replacement, or force-update operation here.
    """

    if not isinstance(method, str) or not method:
        raise ValueError(f"invalid GitHub API method {method!r}")
    if not isinstance(path, str) or not path.startswith("/"):
        raise ValueError(f"invalid GitHub API path {path!r}")
    argv = ["api", "-X", method, path]
    for name, value in (fields or {}).items():
        if not isinstance(name, str) or not name:
            raise ValueError(f"invalid GitHub API field name {name!r}")
        if isinstance(value, bool):
            argv.extend(["-F", f"{name}={'true' if value else 'false'}"])
        elif isinstance(value, int):
            argv.extend(["-F", f"{name}={value}"])
        elif isinstance(value, str):
            argv.extend(["-f", f"{name}={value}"])
        else:
            raise ValueError(f"GitHub API field {name!r} has unsupported value {value!r}")
    return _gh_json(*argv)


def _record_identity(
    record: CandidateRecord | Mapping[str, object],
) -> tuple[CandidateRecord, dict[str, object]]:
    """Return a validated record and its wire-compatible candidate identity."""

    try:
        validated = candidate.validate_record(record)
    except (TypeError, ValueError) as error:
        raise ValueError(f"candidate record is invalid: {error}") from error
    identity = {
        "line": validated.line,
        "pr_number": validated.pr_number,
        "base_sha": validated.base_sha,
        "source_sha": validated.source_sha,
        "build_sha": validated.build_sha,
        "source_snapshot": validated.source_snapshot,
        "phase": validated.phase,
        "version": validated.version,
        "channel": "testing" if validated.phase is not None else "stable",
    }
    return validated, identity


def _assert_record_identity(
    record: CandidateRecord | Mapping[str, object],
    expected: CandidateIdentity,
) -> CandidateRecord:
    validated, actual = _record_identity(record)
    if actual != expected.as_dict():
        differences = [
            f"{name}={actual[name]!r} (expected {wanted!r})"
            for name, wanted in expected.as_dict().items()
            if actual.get(name) != wanted
        ]
        raise ValueError(
            "candidate record identity does not match selected identity: " + ", ".join(differences)
        )
    return validated


def _release_asset_bytes(release: Mapping[str, object], name: str) -> bytes | None:
    """Read fixture/API-wrapper bytes when a release payload carries them."""

    containers = ("asset_bytes", "assets_bytes", "bytes")
    for container_name in containers:
        container = release.get(container_name)
        if isinstance(container, Mapping) and name in container:
            value = container[name]
            if isinstance(value, bytes):
                return value
            if isinstance(value, bytearray):
                return bytes(value)
            if isinstance(value, str):
                return value.encode()
            if isinstance(value, Path):
                try:
                    return value.read_bytes()
                except OSError as error:
                    raise ValueError(f"could not read release asset {name}: {error}") from error
            raise ValueError(f"release asset {name} has unsupported inline bytes")
    listed = release.get("assets")
    if isinstance(listed, Mapping) and name in listed:
        value = listed[name]
        if isinstance(value, bytes):
            return value
        if isinstance(value, bytearray):
            return bytes(value)
        if isinstance(value, str):
            return value.encode()
        if isinstance(value, Path):
            try:
                return value.read_bytes()
            except OSError as error:
                raise ValueError(f"could not read release asset {name}: {error}") from error
        raise ValueError(f"release asset {name} has unsupported inline bytes")
    if isinstance(listed, list):
        for item in listed:
            if not isinstance(item, Mapping) or item.get("name") != name:
                continue
            for key in ("bytes", "content", "data"):
                value = item.get(key)
                if isinstance(value, bytes):
                    return value
                if isinstance(value, bytearray):
                    return bytes(value)
                if isinstance(value, str):
                    return value.encode()
            path = item.get("path")
            if isinstance(path, (str, Path)):
                try:
                    return Path(path).read_bytes()
                except OSError as error:
                    raise ValueError(f"could not read release asset {name}: {error}") from error
    return None


def _assert_release_shape(
    release: Mapping[str, object],
    expected: CandidateIdentity,
    record: CandidateRecord,
    *,
    allow_published: bool,
) -> bool:
    """Validate release identity, state, and any API-listed asset metadata."""

    if not isinstance(release, Mapping):
        raise ValueError("GitHub release is not an object")
    release_id = release.get("id")
    if isinstance(release_id, bool) or not isinstance(release_id, int):
        raise ValueError("GitHub release has no valid release id")
    if release_id != record.draft_release_id:
        raise ValueError(
            f"release id {release_id} does not match candidate draft {record.draft_release_id}"
        )
    if release.get("tag_name") != expected.tag or release.get("name") != expected.tag:
        raise ValueError(f"release tag/name does not match candidate {expected.tag}")
    draft = release.get("draft")
    if not isinstance(draft, bool):
        raise ValueError(f"release {expected.tag} has no valid draft state")
    if not draft and not allow_published:
        raise ValueError(f"release {expected.tag} is already published")
    expected_prerelease = expected.phase is not None
    if release.get("prerelease") is not expected_prerelease:
        raise ValueError(
            f"release {expected.tag} prerelease state {release.get('prerelease')!r} "
            f"does not match candidate phase {expected.phase!r}"
        )
    payload = _body_identity(release)
    if payload is None:
        # Some GitHub API adapters expose the persisted record under
        # ``candidate``/``record`` instead of serializing the identity into
        # the release body.  Normalize those forms at this boundary while
        # keeping the exact CandidateIdentity comparison below.
        for key in ("identity", "candidate", "record", "metadata"):
            value = release.get(key)
            if not isinstance(value, Mapping):
                continue
            nested = value.get("candidate_identity")
            payload = nested if isinstance(nested, Mapping) else value
            if "schema_version" in payload:
                try:
                    _, payload = _record_identity(payload)
                except ValueError as error:
                    raise ValueError(
                        f"release {expected.tag} candidate record is invalid"
                    ) from error
            break
    if payload is None:
        raise ValueError(f"release {expected.tag} has no candidate identity")
    if "schema_version" in payload:
        try:
            _, payload = _record_identity(payload)
        except ValueError as error:
            raise ValueError(f"release {expected.tag} candidate record is invalid") from error
    try:
        release_identity = CandidateIdentity.from_dict(payload)
    except (TypeError, ValueError) as error:
        raise ValueError(f"release {expected.tag} has an invalid candidate identity") from error
    if release_identity.as_dict() != expected.as_dict():
        raise ValueError(
            f"release {expected.tag} candidate identity does not match selected identity"
        )

    listed = release.get("assets")
    if listed is not None:
        if not isinstance(listed, list):
            raise ValueError(f"release {expected.tag} assets are not a list")
        normalized: list[dict] = []
        seen_names: set[str] = set()
        seen_ids: set[int] = set()
        for index, item in enumerate(listed):
            if not isinstance(item, Mapping):
                raise ValueError(f"release {expected.tag} asset {index} is not an object")
            name = item.get("name")
            asset_id = item.get("id")
            size = item.get("size")
            if not isinstance(name, str) or not name:
                raise ValueError(f"release {expected.tag} asset {index} has an invalid name")
            if isinstance(asset_id, bool) or not isinstance(asset_id, int) or asset_id <= 0:
                raise ValueError(f"release {expected.tag} asset {name} has an invalid id")
            if isinstance(size, bool) or not isinstance(size, int) or size <= 0:
                raise ValueError(f"release {expected.tag} asset {name} has an invalid size")
            if name in seen_names or asset_id in seen_ids:
                raise ValueError(f"release {expected.tag} has duplicate asset metadata")
            seen_names.add(name)
            seen_ids.add(asset_id)
            normalized.append({"id": asset_id, "name": name, "size": size})
        try:
            verify_draft.check_inventory(normalized, expected.version, phase=expected.phase)
        except verify_draft.DraftVerificationError as error:
            raise ValueError(f"release {expected.tag} inventory mismatch: {error}") from error
        by_name = {item["name"]: item for item in normalized}
        for entry in record.assets:
            remote = by_name.get(entry.name)
            if remote is None:
                raise ValueError(f"release {expected.tag} is missing recorded asset {entry.name}")
            if remote["id"] != entry.id or remote["size"] != entry.size:
                raise ValueError(f"release {expected.tag} asset {entry.name} metadata changed")
    return draft


def _assert_inline_record_bytes(
    release: Mapping[str, object],
    record: CandidateRecord,
    *,
    required: bool = False,
) -> bool:
    """Check a record asset carried inline by tests or an API adapter."""

    name = candidate.record_asset_name(record)
    actual = _release_asset_bytes(release, name)
    if actual is None:
        if required:
            raise ValueError(f"release {record.tag} has no {name} bytes to verify")
        return False
    try:
        candidate.verify_record_bytes(record, actual)
    except candidate.CandidateMismatch as error:
        raise ValueError(f"release {record.tag} candidate bytes changed: {error}") from error
    return True


def _assert_inline_distribution_bytes(
    release: Mapping[str, object],
    record: CandidateRecord,
) -> None:
    """Check distribution bytes when a fixture or API adapter includes them."""

    for entry in record.assets:
        actual = _release_asset_bytes(release, entry.name)
        if actual is None:
            continue
        if len(actual) != entry.size:
            raise ValueError(f"release {record.tag} asset {entry.name} size changed")
        if hashlib.sha256(actual).hexdigest() != entry.sha256:
            raise ValueError(f"release {record.tag} asset {entry.name} SHA-256 changed")
        if hashlib.blake2b(actual, digest_size=64).hexdigest() != entry.blake2b512:
            raise ValueError(f"release {record.tag} asset {entry.name} BLAKE2b changed")


def _verify_draft_before_publication(
    record: CandidateRecord,
    expected: CandidateIdentity,
    *,
    expected_tag_target: str | None = None,
) -> None:
    """Read every draft asset through the authenticated API before publishing."""

    with tempfile.TemporaryDirectory(prefix="release-publication-verify-") as directory:
        try:
            verify_draft.verify(
                record,
                Path(directory),
                assets.REPOSITORY,
                True,
                expected_identity=expected.as_dict(),
                expected_tag_target=expected_tag_target,
            )
        except verify_draft.DraftVerificationError as error:
            raise ValueError(f"draft verification failed: {error}") from error


def _verify_published_asset_bytes(
    record: CandidateRecord,
    release: Mapping[str, object],
) -> None:
    """Verify bytes available from an already published release retry."""

    # A published release can be checked via inline fixture bytes, or by its
    # authenticated asset API when a full API payload is supplied.  Metadata
    # is still checked by ``_assert_release_shape`` for minimal callers.
    inline_names: set[str] = set()
    for entry in record.assets:
        actual = _release_asset_bytes(release, entry.name)
        if actual is None:
            continue
        inline_names.add(entry.name)
        if len(actual) != entry.size:
            raise ValueError(f"published asset {entry.name} size changed")
        if hashlib.sha256(actual).hexdigest() != entry.sha256:
            raise ValueError(f"published asset {entry.name} SHA-256 changed")
        if hashlib.blake2b(actual, digest_size=64).hexdigest() != entry.blake2b512:
            raise ValueError(f"published asset {entry.name} BLAKE2b changed")
    if _assert_inline_record_bytes(release, record, required=False):
        inline_names.add(candidate.record_asset_name(record))

    # A real GitHub release payload lists asset ids but does not include bytes.
    # Download those bytes through the authenticated API before treating an
    # already published retry as successful.  Minimal test/API fixtures can
    # omit ``assets`` and supply only inline bytes or digest metadata.
    listed = release.get("assets")
    if not isinstance(listed, list) or not listed:
        return
    by_name: dict[str, Mapping[str, object]] = {
        item["name"]: item
        for item in listed
        if isinstance(item, Mapping) and isinstance(item.get("name"), str)
    }
    names = [entry.name for entry in record.assets]
    if record.phase is not None:
        names.append(candidate.PREVIEW_RECORD_NAME)
    with tempfile.TemporaryDirectory(prefix="release-published-verify-") as directory:
        for name in names:
            if name in inline_names:
                continue
            item = by_name.get(name)
            asset_id = item.get("id") if item is not None else None
            if isinstance(asset_id, bool) or not isinstance(asset_id, int) or asset_id <= 0:
                raise ValueError(f"published release is missing asset id for {name}")
            path = Path(directory) / name.replace("/", "_")
            try:
                verify_draft.download_asset(asset_id, path, assets.REPOSITORY)
            except (OSError, subprocess.CalledProcessError) as error:
                raise ValueError(f"could not download published asset {name}: {error}") from error
            actual = path.read_bytes()
            if name == candidate.PREVIEW_RECORD_NAME:
                try:
                    candidate.verify_record_bytes(record, actual)
                except candidate.CandidateMismatch as error:
                    raise ValueError(f"published candidate bytes changed: {error}") from error
                continue
            expected = next(entry for entry in record.assets if entry.name == name)
            if len(actual) != expected.size:
                raise ValueError(f"published asset {name} size changed")
            if hashlib.sha256(actual).hexdigest() != expected.sha256:
                raise ValueError(f"published asset {name} SHA-256 changed")
            if hashlib.blake2b(actual, digest_size=64).hexdigest() != expected.blake2b512:
                raise ValueError(f"published asset {name} BLAKE2b changed")


def _tag_target(
    tag: str,
    _release: Mapping[str, object],
) -> str | None:
    """Resolve the actual GitHub ref target, including annotated tags."""

    try:
        return verify_draft.resolve_tag_commit(tag, assets.REPOSITORY)
    except verify_draft.DraftVerificationError as error:
        raise ValueError(f"could not verify tag {tag}: {error}") from error


def _ensure_tag(tag: str, target: str, release: Mapping[str, object]) -> None:
    existing = _tag_target(tag, release)
    if existing is not None:
        if existing.lower() != target.lower():
            raise ValueError(f"tag {tag} points at {existing}, expected immutable target {target}")
        return
    _gh_mutate(
        "POST",
        f"/repos/{assets.REPOSITORY}/git/refs",
        {"ref": f"refs/tags/{tag}", "sha": target},
    )


def _publish_release(
    record: CandidateRecord,
    *,
    prerelease: bool,
    make_latest: bool,
) -> None:
    _gh_mutate(
        "PATCH",
        f"/repos/{assets.REPOSITORY}/releases/{record.draft_release_id}",
        {
            "tag_name": record.tag,
            "draft": False,
            "prerelease": prerelease,
            "make_latest": make_latest,
        },
    )


def _preview_authorized(live_pr: Mapping[str, object], phase: str) -> None:
    """Require fresh phase authority supplied by the workflow/API adapter."""

    for key in ("phase_authorized", "authorized"):
        if key in live_pr:
            if live_pr[key] is not True:
                raise ValueError(f"preview phase {phase} is not authorized by a maintainer")
            return
    timeline = live_pr.get("timeline", live_pr.get("phase_timeline"))
    permissions = live_pr.get("permissions", live_pr.get("phase_permissions"))
    if isinstance(timeline, list) and isinstance(permissions, Mapping):
        if not phase_authorized(timeline, phase, permissions):
            raise ValueError(f"preview phase {phase} is not authorized by a maintainer")
        return
    raise ValueError(f"preview phase {phase} has no fresh authorization proof")


def publish_preview(
    identity: CandidateIdentity | Mapping[str, object],
    record: CandidateRecord | Mapping[str, object],
    live_pr: Mapping[str, object],
    release: Mapping[str, object],
) -> None:
    """Publish an already verified preview draft after fresh identity checks.

    The build workflow owns all distribution bytes.  This function only
    checks those bytes and the live PR/release state, creates the immutable
    version tag at ``build_sha`` when needed, and flips the existing draft to
    a prerelease.  A matching published release is an idempotent success.
    """

    expected = _coerce_identity(identity)
    if expected.phase is None:
        raise ValueError("preview publication requires an active phase")
    if expected.build_sha == expected.source_sha:
        raise ValueError("preview publication requires a derived build SHA")
    validated = _assert_record_identity(record, expected)
    if not isinstance(live_pr, Mapping):
        raise ValueError("live release PR is not an object")
    if not isinstance(release, Mapping):
        raise ValueError("GitHub release is not an object")
    try:
        assert_live_identity(expected, dict(live_pr))
    except ValueError as error:
        raise ValueError(f"preview live identity rejected: {error}") from error
    _preview_authorized(live_pr, expected.phase)
    draft = _assert_release_shape(release, expected, validated, allow_published=True)
    _assert_inline_record_bytes(release, validated, required=False)
    _assert_inline_distribution_bytes(release, validated)

    target = _tag_target(expected.tag, release)
    if target is not None and target.lower() != expected.build_sha.lower():
        raise ValueError(
            f"preview tag {expected.tag} points at {target}, expected build SHA "
            f"{expected.build_sha}"
        )
    if not draft:
        _verify_published_asset_bytes(validated, release)
        print(
            f"release publication line={expected.line} pr={expected.pr_number} "
            f"phase={expected.phase} "
            f"source_sha={expected.source_sha} version={expected.version} "
            f"draft_release_id={validated.draft_release_id} already published"
        )
        return

    # Draft API readback is the final byte/draft gate before either mutation.
    _verify_draft_before_publication(validated, expected)
    _ensure_tag(expected.tag, expected.build_sha, release)
    _publish_release(validated, prerelease=True, make_latest=False)
    print(
        f"release publication line={expected.line} pr={expected.pr_number} phase={expected.phase} "
        f"source_sha={expected.source_sha} version={expected.version} "
        f"draft_release_id={validated.draft_release_id} published"
    )


def _merged_pr_from_release(
    record: CandidateRecord,
    release: Mapping[str, object],
) -> Mapping[str, object] | None:
    for key in ("merged_pr", "pull_request", "merge_pr", "pr"):
        if key not in release:
            continue
        value = release[key]
        if value is None:
            return None
        if not isinstance(value, Mapping):
            raise ValueError(f"release {record.tag} has an invalid merged PR payload")
        return value
    try:
        payload = _gh_json(
            "api",
            f"/repos/{assets.REPOSITORY}/pulls/{record.pr_number}",
        )
    except ValueError as error:
        raise ValueError(f"could not fetch merged PR #{record.pr_number}: {error}") from error
    if payload is None:
        return None
    if not isinstance(payload, Mapping):
        raise ValueError(f"merged PR #{record.pr_number} response is not an object")
    return payload


def _matching_merge_pr(
    record: CandidateRecord,
    line_push_sha: str,
    merged_pr: Mapping[str, object] | None,
) -> bool:
    """Check whether this line push is the candidate PR merge.

    A normal backport push returns ``False`` and is a successful no-op.  Once
    the PR number and merge commit identify this candidate, malformed branch,
    repository, or head identity is a hard rejection.
    """

    if merged_pr is None:
        return False
    number = merged_pr.get("number")
    merge_commit = merged_pr.get("merge_commit_sha", merged_pr.get("merge_sha"))
    if isinstance(number, bool) or number != record.pr_number or merge_commit != line_push_sha:
        return False
    base = merged_pr.get("base")
    head = merged_pr.get("head")
    if not isinstance(base, Mapping) or not isinstance(head, Mapping):
        raise ValueError(f"merged PR #{record.pr_number} has incomplete branch identity")
    if base.get("ref") != record.line:
        raise ValueError(f"merged PR #{record.pr_number} base does not match {record.line}")
    expected_head = release_state.expected_head(release_state.parse_line(record.line))
    if head.get("ref") != expected_head:
        raise ValueError(f"merged PR #{record.pr_number} head does not match {expected_head}")
    for side, value in (("base", base), ("head", head)):
        repo = value.get("repo")
        full_name = repo.get("full_name") if isinstance(repo, Mapping) else None
        if full_name != assets.REPOSITORY:
            raise ValueError(f"merged PR #{record.pr_number} has a forked {side} repository")
    head_sha = head.get("sha")
    if not isinstance(head_sha, str) or _GIT_SHA.fullmatch(head_sha) is None:
        raise ValueError(f"merged PR #{record.pr_number} has an invalid head SHA")
    if head_sha != record.source_sha:
        # The stable stage commits packaging/release-candidate.json as one
        # record-only successor of the tested source.  GitHub reports that
        # successor as the merged PR head, while the record intentionally
        # retains the original source SHA.  Verify that exact successor when
        # the local checkout contains it.
        try:
            source_equivalence.assert_record_successor(
                record.source_sha,
                head_sha,
                candidate.dump(record),
                Path("."),
            )
        except source_equivalence.SourceDrift as error:
            raise ValueError(
                f"merged PR #{record.pr_number} head {head_sha} does not match candidate "
                f"source {record.source_sha}: {error}"
            ) from error
    merged = merged_pr.get("merged")
    if merged is False or merged_pr.get("state") not in (None, "closed", "merged"):
        return False
    if merged is not True and not merged_pr.get("merge_commit_sha"):
        return False
    return True


def _assert_line_push_base(record: CandidateRecord, line_push_sha: str) -> None:
    """Bind the candidate base to the release-line push's first parent."""

    try:
        parents = _run_git(Path("."), "rev-list", "--parents", "-n", "1", line_push_sha).split()
    except ValueError as error:
        raise ValueError(
            f"line push {line_push_sha} has no inspectable merge topology: {error}"
        ) from error
    if len(parents) < 2:
        raise ValueError(f"line push {line_push_sha} has no first parent")
    if parents[1] != record.base_sha:
        raise ValueError(
            f"line push {line_push_sha} first parent {parents[1]} does not match "
            f"candidate base {record.base_sha}"
        )


def _assert_record_introduced(
    record: CandidateRecord,
    line_push_sha: str,
    release: Mapping[str, object],
) -> None:
    """Require the exact record to be newly introduced by the line push."""

    if "record_introduced" in release:
        if release["record_introduced"] is not True:
            raise ValueError(f"candidate record was not introduced by push {line_push_sha}")
        return
    expected_record = candidate.dump(record)
    try:
        source_equivalence.assert_record_present(line_push_sha, expected_record, Path("."))
        parents = _run_git(Path("."), "rev-list", "--parents", "-n", "1", line_push_sha).split()
        if len(parents) < 2:
            raise ValueError(f"line push {line_push_sha} has no parent")
        changed = _run_git(
            Path("."),
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            line_push_sha,
            parents[1],
        ).splitlines()
    except (ValueError, source_equivalence.SourceDrift) as error:
        raise ValueError(
            f"candidate record was not newly introduced by push {line_push_sha}: {error}"
        ) from error
    if source_equivalence.RECORD_PATH not in changed:
        raise ValueError(
            f"candidate record was not introduced by push {line_push_sha}; changed={changed}"
        )


def _assert_merge_source(
    record: CandidateRecord,
    line_push_sha: str,
    release: Mapping[str, object],
) -> None:
    if "source_equivalent" in release:
        if release["source_equivalent"] is not True:
            raise ValueError(f"line push {line_push_sha} changed source bytes")
        return
    try:
        source_equivalence.assert_merge_equivalent(record.source_sha, line_push_sha, Path("."))
    except source_equivalence.SourceDrift as error:
        raise ValueError(f"line push {line_push_sha} changed source bytes: {error}") from error


def _published_stable_versions(release: Mapping[str, object]) -> list[str]:
    for key in ("published_stable_versions", "stable_versions"):
        if key in release:
            value = release[key]
            if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
                raise ValueError(
                    f"release {release.get('tag_name')} stable version inventory is invalid"
                )
            return list(value)
    for key in ("all_releases", "releases"):
        value = release.get(key)
        if value is not None:
            return _stable_versions_from_payload(value)
    try:
        value = _gh_json("api", "--paginate", f"/repos/{assets.REPOSITORY}/releases")
    except ValueError as error:
        raise ValueError(
            f"could not list published releases for latest selection: {error}"
        ) from error
    return _stable_versions_from_payload(value)


def _stable_versions_from_payload(value: object) -> list[str]:
    if isinstance(value, Mapping):
        value = value.get("releases", value.get("items"))
    if not isinstance(value, list):
        raise ValueError("published release inventory must be a list")
    entries: list[object] = []
    for page in value:
        if isinstance(page, list):
            entries.extend(page)
        else:
            entries.append(page)
    versions: list[str] = []
    for item in entries:
        if not isinstance(item, Mapping):
            continue
        if item.get("draft") is True or item.get("prerelease") is True:
            continue
        tag = item.get("tag_name", item.get("version"))
        if isinstance(tag, str):
            versions.append(tag[1:] if tag.startswith("v") else tag)
    return versions


def _semver_tuple(value: str) -> tuple[int, int, int] | None:
    normalized = value[1:] if isinstance(value, str) and value.startswith("v") else value
    match = _STABLE_VERSION.fullmatch(normalized) if isinstance(normalized, str) else None
    if match is None:
        return None
    major, minor, patch = (int(part) for part in normalized.split("."))
    return major, minor, patch


def should_make_latest(version: str, published_stable_versions: list[str]) -> bool:
    """Return whether ``version`` is the numeric maximum stable SemVer."""

    selected = _semver_tuple(version)
    if selected is None:
        raise ValueError(f"latest selection requires a plain stable SemVer, got {version!r}")
    if not isinstance(published_stable_versions, list):
        raise ValueError("published stable versions must be a list")
    maximum = selected
    for value in published_stable_versions:
        if not isinstance(value, str):
            raise ValueError("published stable versions must contain strings")
        parsed = _semver_tuple(value)
        if parsed is not None and parsed > maximum:
            maximum = parsed
    return selected == maximum


def publish_stable(
    record: CandidateRecord | Mapping[str, object],
    line_push_sha: str,
    release: Mapping[str, object],
) -> None:
    """Publish the reviewed stable draft for the merge that introduced it."""

    validated, identity = _record_identity(record)
    if validated.phase is not None:
        raise ValueError(f"stable publication cannot use preview phase {validated.phase!r}")
    expected = CandidateIdentity.from_dict(identity)
    if not isinstance(line_push_sha, str) or _GIT_SHA.fullmatch(line_push_sha) is None:
        raise ValueError(f"invalid release-line push SHA {line_push_sha!r}")
    if not isinstance(release, Mapping):
        raise ValueError("GitHub release is not an object")

    merged_pr = _merged_pr_from_release(validated, release)
    if not _matching_merge_pr(validated, line_push_sha, merged_pr):
        print(
            f"release publication line={validated.line} pr={validated.pr_number} phase=stable "
            f"source_sha={validated.source_sha} version={validated.version} "
            f"draft_release_id={validated.draft_release_id} "
            "rejection=no merge-matching candidate record"
        )
        return
    _assert_line_push_base(validated, line_push_sha)
    _assert_record_introduced(validated, line_push_sha, release)
    _assert_merge_source(validated, line_push_sha, release)
    draft = _assert_release_shape(release, expected, validated, allow_published=True)
    _assert_inline_distribution_bytes(release, validated)
    target = _tag_target(validated.tag, release)
    if target is not None and target.lower() != line_push_sha.lower():
        raise ValueError(
            f"stable tag {validated.tag} points at {target}, expected merge {line_push_sha}"
        )

    if not draft:
        _verify_published_asset_bytes(validated, release)
        print(
            f"release publication line={validated.line} pr={validated.pr_number} phase=stable "
            f"source_sha={validated.source_sha} version={validated.version} "
            f"draft_release_id={validated.draft_release_id} already published"
        )
        return

    _verify_draft_before_publication(
        validated,
        expected,
        expected_tag_target=line_push_sha,
    )
    _ensure_tag(validated.tag, line_push_sha, release)
    make_latest = should_make_latest(validated.version, _published_stable_versions(release))
    _publish_release(validated, prerelease=False, make_latest=make_latest)
    print(
        f"release publication line={validated.line} pr={validated.pr_number} phase=stable "
        f"source_sha={validated.source_sha} version={validated.version} "
        f"draft_release_id={validated.draft_release_id} published make_latest={make_latest}"
    )


def _run_git(
    repo: Path, *args: str, env: Mapping[str, str] | None = None, input: bytes | None = None
) -> str:
    try:
        completed = subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            input=input,
            env=dict(env) if env is not None else None,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = (
            error.stderr.decode(errors="replace").strip()
            if isinstance(error, subprocess.CalledProcessError)
            else str(error)
        )
        raise ValueError(f"git {' '.join(args)} failed in {repo}: {detail}") from error
    return completed.stdout.decode().strip()


def _replace_package_version(cargo: str, version: str) -> str:
    try:
        document = tomllib.loads(cargo)
    except Exception as error:  # pragma: no cover - error text is asserted by caller
        raise ValueError(f"Cargo.toml is invalid: {error}") from error
    package = document.get("package")
    if not isinstance(package, Mapping):
        raise ValueError("Cargo.toml has no package table")
    package_name = package.get("name")
    if package_name != "gel-cli":
        raise ValueError(f"Cargo.toml package is {package_name!r}, expected 'gel-cli'")
    start = re.search(r"(?m)^\[package\]\s*$", cargo)
    if start is None:
        raise ValueError("Cargo.toml has no [package] section")
    next_section = re.search(r"(?m)^\[(?!\[)[^\n]+\]\s*$", cargo[start.end() :])
    end = start.end() + next_section.start() if next_section else len(cargo)
    section = cargo[start.end() : end]
    version_match = re.search(r'(?m)^(version\s*=\s*)"[^"]+"(\s*)$', section)
    if version_match is None:
        # A workspace version is still a Cargo version field and is common in
        # small fixture workspaces.  Update it in the workspace package table.
        workspace = re.search(r"(?m)^\[workspace\.package\]\s*$", cargo)
        if workspace is None:
            raise ValueError("Cargo.toml package has no concrete version field")
        workspace_next = re.search(r"(?m)^\[(?!\[)[^\n]+\]\s*$", cargo[workspace.end() :])
        workspace_end = workspace.end() + workspace_next.start() if workspace_next else len(cargo)
        workspace_section = cargo[workspace.end() : workspace_end]
        version_match = re.search(r'(?m)^(version\s*=\s*)"[^"]+"(\s*)$', workspace_section)
        if version_match is None:
            raise ValueError("Cargo.toml workspace package has no concrete version field")
        replacement = (
            workspace_section[: version_match.start()]
            + f'{version_match.group(1)}"{version}"{version_match.group(2)}'
            + workspace_section[version_match.end() :]
        )
        return cargo[: workspace.end()] + replacement + cargo[workspace_end:]
    replacement = (
        section[: version_match.start()]
        + f'{version_match.group(1)}"{version}"{version_match.group(2)}'
        + section[version_match.end() :]
    )
    return cargo[: start.end()] + replacement + cargo[end:]


def _replace_lock_version(lock: str, version: str) -> str:
    try:
        document = tomllib.loads(lock)
    except Exception as error:  # pragma: no cover - error text is asserted by caller
        raise ValueError(f"Cargo.lock is invalid: {error}") from error
    packages = document.get("package")
    matches = [
        package
        for package in packages or []
        if isinstance(package, Mapping) and package.get("name") == "gel-cli"
    ]
    if len(matches) != 1:
        raise ValueError(f"Cargo.lock has {len(matches)} gel-cli package entries")
    blocks = list(re.finditer(r"(?ms)^\[\[package\]\]\s*\n.*?(?=^\[\[package\]\]|\Z)", lock))
    for block in blocks:
        text = block.group(0)
        if re.search(r'(?m)^name\s*=\s*"gel-cli"\s*$', text):
            updated, count = re.subn(
                r'(?m)^(version\s*=\s*)"[^"]+"(\s*)$',
                rf'\1"{version}"\2',
                text,
                count=1,
            )
            if count != 1:
                raise ValueError("Cargo.lock gel-cli entry has no version field")
            return lock[: block.start()] + updated + lock[block.end() :]
    raise ValueError("Cargo.lock gel-cli entry could not be located")


def _git_blob(repo: Path, source_sha: str, path: str) -> bytes:
    try:
        completed = subprocess.run(
            ["git", "-C", str(repo), "show", f"{source_sha}:{path}"],
            check=True,
            capture_output=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = (
            error.stderr.decode(errors="replace").strip()
            if isinstance(error, subprocess.CalledProcessError)
            else str(error)
        )
        raise ValueError(f"could not read {path} from {source_sha}: {detail}") from error
    return completed.stdout


def _metadata_version(worktree: Path, expected: str) -> str:
    try:
        completed = subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--no-deps",
                "--format-version",
                "1",
                "--manifest-path",
                str(worktree / "Cargo.toml"),
            ],
            check=True,
            capture_output=True,
            text=True,
            cwd=worktree,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = (
            error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) else str(error)
        )
        raise ValueError(f"cargo metadata failed for derived preview tree: {detail}") from error
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise ValueError(f"cargo metadata returned invalid JSON: {error}") from error
    packages = payload.get("packages")
    if not isinstance(packages, list):
        raise ValueError("cargo metadata returned no package list")
    versions = {
        package.get("version")
        for package in packages
        if isinstance(package, Mapping) and package.get("name") == "gel-cli"
    }
    if versions != {expected}:
        raise ValueError(f"cargo metadata resolved gel-cli versions {versions!r}")
    return next(iter(versions))


def derive_preview_commit(source_sha: str, version: str, repo: Path = Path(".")) -> str:
    """Create a deterministic preview commit parented to ``source_sha``.

    The current checkout and index are never changed.  The returned commit is
    reproducible for the same source and version, which lets retries reuse the
    exact temporary ref and build SHA.
    """

    if _GIT_SHA.fullmatch(source_sha) is None:
        raise ValueError(f"invalid source SHA {source_sha!r}")
    preview_match = _PREVIEW_VERSION.fullmatch(version)
    if preview_match is None:
        raise ValueError(f"unsupported preview version {version!r}")
    repo = Path(repo)
    if not repo.is_dir():
        raise ValueError(f"repository does not exist: {repo}")
    _run_git(repo, "rev-parse", "--verify", f"{source_sha}^{{commit}}")
    cargo = _git_blob(repo, source_sha, "Cargo.toml").decode()
    lock = _git_blob(repo, source_sha, "Cargo.lock").decode()
    updated_cargo = _replace_package_version(cargo, version)
    updated_lock = _replace_lock_version(lock, version)

    with tempfile.TemporaryDirectory(prefix="gel-preview-index-") as temp:
        index = Path(temp) / "index"
        env = os.environ.copy()
        env["GIT_INDEX_FILE"] = str(index)
        _run_git(repo, "read-tree", source_sha, env=env)
        for path, contents in (
            ("Cargo.toml", updated_cargo.encode()),
            ("Cargo.lock", updated_lock.encode()),
        ):
            blob = (
                subprocess.run(
                    ["git", "-C", str(repo), "hash-object", "-w", "--stdin"],
                    input=contents,
                    check=True,
                    capture_output=True,
                )
                .stdout.decode()
                .strip()
            )
            _run_git(repo, "update-index", "--add", "--cacheinfo", f"100644,{blob},{path}", env=env)
        tree = _run_git(repo, "write-tree", env=env)

        # Archive this synthetic tree into a temporary directory so Cargo sees
        # exactly the tree that will be committed, including the selected lock
        # version.  ``--locked`` proves Cargo.lock is consistent with Cargo's
        # resolver under the repository's rust-toolchain file.
        with tempfile.TemporaryDirectory(prefix="gel-preview-tree-") as checkout:
            archive = subprocess.run(
                ["git", "-C", str(repo), "archive", tree],
                check=True,
                capture_output=True,
            ).stdout
            with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as tar:
                try:
                    tar.extractall(checkout, filter="data")
                except TypeError:  # Python 3.11 has no extraction filter argument.
                    tar.extractall(checkout)
            if _metadata_version(Path(checkout), version) != version:
                raise ValueError(f"cargo metadata did not resolve selected version {version}")

        changed = _run_git(
            repo,
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            source_sha,
            tree,
        ).splitlines()
        if set(changed) != {"Cargo.toml", "Cargo.lock"}:
            raise ValueError(
                "derived preview tree changed paths outside Cargo.toml and Cargo.lock: "
                + ", ".join(changed)
            )

        commit_metadata = _run_git(
            repo,
            "show",
            "-s",
            "--format=%an%n%ae%n%aI%n%cn%n%ce%n%cI",
            source_sha,
        ).splitlines()
        if len(commit_metadata) != 6:
            raise ValueError("source commit has incomplete author metadata")
        commit_env = os.environ.copy()
        (
            commit_env["GIT_AUTHOR_NAME"],
            commit_env["GIT_AUTHOR_EMAIL"],
            commit_env["GIT_AUTHOR_DATE"],
            commit_env["GIT_COMMITTER_NAME"],
            commit_env["GIT_COMMITTER_EMAIL"],
            commit_env["GIT_COMMITTER_DATE"],
        ) = commit_metadata
        return _run_git(
            repo,
            "commit-tree",
            tree,
            "-p",
            source_sha,
            "-m",
            f"chore: derive preview {version}",
            env=commit_env,
        )
