"""GitHub release-line identity and preview commit mechanics.

The controller deliberately keeps GitHub API concerns at the edge.  It passes
the freshly fetched pull request and the published release inventory into
``resolve_candidate``; that function returns one immutable candidate identity
or ``None`` when the current phase and source snapshot have already been
published.  Preview builds are derived from the original pull-request head by
Git plumbing, leaving the prepared stable branch unchanged.
"""

from __future__ import annotations

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

from . import preview, release_state

_GIT_SHA = re.compile(r"^[0-9a-f]{40}$")
_SNAPSHOT = re.compile(r"^[0-9a-f]{64}$")
_PREVIEW_VERSION = re.compile(
    r"^(?P<base>(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))"
    r"-(?P<phase>alpha|beta|rc)\.(?P<number>[1-9][0-9]*)$"
)
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

    checks: tuple[tuple[str, object, object], ...] = (
        ("PR number", live.number, expected.pr_number),
        ("release line", live.base_ref, expected.line),
        ("base SHA", live.base_sha, expected.base_sha),
        ("source SHA", live.head_sha, expected.source_sha),
        ("prepared version", _prepared_version(live_pr), expected.version),
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
