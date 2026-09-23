"""Single command-line entry point for repository-owned release packaging."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

from pydantic import ValidationError

from . import assets, linux_packages, package_target, registry_manifest


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="gel-release")
    commands = parser.add_subparsers(dest="command", required=True)
    matrix = commands.add_parser("matrix")
    matrix.add_argument("kind", choices=("build", "smoke"))
    channel = commands.add_parser("channel")
    channel.add_argument("--version", required=True)
    completions = commands.add_parser("completions")
    completions.add_argument("--binary", required=True, type=Path)
    completions.add_argument("--out-dir", required=True, type=Path)
    package = commands.add_parser("package-target")
    for name in ("target", "version", "binary", "completions-dir", "out-dir"):
        package.add_argument(
            f"--{name}",
            required=True,
            type=Path if name.endswith("dir") or name == "binary" else str,
        )
    package.add_argument("--repo-root", default=Path("."), type=Path)
    linux = commands.add_parser("linux-packages")
    linux.add_argument("--target", required=True)
    linux.add_argument("--version", required=True)
    linux.add_argument("--completions-dir", required=True, type=Path)
    linux.add_argument("--out-dir", required=True, type=Path)
    stage = commands.add_parser("assemble-stage")
    stage.add_argument("--version", required=True)
    stage.add_argument("--build-date", required=True)
    stage.add_argument("--dist-dir", required=True, type=Path)
    manifest = commands.add_parser("registry-manifest")
    manifest.add_argument("--version", required=True)
    manifest.add_argument("--build-date", required=True)
    manifest.add_argument("--dist-dir", required=True, type=Path)
    manifest.add_argument("--out", required=True, type=Path)
    return parser


def _matrix(kind: str) -> dict[str, list[dict[str, object]]]:
    targets = assets.TARGETS if kind == "build" else assets.REGISTRY_TARGETS
    return {
        "include": [
            {
                "target": t.triple,
                "runner": t.runner,
                **({"linux_packages": t.deb_arch is not None} if kind == "build" else {}),
            }
            for t in targets
        ]
    }


def main(argv: list[str] | None = None) -> int:
    try:
        args = _parser().parse_args(argv)
        if args.command == "matrix":
            print(json.dumps(_matrix(args.kind), separators=(",", ":")))
        elif args.command == "channel":
            print(registry_manifest.release_channel(args.version))
        elif args.command == "completions":
            package_target.generate_completions(args.binary, args.out_dir)
        elif args.command == "package-target":
            target = assets.BY_TRIPLE[args.target]
            if target.registry:
                for path in package_target.build_registry_payload(
                    args.binary, target, args.out_dir
                ):
                    print(path.name)
            archive = package_target.build_archive(
                args.binary,
                target,
                args.version,
                args.completions_dir,
                [args.repo_root / n for n in package_target.ARCHIVE_EXTRA_FILES],
                args.out_dir,
            )
            print(archive.name)
        elif args.command == "linux-packages":
            for path in linux_packages.build(
                assets.BY_TRIPLE[args.target], args.version, args.completions_dir, args.out_dir
            ):
                print(path.name)
        elif args.command == "assemble-stage":
            registry_manifest.assemble_stage(args.version, args.build_date, args.dist_dir)
            print(f"assembled {len(assets.expected_assets(args.version))} release assets")
        elif args.command == "registry-manifest":
            registry_manifest.write_manifest(args.version, args.build_date, args.dist_dir, args.out)
            print(f"wrote {args.out}")
        return 0
    except (KeyError, OSError, ValueError, ValidationError, subprocess.CalledProcessError) as error:
        print(f"validation error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
