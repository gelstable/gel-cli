"""Read the staged candidate back from the GitHub API and verify it.

Draft release assets are not publicly downloadable, so every read goes through
the authenticated assets API. The bytes verified here are the bytes GitHub
stored, not the bytes the build job happened to leave on disk.
"""

from __future__ import annotations

import json
import re
import subprocess
from collections.abc import Mapping
from pathlib import Path

from . import assets, candidate, digests, registry_manifest
from .models import CandidateRecord, GithubAsset

DISTRIBUTION_SUFFIXES = (".tar.gz", ".zip", ".deb", ".rpm")
DIGEST_MANIFEST_NAMES = (assets.SHA256SUMS_NAME, assets.BLAKE2B_SUMS_NAME)
SHA1_PATTERN = re.compile(r"^[0-9a-fA-F]{40}$")


class DraftVerificationError(ValueError):
    """The staged release does not match the reviewed candidate."""


def _gh_json(*argv: str) -> object:
    completed = subprocess.run(["gh", *argv], check=True, capture_output=True, text=True)
    text = completed.stdout.strip()
    if not text:
        return None
    decoder = json.JSONDecoder()
    pos = 0
    results = []
    while pos < len(text):
        while pos < len(text) and text[pos].isspace():
            pos += 1
        if pos >= len(text):
            break
        obj, end = decoder.raw_decode(text, pos)
        results.append(obj)
        pos = end
    if len(results) == 1:
        return results[0]
    return results


def list_release_assets(
    release_id: str | None = None,
    repo: str = assets.REPOSITORY,
    *,
    tag_or_id: str | None = None,
) -> list[dict]:
    target_id = release_id if release_id is not None else tag_or_id
    if target_id is None:
        raise ValueError("release_id or tag_or_id is required")
    payload = _gh_json(
        "api",
        "--paginate",
        f"/repos/{repo}/releases/{target_id}/assets",
        "--jq",
        "[.[] | {id: .id, name: .name, size: .size}]",
    )
    if payload is None:
        return []
    if not isinstance(payload, list):
        raise DraftVerificationError("GitHub assets response must be a list")
    flattened: list[dict] = []
    if payload and isinstance(payload[0], list):
        for page in payload:
            if not isinstance(page, list):
                raise DraftVerificationError("GitHub paginated assets response is invalid")
            flattened.extend(page)
        return [item.model_dump() for item in map(GithubAsset.model_validate, flattened)]
    return [GithubAsset.model_validate(item).model_dump() for item in payload]


def get_release(release_id: str, repo: str = assets.REPOSITORY) -> dict:
    payload = _gh_json("api", f"/repos/{repo}/releases/{release_id}")
    if not isinstance(payload, dict):
        raise DraftVerificationError("GitHub release response must be an object")
    return payload


def _tag_object(payload: object, context: str) -> tuple[str, str]:
    if not isinstance(payload, dict) or not isinstance(payload.get("object"), dict):
        raise DraftVerificationError(f"GitHub {context} response has no tag object")
    object_data = payload["object"]
    object_type = object_data.get("type")
    object_sha = object_data.get("sha")
    if not isinstance(object_type, str) or not isinstance(object_sha, str):
        raise DraftVerificationError(f"GitHub {context} response has an invalid tag object")
    if not SHA1_PATTERN.fullmatch(object_sha):
        raise DraftVerificationError(f"GitHub {context} response has an invalid object SHA")
    return object_type, object_sha


def _is_http_404(error: subprocess.CalledProcessError) -> bool:
    output = "\n".join(
        value.decode(errors="replace") if isinstance(value, bytes) else str(value)
        for value in (error.stdout, error.stderr)
        if value
    )
    statuses = set(re.findall(r"\bHTTP[ \t]+(\d{3})\b", output, flags=re.IGNORECASE))
    return statuses == {"404"}


def resolve_tag_commit(tag: str, repo: str = assets.REPOSITORY) -> str | None:
    try:
        payload = _gh_json("api", f"/repos/{repo}/git/ref/tags/{tag}")
    except subprocess.CalledProcessError as error:
        if _is_http_404(error):
            # Candidate drafts are verified before publication, so their tag
            # may not exist yet. Attestation verification remains the source
            # proof.
            return None
        raise DraftVerificationError(
            f"tag lookup failed for {tag}; refusing to treat the API error as a missing tag"
        ) from error

    object_type, object_sha = _tag_object(payload, f"tag ref {tag}")
    seen: set[str] = set()
    for _ in range(8):
        if object_sha in seen:
            raise DraftVerificationError(f"tag {tag} contains a dereference cycle")
        seen.add(object_sha)
        if object_type == "commit":
            return object_sha
        if object_type != "tag":
            raise DraftVerificationError(
                f"tag {tag} resolves to unsupported Git object type {object_type!r}"
            )
        try:
            payload = _gh_json("api", f"/repos/{repo}/git/tags/{object_sha}")
        except subprocess.CalledProcessError as error:
            raise DraftVerificationError(
                f"annotated tag object {object_sha} for {tag} could not be resolved"
            ) from error
        object_type, object_sha = _tag_object(payload, f"annotated tag {object_sha}")
    raise DraftVerificationError(f"tag {tag} has too many annotated tag layers")


def check_release_identity(
    release: dict,
    record: Mapping[str, object] | CandidateRecord,
    repo: str = assets.REPOSITORY,
) -> str | None:
    if isinstance(record, CandidateRecord):
        record = record.model_dump(mode="json")
    release_id = release.get("id")
    expected_release_id = record.get("draft_release_id")
    if isinstance(release_id, bool) or not isinstance(release_id, int):
        raise DraftVerificationError("draft release response has no valid release id")
    if release_id != expected_release_id:
        raise DraftVerificationError(
            f"draft release id {release_id} does not match candidate release id "
            f"{expected_release_id}"
        )

    expected_tag = record["tag"]
    actual_tag = release.get("tag_name")
    if actual_tag != expected_tag:
        raise DraftVerificationError(
            f"draft release tag {actual_tag!r} does not match candidate tag {expected_tag!r}"
        )
    if release.get("name") != expected_tag:
        raise DraftVerificationError(
            f"draft release name {release.get('name')!r} does not match candidate tag "
            f"{expected_tag!r}"
        )
    if release.get("draft") is not True:
        raise DraftVerificationError("candidate release must still be a draft")
    expected_prerelease = record.get("phase") is not None
    if release.get("prerelease") is not expected_prerelease:
        raise DraftVerificationError(
            f"draft release prerelease flag {release.get('prerelease')!r} does not match "
            f"candidate phase {record.get('phase')!r}"
        )

    resolved = resolve_tag_commit(expected_tag, repo)
    expected_build_sha = record.get("build_sha", record["source_sha"])
    if resolved is not None and resolved.lower() != expected_build_sha.lower():
        raise DraftVerificationError(
            f"tag {expected_tag} resolves to {resolved}, expected candidate build/source "
            f"SHA {expected_build_sha}"
        )
    return resolved


def download_asset(asset_id: int, destination: Path, repo: str = assets.REPOSITORY) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with open(destination, "wb") as handle:
        subprocess.run(
            [
                "gh",
                "api",
                "-H",
                "Accept: application/octet-stream",
                f"/repos/{repo}/releases/assets/{asset_id}",
            ],
            check=True,
            stdout=handle,
        )


def verify_attestations(paths: list[Path], repo: str, source_sha: str) -> None:
    for path in paths:
        subprocess.run(
            [
                "gh",
                "attestation",
                "verify",
                str(path),
                "--repo",
                repo,
                "--source-digest",
                source_sha,
            ],
            check=True,
        )


def check_inventory(
    listed: list[dict],
    version: str,
    phase: str | Mapping[str, object] | CandidateRecord | None = None,
    *,
    record: Mapping[str, object] | CandidateRecord | None = None,
    preview: bool | None = None,
    record_asset_name: str | None = None,
) -> None:
    if isinstance(phase, Mapping):
        record = phase
        phase = record.get("phase")
    elif isinstance(phase, CandidateRecord):
        record = phase
        phase = record.phase
    elif record is not None and phase is None:
        phase = record.phase if isinstance(record, CandidateRecord) else record.get("phase")
    if preview is True and phase is None:
        phase = "preview"
    if preview is False:
        phase = None
    if phase is None and preview is None and record_asset_name is None:
        if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+-(?:alpha|beta|rc)\.[1-9][0-9]*", version):
            phase = "preview"
    if phase is not None and phase not in {"alpha", "beta", "rc"}:
        if phase != "preview":
            raise DraftVerificationError(f"unsupported candidate phase {phase!r}")

    names = sorted(entry["name"] for entry in listed)
    expected = assets.expected_assets(version)
    if phase is not None or record_asset_name is not None:
        expected = sorted([*expected, record_asset_name or candidate.PREVIEW_RECORD_NAME])
    if names != expected:
        missing = sorted(set(expected) - set(names))
        extra = sorted(set(names) - set(expected))
        raise DraftVerificationError(
            f"draft inventory mismatch; missing={missing} unexpected={extra}"
        )


def check_manifest_urls(manifest: dict, version: str) -> None:
    prefix = f"https://github.com/{assets.REPOSITORY}/releases/download/v{version}/"
    permitted = set()
    for target in assets.REGISTRY_TARGETS:
        permitted.add(assets.registry_identity_name(target))
        permitted.add(assets.registry_zstd_name(target))

    refs: list[str] = []
    for fragment in manifest["indexes"]:
        for package in fragment["packages"]:
            refs.append(package["installref"])
            refs.extend(ref["ref"] for ref in package["installrefs"])

    for ref in refs:
        if not ref.startswith(prefix):
            raise DraftVerificationError(
                f"gel-registry.json must use public release URLs under {prefix}; got {ref}"
            )
        name = ref[len(prefix) :]
        if name.endswith(DISTRIBUTION_SUFFIXES) or name in DIGEST_MANIFEST_NAMES:
            raise DraftVerificationError(
                f"isolation invariant violated: gel-registry.json references {name}"
            )
        if name not in permitted:
            raise DraftVerificationError(f"gel-registry.json references unknown asset {name}")


def check_manifest_digests(manifest: dict, download_dir: Path) -> None:
    for fragment in manifest["indexes"]:
        for package in fragment["packages"]:
            for ref in package["installrefs"]:
                name = ref["ref"].rsplit("/", 1)[-1]
                actual = digests.digest_file(download_dir / name)
                expected = ref["verification"]
                if actual.blake2b512 != expected["blake2b"]:
                    raise DraftVerificationError(
                        f"{name}: gel-registry.json BLAKE2b does not match the staged bytes"
                    )
                if actual.sha256 != expected["sha256"]:
                    raise DraftVerificationError(
                        f"{name}: gel-registry.json SHA-256 does not match the staged bytes"
                    )
                if actual.size != expected["size"]:
                    raise DraftVerificationError(
                        f"{name}: gel-registry.json size does not match the staged bytes"
                    )


def verify(
    record: Mapping[str, object] | CandidateRecord,
    download_dir: Path,
    repo: str = assets.REPOSITORY,
    verify_attestations_flag: bool = True,
    expected_source_snapshot: str | None = None,
    *,
    expected_version: str | None = None,
    expected_line: str | None = None,
    expected_pr_number: int | None = None,
    expected_phase: str | None = None,
    expected_source_sha: str | None = None,
    expected_build_sha: str | None = None,
    expected_base_sha: str | None = None,
    source_snapshot: str | None = None,
) -> None:
    if expected_source_snapshot is not None and source_snapshot is not None:
        if expected_source_snapshot != source_snapshot:
            raise DraftVerificationError(
                "conflicting expected source snapshots were supplied for draft verification"
            )
    if expected_source_snapshot is None:
        expected_source_snapshot = source_snapshot
    record = candidate.validate_record(record).model_dump(mode="json")
    expected_identity = {
        "version": expected_version,
        "line": expected_line,
        "pr_number": expected_pr_number,
        "phase": expected_phase,
        "source_sha": expected_source_sha,
        "build_sha": expected_build_sha,
        "source_snapshot": expected_source_snapshot,
        "base_sha": expected_base_sha,
    }
    for field, expected in expected_identity.items():
        if expected is not None and record[field] != expected:
            raise DraftVerificationError(
                f"candidate {field} {record[field]!r} does not match expected {expected!r}"
            )
    version = record["version"]
    release = get_release(str(record["draft_release_id"]), repo)
    tag_commit = check_release_identity(release, record, repo)
    if tag_commit is None and not verify_attestations_flag:
        raise DraftVerificationError(
            f"tag {record['tag']} is not created; source cannot be checked with "
            "attestation verification disabled"
        )
    listed = list_release_assets(str(record["draft_release_id"]), repo)
    check_inventory(listed, version, record=record)

    by_name = {entry["name"]: entry for entry in listed}
    for entry in record["assets"]:
        remote = by_name[entry["name"]]
        if remote["id"] != entry["id"]:
            raise DraftVerificationError(
                f"{entry['name']}: asset id {remote['id']} replaced recorded {entry['id']}"
            )
        if remote["size"] != entry["size"]:
            raise DraftVerificationError(
                f"{entry['name']}: size {remote['size']} replaced recorded {entry['size']}"
            )

    download_dir.mkdir(parents=True, exist_ok=True)
    for entry in record["assets"]:
        download_asset(entry["id"], download_dir / entry["name"], repo)

    try:
        candidate.verify_record(
            record,
            version,
            download_dir,
            expected_source_snapshot=expected_source_snapshot,
        )
    except candidate.CandidateMismatch as error:
        raise DraftVerificationError(str(error)) from error

    for name in DIGEST_MANIFEST_NAMES:
        try:
            digests.verify_sums(download_dir, download_dir / name)
        except (digests.DigestMismatch, OSError, ValueError) as error:
            raise DraftVerificationError(
                f"{name}: digest manifest verification failed: {error}"
            ) from error

    manifest = json.loads((download_dir / assets.REGISTRY_MANIFEST_NAME).read_bytes())
    registry_manifest.validate_manifest(manifest)
    check_manifest_urls(manifest, version)
    check_manifest_digests(manifest, download_dir)

    if verify_attestations_flag:
        verify_attestations(
            [download_dir / entry["name"] for entry in record["assets"]],
            repo,
            record["build_sha"],
        )

    if record["phase"] is not None:
        record_asset = by_name[candidate.PREVIEW_RECORD_NAME]
        expected_record_bytes = candidate.dump(record)
        if record_asset["size"] != len(expected_record_bytes):
            raise DraftVerificationError(
                f"{candidate.PREVIEW_RECORD_NAME}: size {record_asset['size']} does not match "
                "expected candidate record bytes"
            )
        record_path = download_dir / candidate.PREVIEW_RECORD_NAME
        download_asset(record_asset["id"], record_path, repo)
        try:
            candidate.verify_record_asset(record, record_path)
        except candidate.CandidateMismatch as error:
            raise DraftVerificationError(str(error)) from error
