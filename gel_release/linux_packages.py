"""Build the standalone .deb and .rpm packages for one Linux target.

Both tools package an already-built binary. Neither recompiles: the release
matrix has already produced target/<triple>/release/gel and the completions.
"""

from __future__ import annotations

import shutil
import subprocess
import tempfile
from pathlib import Path

from . import assets

TARGET_PLACEHOLDER = "RELEASE_TARGET"


def _require_linux(target: assets.Target) -> None:
    if target.deb_arch is None or target.rpm_arch is None:
        raise ValueError(f"{target.triple} does not produce Linux packages")


def deb_command(target: assets.Target, version: str, completions_dir: Path) -> list[str]:
    _require_linux(target)
    return [
        "cargo",
        "deb",
        "--no-build",
        "--no-strip",
        "--target",
        target.triple,
        "--output",
        str(Path("dist") / assets.deb_name(version, target)),
    ]


def rpm_command(target: assets.Target, version: str) -> list[str]:
    _require_linux(target)
    return [
        "cargo",
        "generate-rpm",
        "--target",
        target.triple,
        "--arch",
        target.rpm_arch,
        "--output",
        str(Path("dist") / assets.rpm_name(version, target)),
    ]


def _manifest_with_target(repo_root: Path, target: assets.Target) -> None:
    """Rewrite Cargo.toml in place so asset sources point at this target's dir."""
    manifest = repo_root / "Cargo.toml"
    text = manifest.read_text()
    if TARGET_PLACEHOLDER not in text:
        raise ValueError(f"Placeholder {TARGET_PLACEHOLDER} not found in Cargo.toml")
    manifest.write_text(text.replace(TARGET_PLACEHOLDER, target.triple))


def build(target: assets.Target, version: str, completions_dir: Path, out_dir: Path) -> list[Path]:
    _require_linux(target)
    repo_root = Path.cwd()
    out_dir.mkdir(parents=True, exist_ok=True)

    original = (repo_root / "Cargo.toml").read_text()
    backup = tempfile.NamedTemporaryFile("w", delete=False, suffix=".toml")
    backup.write(original)
    backup.close()
    try:
        _manifest_with_target(repo_root, target)
        subprocess.run(deb_command(target, version, completions_dir), check=True)
        subprocess.run(rpm_command(target, version), check=True)
    finally:
        (repo_root / "Cargo.toml").write_text(original)
        Path(backup.name).unlink()

    produced = []
    for name in (assets.deb_name(version, target), assets.rpm_name(version, target)):
        path = Path("dist") / name
        if not path.is_file():
            raise SystemExit(f"{name} was not produced")
        final = out_dir / name
        if path.resolve() != final.resolve():
            shutil.move(str(path), str(final))
        produced.append(final)
    return produced
