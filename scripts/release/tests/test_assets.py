import unittest

from gel_release import assets


class TargetInventoryTests(unittest.TestCase):
    def test_five_registry_targets(self):
        self.assertEqual(
            [target.triple for target in assets.REGISTRY_TARGETS],
            [
                "x86_64-unknown-linux-musl",
                "aarch64-unknown-linux-musl",
                "aarch64-apple-darwin",
                "x86_64-pc-windows-msvc",
                "aarch64-pc-windows-msvc",
            ],
        )

    def test_intel_macos_is_distribution_only(self):
        target = assets.BY_TRIPLE["x86_64-apple-darwin"]
        self.assertFalse(target.registry)
        self.assertNotIn(target, assets.REGISTRY_TARGETS)


class AssetNameTests(unittest.TestCase):
    def test_unix_registry_names(self):
        target = assets.BY_TRIPLE["aarch64-apple-darwin"]
        self.assertEqual(assets.registry_identity_name(target), "gel-cli-aarch64-apple-darwin")
        self.assertEqual(assets.registry_zstd_name(target), "gel-cli-aarch64-apple-darwin.zst")

    def test_windows_registry_names_keep_exe_suffix(self):
        target = assets.BY_TRIPLE["x86_64-pc-windows-msvc"]
        self.assertEqual(
            assets.registry_identity_name(target), "gel-cli-x86_64-pc-windows-msvc.exe"
        )
        self.assertEqual(
            assets.registry_zstd_name(target),
            "gel-cli-x86_64-pc-windows-msvc.exe.zst",
        )

    def test_archive_names(self):
        self.assertEqual(
            assets.archive_name("7.11.0", assets.BY_TRIPLE["x86_64-unknown-linux-musl"]),
            "gel-v7.11.0-x86_64-unknown-linux-musl.tar.gz",
        )
        self.assertEqual(
            assets.archive_name("7.11.0", assets.BY_TRIPLE["aarch64-pc-windows-msvc"]),
            "gel-v7.11.0-aarch64-pc-windows-msvc.zip",
        )

    def test_linux_package_names(self):
        self.assertEqual(
            assets.deb_name("7.11.0", assets.BY_TRIPLE["x86_64-unknown-linux-musl"]),
            "gel_7.11.0_amd64.deb",
        )
        self.assertEqual(
            assets.rpm_name("7.11.0", assets.BY_TRIPLE["aarch64-unknown-linux-musl"]),
            "gel-7.11.0-1.aarch64.rpm",
        )

    def test_download_url(self):
        self.assertEqual(
            assets.release_download_url("7.11.0", "SHA256SUMS"),
            "https://github.com/gelstable/gel-cli/releases/download/v7.11.0/SHA256SUMS",
        )


class ExpectedInventoryTests(unittest.TestCase):
    def test_inventory_is_exactly_twenty_three_sorted_names(self):
        names = assets.expected_assets("7.11.0")
        self.assertEqual(names, sorted(names))
        self.assertEqual(len(names), 23)
        self.assertEqual(len(set(names)), 23)

    def test_inventory_contents(self):
        names = set(assets.expected_assets("7.11.0"))
        self.assertEqual(
            len([n for n in names if n.startswith("gel-cli-") and not n.endswith(".zst")]),
            5,
        )
        self.assertEqual(len([n for n in names if n.endswith(".zst")]), 5)
        self.assertEqual(len([n for n in names if n.endswith(".tar.gz")]), 4)
        self.assertEqual(len([n for n in names if n.endswith(".zip")]), 2)
        self.assertEqual(len([n for n in names if n.endswith(".deb")]), 2)
        self.assertEqual(len([n for n in names if n.endswith(".rpm")]), 2)
        self.assertIn("gel-registry.json", names)
        self.assertIn("SHA256SUMS", names)
        self.assertIn("BLAKE2B512SUMS", names)


if __name__ == "__main__":
    unittest.main()
