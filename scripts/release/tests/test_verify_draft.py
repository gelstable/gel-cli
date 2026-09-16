import unittest

from gel_release import assets, registry_manifest, verify_draft


def _manifest(version: str) -> dict:
    from gel_release import digests

    entries = {}
    for index, target in enumerate(assets.REGISTRY_TARGETS):
        entries[assets.registry_identity_name(target)] = digests.FileDigest(
            size=10 + index, sha256=f"{index:064x}", blake2b512=f"{index:0128x}"
        )
        entries[assets.registry_zstd_name(target)] = digests.FileDigest(
            size=5 + index, sha256=f"{index + 9:064x}", blake2b512=f"{index + 9:0128x}"
        )
    return registry_manifest.build_manifest(version, "2026-09-12T00:00:00+00:00", entries)


class ManifestUrlTests(unittest.TestCase):
    def test_public_release_urls_pass(self):
        verify_draft.check_manifest_urls(_manifest("7.11.0"), "7.11.0")

    def test_api_urls_are_rejected(self):
        manifest = _manifest("7.11.0")
        manifest["indexes"][0]["packages"][0]["installrefs"][0]["ref"] = (
            "https://api.github.com/repos/gelstable/gel-cli/releases/assets/1"
        )
        with self.assertRaises(verify_draft.DraftVerificationError):
            verify_draft.check_manifest_urls(manifest, "7.11.0")

    def test_wrong_version_in_url_is_rejected(self):
        manifest = _manifest("7.11.0")
        with self.assertRaises(verify_draft.DraftVerificationError):
            verify_draft.check_manifest_urls(manifest, "7.12.0")

    def test_distribution_asset_reference_is_rejected(self):
        manifest = _manifest("7.11.0")
        manifest["indexes"][0]["packages"][0]["installrefs"][0]["ref"] = (
            assets.release_download_url("7.11.0", "gel-v7.11.0-x86_64-unknown-linux-musl.tar.gz")
        )
        with self.assertRaises(verify_draft.DraftVerificationError) as raised:
            verify_draft.check_manifest_urls(manifest, "7.11.0")
        self.assertIn("isolation", str(raised.exception).lower())


class InventoryTests(unittest.TestCase):
    def test_unexpected_release_asset_is_rejected(self):
        listed = [
            {"id": i, "name": name, "size": 1}
            for i, name in enumerate(assets.expected_assets("7.11.0"))
        ]
        listed.append({"id": 999, "name": "leftover.bin", "size": 1})
        with self.assertRaises(verify_draft.DraftVerificationError):
            verify_draft.check_inventory(listed, "7.11.0")

    def test_missing_release_asset_is_rejected(self):
        listed = [
            {"id": i, "name": name, "size": 1}
            for i, name in enumerate(assets.expected_assets("7.11.0"))
        ][:-1]
        with self.assertRaises(verify_draft.DraftVerificationError):
            verify_draft.check_inventory(listed, "7.11.0")

    def test_exact_inventory_passes(self):
        listed = [
            {"id": i, "name": name, "size": 1}
            for i, name in enumerate(assets.expected_assets("7.11.0"))
        ]
        verify_draft.check_inventory(listed, "7.11.0")


if __name__ == "__main__":
    unittest.main()
