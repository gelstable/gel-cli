import tempfile
import unittest
from pathlib import Path
from unittest import mock

from gel_release import assets, digests, registry_manifest, verify_draft


def _manifest(version: str) -> dict:
    entries = {}
    for index, target in enumerate(assets.REGISTRY_TARGETS):
        entries[assets.registry_identity_name(target)] = digests.FileDigest(
            size=10 + index, sha256=f"{index:064x}", blake2b512=f"{index:0128x}"
        )
        entries[assets.registry_zstd_name(target)] = digests.FileDigest(
            size=5 + index, sha256=f"{index + 9:064x}", blake2b512=f"{index + 9:0128x}"
        )
    return registry_manifest.build_manifest(version, "2026-09-12T00:00:00+00:00", entries)


def _manifest_with_downloads(root: Path, version: str = "7.11.0") -> dict:
    manifest = _manifest(version)
    for fragment in manifest["indexes"]:
        for package in fragment["packages"]:
            for ref in package["installrefs"]:
                path = root / ref["ref"].rsplit("/", 1)[-1]
                path.write_bytes(path.name.encode())
                digest = digests.digest_file(path)
                ref["verification"] = {
                    "size": digest.size,
                    "blake2b": digest.blake2b512,
                    "sha256": digest.sha256,
                }
    return manifest


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


class ReleaseIdentityTests(unittest.TestCase):
    RECORD = {"tag": "v7.11.0", "source_sha": "a" * 40}

    def test_release_tag_must_match_candidate(self):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "tag"):
            verify_draft.check_release_identity(
                {"tag_name": "v7.12.0", "target_commitish": "a" * 40, "body": ""},
                self.RECORD,
            )

    def test_release_source_must_match_candidate(self):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "source"):
            verify_draft.check_release_identity(
                {"tag_name": "v7.11.0", "target_commitish": "b" * 40, "body": ""},
                self.RECORD,
            )

    def test_release_body_source_proof_is_accepted_for_draft(self):
        verify_draft.check_release_identity(
            {
                "tag_name": "v7.11.0",
                "target_commitish": "master",
                "body": "Candidate staged from " + self.RECORD["source_sha"] + ".",
            },
            self.RECORD,
        )

    def test_release_target_commit_source_proof_is_accepted(self):
        verify_draft.check_release_identity(
            {"tag_name": "v7.11.0", "target_commitish": "a" * 40, "body": ""},
            self.RECORD,
        )


class ManifestDigestTests(unittest.TestCase):
    def test_manifest_sha256_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _manifest_with_downloads(Path(tmp))
            manifest["indexes"][0]["packages"][0]["installrefs"][0]["verification"]["sha256"] = (
                "0" * 64
            )
            with self.assertRaisesRegex(verify_draft.DraftVerificationError, "SHA-256"):
                verify_draft.check_manifest_digests(manifest, Path(tmp))


class AttestationPolicyTests(unittest.TestCase):
    @mock.patch("gel_release.verify_draft.subprocess.run")
    def test_attestation_verification_pins_source_digest(self, run):
        verify_draft.verify_attestations([Path("asset")], "gelstable/gel-cli", "a" * 40)
        run.assert_called_once_with(
            [
                "gh",
                "attestation",
                "verify",
                "asset",
                "--repo",
                "gelstable/gel-cli",
                "--source-digest",
                "a" * 40,
            ],
            check=True,
        )


if __name__ == "__main__":
    unittest.main()
