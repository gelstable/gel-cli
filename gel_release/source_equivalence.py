"""Compare two Git trees outside the generated-metadata allowlist.

Observing that a commit only touched packaging paths is not sufficient: a
merge, a revert pair, or a rebase can move source bytes without a single
packaging-only diff. This module compares complete trees instead.
"""

from __future__ import annotations

import hashlib
import subprocess
from pathlib import Path

ALLOWLIST = frozenset(
    {
        "Formula/gel.rb",
        "bucket/gel.json",
        "packaging/aur/PKGBUILD",
        "packaging/release-candidate.json",
    }
)
RECORD_PATH = "packaging/release-candidate.json"


class SourceDrift(ValueError):
    """Two trees differ outside the allowlist."""


def tree_entries(rev: str, repo: Path = Path(".")) -> dict[str, str]:
    completed = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "ls-tree",
            "-r",
            "-z",
            "--format=%(objectmode) %(objectname) %(path)",
            rev,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    entries: dict[str, str] = {}
    for record in completed.stdout.split("\0"):
        if not record:
            continue
        mode, objectname, path = record.split(" ", 2)
        entries[path] = f"{mode} {objectname}"
    return entries


def meaningful_tree(rev: str, repo: Path = Path(".")) -> str:
    """Return a stable digest of the source-relevant tree at ``rev``.

    Git tree object IDs include every file, including generated release
    metadata.  Preview identity must ignore exactly those generated paths, so
    hash the remaining mode, path, and blob object ID entries in a canonical
    order instead.
    """

    entries = tree_entries(rev, repo)
    canonical: list[str] = []
    for path, value in entries.items():
        if path in ALLOWLIST:
            continue
        mode, objectname = value.split(" ", 1)
        canonical.append(f"{mode} {path} {objectname}\n")
    canonical.sort()
    return hashlib.sha256("".join(canonical).encode("utf-8")).hexdigest()


def assert_snapshot(expected: str, rev: str, repo: Path = Path(".")) -> None:
    """Require a revision's meaningful source snapshot to match ``expected``."""

    # Accept either natural positional ordering when called by integrations:
    # ``assert_snapshot(expected, rev)`` is the canonical form, while
    # ``assert_snapshot(rev, expected)`` remains unambiguous by digest length.
    if len(expected) == 40 and len(rev) == 64:
        expected, rev = rev, expected
    actual = meaningful_tree(rev, repo)
    if actual != expected:
        raise SourceDrift(
            f"{rev} meaningful source snapshot {actual} does not match expected {expected}"
        )


def compare(base_rev: str, head_rev: str, repo: Path = Path(".")) -> list[str]:
    base = tree_entries(base_rev, repo)
    head = tree_entries(head_rev, repo)
    differences: list[str] = []
    for path in sorted(set(base) | set(head)):
        if path in ALLOWLIST:
            continue
        in_base = base.get(path)
        in_head = head.get(path)
        if in_base == in_head:
            continue
        if in_base is None:
            differences.append(f"added outside allowlist: {path}")
        elif in_head is None:
            differences.append(f"removed outside allowlist: {path}")
        else:
            differences.append(f"changed outside allowlist: {path}")
    return differences


def assert_equivalent(base_rev: str, head_rev: str, repo: Path = Path(".")) -> None:
    differences = compare(base_rev, head_rev, repo)
    if differences:
        raise SourceDrift(
            f"{head_rev} is not source-equivalent to {base_rev}:\n"
            + "\n".join(f"  {line}" for line in differences)
        )


def assert_merge_equivalent(
    tested_source: str, merge_revision: str, repo: Path = Path(".")
) -> None:
    """Require a prospective merge tree to preserve the tested source tree.

    ``compare`` already excludes the four generated paths exactly. Keeping a
    named merge helper makes that invariant explicit at the stable gate call
    site and ensures callers do not accidentally compare only a commit diff.
    """

    assert_equivalent(tested_source, merge_revision, repo)


def _record_bytes(rev: str, repo: Path) -> bytes:
    try:
        completed = subprocess.run(
            ["git", "-C", str(repo), "show", f"{rev}:{RECORD_PATH}"],
            check=True,
            capture_output=True,
        )
    except subprocess.CalledProcessError as error:
        raise SourceDrift(
            f"{rev} does not contain {RECORD_PATH}: {error.stderr.decode(errors='replace')}"
        ) from error
    return completed.stdout


def assert_record_present(rev: str, expected_record: bytes, repo: Path = Path(".")) -> None:
    """Require a revision to contain exactly the staged candidate record."""

    actual = _record_bytes(rev, repo)
    if actual != expected_record:
        raise SourceDrift(f"{rev} contains a candidate record with unexpected bytes")


def assert_record_successor(
    tested_source: str,
    successor: str,
    expected_record: bytes,
    repo: Path = Path("."),
) -> None:
    """Require ``successor`` to be the single record-only commit after source.

    The release workflow records the candidate after testing ``tested_source``.
    This check binds the live PR head to that exact staging operation: it must
    have the tested source as its only parent, change only the candidate record,
    and contain the exact bytes that were verified.
    """

    try:
        completed = subprocess.run(
            ["git", "-C", str(repo), "rev-list", "--parents", "-n", "1", successor],
            check=True,
            capture_output=True,
            text=True,
        )
    except subprocess.CalledProcessError as error:
        raise SourceDrift(f"could not inspect candidate successor {successor}: {error}") from error
    parents = completed.stdout.split()
    if len(parents) != 2 or parents[1] != tested_source:
        actual = " ".join(parents[1:]) or "no parent"
        raise SourceDrift(
            f"{successor} is not the single record successor of {tested_source}; parents={actual}"
        )

    base = tree_entries(tested_source, repo)
    head = tree_entries(successor, repo)
    changed = [path for path in sorted(set(base) | set(head)) if base.get(path) != head.get(path)]
    if changed != [RECORD_PATH]:
        details = ", ".join(changed) if changed else "no paths"
        raise SourceDrift(
            f"{successor} changes {details}; expected only {RECORD_PATH} after {tested_source}"
        )
    assert_record_present(successor, expected_record, repo)
