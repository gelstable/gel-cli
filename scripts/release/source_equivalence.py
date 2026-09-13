"""Compare two Git trees outside the generated-metadata allowlist.

Observing that a commit only touched packaging paths is not sufficient: a
merge, a revert pair, or a rebase can move source bytes without a single
packaging-only diff. This module compares complete trees instead.
"""

from __future__ import annotations

import argparse
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


class SourceDrift(Exception):
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="the staged source SHA")
    parser.add_argument("--head", required=True, help="the prospective or actual merge SHA")
    parser.add_argument("--repo", default=Path("."), type=Path)
    args = parser.parse_args()

    assert_equivalent(args.base, args.head, args.repo)
    print(f"{args.head} is source-equivalent to {args.base} outside the allowlist")


if __name__ == "__main__":
    main()
