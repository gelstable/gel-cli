"""Generate the gel-registry.json release manifest.

The manifest binds portable executables and native packages to the same release.
Native digests are collected after RPM signing.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Literal

import jsonschema
from pydantic import BaseModel

from . import assets, digests
from .models import LegacyReplacementsManifest, NativeReleaseManifest, ReleaseManifest

SCHEMA_PATH = (
    Path(__file__).resolve().parent.parent / "packaging" / "schema" / "release-manifest.schema.json"
)
REVISION = "1"
VERSION_PATTERN = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-(alpha|beta|rc)\.([1-9][0-9]*))?$"
)


def _version_details(version: str) -> dict:
    match = VERSION_PATTERN.fullmatch(version)
    if match is None:
        raise ValueError(f"unsupported release version: {version}")
    major, minor, patch = (int(match.group(i)) for i in (1, 2, 3))
    phase, number = match.group(4, 5)
    return {
        "major": major,
        "minor": minor,
        "patch": patch,
        "prerelease": [{"phase": phase, "number": int(number)}] if phase else [],
        "metadata": {},
    }


def release_channel(version: str) -> Literal["stable", "testing"]:
    details = _version_details(version)
    if not details["prerelease"]:
        return "stable"
    return "testing"


def _install_ref(
    version: str,
    name: str,
    target: assets.Target,
    encoding: Literal["identity", "zstd"],
    digest: digests.FileDigest,
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
    details = _version_details(version)
    channel = release_channel(version)
    permitted = set()
    for target in assets.REGISTRY_TARGETS:
        permitted.add(assets.registry_identity_name(target))
        permitted.add(assets.registry_zstd_name(target))

    native_names = []
    for target in assets.TARGETS:
        if target.deb_arch is not None:
            native_names.extend(
                [assets.deb_name(version, target), assets.rpm_name(version, target)]
            )
    permitted.update(native_names)

    unexpected = sorted(set(entries) - permitted)
    if unexpected:
        raise ValueError(
            "gel-registry.json may only reference executables and native packages; refused: "
            + ", ".join(unexpected)
        )

    indexes = []
    for target in assets.REGISTRY_TARGETS:
        identity_name = assets.registry_identity_name(target)
        zstd_name = assets.registry_zstd_name(target)
        indexes.append(
            {
                "channel": channel,
                "platform": target.triple,
                "packages": [
                    {
                        "basename": assets.REGISTRY_BASENAME,
                        "name": assets.REGISTRY_BASENAME,
                        "version": version,
                        "version_details": details,
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
    return {
        "schema_version": 2,
        "indexes": indexes,
        "native": {
            "channel": channel,
            "packages": [
                {
                    "url": assets.release_download_url(version, name),
                    "sha256": entries[name].sha256,
                    "size": entries[name].size,
                }
                for name in native_names
            ],
        },
    }


def _manifest_model(manifest: dict) -> type[BaseModel]:
    """Pick the document model for one manifest.

    Two unrelated documents share ``schema_version: 1``: the ``replacements``
    form published for v7.10.x, and the historical ``indexes`` form. Version 2 is dispatched
    separately before either legacy shape. What tells them apart is the presence of a non-empty
    ``replacements`` array rather than the value of any single field, so
    Pydantic's discriminated union cannot express the choice. A plain
    (non-discriminated) union would be worse than an explicit dispatch: a
    freshly generated manifest with a malformed index would fall through to
    the permissive legacy branch and be reported as failing both arms, which
    is exactly the loss of strictness this split exists to prevent.
    """
    if manifest.get("schema_version") == 2:
        return NativeReleaseManifest
    if manifest.get("replacements"):
        return LegacyReplacementsManifest
    if manifest.get("indexes"):
        return ReleaseManifest
    raise ValueError("a release manifest must contain indexes or replacements")


def validate_manifest(manifest: dict) -> None:
    _manifest_model(manifest).model_validate(manifest)
    if manifest.get("schema_version") == 2:
        prefix = assets.release_download_url(manifest["indexes"][0]["packages"][0]["version"], "")
        tag = prefix.split("/download/", 1)[1].split("/", 1)[0]
        if re.fullmatch(r"[A-Za-z0-9._-]+", tag) is None:
            raise ValueError("unsafe native release tag")
        for package in manifest["native"]["packages"]:
            name = package["url"].removeprefix(prefix)
            if (
                not package["url"].startswith(prefix)
                or re.fullmatch(r"[A-Za-z0-9._-]+\.(deb|rpm)", name) is None
            ):
                raise ValueError("native URL must name a safe asset on the declaring release")
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
    for target in assets.TARGETS:
        if target.deb_arch is not None:
            for name in (assets.deb_name(version, target), assets.rpm_name(version, target)):
                entries[name] = digests.digest_file(dist_dir / name)
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
    for target in assets.TARGETS:
        if target.deb_arch is not None:
            for name in (assets.deb_name(version, target), assets.rpm_name(version, target)):
                entries[name] = digests.digest_file(dist_dir / name)
    manifest = build_manifest(version, build_date, entries)
    validate_manifest(manifest)
    out.write_bytes(dump(manifest))


def dump(manifest: dict) -> bytes:
    return (json.dumps(manifest, indent=2, sort_keys=False) + "\n").encode()
