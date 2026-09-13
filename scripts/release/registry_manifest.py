"""Generate the gel-registry.json release manifest.

The manifest is the only contract between a gel-cli release and
gelstable/gel-registry. It references exactly the ten bare and zstd registry
executables. Distribution archives, Linux packages, and digest manifests are
deliberately absent: gel-registry checks that every URL in this document exists
on the release and ignores every other asset.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import jsonschema

from . import assets, digests

SCHEMA_PATH = Path("packaging/schema/release-manifest.schema.json")
SCHEMA_URL = "https://registry.gelstable.com/v1/schema/release-manifest.json"
CHANNEL = "stable"
REVISION = "1"


def _version_details(version: str) -> dict:
    major, minor, patch = (int(part) for part in version.split("."))
    return {
        "major": major,
        "minor": minor,
        "patch": patch,
        "prerelease": [],
        "metadata": [],
    }


def _install_ref(
    version: str, name: str, target: assets.Target, encoding: str, digest: digests.FileDigest
) -> dict:
    return {
        "ref": assets.release_download_url(version, name),
        "type": target.media_type,
        "encoding": encoding,
        "verification": {
            "size": digest.size,
            "blake2b": digest.blake2b512,
            "sha256": digest.sha256,
        },
    }


def build_manifest(
    version: str, build_date: str, entries: dict[str, digests.FileDigest]
) -> dict:
    permitted = set()
    for target in assets.REGISTRY_TARGETS:
        permitted.add(assets.registry_identity_name(target))
        permitted.add(assets.registry_zstd_name(target))

    unexpected = sorted(set(entries) - permitted)
    if unexpected:
        raise ValueError(
            "gel-registry.json may only reference registry executables; refused: "
            + ", ".join(unexpected)
        )

    indexes = []
    for target in assets.REGISTRY_TARGETS:
        identity_name = assets.registry_identity_name(target)
        zstd_name = assets.registry_zstd_name(target)
        indexes.append(
            {
                "channel": CHANNEL,
                "platform": target.triple,
                "packages": [
                    {
                        "basename": assets.REGISTRY_BASENAME,
                        "name": assets.REGISTRY_BASENAME,
                        "version": version,
                        "version_details": _version_details(version),
                        "version_key": version,
                        "revision": REVISION,
                        "build_date": build_date,
                        "architecture": target.arch,
                        "slot": "",
                        "installref": assets.release_download_url(version, zstd_name),
                        "installrefs": [
                            _install_ref(
                                version,
                                identity_name,
                                target,
                                "identity",
                                entries[identity_name],
                            ),
                            _install_ref(
                                version, zstd_name, target, "zstd", entries[zstd_name]
                            ),
                        ],
                        "tags": {},
                    }
                ],
            }
        )
    return {"schema_version": 1, "indexes": indexes}


def validate_manifest(manifest: dict) -> None:
    schema = json.loads(SCHEMA_PATH.read_bytes())
    jsonschema.validate(instance=manifest, schema=schema)


def dump(manifest: dict) -> bytes:
    return (json.dumps(manifest, indent=2, sort_keys=False) + "\n").encode()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build-date", required=True)
    parser.add_argument("--dist-dir", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    entries: dict[str, digests.FileDigest] = {}
    for target in assets.REGISTRY_TARGETS:
        for name in (
            assets.registry_identity_name(target),
            assets.registry_zstd_name(target),
        ):
            path = args.dist_dir / name
            if not path.is_file():
                raise SystemExit(f"missing registry asset {name} in {args.dist_dir}")
            entries[name] = digests.digest_file(path)

    manifest = build_manifest(args.version, args.build_date, entries)
    validate_manifest(manifest)
    args.out.write_bytes(dump(manifest))
    print(f"wrote {args.out} referencing {len(entries)} registry assets")


if __name__ == "__main__":
    main()
