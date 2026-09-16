"""Single command-line entry point for all repository-owned release behavior."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

from pydantic import ValidationError

from . import (
    assets,
    candidate,
    github_release,
    linux_packages,
    package_target,
    preview,
    registry_manifest,
    release_state,
    source_equivalence,
    verify_draft,
)


@dataclass(frozen=True, slots=True)
class LinePreparation:
    """Validated state used to prepare one generated PR for a release line."""

    base_ref: str
    base_sha: str
    major: int
    generated_head: str
    head_ref: str | None
    pending: bool
    prepared_version: str | None
    pending_files: tuple[str, ...]

    def as_dict(self) -> dict[str, object]:
        return {
            "base_ref": self.base_ref,
            "base_sha": self.base_sha,
            "major": self.major,
            "generated_head": self.generated_head,
            "head_ref": self.head_ref,
            "pending": self.pending,
            "prepared_version": self.prepared_version,
            "pending_files": list(self.pending_files),
        }


@dataclass(frozen=True, slots=True)
class ReleasePrOperation:
    """The create or refresh operation selected for a line's open PRs."""

    operation: str
    number: int | None

    def as_dict(self) -> dict[str, int | str | None]:
        return {"operation": self.operation, "number": self.number}


def release_pr_operation(
    open_prs: list[dict[str, object]], *, base_ref: str, head_ref: str
) -> ReleasePrOperation:
    """Choose whether the line workflow creates or refreshes its release PR.

    The input is the complete JSON list returned by ``gh pr list`` after it
    has been filtered by the exact base and head refs.  Requiring at most one
    matching open PR prevents a race or an ambiguous stale PR from selecting
    the wrong candidate.
    """

    major = release_state.parse_line(base_ref)
    expected_head = release_state.expected_head(major)
    if head_ref != expected_head:
        raise ValueError(
            f"generated head {head_ref!r} does not match release line {base_ref!r}; "
            f"expected {expected_head!r}"
        )
    if not isinstance(open_prs, list):
        raise ValueError("open release PRs must be a JSON list")

    numbers: list[int] = []
    for index, pr in enumerate(open_prs):
        if not isinstance(pr, dict):
            raise ValueError(f"open release PR entry {index} is not an object")
        number = pr.get("number")
        if isinstance(number, bool) or not isinstance(number, int) or number < 1:
            raise ValueError(f"open release PR entry {index} has invalid number {number!r}")
        state = pr.get("state")
        if state is not None and state != "OPEN" and state != "open":
            raise ValueError(f"open release PR #{number} has state {state!r}")
        listed_base = pr.get("baseRefName")
        listed_head = pr.get("headRefName")
        if isinstance(pr.get("base"), dict):
            listed_base = listed_base or pr["base"].get("ref")
        if isinstance(pr.get("head"), dict):
            listed_head = listed_head or pr["head"].get("ref")
        if listed_base is not None and listed_base != base_ref:
            raise ValueError(
                f"open release PR #{number} has base {listed_base!r}, expected {base_ref!r}"
            )
        if listed_head is not None and listed_head != head_ref:
            raise ValueError(
                f"open release PR #{number} has head {listed_head!r}, expected {head_ref!r}"
            )
        numbers.append(number)

    if len(numbers) > 1:
        raise ValueError(f"more than one open release PR exists for {base_ref!r} -> {head_ref!r}")
    if not numbers:
        return ReleasePrOperation(operation="create", number=None)
    return ReleasePrOperation(operation="refresh", number=numbers[0])


def _gh(*args: str) -> str:
    """Run a GitHub CLI command and return its trimmed standard output."""

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
    return completed.stdout.strip()


def sync_release_pr(
    *,
    repo: str,
    base_ref: str,
    head_ref: str,
    version: str,
    body_file: Path,
) -> int:
    """Create or refresh the one release PR for a line.

    This owns the GitHub side effect behind ``pr-sync`` so the workflow and
    an executable regression test use the same create/refresh path. The open
    PR list is read immediately before selecting the operation; a create
    result is parsed from ``gh pr create`` rather than queried through a
    second, race-prone list request.
    """

    if not body_file.is_file():
        raise ValueError(f"release PR body file does not exist: {body_file}")
    open_prs_output = _gh(
        "pr",
        "list",
        "--repo",
        repo,
        "--state",
        "open",
        "--base",
        base_ref,
        "--head",
        head_ref,
        "--json",
        "number,baseRefName,headRefName",
        "--limit",
        "2",
    )
    try:
        open_prs = json.loads(open_prs_output)
    except json.JSONDecodeError as error:
        raise ValueError(f"gh pr list returned invalid JSON: {error}") from error
    operation = release_pr_operation(open_prs, base_ref=base_ref, head_ref=head_ref)
    title = f"chore: prepare release {version}"
    if operation.operation == "refresh":
        assert operation.number is not None
        _gh(
            "pr",
            "edit",
            str(operation.number),
            "--repo",
            repo,
            "--base",
            base_ref,
            "--title",
            title,
            "--body-file",
            str(body_file),
        )
        return operation.number

    create_output = _gh(
        "pr",
        "create",
        "--repo",
        repo,
        "--base",
        base_ref,
        "--head",
        head_ref,
        "--title",
        title,
        "--body-file",
        str(body_file),
    )
    match = re.search(r"/pull/([1-9][0-9]*)\b", create_output)
    if match is None:
        raise ValueError(f"gh pr create did not return a pull request URL: {create_output!r}")
    return int(match.group(1))


def _git(repo: Path, *args: str, optional: bool = False) -> str | None:
    """Run a read-only Git command in ``repo`` and return its trimmed output."""

    try:
        completed = subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        if optional:
            return None
        detail = (
            error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) else str(error)
        )
        raise ValueError(f"git {' '.join(args)} failed in {repo}: {detail}") from error
    return completed.stdout.strip()


def _cargo_version(repo: Path) -> str:
    cargo_path = repo / "Cargo.toml"
    try:
        document = tomllib.loads(cargo_path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ValueError(f"could not read {cargo_path}: {error}") from error

    package = document.get("package")
    if not isinstance(package, dict):
        raise ValueError(f"{cargo_path} has no package table")
    version = package.get("version")
    if version is True or (isinstance(version, dict) and version.get("workspace") is True):
        workspace = document.get("workspace")
        if isinstance(workspace, dict):
            workspace_package = workspace.get("package")
            if isinstance(workspace_package, dict):
                version = workspace_package.get("version")
    if not isinstance(version, str) or not version:
        raise ValueError(f"{cargo_path} has no concrete package version")
    return version


def _locked_cargo_version(repo: Path) -> str | None:
    lock_path = repo / "Cargo.lock"
    if not lock_path.exists():
        return None
    try:
        document = tomllib.loads(lock_path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ValueError(f"could not read {lock_path}: {error}") from error
    packages = document.get("package")
    if not isinstance(packages, list):
        raise ValueError(f"{lock_path} has no package entries")
    versions = {
        package.get("version")
        for package in packages
        if isinstance(package, dict) and package.get("name") == "gel-cli"
    }
    versions.discard(None)
    if len(versions) > 1:
        raise ValueError(f"{lock_path} has ambiguous gel-cli package versions")
    return next(iter(versions), None)


def _pending_change_files(repo: Path) -> tuple[str, ...]:
    directory = repo / ".changeset"
    if not directory.is_dir():
        return ()
    return tuple(
        str(path.relative_to(repo)) for path in sorted(directory.glob("*.md")) if path.is_file()
    )


def prepare_line(
    base_ref: str,
    repo: Path = Path("."),
    *,
    prepared: bool = False,
    expected_base_sha: str | None = None,
    head_ref: str | None = None,
    prepared_version: str | None = None,
) -> LinePreparation:
    """Validate release-line state before creating or refreshing its PR.

    The normal invocation runs on the release line before Knope consumes the
    change files.  ``prepared=True`` is used by the workflow after Knope has
    created the generated branch and removed those files; it retains the same
    line and base checks while requiring a concrete prepared version.
    """

    repo = Path(repo)
    major = release_state.parse_line(base_ref)
    generated_head = release_state.expected_head(major)
    if head_ref is not None and head_ref != generated_head:
        raise ValueError(
            f"generated head {head_ref!r} does not match release line {base_ref!r}; "
            f"expected {generated_head!r}"
        )

    base_sha = _git(repo, "rev-parse", "--verify", f"{base_ref}^{{commit}}")
    if base_sha is None:
        raise ValueError(f"could not resolve release line {base_ref!r}")

    remote_sha = _git(
        repo,
        "rev-parse",
        "--verify",
        f"refs/remotes/origin/{base_ref}^{{commit}}",
        optional=True,
    )
    if remote_sha is not None and remote_sha != base_sha:
        raise ValueError(
            f"stale release line {base_ref!r}: local base {base_sha} does not match "
            f"origin/{base_ref} at {remote_sha}"
        )

    head_sha = _git(repo, "rev-parse", "--verify", "HEAD^{commit}")
    current_ref = _git(repo, "symbolic-ref", "--quiet", "--short", "HEAD", optional=True)
    if not prepared:
        if current_ref is not None and current_ref != base_ref:
            raise ValueError(
                f"preparation must start from release line {base_ref!r}; current ref is "
                f"{current_ref!r}"
            )
        if current_ref is None and head_sha != base_sha:
            raise ValueError(
                f"detached HEAD {head_sha} does not match release line {base_ref!r} at {base_sha}"
            )
    elif current_ref is not None and current_ref != generated_head:
        raise ValueError(
            f"prepared release must be on generated head {generated_head!r}; current ref is "
            f"{current_ref!r}"
        )

    if expected_base_sha is not None:
        if re.fullmatch(r"[0-9a-f]{40}", expected_base_sha) is None:
            raise ValueError(f"invalid expected release-line base SHA {expected_base_sha!r}")
        if expected_base_sha != base_sha:
            raise ValueError(
                f"release line {base_ref!r} moved from expected base {expected_base_sha} "
                f"to {base_sha}"
            )

    pending_files = _pending_change_files(repo)
    cargo_version = (
        _cargo_version(repo) if (prepared or pending_files or prepared_version) else None
    )
    if prepared_version is None:
        prepared_version = cargo_version
    elif cargo_version is not None and cargo_version != prepared_version:
        raise ValueError(
            f"prepared version {prepared_version!r} does not match Cargo.toml version "
            f"{cargo_version!r}"
        )
    if prepared_version is not None:
        preview.stable_version(prepared_version, major)
        locked_version = _locked_cargo_version(repo)
        if locked_version is not None and locked_version != prepared_version:
            raise ValueError(
                f"Cargo.lock gel-cli version {locked_version!r} does not match prepared "
                f"Cargo.toml version {prepared_version!r}"
            )

    return LinePreparation(
        base_ref=base_ref,
        base_sha=base_sha,
        major=major,
        generated_head=generated_head,
        head_ref=generated_head,
        pending=bool(pending_files),
        prepared_version=prepared_version,
        pending_files=pending_files,
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="gel-release")
    commands = parser.add_subparsers(dest="command", required=True)
    identity = commands.add_parser("pr-identity")
    identity.add_argument("--pr-json", required=True, type=Path)
    identity.add_argument("--repo", required=True)
    operation = commands.add_parser("pr-operation")
    operation.add_argument("--pr-json", required=True, type=Path)
    operation.add_argument("--base-ref", required=True)
    operation.add_argument("--head-ref", required=True)
    sync = commands.add_parser("pr-sync")
    sync.add_argument("--repo", required=True)
    sync.add_argument("--base-ref", required=True)
    sync.add_argument("--head-ref", required=True)
    sync.add_argument("--version", required=True)
    sync.add_argument("--body-file", required=True, type=Path)
    resolve = commands.add_parser("resolve-candidate")
    resolve.add_argument("--pr-json", required=True, type=Path)
    resolve.add_argument("--live-pr-json", required=True, type=Path)
    resolve.add_argument("--repo", default=assets.REPOSITORY)
    resolve.add_argument("--tags-json", required=True, type=Path)
    resolve.add_argument("--releases-json", required=True, type=Path)
    resolve.add_argument("--prepared-version")
    resolve.add_argument("--source-snapshot")
    derive = commands.add_parser("derive-preview-commit")
    derive.add_argument("--source-sha", required=True)
    derive.add_argument("--version", required=True)
    derive.add_argument("--repo-root", "--repo", dest="repo_root", default=Path("."), type=Path)
    authorize = commands.add_parser("phase-authorized")
    authorize.add_argument("--timeline-json", required=True, type=Path)
    authorize.add_argument("--phase", required=True)
    authorize.add_argument("--permissions-json", required=True, type=Path)
    preparation = commands.add_parser("prepare-line")
    preparation.add_argument("--base-ref", required=True)
    preparation.add_argument(
        "--repo-root", "--repo", dest="repo_root", default=Path("."), type=Path
    )
    preparation.add_argument("--prepared", action="store_true")
    preparation.add_argument(
        "--expected-base-sha",
        "--base-sha",
        dest="expected_base_sha",
    )
    preparation.add_argument("--head-ref")
    preparation.add_argument("--prepared-version")
    matrix = commands.add_parser("matrix")
    matrix.add_argument("kind", choices=("build", "smoke"))
    channel = commands.add_parser("channel")
    channel.add_argument("--version", required=True)
    completions = commands.add_parser("completions")
    completions.add_argument("--binary", required=True, type=Path)
    completions.add_argument("--out-dir", required=True, type=Path)
    package = commands.add_parser("package-target")
    for name in ("target", "version", "binary", "completions-dir", "out-dir"):
        package.add_argument(
            f"--{name}",
            required=True,
            type=Path if name.endswith("dir") or name == "binary" else str,
        )
    package.add_argument("--repo-root", default=Path("."), type=Path)
    linux = commands.add_parser("linux-packages")
    linux.add_argument("--target", required=True)
    linux.add_argument("--version", required=True)
    linux.add_argument("--completions-dir", required=True, type=Path)
    linux.add_argument("--out-dir", required=True, type=Path)
    stage = commands.add_parser("assemble-stage")
    stage.add_argument("--version", required=True)
    stage.add_argument("--build-date", required=True)
    stage.add_argument("--dist-dir", required=True, type=Path)
    manifest = commands.add_parser("registry-manifest")
    manifest.add_argument("--version", required=True)
    manifest.add_argument("--build-date", required=True)
    manifest.add_argument("--dist-dir", required=True, type=Path)
    manifest.add_argument("--out", required=True, type=Path)
    record = commands.add_parser("candidate").add_subparsers(
        dest="candidate_command", required=True
    )
    write = record.add_parser("write")
    write.add_argument("--line", required=True)
    write.add_argument("--version", required=True)
    write.add_argument("--source-sha", "--original-source-sha", dest="source_sha", required=True)
    write.add_argument("--build-sha", required=True)
    write.add_argument("--source-snapshot", "--snapshot", dest="source_snapshot", required=True)
    write.add_argument("--base-sha", required=True)
    write.add_argument("--build-date", required=True)
    write.add_argument("--pr-number", "--pr", dest="pr_number", required=True, type=int)
    write.add_argument("--phase", choices=("alpha", "beta", "rc"))
    for flag in ("draft-release-id", "run-id", "run-attempt"):
        write.add_argument(f"--{flag}", required=True, type=int)
    for flag in ("dist-dir", "asset-ids", "out"):
        write.add_argument(f"--{flag}", required=True, type=Path)
    verify = record.add_parser("verify")
    verify.add_argument("--version", required=True)
    verify.add_argument("--dist-dir", required=True, type=Path)
    verify.add_argument("--record", default=candidate.CANDIDATE_PATH, type=Path)
    source = commands.add_parser("source-equivalence")
    source.add_argument("--base", required=True)
    source.add_argument("--head", required=True)
    source.add_argument("--repo", default=Path("."), type=Path)
    snapshot = commands.add_parser("snapshot")
    snapshot.add_argument("--rev", required=True)
    snapshot.add_argument("--repo", default=Path("."), type=Path)
    preview_version = commands.add_parser("preview-version")
    preview_version.add_argument("--base", required=True)
    preview_version.add_argument("--phase", required=True)
    preview_version.add_argument(
        "--snapshot", "--current-snapshot", dest="current_snapshot", required=True
    )
    preview_version.add_argument("--tags-json", required=True, type=Path)
    preview_version.add_argument("--published-json", required=True, type=Path)
    draft = commands.add_parser("verify-draft")
    draft.add_argument("--record", default=candidate.CANDIDATE_PATH, type=Path)
    draft.add_argument("--download-dir", required=True, type=Path)
    draft.add_argument("--repo", default=assets.REPOSITORY)
    draft.add_argument("--skip-attestations", action="store_true")
    draft.add_argument("--version")
    draft.add_argument("--line")
    draft.add_argument("--pr-number", "--pr", dest="pr_number", type=int)
    draft.add_argument("--phase", choices=("alpha", "beta", "rc"))
    draft.add_argument("--source-sha", "--original-source-sha", dest="source_sha")
    draft.add_argument("--build-sha")
    draft.add_argument("--source-snapshot", "--snapshot", dest="source_snapshot")
    draft.add_argument("--base-sha")
    public = commands.add_parser("verify-public")
    public.add_argument("--record", default=candidate.CANDIDATE_PATH, type=Path)
    public.add_argument("--download-dir", required=True, type=Path)
    return parser


def _matrix(kind: str) -> dict[str, list[dict[str, object]]]:
    targets = assets.TARGETS if kind == "build" else assets.REGISTRY_TARGETS
    return {
        "include": [
            {
                "target": t.triple,
                "runner": t.runner,
                **({"linux_packages": t.deb_arch is not None} if kind == "build" else {}),
            }
            for t in targets
        ]
    }


def _read_tags(path: Path) -> list[str]:
    value = json.loads(path.read_text())
    if isinstance(value, dict):
        value = value.get("tags")
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise ValueError(f"{path} must contain a JSON list of tag strings")
    return value


def _read_published_snapshots(path: Path) -> set[tuple[str, str]]:
    value = json.loads(path.read_text())
    if isinstance(value, dict):
        value = value.get("published")
    if not isinstance(value, list):
        raise ValueError(f"{path} must contain a JSON list of published snapshots")

    result: set[tuple[str, str]] = set()
    for entry in value:
        if isinstance(entry, list) and len(entry) == 2:
            phase, snapshot = entry
        elif isinstance(entry, dict):
            phase = entry.get("phase")
            snapshot = entry.get("snapshot", entry.get("meaningful_tree"))
            if snapshot is None:
                snapshot = entry.get("source_snapshot")
        else:
            raise ValueError(f"{path} contains an invalid published snapshot entry")
        if not isinstance(phase, str) or not isinstance(snapshot, str):
            raise ValueError(f"{path} contains an invalid published snapshot entry")
        result.add((phase, snapshot))
    return result


def _read_releases(path: Path) -> list[dict]:
    value = json.loads(path.read_text())
    if isinstance(value, dict):
        value = value.get("releases")
    if not isinstance(value, list) or not all(isinstance(item, dict) for item in value):
        raise ValueError(f"{path} must contain a JSON list of release objects")
    return value


def main(argv: list[str] | None = None) -> int:
    try:
        args = _parser().parse_args(argv)
        if args.command == "pr-identity":
            identity = release_state.validate_pr(
                json.loads(args.pr_json.read_bytes()),
                args.repo,
            )
            print(json.dumps(identity.as_dict(), separators=(",", ":"), sort_keys=True))
        elif args.command == "pr-operation":
            operation = release_pr_operation(
                json.loads(args.pr_json.read_bytes()),
                base_ref=args.base_ref,
                head_ref=args.head_ref,
            )
            print(json.dumps(operation.as_dict(), separators=(",", ":"), sort_keys=True))
        elif args.command == "pr-sync":
            print(
                sync_release_pr(
                    repo=args.repo,
                    base_ref=args.base_ref,
                    head_ref=args.head_ref,
                    version=args.version,
                    body_file=args.body_file,
                )
            )
        elif args.command == "resolve-candidate":
            pr = release_state.validate_pr(json.loads(args.pr_json.read_bytes()), args.repo)
            live_pr = json.loads(args.live_pr_json.read_bytes())
            if not isinstance(live_pr, dict):
                raise ValueError("live PR JSON must be an object")
            if args.prepared_version is not None:
                live_pr["prepared_version"] = args.prepared_version
            if args.source_snapshot is not None:
                live_pr["source_snapshot"] = args.source_snapshot
            identity = github_release.resolve_candidate(
                pr,
                live_pr,
                _read_tags(args.tags_json),
                _read_releases(args.releases_json),
            )
            if identity is not None:
                print(json.dumps(identity.as_dict(), separators=(",", ":"), sort_keys=True))
        elif args.command == "derive-preview-commit":
            print(
                github_release.derive_preview_commit(
                    args.source_sha,
                    args.version,
                    args.repo_root,
                )
            )
        elif args.command == "phase-authorized":
            timeline = json.loads(args.timeline_json.read_bytes())
            permissions = json.loads(args.permissions_json.read_bytes())
            print(
                "true"
                if github_release.phase_authorized(timeline, args.phase, permissions)
                else "false"
            )
        elif args.command == "prepare-line":
            result = prepare_line(
                args.base_ref,
                args.repo_root,
                prepared=args.prepared,
                expected_base_sha=args.expected_base_sha,
                head_ref=args.head_ref,
                prepared_version=args.prepared_version,
            )
            print(json.dumps(result.as_dict(), separators=(",", ":"), sort_keys=True))
        elif args.command == "matrix":
            print(json.dumps(_matrix(args.kind), separators=(",", ":")))
        elif args.command == "channel":
            print(registry_manifest.release_channel(args.version))
        elif args.command == "completions":
            package_target.generate_completions(args.binary, args.out_dir)
        elif args.command == "package-target":
            target = assets.BY_TRIPLE[args.target]
            if target.registry:
                for path in package_target.build_registry_payload(
                    args.binary, target, args.out_dir
                ):
                    print(path.name)
            archive = package_target.build_archive(
                args.binary,
                target,
                args.version,
                args.completions_dir,
                [args.repo_root / n for n in package_target.ARCHIVE_EXTRA_FILES],
                args.out_dir,
            )
            print(archive.name)
        elif args.command == "linux-packages":
            for path in linux_packages.build(
                assets.BY_TRIPLE[args.target], args.version, args.completions_dir, args.out_dir
            ):
                print(path.name)
        elif args.command == "assemble-stage":
            registry_manifest.assemble_stage(args.version, args.build_date, args.dist_dir)
            print(f"assembled {len(assets.expected_assets(args.version))} release assets")
        elif args.command == "registry-manifest":
            registry_manifest.write_manifest(args.version, args.build_date, args.dist_dir, args.out)
            print(f"wrote {args.out}")
        elif args.command == "candidate" and args.candidate_command == "write":
            validated = candidate.write_record(
                line=args.line,
                pr_number=args.pr_number,
                phase=args.phase,
                version=args.version,
                draft_release_id=args.draft_release_id,
                source_sha=args.source_sha,
                build_sha=args.build_sha,
                source_snapshot=args.source_snapshot,
                base_sha=args.base_sha,
                build_date=args.build_date,
                run_id=args.run_id,
                run_attempt=args.run_attempt,
                dist_dir=args.dist_dir,
                asset_ids_path=args.asset_ids,
                out=args.out,
            )
            print(f"wrote {args.out} for {validated.tag}")
        elif args.command == "candidate":
            candidate.verify_record(candidate.load(args.record), args.version, args.dist_dir)
            print(f"candidate record matches {args.dist_dir}")
        elif args.command == "source-equivalence":
            source_equivalence.assert_equivalent(args.base, args.head, args.repo)
            print(f"{args.head} is source-equivalent to {args.base} outside the allowlist")
        elif args.command == "snapshot":
            print(source_equivalence.meaningful_tree(args.rev, args.repo))
        elif args.command == "preview-version":
            selected = preview.next_preview_version(
                args.base,
                args.phase,
                _read_tags(args.tags_json),
                _read_published_snapshots(args.published_json),
                args.current_snapshot,
            )
            if selected is not None:
                print(selected)
        elif args.command == "verify-draft":
            verify_draft.verify(
                candidate.load(args.record),
                args.download_dir,
                args.repo,
                not args.skip_attestations,
                expected_version=args.version,
                expected_line=args.line,
                expected_pr_number=args.pr_number,
                expected_phase=args.phase,
                expected_source_sha=args.source_sha,
                expected_build_sha=args.build_sha,
                expected_source_snapshot=args.source_snapshot,
                expected_base_sha=args.base_sha,
            )
            print("staged candidate verified against the draft release")
        elif args.command == "verify-public":
            candidate.verify_public(candidate.load(args.record), args.download_dir)
            print("public downloads match the reviewed candidate")
        return 0
    except (KeyError, OSError, ValueError, ValidationError, subprocess.CalledProcessError) as error:
        print(f"validation error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
