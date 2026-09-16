"""Validation of release-line and generated pull-request identity."""

from __future__ import annotations

import re
from dataclasses import dataclass

_LINE_PATTERN = re.compile(r"^release/v([1-9][0-9]*)\.x$")
_HEAD_PATTERN = re.compile(r"^knope/release-v([1-9][0-9]*)\.x$")
_SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
_PHASE_LABELS = {
    "prerelease:alpha": "alpha",
    "prerelease:beta": "beta",
    "prerelease:rc": "rc",
}


@dataclass(frozen=True, slots=True)
class ReleasePr:
    """The immutable identity of a generated release pull request."""

    number: int
    base_ref: str
    base_sha: str
    head_ref: str
    head_sha: str
    repository: str
    major: int

    @property
    def pr_number(self) -> int:
        """The PR number under the name used by candidate records."""

        return self.number

    def as_dict(self) -> dict[str, int | str]:
        return {
            "number": self.number,
            "base_ref": self.base_ref,
            "base_sha": self.base_sha,
            "head_ref": self.head_ref,
            "head_sha": self.head_sha,
            "repository": self.repository,
            "major": self.major,
        }


def parse_line(base: str) -> int:
    """Return the major encoded by a release branch name."""

    if not isinstance(base, str):
        raise ValueError(f"invalid release line {base!r}")
    match = _LINE_PATTERN.fullmatch(base)
    if match is None:
        raise ValueError(f"invalid release line {base!r}")
    return int(match.group(1))


def expected_head(major: int) -> str:
    """Return the generated branch expected for a release-line major."""

    if isinstance(major, bool) or not isinstance(major, int) or major < 1:
        raise ValueError(f"invalid release major {major!r}")
    return f"knope/release-v{major}.x"


def phase_from_labels(labels: list[str]) -> str | None:
    """Resolve the one active prerelease phase from exact label names."""

    if not isinstance(labels, list):
        raise ValueError("phase labels must be a list of strings")

    active = []
    for label in labels:
        if not isinstance(label, str):
            raise ValueError("phase labels must be a list of strings")
        if label in _PHASE_LABELS and label not in active:
            active.append(label)

    if len(active) > 1:
        raise ValueError(f"multiple active phase labels: {', '.join(active)}")
    return _PHASE_LABELS[active[0]] if active else None


def _phase_labels(pr: dict, number: int) -> None:
    raw_labels = pr.get("labels", [])
    if not isinstance(raw_labels, list):
        raise ValueError(f"PR #{number} has invalid phase labels")

    labels = []
    for label in raw_labels:
        if isinstance(label, str):
            labels.append(label)
            continue
        if isinstance(label, dict) and isinstance(label.get("name"), str):
            labels.append(label["name"])
            continue
        raise ValueError(f"PR #{number} has invalid phase labels")

    try:
        phase_from_labels(labels)
    except ValueError as error:
        raise ValueError(f"PR #{number} has invalid phase labels: {error}") from None


def _nested_repo(value: object) -> str | None:
    if not isinstance(value, dict):
        return None
    full_name = value.get("full_name")
    return full_name if isinstance(full_name, str) else None


def _sha(value: object, label: str, number: int, base_ref: str, head_ref: str) -> str:
    if not isinstance(value, str) or _SHA_PATTERN.fullmatch(value) is None:
        raise ValueError(
            f"PR #{number} has invalid {label} SHA for {base_ref!r} -> {head_ref!r}: {value!r}"
        )
    return value


def validate_pr(pr: dict, repo: str) -> ReleasePr:
    """Validate a freshly fetched GitHub PR and return its release identity."""

    if not isinstance(pr, dict):
        raise ValueError("release PR identity must be a JSON object")
    if not isinstance(repo, str) or not repo:
        raise ValueError(f"invalid trusted repository {repo!r}")

    number_value = pr.get("number")
    if isinstance(number_value, bool) or not isinstance(number_value, int) or number_value < 1:
        raise ValueError(f"invalid release PR identity number {number_value!r}")
    number = number_value

    state = pr.get("state")
    if state != "open":
        raise ValueError(f"PR #{number} is not open (state {state!r})")
    _phase_labels(pr, number)

    base = pr.get("base")
    head = pr.get("head")
    if not isinstance(base, dict) or not isinstance(head, dict):
        raise ValueError(f"PR #{number} has incomplete release line identity")

    base_ref = base.get("ref")
    head_ref = head.get("ref")
    if not isinstance(base_ref, str):
        raise ValueError(f"PR #{number} has invalid base release line {base_ref!r}")
    if not isinstance(head_ref, str):
        raise ValueError(f"PR #{number} has invalid generated head {head_ref!r}")

    try:
        major = parse_line(base_ref)
    except ValueError as error:
        raise ValueError(f"PR #{number} has invalid release line {base_ref!r}: {error}") from None

    base_repository = _nested_repo(base.get("repo"))
    if base_repository != repo:
        raise ValueError(
            f"PR #{number} release line {base_ref!r} belongs to repository "
            f"{base_repository!r}, expected {repo!r}"
        )

    head_repository = _nested_repo(head.get("repo"))
    if head_repository != repo:
        raise ValueError(
            f"PR #{number} has a forked head repository {head_repository!r}; expected {repo!r}"
        )

    expected = expected_head(major)
    head_match = _HEAD_PATTERN.fullmatch(head_ref)
    if head_match is None or int(head_match.group(1)) != major or head_ref != expected:
        raise ValueError(
            f"PR #{number} release identity {base_ref!r} -> {head_ref!r} does not match "
            f"expected generated head {expected!r}"
        )

    base_sha = _sha(base.get("sha"), "base", number, base_ref, head_ref)
    head_sha = _sha(head.get("sha"), "head", number, base_ref, head_ref)
    return ReleasePr(
        number=number,
        base_ref=base_ref,
        base_sha=base_sha,
        head_ref=head_ref,
        head_sha=head_sha,
        repository=repo,
        major=major,
    )
