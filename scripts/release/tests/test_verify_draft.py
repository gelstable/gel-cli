import subprocess
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

    RELEASE = {
        "tag_name": "v7.11.0",
        "target_commitish": "master",
        "body": "Candidate staged from " + RECORD["source_sha"] + ".",
    }

    def test_release_tag_must_match_candidate(self):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "tag"):
            verify_draft.check_release_identity(
                {**self.RELEASE, "tag_name": "v7.12.0"},
                self.RECORD,
            )

    @mock.patch(
        "gel_release.verify_draft._gh_json",
        return_value={"object": {"type": "commit", "sha": "b" * 40}},
    )
    def test_release_source_must_match_candidate(self, gh_json):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "source"):
            verify_draft.check_release_identity(self.RELEASE, self.RECORD)

    @mock.patch(
        "gel_release.verify_draft._gh_json",
        return_value={"object": {"type": "commit", "sha": "a" * 40}},
    )
    def test_lightweight_tag_source_matches_candidate(self, gh_json):
        resolved = verify_draft.check_release_identity(self.RELEASE, self.RECORD)
        self.assertEqual(resolved, "a" * 40)
        gh_json.assert_called_once_with(
            "api", "/repos/gelstable/gel-cli/git/ref/tags/v7.11.0"
        )

    @mock.patch(
        "gel_release.verify_draft._gh_json",
        side_effect=[
            {"object": {"type": "tag", "sha": "c" * 40}},
            {"object": {"type": "commit", "sha": "a" * 40}},
        ],
    )
    def test_annotated_tag_is_dereferenced_to_matching_commit(self, gh_json):
        resolved = verify_draft.check_release_identity(self.RELEASE, self.RECORD)
        self.assertEqual(resolved, "a" * 40)
        self.assertEqual(
            [call.args for call in gh_json.call_args_list],
            [
                ("api", "/repos/gelstable/gel-cli/git/ref/tags/v7.11.0"),
                ("api", "/repos/gelstable/gel-cli/git/tags/" + "c" * 40),
            ],
        )

    @mock.patch(
        "gel_release.verify_draft._gh_json",
        side_effect=[
            {"object": {"type": "tag", "sha": "c" * 40}},
            {"object": {"type": "commit", "sha": "b" * 40}},
        ],
    )
    def test_annotated_tag_source_mismatch_is_rejected(self, gh_json):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "source"):
            verify_draft.check_release_identity(self.RELEASE, self.RECORD)

    @mock.patch(
        "gel_release.verify_draft._gh_json",
        side_effect=subprocess.CalledProcessError(1, ["gh", "api"]),
    )
    def test_uncreated_draft_tag_is_not_proven_by_release_body(self, gh_json):
        resolved = verify_draft.check_release_identity(self.RELEASE, self.RECORD)
        self.assertIsNone(resolved)
        gh_json.assert_called_once_with(
            "api", "/repos/gelstable/gel-cli/git/ref/tags/v7.11.0"
        )

    @mock.patch("gel_release.verify_draft.resolve_tag_commit", return_value=None)
    @mock.patch("gel_release.verify_draft.get_release", return_value=RELEASE)
    def test_skip_attestations_rejects_uncreated_draft_tag(self, get_release, resolve_tag):
        record = {**self.RECORD, "version": "7.11.0", "draft_release_id": 1}
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(verify_draft.DraftVerificationError, "not created"):
                verify_draft.verify(
                    record, Path(tmp), verify_attestations_flag=False
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
