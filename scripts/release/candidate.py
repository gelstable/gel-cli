"""The reviewed candidate's identity record.

packaging/release-candidate.json is the only link between the assets a
maintainer reviewed on the draft release and the release that publication
produces. Publication compares against this record, never against the moving
tip of a branch or the latest successful workflow run.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from . import assets, digests

SCHEMA_VERSION = 1
CANDIDATE_PATH = Path("packaging/release-candidate.json")


class CandidateMismatch(Exception):
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


def dump(record: dict) -> bytes:
    return (json.dumps(record, indent=2, sort_keys=False) + "\n").encode()


def load(path: Path = CANDIDATE_PATH) -> dict:
    return json.loads(path.read_bytes())


def verify_record(record: dict, version: str, dist_dir: Path) -> None:
    if record["schema_version"] != SCHEMA_VERSION:
        raise CandidateMismatch(
            f"unsupported candidate schema_version {record['schema_version']}"
        )
    if record["version"] != version:
        raise CandidateMismatch(
            f"candidate records version {record['version']}, expected {version}"
        )
    if record["tag"] != f"v{version}":
        raise CandidateMismatch(
            f"candidate records tag {record['tag']}, expected v{version}"
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
        raise CandidateMismatch(
            f"staged directory mismatch; missing={missing} extra={extra}"
        )

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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    write = sub.add_parser("write")
    write.add_argument("--version", required=True)
    write.add_argument("--draft-release-id", required=True, type=int)
    write.add_argument("--source-sha", required=True)
    write.add_argument("--build-date", required=True)
    write.add_argument("--run-id", required=True, type=int)
    write.add_argument("--run-attempt", required=True, type=int)
    write.add_argument("--dist-dir", required=True, type=Path)
    write.add_argument("--asset-ids", required=True, type=Path)
    write.add_argument("--out", default=CANDIDATE_PATH, type=Path)

    verify = sub.add_parser("verify")
    verify.add_argument("--version", required=True)
    verify.add_argument("--dist-dir", required=True, type=Path)
    verify.add_argument("--record", default=CANDIDATE_PATH, type=Path)

    args = parser.parse_args()

    if args.command == "write":
        asset_ids = {
            item["name"]: item["id"] for item in json.loads(args.asset_ids.read_bytes())
        }
        record = build_record(
            version=args.version,
            tag=f"v{args.version}",
            draft_release_id=args.draft_release_id,
            source_sha=args.source_sha,
            build_date=args.build_date,
            workflow_runs=[
                {
                    "workflow": "release-candidate.yml",
                    "run_id": args.run_id,
                    "run_attempt": args.run_attempt,
                }
            ],
            attestation={
                "predicate_type": "https://slsa.dev/provenance/v1",
                "subject_count": len(assets.expected_assets(args.version)),
            },
            dist_dir=args.dist_dir,
            asset_ids=asset_ids,
        )
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_bytes(dump(record))
        print(f"wrote {args.out} for {record['tag']}")
        return

    verify_record(load(args.record), args.version, args.dist_dir)
    print(f"candidate record matches {args.dist_dir}")


if __name__ == "__main__":
    main()
