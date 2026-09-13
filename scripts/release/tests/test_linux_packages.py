import tomllib
import unittest
from pathlib import Path

from scripts.release import assets, linux_packages

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
        self.assertEqual(
            CARGO["package"]["metadata"]["generate-rpm"]["auto-req"], "no"
        )


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


if __name__ == "__main__":
    unittest.main()
