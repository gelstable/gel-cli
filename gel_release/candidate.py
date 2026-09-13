"""The reviewed candidate's identity record.

packaging/release-candidate.json is the only link between the assets a
maintainer reviewed on the draft release and the release that publication
produces. Publication compares against this record, never against the moving
tip of a branch or the latest successful workflow run.
"""

from __future__ import annotations

import json
import urllib.request
from pathlib import Path

from pydantic import ValidationError

from . import assets, digests
from .models import CandidateRecord

SCHEMA_VERSION = 1
CANDIDATE_PATH = Path("packaging/release-candidate.json")


class CandidateMismatch(ValueError):
    """On-disk assets disagree with the recorded candidate."""


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
) -> dict:
    entries = []
    for name in assets.expected_assets(version):
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
    return {
        "schema_version": SCHEMA_VERSION,
        "version": version,
        "tag": tag,
        "draft_release_id": draft_release_id,
        "source_sha": source_sha,
        "build_date": build_date,
        "workflow_runs": workflow_runs,
        "attestation": attestation,
        "assets": entries,
    }


def dump(record: dict | CandidateRecord) -> bytes:
    if isinstance(record, CandidateRecord):
        record = record.model_dump(mode="json")
    return (json.dumps(record, indent=2, sort_keys=False) + "\n").encode()


def load(path: Path = CANDIDATE_PATH) -> dict:
    return CandidateRecord.model_validate_json(path.read_bytes()).model_dump(mode="json")


def verify_record(record: dict, version: str, dist_dir: Path) -> None:
    try:
        record = CandidateRecord.model_validate(record).model_dump(mode="json")
    except ValidationError as error:
        raise CandidateMismatch(str(error)) from error
    if record["schema_version"] != SCHEMA_VERSION:
        raise CandidateMismatch(f"unsupported candidate schema_version {record['schema_version']}")
    if record["version"] != version:
        raise CandidateMismatch(
            f"candidate records version {record['version']}, expected {version}"
        )
    if record["tag"] != f"v{version}":
        raise CandidateMismatch(f"candidate records tag {record['tag']}, expected v{version}")

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
    version: str,
    draft_release_id: int,
    source_sha: str,
    build_date: str,
    run_id: int,
    run_attempt: int,
    dist_dir: Path,
    asset_ids_path: Path,
    out: Path,
) -> CandidateRecord:
    asset_ids = {item["name"]: item["id"] for item in json.loads(asset_ids_path.read_bytes())}
    record = build_record(
        version=version,
        tag=f"v{version}",
        draft_release_id=draft_release_id,
        source_sha=source_sha,
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
    validated = CandidateRecord.model_validate(record)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(dump(validated))
    return validated


def verify_public(record: dict, download_dir: Path) -> None:
    validated = CandidateRecord.model_validate(record)
    download_dir.mkdir(parents=True, exist_ok=True)
    for entry in validated.assets:
        target = download_dir / entry.name
        urllib.request.urlretrieve(
            assets.release_download_url(validated.version, entry.name), target
        )
        digest = digests.digest_file(target)
        if digest.sha256 != entry.sha256 or digest.size != entry.size:
            raise CandidateMismatch(f"{entry.name}: public bytes differ from candidate")
