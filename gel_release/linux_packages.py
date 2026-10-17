"""Package prebuilt static Linux binaries with the shared nFPM configuration."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from . import assets, registry_manifest

CONFIG_PATH = Path(__file__).resolve().parent.parent / "packaging/native/gel-cli.nfpm.yaml"


def package_manager_version(version: str) -> str:
    """Full native version, with an epoch that upgrades every legacy build."""
    registry_manifest.release_channel(version)
    return f"1:{version.replace('-', '~', 1)}-1"


def build(target: assets.Target, version: str, completions_dir: Path, out_dir: Path) -> list[Path]:
    if target.deb_arch is None or target.rpm_arch is None:
        raise ValueError(f"{target.triple} does not produce Linux packages")
    registry_manifest.release_channel(version)
    native_version = version.replace("-", "~", 1)
    env = dict(
        os.environ,
        NATIVE_VERSION=native_version,
        NATIVE_ARCH=target.deb_arch,
        NATIVE_BINARY=str((Path("target") / target.triple / "release/gel").resolve()),
        COMPLETIONS_DIR=str(completions_dir.resolve()),
        BUILD_VERSION=os.environ.get("BUILD_VERSION", version),
    )
    out_dir.mkdir(parents=True, exist_ok=True)
    produced = []
    for packager, name in (
        ("deb", assets.deb_name(version, target)),
        ("rpm", assets.rpm_name(version, target)),
    ):
        path = out_dir / name
        subprocess.run(
            [
                "nfpm",
                "package",
                "--config",
                str(CONFIG_PATH),
                "--packager",
                packager,
                "--target",
                str(path),
            ],
            env=env,
            check=True,
        )
        if not path.is_file():
            raise ValueError(f"{name} was not produced")
        produced.append(path)
    return produced
