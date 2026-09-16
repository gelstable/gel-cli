"""The reviewed candidate's identity record.

packaging/release-candidate.json is the only link between the assets a
maintainer reviewed on the draft release and the release that publication
produces. Publication compares against this record, never against the moving
tip of a branch or the latest successful workflow run.
"""

from __future__ import annotations

import json
import urllib.request
from collections.abc import Mapping
from pathlib import Path

from pydantic import ValidationError

from . import assets, digests
from .models import CandidateRecord

SCHEMA_VERSION = 2
CANDIDATE_PATH = Path("packaging/release-candidate.json")
PREVIEW_RECORD_NAME = "gel-candidate.json"
PREVIEW_RECORD_PATH = Path(PREVIEW_RECORD_NAME)


class CandidateMismatch(ValueError):
    """On-disk assets disagree with the recorded candidate."""


def validate_record(record: Mapping[str, object] | CandidateRecord) -> CandidateRecord:
    """Validate a candidate record and present failures as release errors."""

    try:
        validated = (
            record
            if isinstance(record, CandidateRecord)
            else CandidateRecord.model_validate(record)
        )
    except ValidationError as error:
        raise CandidateMismatch(f"candidate record validation failed: {error}") from error
    if validated.schema_version != SCHEMA_VERSION:
        raise CandidateMismatch(f"unsupported candidate schema_version {validated.schema_version}")
    return validated


def record_asset_name(record: Mapping[str, object] | CandidateRecord) -> str:
    """Return the release asset/path used to persist a candidate record."""

    phase = record.phase if isinstance(record, CandidateRecord) else record.get("phase")
    return PREVIEW_RECORD_NAME if phase is not None else str(CANDIDATE_PATH)


def verify_source_snapshot(
    record: Mapping[str, object] | CandidateRecord,
    revision: str,
    repo: Path = Path("."),
) -> None:
    """Compare a candidate's recorded snapshot to a Git revision's tree."""

    validated = validate_record(record)
    from . import source_equivalence

    try:
        source_equivalence.assert_snapshot(validated.source_snapshot, revision, repo)
    except source_equivalence.SourceDrift as error:
        raise CandidateMismatch(str(error)) from error


def build_record(
    version: str,
    tag: str,
    draft_release_id: int,
    source_sha: str,
    build_date: str,
    workflow_runs: list[dict],
    attestation: dict,
    dist_dir: Path,
    asset_ids: dict[str, int],
    *,
    line: str,
    pr_number: int,
    phase: str | None,
    source_snapshot: str,
    build_sha: str,
    base_sha: str,
) -> dict:
    entries = []
    expected_assets = assets.expected_assets(version)
    if set(asset_ids) != set(expected_assets):
        missing = sorted(set(expected_assets) - set(asset_ids))
        extra = sorted(set(asset_ids) - set(expected_assets))
        raise CandidateMismatch(f"asset id inventory mismatch; missing={missing} extra={extra}")
    for name in expected_assets:
        path = dist_dir / name
        if not path.is_file():
            raise CandidateMismatch(f"missing staged asset {name}")
        digest = digests.digest_file(path)
        entries.append(
            {
                "id": asset_ids[name],
                "name": name,
                "size": digest.size,
                "sha256": digest.sha256,
                "blake2b512": digest.blake2b512,
            }
        )
    record = {
        "schema_version": SCHEMA_VERSION,
        "line": line,
        "pr_number": pr_number,
        "phase": phase,
        "version": version,
        "tag": tag,
        "draft_release_id": draft_release_id,
        "source_sha": source_sha,
        "source_snapshot": source_snapshot,
        "build_sha": build_sha,
        "base_sha": base_sha,
        "build_date": build_date,
        "workflow_runs": workflow_runs,
        "attestation": attestation,
        "assets": entries,
    }
    return validate_record(record).model_dump(mode="json")


def dump(record: dict | CandidateRecord) -> bytes:
    if not isinstance(record, CandidateRecord):
        record = validate_record(record).model_dump(mode="json")
    else:
        record = record.model_dump(mode="json")
    return (json.dumps(record, indent=2, sort_keys=False) + "\n").encode()


def load(path: Path = CANDIDATE_PATH) -> dict:
    try:
        parsed = CandidateRecord.model_validate_json(path.read_bytes())
    except ValidationError as error:
        raise CandidateMismatch(f"candidate record validation failed: {error}") from error
    return validate_record(parsed).model_dump(mode="json")


def load_bytes(data: bytes) -> dict:
    """Parse candidate bytes using the same strict schema as on-disk records."""

    try:
        parsed = CandidateRecord.model_validate_json(data)
    except ValidationError as error:
        raise CandidateMismatch(f"candidate record validation failed: {error}") from error
    return validate_record(parsed).model_dump(mode="json")


def verify_record_bytes(expected: Mapping[str, object] | CandidateRecord, actual: bytes) -> dict:
    """Require uploaded record bytes to be exactly the bytes we expected."""

    validated = validate_record(expected)
    expected_bytes = dump(validated)
    if actual != expected_bytes:
        raise CandidateMismatch("uploaded candidate record bytes differ from expected record")
    return load_bytes(actual)


def verify_record_asset(expected: Mapping[str, object] | CandidateRecord, path: Path) -> dict:
    """Verify a record downloaded from a release asset endpoint."""

    try:
        actual = path.read_bytes()
    except OSError as error:
        raise CandidateMismatch(f"could not read candidate record asset {path}: {error}") from error
    return verify_record_bytes(expected, actual)


def verify_record(
    record: Mapping[str, object] | CandidateRecord,
    version: str,
    dist_dir: Path,
    *,
    expected_source_snapshot: str | None = None,
    source_snapshot: str | None = None,
    source_revision: str | None = None,
    repo: Path = Path("."),
) -> None:
    if expected_source_snapshot is not None and source_snapshot is not None:
        if expected_source_snapshot != source_snapshot:
            raise CandidateMismatch(
                "conflicting expected source snapshots were supplied for candidate verification"
            )
    if expected_source_snapshot is None:
        expected_source_snapshot = source_snapshot
    validated = validate_record(record)
    if source_revision is not None:
        verify_source_snapshot(validated, source_revision, repo)
    record = validated.model_dump(mode="json")
    if record["version"] != version:
        raise CandidateMismatch(
            f"candidate records version {record['version']}, expected {version}"
        )
    if record["tag"] != f"v{version}":
        raise CandidateMismatch(f"candidate records tag {record['tag']}, expected v{version}")
    if (
        expected_source_snapshot is not None
        and record["source_snapshot"] != expected_source_snapshot
    ):
        raise CandidateMismatch(
            "candidate source snapshot "
            f"{record['source_snapshot']} does not match expected {expected_source_snapshot}"
        )

    recorded = {entry["name"]: entry for entry in record["assets"]}
    expected = assets.expected_assets(version)
    if sorted(recorded) != expected:
        missing = sorted(set(expected) - set(recorded))
        extra = sorted(set(recorded) - set(expected))
        raise CandidateMismatch(f"inventory mismatch; missing={missing} extra={extra}")

    on_disk = sorted(p.name for p in dist_dir.iterdir() if p.is_file())
    if on_disk != expected:
        missing = sorted(set(expected) - set(on_disk))
        extra = sorted(set(on_disk) - set(expected))
        raise CandidateMismatch(f"staged directory mismatch; missing={missing} extra={extra}")

    for name, entry in recorded.items():
        digest = digests.digest_file(dist_dir / name)
        if digest.size != entry["size"]:
            raise CandidateMismatch(
                f"{name}: size {digest.size} does not match recorded {entry['size']}"
            )
        if digest.sha256 != entry["sha256"]:
            raise CandidateMismatch(f"{name}: SHA-256 does not match recorded digest")
        if digest.blake2b512 != entry["blake2b512"]:
            raise CandidateMismatch(f"{name}: BLAKE2b does not match recorded digest")


def write_record(
    *,
    line: str,
    pr_number: int,
    phase: str | None,
    version: str,
    draft_release_id: int,
    source_sha: str,
    build_sha: str,
    source_snapshot: str,
    base_sha: str,
    build_date: str,
    run_id: int,
    run_attempt: int,
    dist_dir: Path,
    asset_ids_path: Path,
    out: Path,
) -> CandidateRecord:
    asset_ids = {item["name"]: item["id"] for item in json.loads(asset_ids_path.read_bytes())}
    record = build_record(
        line=line,
        pr_number=pr_number,
        phase=phase,
        version=version,
        tag=f"v{version}",
        draft_release_id=draft_release_id,
        source_sha=source_sha,
        build_sha=build_sha,
        source_snapshot=source_snapshot,
        base_sha=base_sha,
        build_date=build_date,
        workflow_runs=[
            {
                "workflow": "release-candidate.yml",
                "run_id": run_id,
                "run_attempt": run_attempt,
            }
        ],
        attestation={
            "predicate_type": "https://slsa.dev/provenance/v1",
            "subject_count": len(assets.expected_assets(version)),
        },
        dist_dir=dist_dir,
        asset_ids=asset_ids,
    )
    validated = validate_record(record)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(dump(validated))
    return validated


def verify_public(record: dict, download_dir: Path) -> None:
    validated = validate_record(record)
    download_dir.mkdir(parents=True, exist_ok=True)
    for entry in validated.assets:
        target = download_dir / entry.name
        urllib.request.urlretrieve(
            assets.release_download_url(validated.version, entry.name), target
        )
        digest = digests.digest_file(target)
        if (
            digest.sha256 != entry.sha256
            or digest.blake2b512 != entry.blake2b512
            or digest.size != entry.size
        ):
            raise CandidateMismatch(f"{entry.name}: public bytes differ from candidate")
