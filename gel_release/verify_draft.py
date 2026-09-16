"""Read the staged candidate back from the GitHub API and verify it.

Draft release assets are not publicly downloadable, so every read goes through
the authenticated assets API. The bytes verified here are the bytes GitHub
stored, not the bytes the build job happened to leave on disk.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from . import assets, candidate, digests, registry_manifest
from .models import GithubAsset

DISTRIBUTION_SUFFIXES = (".tar.gz", ".zip", ".deb", ".rpm")
DIGEST_MANIFEST_NAMES = (assets.SHA256SUMS_NAME, assets.BLAKE2B_SUMS_NAME)


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


def verify_attestations(paths: list[Path], repo: str = assets.REPOSITORY) -> None:
    for path in paths:
        subprocess.run(["gh", "attestation", "verify", str(path), "--repo", repo], check=True)


def check_inventory(listed: list[dict], version: str) -> None:
    names = sorted(entry["name"] for entry in listed)
    expected = assets.expected_assets(version)
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


def verify(
    record: dict,
    download_dir: Path,
    repo: str = assets.REPOSITORY,
    verify_attestations_flag: bool = True,
) -> None:
    version = record["version"]
    listed = list_release_assets(str(record["draft_release_id"]), repo)
    check_inventory(listed, version)

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

    candidate.verify_record(record, version, download_dir)

    for name in DIGEST_MANIFEST_NAMES:
        digests.verify_sums(download_dir, download_dir / name)

    manifest = json.loads((download_dir / assets.REGISTRY_MANIFEST_NAME).read_bytes())
    registry_manifest.validate_manifest(manifest)
    check_manifest_urls(manifest, version)

    for fragment in manifest["indexes"]:
        for package in fragment["packages"]:
            for ref in package["installrefs"]:
                name = ref["ref"].rsplit("/", 1)[-1]
                actual = digests.digest_file(download_dir / name)
                if actual.blake2b512 != ref["verification"]["blake2b"]:
                    raise DraftVerificationError(
                        f"{name}: gel-registry.json BLAKE2b does not match the staged bytes"
                    )
                if actual.size != ref["verification"]["size"]:
                    raise DraftVerificationError(
                        f"{name}: gel-registry.json size does not match the staged bytes"
                    )

    if verify_attestations_flag:
        verify_attestations([download_dir / entry["name"] for entry in record["assets"]], repo)
