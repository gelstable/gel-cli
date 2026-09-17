import json
import unittest
from pathlib import Path
from typing import Literal, get_type_hints

from gel_release import assets, digests, registry_manifest

FIXTURE = Path("tests/fixtures/registry/release-manifest/gel-registry.json")


def _entries(version: str) -> dict[str, digests.FileDigest]:
    out: dict[str, digests.FileDigest] = {}
    for index, target in enumerate(assets.REGISTRY_TARGETS):
        identity = assets.registry_identity_name(target)
        compressed = assets.registry_zstd_name(target)
        out[identity] = digests.FileDigest(
            size=1000 + index, sha256=f"{index:064x}", blake2b512=f"{index:0128x}"
        )
        out[compressed] = digests.FileDigest(
            size=500 + index, sha256=f"{index + 10:064x}", blake2b512=f"{index + 10:0128x}"
        )
    return out


class ManifestShapeTests(unittest.TestCase):
    def setUp(self):
        self.manifest = registry_manifest.build_manifest(
            "1.2.3", "2026-09-12T00:00:00+00:00", _entries("1.2.3")
        )

    def test_validates_against_vendored_schema(self):
        registry_manifest.validate_manifest(self.manifest)

    def test_one_stable_fragment_per_registry_target(self):
        self.assertEqual(self.manifest["schema_version"], 1)
        self.assertEqual(len(self.manifest["indexes"]), 5)
        self.assertEqual(
            [fragment["platform"] for fragment in self.manifest["indexes"]],
            [target.triple for target in assets.REGISTRY_TARGETS],
        )
        for fragment in self.manifest["indexes"]:
            self.assertEqual(fragment["channel"], "stable")
            self.assertEqual(len(fragment["packages"]), 1)

    def test_installrefs_are_identity_then_zstd(self):
        package = self.manifest["indexes"][0]["packages"][0]
        self.assertEqual([ref["encoding"] for ref in package["installrefs"]], ["identity", "zstd"])
        self.assertTrue(
            package["installrefs"][0]["ref"].startswith(
                "https://github.com/gelstable/gel-cli/releases/download/v1.2.3/"
            )
        )
        self.assertTrue(package["installrefs"][1]["ref"].endswith(".zst"))
        self.assertEqual(package["installrefs"][0]["type"], "application/x-pie-executable")

    def test_verification_digests_are_bare_hex(self):
        verification = self.manifest["indexes"][0]["packages"][0]["installrefs"][0]["verification"]
        self.assertEqual(len(verification["blake2b"]), 128)
        self.assertNotIn(":", verification["blake2b"])
        self.assertEqual(len(verification["sha256"]), 64)
        self.assertGreater(verification["size"], 0)

    def test_package_identity_fields(self):
        package = self.manifest["indexes"][0]["packages"][0]
        self.assertEqual(package["basename"], "gel-cli")
        self.assertEqual(package["name"], "gel-cli")
        self.assertEqual(package["version"], "1.2.3")
        self.assertEqual(package["version_key"], "1.2.3")
        self.assertEqual(package["revision"], "1")
        self.assertEqual(package["slot"], "")
        self.assertEqual(package["tags"], {})
        self.assertEqual(package["architecture"], "x86_64")
        self.assertEqual(
            package["version_details"],
            {"major": 1, "minor": 2, "patch": 3, "prerelease": [], "metadata": {}},
        )

    def test_version_details_handles_testing_prereleases(self):
        self.assertEqual(
            registry_manifest._version_details("7.11.0-rc.1"),
            {
                "major": 7,
                "minor": 11,
                "patch": 0,
                "prerelease": [{"phase": "rc", "number": 1}],
                "metadata": {},
            },
        )
        self.assertEqual(
            registry_manifest._version_details("1.0.0"),
            {"major": 1, "minor": 0, "patch": 0, "prerelease": [], "metadata": {}},
        )

    def test_build_manifest_with_prerelease_version_validates(self):
        manifest = registry_manifest.build_manifest(
            "7.11.0-rc.1", "2026-09-12T00:00:00+00:00", _entries("7.11.0-rc.1")
        )
        registry_manifest.validate_manifest(manifest)
        package = manifest["indexes"][0]["packages"][0]
        self.assertEqual(package["version"], "7.11.0-rc.1")
        self.assertEqual(package["version_key"], "7.11.0-rc.1")
        self.assertEqual(
            package["version_details"],
            {
                "major": 7,
                "minor": 11,
                "patch": 0,
                "prerelease": [{"phase": "rc", "number": 1}],
                "metadata": {},
            },
        )
        self.assertEqual(manifest["indexes"][0]["channel"], "testing")

    def test_dev_manifest_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "unsupported release version"):
            registry_manifest.build_manifest(
                "7.11.0-dev.4121", "2026-09-12T00:00:00+00:00", _entries("7.11.0-dev.4121")
            )

    def test_release_channel_accepts_only_supported_semver_phases(self):
        self.assertEqual(
            get_type_hints(registry_manifest.release_channel)["return"],
            Literal["stable", "testing"],
        )
        for version, expected in (
            ("7.1.0", "stable"),
            ("7.1.0-alpha.1", "testing"),
            ("7.1.0-beta.1", "testing"),
            ("7.1.0-rc.1", "testing"),
        ):
            with self.subTest(version=version):
                self.assertEqual(registry_manifest.release_channel(version), expected)

        for version in (
            "7.1.0-dev.1",
            "7.1.0-preview.1",
            "7.1.0-foo.1",
            "7.1.0+build.123",
            "7.1.0-alpha.0",
            "7.1.0-beta.0",
            "7.1.0-rc.0",
            "7.1.0-alpha",
            "7.1.0-rc.1.2",
        ):
            with self.subTest(version=version):
                with self.assertRaisesRegex(ValueError, "unsupported release version"):
                    registry_manifest.release_channel(version)

    def test_testing_manifest_marks_each_index_with_explicit_testing_channel(self):
        manifest = registry_manifest.build_manifest(
            "7.1.0-beta.1", "2026-09-12T00:00:00+00:00", _entries("7.1.0-beta.1")
        )
        registry_manifest.validate_manifest(manifest)
        self.assertEqual(
            {index["channel"] for index in manifest["indexes"]},
            {"testing"},
        )
        # The consumer selects this explicit field; it does not infer a channel
        # from the package version when a manifest is reviewed or promoted.
        reviewed = json.loads(registry_manifest.dump(manifest))
        reviewed["indexes"][0]["channel"] = "stable"
        registry_manifest.validate_manifest(reviewed)


class IsolationTests(unittest.TestCase):
    def test_distribution_assets_are_rejected(self):
        entries = _entries("1.2.3")
        entries["gel-v1.2.3-x86_64-unknown-linux-musl.tar.gz"] = digests.FileDigest(
            size=1, sha256="0" * 64, blake2b512="0" * 128
        )
        with self.assertRaises(ValueError):
            registry_manifest.build_manifest("1.2.3", "2026-09-12T00:00:00+00:00", entries)

    def test_missing_registry_asset_is_rejected(self):
        entries = _entries("1.2.3")
        del entries["gel-cli-aarch64-apple-darwin.zst"]
        with self.assertRaises(KeyError):
            registry_manifest.build_manifest("1.2.3", "2026-09-12T00:00:00+00:00", entries)

    def test_no_distribution_name_appears_anywhere_in_the_document(self):
        rendered = registry_manifest.dump(
            registry_manifest.build_manifest(
                "1.2.3", "2026-09-12T00:00:00+00:00", _entries("1.2.3")
            )
        ).decode()
        for suffix in (".tar.gz", ".zip", ".deb", ".rpm", "SHA256SUMS", "BLAKE2B512SUMS"):
            self.assertNotIn(suffix, rendered)


class LegacyReplacementsTests(unittest.TestCase):
    """Published v7.10.x manifests use replacements instead of indexes."""

    def manifest(self) -> dict:
        return {
            "schema_version": 1,
            "indexes": [],
            "replacements": [
                {
                    "sha256": f"{index:064x}",
                    "url": (
                        "https://github.com/gelstable/gel-cli/releases/download/v7.10.2/"
                        f"gel-cli-target-{index}"
                    ),
                }
                for index in range(2)
            ],
        }

    def test_published_v7_replacements_manifest_validates(self):
        manifest = self.manifest()
        registry_manifest.validate_manifest(manifest)
        from gel_release.models import ReleaseManifest

        ReleaseManifest.model_validate(manifest)

    def test_replacements_without_indexes_array_validates(self):
        manifest = self.manifest()
        del manifest["indexes"]
        registry_manifest.validate_manifest(manifest)

    def test_empty_manifest_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "indexes or replacements"):
            registry_manifest.validate_manifest({"schema_version": 1})


class GoldenFixtureTests(unittest.TestCase):
    def test_committed_fixture_matches_generator_output(self):
        rendered = registry_manifest.dump(
            registry_manifest.build_manifest(
                "1.2.3", "2026-09-12T00:00:00+00:00", _entries("1.2.3")
            )
        )
        self.assertEqual(
            FIXTURE.read_bytes(),
            rendered,
            "regenerate with: uv run --frozen python -m scripts.release.tests.regenerate_fixture",
        )

    def test_fixture_validates(self):
        registry_manifest.validate_manifest(json.loads(FIXTURE.read_bytes()))


if __name__ == "__main__":
    unittest.main()
