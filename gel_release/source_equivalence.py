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
