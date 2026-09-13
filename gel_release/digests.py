"""Byte digests and digest manifests for release assets."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path

from .assets import BLAKE2B_SUMS_NAME, SHA256SUMS_NAME

CHUNK_SIZE = 1024 * 1024


class DigestMismatch(ValueError):
    """A file on disk does not match its recorded digest."""


@dataclass(frozen=True)
class FileDigest:
    size: int
    sha256: str
    blake2b512: str


def digest_file(path: Path) -> FileDigest:
    sha256 = hashlib.sha256()
    blake2b = hashlib.blake2b(digest_size=64)
    size = 0
    with open(path, "rb") as handle:
        while True:
            chunk = handle.read(CHUNK_SIZE)
            if not chunk:
                break
            size += len(chunk)
            sha256.update(chunk)
            blake2b.update(chunk)
    return FileDigest(size=size, sha256=sha256.hexdigest(), blake2b512=blake2b.hexdigest())


def _select(digest: FileDigest, algorithm: str) -> str:
    if algorithm == "sha256":
        return digest.sha256
    if algorithm == "blake2b512":
        return digest.blake2b512
    raise ValueError(f"unknown algorithm {algorithm!r}")


def write_sums(directory: Path, algorithm: str, out_path: Path, skip: set[str]) -> None:
    lines = []
    for path in sorted(p for p in directory.iterdir() if p.is_file()):
        if path.name in skip:
            continue
        lines.append(f"{_select(digest_file(path), algorithm)}  {path.name}\n")
    out_path.write_bytes("".join(lines).encode())


def verify_sums(directory: Path, sums_path: Path) -> None:
    if sums_path.name == SHA256SUMS_NAME:
        algorithm = "sha256"
    elif sums_path.name == BLAKE2B_SUMS_NAME:
        algorithm = "blake2b512"
    else:
        algorithm = "sha256" if sums_path.name == "SHA256SUMS" else "blake2b512"
    for line in sums_path.read_bytes().decode().splitlines():
        expected, name = line.split("  ", 1)
        target = directory / name
        if not target.is_file():
            raise DigestMismatch(f"{name} listed in {sums_path.name} is missing")
        actual = _select(digest_file(target), algorithm)
        if actual != expected:
            raise DigestMismatch(f"{name}: expected {algorithm} {expected}, computed {actual}")
