import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile
from pathlib import Path

from scripts.release import assets, package_target


def _fake_tree(root: Path) -> tuple[Path, Path, list[Path]]:
    binary = root / "gel"
    binary.write_bytes(b"\x7fELF fake binary payload")
    binary.chmod(0o755)

    completions = root / "completions"
    completions.mkdir()
    for name in ("gel.bash", "_gel", "gel.fish", "gel.ps1"):
        (completions / name).write_text(f"# {name}\n")

    extras = []
    for name in package_target.ARCHIVE_EXTRA_FILES:
        path = root / name
        path.write_text(f"{name} contents\n")
        extras.append(path)
    return binary, completions, extras


@unittest.skipIf(shutil.which("zstd") is None, "zstd is required")
class RegistryPayloadTests(unittest.TestCase):
    def test_identity_and_zstd_round_trip(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary, _, _ = _fake_tree(root)
            out = root / "dist"
            out.mkdir()
            target = assets.BY_TRIPLE["x86_64-unknown-linux-musl"]

            produced = package_target.build_registry_payload(binary, target, out)

            self.assertEqual(
                [p.name for p in produced],
                [
                    "gel-cli-x86_64-unknown-linux-musl",
                    "gel-cli-x86_64-unknown-linux-musl.zst",
                ],
            )
            self.assertEqual(produced[0].read_bytes(), binary.read_bytes())
            self.assertEqual(produced[0].stat().st_mode & 0o777, 0o755)
            decoded = subprocess.run(
                ["zstd", "--decompress", "--stdout", str(produced[1])],
                check=True,
                capture_output=True,
            ).stdout
            self.assertEqual(decoded, binary.read_bytes())

    def test_distribution_only_target_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary, _, _ = _fake_tree(root)
            with self.assertRaises(ValueError):
                package_target.build_registry_payload(
                    binary, assets.BY_TRIPLE["x86_64-apple-darwin"], root
                )


class ArchiveTests(unittest.TestCase):
    def _build(self, triple: str) -> tuple[Path, Path]:
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root)
        binary, completions, extras = _fake_tree(root)
        out = root / "dist"
        out.mkdir()
        archive = package_target.build_archive(
            binary, assets.BY_TRIPLE[triple], "7.11.0", completions, extras, out
        )
        return archive, root

    def test_tar_layout_and_permissions(self):
        archive, _ = self._build("x86_64-unknown-linux-musl")
        self.assertEqual(archive.name, "gel-v7.11.0-x86_64-unknown-linux-musl.tar.gz")
        with tarfile.open(archive) as tar:
            names = tar.getnames()
            self.assertEqual(names, sorted(names))
            self.assertIn("gel-v7.11.0-x86_64-unknown-linux-musl/gel", names)
            self.assertIn(
                "gel-v7.11.0-x86_64-unknown-linux-musl/completions/gel.bash", names
            )
            member = tar.getmember("gel-v7.11.0-x86_64-unknown-linux-musl/gel")
            self.assertEqual(member.mode & 0o777, 0o755)
            self.assertEqual(member.mtime, 0)
            self.assertEqual(member.uid, 0)
            self.assertEqual(member.uname, "")

    def test_zip_layout_uses_exe_name(self):
        archive, _ = self._build("x86_64-pc-windows-msvc")
        self.assertEqual(archive.name, "gel-v7.11.0-x86_64-pc-windows-msvc.zip")
        with zipfile.ZipFile(archive) as zf:
            names = zf.namelist()
            self.assertEqual(names, sorted(names))
            self.assertIn("gel-v7.11.0-x86_64-pc-windows-msvc/gel.exe", names)
            info = zf.getinfo("gel-v7.11.0-x86_64-pc-windows-msvc/gel.exe")
            self.assertEqual(info.date_time, (1980, 1, 1, 0, 0, 0))
            self.assertEqual((info.external_attr >> 16) & stat.S_IFREG, stat.S_IFREG)
            self.assertEqual((info.external_attr >> 16) & 0o777, 0o755)

            comp_info = zf.getinfo("gel-v7.11.0-x86_64-pc-windows-msvc/completions/gel.bash")
            self.assertEqual((comp_info.external_attr >> 16) & stat.S_IFREG, stat.S_IFREG)
            self.assertEqual((comp_info.external_attr >> 16) & 0o777, 0o644)

    def test_archives_are_byte_deterministic(self):
        first, _ = self._build("aarch64-apple-darwin")
        second, _ = self._build("aarch64-apple-darwin")
        self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_raw_tar_unlinked_on_gzip_failure(self):
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root)
        binary, completions, extras = _fake_tree(root)
        out = root / "dist"
        out.mkdir()
        target = assets.BY_TRIPLE["x86_64-unknown-linux-musl"]
        raw = out / (assets.archive_stem("7.11.0", target) + ".tar")

        with mock.patch("gzip.GzipFile", side_effect=RuntimeError("gzip failed")):
            with self.assertRaises(RuntimeError):
                package_target.build_archive(
                    binary, target, "7.11.0", completions, extras, out
                )
        self.assertFalse(raw.exists(), "raw tar must be unlinked even if gzipping fails")

    def test_unsupported_archive_extension_raises(self):
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root)
        binary, completions, extras = _fake_tree(root)
        out = root / "dist"
        out.mkdir()
        bogus_target = assets.Target(
            triple="bogus-target",
            runner="bogus",
            exe_suffix="",
            archive_ext="rar",
            registry=False,
            media_type="application/octet-stream",
            arch="bogus",
            deb_arch=None,
            rpm_arch=None,
        )
        with self.assertRaises(ValueError):
            package_target.build_archive(
                binary, bogus_target, "7.11.0", completions, extras, out
            )


if __name__ == "__main__":
    unittest.main()
