"""Generate the gel-registry.json release manifest.

The manifest is the only contract between a gel-cli release and
gelstable/gel-registry. It references exactly the ten bare and zstd registry
executables. Distribution archives, Linux packages, and digest manifests are
deliberately absent: gel-registry checks that every URL in this document exists
on the release and ignores every other asset.
"""

from __future__ import annotations

import json
from pathlib import Path

import jsonschema

from . import assets, digests
from .models import ReleaseManifest

SCHEMA_PATH = (
    Path(__file__).resolve().parent.parent / "packaging" / "schema" / "release-manifest.schema.json"
)
CHANNEL = "stable"
REVISION = "1"


def _version_details(version: str) -> dict:
    core = version.split("-")[0].split("+")[0]
    major, minor, patch = (int(part) for part in core.split("."))
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


def build_manifest(version: str, build_date: str, entries: dict[str, digests.FileDigest]) -> dict:
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
                            _install_ref(version, zstd_name, target, "zstd", entries[zstd_name]),
                        ],
                        "tags": {},
                    }
                ],
            }
        )
    return {"schema_version": 1, "indexes": indexes}


def validate_manifest(manifest: dict) -> None:
    ReleaseManifest.model_validate(manifest)
    schema = json.loads(SCHEMA_PATH.read_bytes())
    jsonschema.validate(instance=manifest, schema=schema)


def assemble_stage(version: str, build_date: str, dist_dir: Path) -> None:
    entries = {}
    for target in assets.REGISTRY_TARGETS:
        for name in (assets.registry_identity_name(target), assets.registry_zstd_name(target)):
            path = dist_dir / name
            if not path.is_file():
                raise ValueError(f"missing registry asset {name}")
            entries[name] = digests.digest_file(path)
    manifest = build_manifest(version, build_date, entries)
    validate_manifest(manifest)
    (dist_dir / assets.REGISTRY_MANIFEST_NAME).write_bytes(dump(manifest))
    skip = {assets.SHA256SUMS_NAME, assets.BLAKE2B_SUMS_NAME}
    digests.write_sums(dist_dir, "sha256", dist_dir / assets.SHA256SUMS_NAME, skip)
    digests.write_sums(dist_dir, "blake2b512", dist_dir / assets.BLAKE2B_SUMS_NAME, skip)
    found = sorted(path.name for path in dist_dir.iterdir() if path.is_file())
    expected = assets.expected_assets(version)
    if found != expected:
        missing = sorted(set(expected) - set(found))
        extra = sorted(set(found) - set(expected))
        raise ValueError(f"inventory mismatch; missing={missing} extra={extra}")


def write_manifest(version: str, build_date: str, dist_dir: Path, out: Path) -> None:
    entries = {}
    for target in assets.REGISTRY_TARGETS:
        for name in (assets.registry_identity_name(target), assets.registry_zstd_name(target)):
            entries[name] = digests.digest_file(dist_dir / name)
    manifest = build_manifest(version, build_date, entries)
    validate_manifest(manifest)
    out.write_bytes(dump(manifest))


def dump(manifest: dict) -> bytes:
    return (json.dumps(manifest, indent=2, sort_keys=False) + "\n").encode()
