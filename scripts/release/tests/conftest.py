"""Shared helpers for the release-pipeline tests.

The suite is written with :class:`unittest.TestCase`, which cannot receive
pytest fixture arguments, so these fixtures are plain context managers and
helper functions.  pytest puts this directory on ``sys.path`` for the test
modules beside it, so they import the helpers with ``from conftest import ...``.
"""

import contextlib
import os
import subprocess
import tempfile
from collections.abc import Iterator
from pathlib import Path

# gel_release reads the operating repository from the environment at import
# time, and this suite's fixtures assert against the production repository.
# Pin it before any test module imports gel_release so the suite stays
# deterministic even where GITHUB_REPOSITORY is already set, such as the CI
# job that runs this suite on every pull request.
os.environ["GITHUB_REPOSITORY"] = "gelstable/gel-cli"


def _git(repo: Path, *argv: str) -> str:
    """Run ``git`` inside ``repo`` and return its trimmed stdout."""

    return subprocess.run(
        ["git", "-C", str(repo), *argv],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def init_repo(repo: Path, *, user: str = "Test") -> None:
    """Initialize ``repo`` with a deterministic, signature-free commit identity."""

    _git(repo, "init", "-q", ".")
    _git(repo, "config", "user.email", "test@example.com")
    _git(repo, "config", "user.name", user)
    _git(repo, "config", "commit.gpgsign", "false")


@contextlib.contextmanager
def git_repo() -> Iterator[Path]:
    """A throwaway repository holding one committed source file."""

    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)
        init_repo(repo)
        (repo / "src").mkdir()
        (repo / "src" / "main.rs").write_text("fn main() {}\n")
        (repo / "packaging").mkdir()
        _git(repo, "add", "-A")
        _git(repo, "commit", "-qm", "base")
        yield repo


@contextlib.contextmanager
def line_repo(
    *,
    major: int = 7,
    version: str | None = None,
    pending: bool = False,
    change_type: str = "patch",
) -> Iterator[Path]:
    """A throwaway release line checked out at ``release/v<major>.x``.

    The repository carries the Cargo manifest, lock file, and changelog that
    line preparation reads, plus an optional pending change file.
    """

    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)
        init_repo(repo, user="Release Test")
        version = version or f"{major}.1.0"
        (repo / "Cargo.toml").write_text(f'[package]\nname = "gel-cli"\nversion = "{version}"\n')
        (repo / "Cargo.lock").write_text(
            f'version = 4\n\n[[package]]\nname = "gel-cli"\nversion = "{version}"\n'
        )
        (repo / "CHANGELOG.md").write_text("# Changelog\n")
        if pending:
            changeset = repo / ".changeset"
            changeset.mkdir()
            (changeset / "release.md").write_text(
                f"---\ngel-cli: {change_type}\n---\n\nA release line change.\n"
            )
        _git(repo, "add", "-A")
        _git(repo, "commit", "-qm", "base")
        _git(repo, "switch", "-c", f"release/v{major}.x")
        yield repo
