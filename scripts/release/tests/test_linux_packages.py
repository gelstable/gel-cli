import subprocess
import tempfile
import tomllib
import unittest
import unittest.mock
from pathlib import Path

from gel_release import assets, linux_packages

CARGO = tomllib.loads(Path("Cargo.toml").read_text())
AMD64 = assets.BY_TRIPLE["x86_64-unknown-linux-musl"]
ARM64 = assets.BY_TRIPLE["aarch64-unknown-linux-musl"]


class CargoMetadataTests(unittest.TestCase):
    def test_deb_assets_install_binary_and_completions(self):
        deb = CARGO["package"]["metadata"]["deb"]
        destinations = {row[1] for row in deb["assets"]}
        self.assertIn("usr/bin/", destinations)
        self.assertIn("usr/share/bash-completion/completions/gel", destinations)
        self.assertIn("usr/share/zsh/site-functions/_gel", destinations)
        self.assertIn("usr/share/fish/vendor_completions.d/gel.fish", destinations)
        self.assertEqual(deb["name"], "gel")

    def test_rpm_assets_install_binary_and_completions(self):
        rpm = CARGO["package"]["metadata"]["generate-rpm"]
        destinations = {row["dest"] for row in rpm["assets"]}
        self.assertIn("/usr/bin/gel", destinations)
        self.assertIn("/usr/share/bash-completion/completions/gel", destinations)
        self.assertIn("/usr/share/zsh/site-functions/_gel", destinations)
        self.assertIn("/usr/share/fish/vendor_completions.d/gel.fish", destinations)
        self.assertEqual(rpm["release"], "1")

    def test_rpm_and_deb_do_not_depend_on_a_dynamic_libc(self):
        self.assertEqual(CARGO["package"]["metadata"]["deb"]["depends"], "")
        self.assertEqual(CARGO["package"]["metadata"]["generate-rpm"]["auto-req"], "no")


class CommandTests(unittest.TestCase):
    def test_deb_command_targets_the_prebuilt_binary(self):
        argv = linux_packages.deb_command(AMD64, "7.11.0", Path("target/completions"))
        self.assertEqual(argv[:2], ["cargo", "deb"])
        self.assertIn("--no-build", argv)
        self.assertIn("--target", argv)
        self.assertIn(AMD64.triple, argv)

    def test_rpm_command_uses_matching_arch(self):
        argv = linux_packages.rpm_command(ARM64, "7.11.0")
        self.assertEqual(argv[:2], ["cargo", "generate-rpm"])
        self.assertIn("--target", argv)
        self.assertIn(ARM64.triple, argv)
        self.assertIn("--arch", argv)
        self.assertIn("aarch64", argv)

    def test_non_linux_targets_are_rejected(self):
        with self.assertRaises(ValueError):
            linux_packages.deb_command(
                assets.BY_TRIPLE["aarch64-apple-darwin"], "7.11.0", Path(".")
            )


class NamingTests(unittest.TestCase):
    def test_canonical_output_names(self):
        self.assertEqual(assets.deb_name("7.11.0", AMD64), "gel_7.11.0_amd64.deb")
        self.assertEqual(assets.deb_name("7.11.0", ARM64), "gel_7.11.0_arm64.deb")
        self.assertEqual(assets.rpm_name("7.11.0", AMD64), "gel-7.11.0-1.x86_64.rpm")
        self.assertEqual(assets.rpm_name("7.11.0", ARM64), "gel-7.11.0-1.aarch64.rpm")


class BuildTests(unittest.TestCase):
    def test_placeholder_missing_raises_value_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.toml").write_text("[package]\nname = 'gel'\n")
            with self.assertRaises(ValueError) as ctx:
                linux_packages._manifest_with_target(root, AMD64)
            self.assertEqual(
                str(ctx.exception),
                f"Placeholder {linux_packages.TARGET_PLACEHOLDER} not found in Cargo.toml",
            )

    def test_build_replaces_placeholder_and_rolls_back_on_failure(self):
        manifest_during_run = None
        created_backup = None

        orig_named_temp = tempfile.NamedTemporaryFile

        def spy_named_temp(*args, **kwargs):
            nonlocal created_backup
            f = orig_named_temp(*args, **kwargs)
            created_backup = Path(f.name)
            return f

        def fake_run(cmd, *args, **kwargs):
            nonlocal manifest_during_run
            manifest_during_run = Path("Cargo.toml").read_text()
            raise subprocess.CalledProcessError(1, cmd)

        original_manifest = Path("Cargo.toml").read_text()
        with (
            unittest.mock.patch("tempfile.NamedTemporaryFile", side_effect=spy_named_temp),
            unittest.mock.patch("subprocess.run", side_effect=fake_run),
            tempfile.TemporaryDirectory() as tmp_out,
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                linux_packages.build(AMD64, "7.11.0", Path("target/completions"), Path(tmp_out))

        self.assertIsNotNone(manifest_during_run)
        self.assertIn(AMD64.triple, manifest_during_run)
        self.assertNotIn(linux_packages.TARGET_PLACEHOLDER, manifest_during_run)
        self.assertEqual(Path("Cargo.toml").read_text(), original_manifest)
        self.assertIsNotNone(created_backup)
        self.assertFalse(created_backup.exists())

    def test_build_success_restores_manifest_and_cleans_backup(self):
        manifest_during_run = None
        created_backup = None

        orig_named_temp = tempfile.NamedTemporaryFile

        def spy_named_temp(*args, **kwargs):
            nonlocal created_backup
            f = orig_named_temp(*args, **kwargs)
            created_backup = Path(f.name)
            return f

        def fake_run(cmd, *args, **kwargs):
            nonlocal manifest_during_run
            manifest_during_run = Path("Cargo.toml").read_text()
            dist = Path("dist")
            dist.mkdir(parents=True, exist_ok=True)
            (dist / assets.deb_name("7.11.0", AMD64)).write_text("dummy deb")
            (dist / assets.rpm_name("7.11.0", AMD64)).write_text("dummy rpm")

        original_manifest = Path("Cargo.toml").read_text()
        with (
            unittest.mock.patch("tempfile.NamedTemporaryFile", side_effect=spy_named_temp),
            unittest.mock.patch("subprocess.run", side_effect=fake_run),
            tempfile.TemporaryDirectory() as tmp_out,
        ):
            out_dir = Path(tmp_out)
            produced = linux_packages.build(AMD64, "7.11.0", Path("target/completions"), out_dir)
            self.assertEqual(len(produced), 2)
            self.assertEqual(produced[0], out_dir / assets.deb_name("7.11.0", AMD64))
            self.assertEqual(produced[1], out_dir / assets.rpm_name("7.11.0", AMD64))
            self.assertTrue(all(p.is_file() for p in produced))

        self.assertIsNotNone(manifest_during_run)
        self.assertIn(AMD64.triple, manifest_during_run)
        self.assertNotIn(linux_packages.TARGET_PLACEHOLDER, manifest_during_run)
        self.assertEqual(Path("Cargo.toml").read_text(), original_manifest)
        self.assertIsNotNone(created_backup)
        self.assertFalse(created_backup.exists())


if __name__ == "__main__":
    unittest.main()
