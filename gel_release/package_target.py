"""Package one built binary into its registry payload and distribution archive."""

from __future__ import annotations

import gzip
import shutil
import stat
import subprocess
import tarfile
import zipfile
from pathlib import Path

from . import assets

ARCHIVE_EXTRA_FILES = ("README.md", "LICENSE-APACHE", "LICENSE-MIT")
COMPLETIONS = (
    ("bash", "gel.bash"),
    ("zsh", "_gel"),
    ("fish", "gel.fish"),
    ("power-shell", "gel.ps1"),
)
ZIP_DATE_TIME = (1980, 1, 1, 0, 0, 0)


def generate_completions(host_binary: Path, out_dir: Path) -> None:
    """Run the freshly built host binary once per shell.

    Completion scripts do not vary by target, so a single host build supplies
    the completions embedded in every archive and Linux package.
    """
    out_dir.mkdir(parents=True, exist_ok=True)
    for shell, filename in COMPLETIONS:
        completed = subprocess.run(
            [str(host_binary), "_gen_completions", f"--shell={shell}"],
            check=True,
            capture_output=True,
        )
        (out_dir / filename).write_bytes(completed.stdout)


def build_registry_payload(binary: Path, target: assets.Target, out_dir: Path) -> list[Path]:
    if not target.registry:
        raise ValueError(f"{target.triple} is a distribution-only target")
    out_dir.mkdir(parents=True, exist_ok=True)

    identity = out_dir / assets.registry_identity_name(target)
    shutil.copyfile(binary, identity)
    identity.chmod(0o755)

    compressed = out_dir / assets.registry_zstd_name(target)
    subprocess.run(
        [
            "zstd",
            "--quiet",
            "--force",
            "--ultra",
            "-19",
            str(identity),
            "-o",
            str(compressed),
        ],
        check=True,
    )
    return [identity, compressed]


def _archive_entries(
    binary: Path,
    target: assets.Target,
    version: str,
    completions_dir: Path,
    extra_files: list[Path],
) -> list[tuple[str, Path, int]]:
    root = assets.archive_stem(version, target)
    entries: list[tuple[str, Path, int]] = [
        (f"{root}/{assets.DIST_BASENAME}{target.exe_suffix}", binary, 0o755)
    ]
    for path in extra_files:
        entries.append((f"{root}/{path.name}", path, 0o644))
    for _, filename in COMPLETIONS:
        entries.append((f"{root}/completions/{filename}", completions_dir / filename, 0o644))
    return sorted(entries, key=lambda entry: entry[0])


def build_archive(
    binary: Path,
    target: assets.Target,
    version: str,
    completions_dir: Path,
    extra_files: list[Path],
    out_dir: Path,
) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    archive = out_dir / assets.archive_name(version, target)
    entries = _archive_entries(binary, target, version, completions_dir, extra_files)

    if target.archive_ext == "tar.gz":
        raw = out_dir / (assets.archive_stem(version, target) + ".tar")
        try:
            with tarfile.open(raw, "w", format=tarfile.GNU_FORMAT) as tar:
                for name, source, mode in entries:
                    info = tar.gettarinfo(str(source), arcname=name)
                    info.mode = mode
                    info.mtime = 0
                    info.uid = 0
                    info.gid = 0
                    info.uname = ""
                    info.gname = ""
                    with open(source, "rb") as handle:
                        tar.addfile(info, handle)
            with open(raw, "rb") as plain, open(archive, "wb") as out:
                with gzip.GzipFile(fileobj=out, mode="wb", compresslevel=9, mtime=0) as gz:
                    shutil.copyfileobj(plain, gz)
        finally:
            raw.unlink(missing_ok=True)
        return archive
    elif target.archive_ext == "zip":
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zf:
            for name, source, mode in entries:
                info = zipfile.ZipInfo(filename=name, date_time=ZIP_DATE_TIME)
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = ((mode & 0o7777) | stat.S_IFREG) << 16
                info.create_system = 3
                zf.writestr(info, source.read_bytes())
        return archive
    else:
        raise ValueError(f"unsupported archive extension: {target.archive_ext}")
