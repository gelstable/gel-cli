"""Canonical release target and asset naming.

Every release script derives filenames from this module. Nothing else in the
repository hard-codes a release asset name.
"""

from __future__ import annotations

import os
from dataclasses import dataclass

# The repository every release API call targets. GitHub Actions sets
# GITHUB_REPOSITORY to the operating repository, which keeps the same-repo
# enforcement exact while letting any repository run this pipeline against
# itself (a rehearsal scratch repo included). Outside CI this resolves to the
# production repository.
REPOSITORY = os.environ.get("GITHUB_REPOSITORY") or "gelstable/gel-cli"
REGISTRY_BASENAME = "gel-cli"
DIST_BASENAME = "gel"
REGISTRY_MANIFEST_NAME = "gel-registry.json"
SHA256SUMS_NAME = "SHA256SUMS"
BLAKE2B_SUMS_NAME = "BLAKE2B512SUMS"


@dataclass(frozen=True)
class Target:
    triple: str
    runner: str
    exe_suffix: str
    archive_ext: str
    registry: bool
    media_type: str
    arch: str
    deb_arch: str | None
    rpm_arch: str | None


TARGETS: tuple[Target, ...] = (
    Target(
        triple="x86_64-unknown-linux-musl",
        runner="ubuntu-24.04",
        exe_suffix="",
        archive_ext="tar.gz",
        registry=True,
        media_type="application/x-pie-executable",
        arch="x86_64",
        deb_arch="amd64",
        rpm_arch="x86_64",
    ),
    Target(
        triple="aarch64-unknown-linux-musl",
        runner="ubuntu-24.04-arm",
        exe_suffix="",
        archive_ext="tar.gz",
        registry=True,
        media_type="application/x-pie-executable",
        arch="aarch64",
        deb_arch="arm64",
        rpm_arch="aarch64",
    ),
    Target(
        triple="aarch64-apple-darwin",
        runner="macos-15",
        exe_suffix="",
        archive_ext="tar.gz",
        registry=True,
        media_type="application/x-mach-binary",
        arch="aarch64",
        deb_arch=None,
        rpm_arch=None,
    ),
    Target(
        triple="x86_64-apple-darwin",
        runner="macos-15",
        exe_suffix="",
        archive_ext="tar.gz",
        registry=False,
        media_type="application/x-mach-binary",
        arch="x86_64",
        deb_arch=None,
        rpm_arch=None,
    ),
    Target(
        triple="x86_64-pc-windows-msvc",
        runner="windows-2025",
        exe_suffix=".exe",
        archive_ext="zip",
        registry=True,
        media_type="application/x-dosexec",
        arch="x86_64",
        deb_arch=None,
        rpm_arch=None,
    ),
    Target(
        triple="aarch64-pc-windows-msvc",
        runner="windows-11-arm",
        exe_suffix=".exe",
        archive_ext="zip",
        registry=True,
        media_type="application/x-dosexec",
        arch="aarch64",
        deb_arch=None,
        rpm_arch=None,
    ),
)

BY_TRIPLE: dict[str, Target] = {target.triple: target for target in TARGETS}
REGISTRY_TARGETS: tuple[Target, ...] = tuple(t for t in TARGETS if t.registry)


def registry_identity_name(target: Target) -> str:
    return f"{REGISTRY_BASENAME}-{target.triple}{target.exe_suffix}"


def registry_zstd_name(target: Target) -> str:
    return registry_identity_name(target) + ".zst"


def archive_stem(version: str, target: Target) -> str:
    return f"{DIST_BASENAME}-v{version}-{target.triple}"


def archive_name(version: str, target: Target) -> str:
    return f"{archive_stem(version, target)}.{target.archive_ext}"


def deb_name(version: str, target: Target) -> str:
    if target.deb_arch is None:
        raise ValueError(f"{target.triple} does not produce a .deb")
    return f"{DIST_BASENAME}_{version}_{target.deb_arch}.deb"


def rpm_name(version: str, target: Target) -> str:
    if target.rpm_arch is None:
        raise ValueError(f"{target.triple} does not produce an .rpm")
    return f"{DIST_BASENAME}-{version}-1.{target.rpm_arch}.rpm"


def expected_assets(version: str) -> list[str]:
    names: list[str] = []
    for target in REGISTRY_TARGETS:
        names.append(registry_identity_name(target))
        names.append(registry_zstd_name(target))
    for target in TARGETS:
        names.append(archive_name(version, target))
        if target.deb_arch is not None:
            names.append(deb_name(version, target))
        if target.rpm_arch is not None:
            names.append(rpm_name(version, target))
    names.extend([REGISTRY_MANIFEST_NAME, SHA256SUMS_NAME, BLAKE2B_SUMS_NAME])
    return sorted(names)


def release_download_url(version: str, name: str) -> str:
    return f"https://github.com/{REPOSITORY}/releases/download/v{version}/{name}"
