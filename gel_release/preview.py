"""Version and source-snapshot decisions for preview releases."""

from __future__ import annotations

import re

_BASE_COMPONENT = r"(?:0|[1-9][0-9]*)"
_BASE_PATTERN = re.compile(
    rf"^(?P<major>{_BASE_COMPONENT})\.(?P<minor>{_BASE_COMPONENT})\.(?P<patch>{_BASE_COMPONENT})$"
)
_TAG_PATTERN = re.compile(
    rf"^v(?P<base>{_BASE_COMPONENT}\.{_BASE_COMPONENT}\.{_BASE_COMPONENT})"
    r"-(?P<phase>alpha|beta|rc)\.(?P<number>[1-9][0-9]*)$"
)
_PHASES = frozenset(("alpha", "beta", "rc"))


def _base_parts(base: str) -> tuple[int, int, int]:
    if not isinstance(base, str):
        raise ValueError(f"unsupported release base version: {base!r}")
    match = _BASE_PATTERN.fullmatch(base)
    if match is None:
        raise ValueError(f"unsupported release base version: {base!r}")
    return (
        int(match.group("major")),
        int(match.group("minor")),
        int(match.group("patch")),
    )


def stable_version(cargo_version: str, major: int) -> str:
    """Validate and return the plain stable version prepared by Cargo."""

    if isinstance(major, bool) or not isinstance(major, int) or major < 1:
        raise ValueError(f"invalid release major: {major!r}")
    parsed_major, _, _ = _base_parts(cargo_version)
    if parsed_major != major:
        raise ValueError(
            f"stable version {cargo_version!r} has major {parsed_major}, expected {major}"
        )
    return cargo_version


def _validate_phase(phase: str) -> None:
    if not isinstance(phase, str) or phase not in _PHASES:
        raise ValueError(f"unsupported preview phase: {phase!r}")


def _published_snapshot_exists(
    phase: str,
    current_snapshot: str,
    already_published: set[tuple[str, str]],
) -> bool:
    """Return whether the caller has already published this phase snapshot.

    The snapshot digest is selected by the controller from the current PR
    head.  Compare both fields so an older snapshot in the same phase does not
    suppress a new preview.
    """

    for entry in already_published:
        if not isinstance(entry, tuple) or len(entry) != 2:
            raise ValueError(f"invalid published snapshot entry: {entry!r}")
        entry_phase, snapshot = entry
        if not isinstance(entry_phase, str) or not isinstance(snapshot, str):
            raise ValueError(f"invalid published snapshot entry: {entry!r}")
        if entry_phase == phase and snapshot == current_snapshot:
            return True
    return False


def next_preview_version(
    base: str,
    phase: str,
    published_tags: list[str],
    already_published: set[tuple[str, str]],
    current_snapshot: str,
) -> str | None:
    """Select the next unpublished preview version for a base and phase.

    Only immutable published Git tags advance the suffix.  Draft names and
    unsupported tag suffixes are ignored so a failed draft leaves no gap.
    """

    _base_parts(base)
    _validate_phase(phase)
    if not isinstance(published_tags, list):
        raise ValueError("published tags must be a list of strings")
    if not isinstance(already_published, set):
        raise ValueError("already published snapshots must be a set")
    if not isinstance(current_snapshot, str) or not current_snapshot:
        raise ValueError("current snapshot must be a non-empty string")
    if _published_snapshot_exists(phase, current_snapshot, already_published):
        return None

    highest = 0
    for tag in published_tags:
        if not isinstance(tag, str):
            raise ValueError("published tags must be a list of strings")
        match = _TAG_PATTERN.fullmatch(tag)
        if match is None:
            # Git tags from other release mechanisms, draft labels, and
            # unsupported prerelease forms do not allocate preview suffixes.
            continue
        if match.group("base") != base or match.group("phase") != phase:
            continue
        highest = max(highest, int(match.group("number")))

    return f"{base}-{phase}.{highest + 1}"
