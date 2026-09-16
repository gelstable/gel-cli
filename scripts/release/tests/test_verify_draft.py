import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from gel_release import assets, candidate, digests, registry_manifest, verify_draft


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

    def test_preview_inventory_requires_candidate_record_asset(self):
        listed = [
            {"id": i, "name": name, "size": 1}
            for i, name in enumerate(assets.expected_assets("7.11.0"))
        ]
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "gel-candidate.json"):
            verify_draft.check_inventory(listed, "7.11.0", phase="alpha")

    def test_prerelease_version_requires_candidate_record_asset(self):
        listed = [
            {"id": i, "name": name, "size": 1}
            for i, name in enumerate(assets.expected_assets("7.11.0-alpha.1"))
        ]
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "gel-candidate.json"):
            verify_draft.check_inventory(listed, "7.11.0-alpha.1")

    def test_preview_inventory_accepts_candidate_record_asset(self):
        listed = [
            {"id": i, "name": name, "size": 1}
            for i, name in enumerate(assets.expected_assets("7.11.0"))
        ]
        listed.append({"id": 999, "name": "gel-candidate.json", "size": 1})
        verify_draft.check_inventory(listed, "7.11.0", phase="alpha")

    def test_preview_inventory_accepts_record_argument(self):
        listed = [
            {"id": i, "name": name, "size": 1}
            for i, name in enumerate(assets.expected_assets("7.11.0"))
        ]
        listed.append({"id": 999, "name": "gel-candidate.json", "size": 1})
        verify_draft.check_inventory(listed, "7.11.0", record={"phase": "alpha"})

    def test_stable_inventory_rejects_candidate_record_asset(self):
        listed = [
            {"id": i, "name": name, "size": 1}
            for i, name in enumerate(assets.expected_assets("7.11.0"))
        ]
        listed.append({"id": 999, "name": "gel-candidate.json", "size": 1})
        with self.assertRaises(verify_draft.DraftVerificationError):
            verify_draft.check_inventory(listed, "7.11.0")


class ReleaseIdentityTests(unittest.TestCase):
    RECORD = {
        "schema_version": 2,
        "line": "release/v7.x",
        "pr_number": 321,
        "phase": None,
        "version": "7.11.0",
        "tag": "v7.11.0",
        "draft_release_id": 123456789,
        "source_sha": "a" * 40,
        "build_sha": "a" * 40,
        "source_snapshot": "d" * 64,
        "base_sha": "b" * 40,
        "build_date": "2026-09-12T00:00:00+00:00",
        "workflow_runs": [],
        "attestation": {"predicate_type": "https://slsa.dev/provenance/v1", "subject_count": 1},
        "assets": [
            {
                "id": 1,
                "name": "asset",
                "size": 1,
                "sha256": "0" * 64,
                "blake2b512": "0" * 128,
            }
        ],
    }

    RELEASE = {
        "id": 123456789,
        "tag_name": "v7.11.0",
        "name": "v7.11.0",
        "draft": True,
        "prerelease": False,
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
        gh_json.assert_called_once_with("api", "/repos/gelstable/gel-cli/git/ref/tags/v7.11.0")

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
        side_effect=subprocess.CalledProcessError(
            1, ["gh", "api"], stderr="gh: Not Found (HTTP 404)"
        ),
    )
    def test_uncreated_draft_tag_is_not_proven_by_release_body(self, gh_json):
        resolved = verify_draft.check_release_identity(self.RELEASE, self.RECORD)
        self.assertIsNone(resolved)
        gh_json.assert_called_once_with("api", "/repos/gelstable/gel-cli/git/ref/tags/v7.11.0")

    @mock.patch(
        "gel_release.verify_draft._gh_json",
        side_effect=subprocess.CalledProcessError(
            1, ["gh", "api"], stderr="gh: Forbidden (HTTP 403)"
        ),
    )
    def test_tag_lookup_forbidden_fails_closed(self, gh_json):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "lookup"):
            verify_draft.check_release_identity(self.RELEASE, self.RECORD)

    @mock.patch(
        "gel_release.verify_draft._gh_json",
        side_effect=subprocess.CalledProcessError(
            1, ["gh", "api"], stderr="gh: Internal Server Error (HTTP 500)"
        ),
    )
    def test_tag_lookup_server_error_fails_closed(self, gh_json):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "lookup"):
            verify_draft.check_release_identity(self.RELEASE, self.RECORD)

    def test_tag_lookup_ignores_unrelated_404(self):
        for stderr in (
            "gh: Forbidden (HTTP 403)\nRequest ID: 404",
            "gh: Internal Server Error (HTTP 500)\nRequest ID: 404",
            "gh: lookup failed\nRequest ID: 404",
        ):
            with self.subTest(stderr=stderr):
                error = subprocess.CalledProcessError(1, ["gh", "api"], stderr=stderr)
                with mock.patch("gel_release.verify_draft._gh_json", side_effect=error):
                    with self.assertRaisesRegex(verify_draft.DraftVerificationError, "lookup"):
                        verify_draft.check_release_identity(self.RELEASE, self.RECORD)

    def test_tag_lookup_rejects_conflicting_http_statuses(self):
        for stderr in ("gh: Forbidden (HTTP 403)", "gh: Forbidden HTTP 403"):
            with self.subTest(stderr=stderr):
                error = subprocess.CalledProcessError(
                    1,
                    ["gh", "api"],
                    output="gh: Not Found (HTTP 404)",
                    stderr=stderr,
                )
                with mock.patch("gel_release.verify_draft._gh_json", side_effect=error):
                    with self.assertRaisesRegex(verify_draft.DraftVerificationError, "lookup"):
                        verify_draft.check_release_identity(self.RELEASE, self.RECORD)

    @mock.patch("gel_release.verify_draft.resolve_tag_commit", return_value=None)
    @mock.patch("gel_release.verify_draft.get_release", return_value=RELEASE)
    def test_skip_attestations_rejects_uncreated_draft_tag(self, get_release, resolve_tag):
        record = {**self.RECORD, "version": "7.11.0"}
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(verify_draft.DraftVerificationError, "not created"):
                verify_draft.verify(record, Path(tmp), verify_attestations_flag=False)

    def test_release_id_must_match_candidate(self):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "release id"):
            verify_draft.check_release_identity({**self.RELEASE, "id": 999}, self.RECORD)

    def test_release_must_be_draft(self):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "draft"):
            verify_draft.check_release_identity({**self.RELEASE, "draft": False}, self.RECORD)

    def test_release_name_must_match_tag(self):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "name"):
            verify_draft.check_release_identity({**self.RELEASE, "name": "wrong"}, self.RECORD)

    def test_release_prerelease_flag_matches_phase(self):
        with self.assertRaisesRegex(verify_draft.DraftVerificationError, "prerelease"):
            verify_draft.check_release_identity({**self.RELEASE, "prerelease": True}, self.RECORD)

    @mock.patch(
        "gel_release.verify_draft._gh_json",
        return_value={"object": {"type": "commit", "sha": "c" * 40}},
    )
    def test_preview_release_tag_resolves_to_build_sha(self, gh_json):
        record = {
            **self.RECORD,
            "version": "7.11.0-alpha.1",
            "tag": "v7.11.0-alpha.1",
            "phase": "alpha",
            "build_sha": "c" * 40,
            "source_sha": "a" * 40,
        }
        release = {
            **self.RELEASE,
            "tag_name": record["tag"],
            "name": record["tag"],
            "prerelease": True,
        }
        self.assertEqual(verify_draft.check_release_identity(release, record), "c" * 40)


class ManifestDigestTests(unittest.TestCase):
    def test_manifest_sha256_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = _manifest_with_downloads(Path(tmp))
            manifest["indexes"][0]["packages"][0]["installrefs"][0]["verification"]["sha256"] = (
                "0" * 64
            )
            with self.assertRaisesRegex(verify_draft.DraftVerificationError, "SHA-256"):
                verify_draft.check_manifest_digests(manifest, Path(tmp))


def _preview_candidate(root: Path) -> tuple[dict, Path, dict[int, bytes]]:
    version = "7.11.0-alpha.1"
    dist = root / "dist"
    dist.mkdir()
    for name in assets.expected_assets(version):
        if name in {
            *(assets.registry_identity_name(target) for target in assets.REGISTRY_TARGETS),
            *(assets.registry_zstd_name(target) for target in assets.REGISTRY_TARGETS),
            assets.REGISTRY_MANIFEST_NAME,
            assets.SHA256SUMS_NAME,
            assets.BLAKE2B_SUMS_NAME,
        }:
            continue
        (dist / name).write_bytes(f"payload for {name}".encode())
    manifest = _manifest_with_downloads(dist, version)
    (dist / assets.REGISTRY_MANIFEST_NAME).write_bytes(registry_manifest.dump(manifest))
    skip = {assets.SHA256SUMS_NAME, assets.BLAKE2B_SUMS_NAME}
    digests.write_sums(dist, "sha256", dist / assets.SHA256SUMS_NAME, skip)
    digests.write_sums(dist, "blake2b512", dist / assets.BLAKE2B_SUMS_NAME, skip)
    ids = {name: 1000 + index for index, name in enumerate(assets.expected_assets(version))}
    record = candidate.build_record(
        line="release/v7.x",
        pr_number=321,
        phase="alpha",
        version=version,
        tag=f"v{version}",
        draft_release_id=123456789,
        source_sha="a" * 40,
        build_sha="c" * 40,
        source_snapshot="d" * 64,
        base_sha="b" * 40,
        build_date="2026-09-12T00:00:00+00:00",
        workflow_runs=[],
        attestation={
            "predicate_type": "https://slsa.dev/provenance/v1",
            "subject_count": len(assets.expected_assets(version)),
        },
        dist_dir=dist,
        asset_ids=ids,
    )
    payloads = {entry["id"]: (dist / entry["name"]).read_bytes() for entry in record["assets"]}
    payloads[9000] = candidate.dump(record)
    return record, dist, payloads


class CandidateReadbackTests(unittest.TestCase):
    @mock.patch("gel_release.verify_draft.verify_attestations")
    @mock.patch("gel_release.verify_draft.download_asset")
    @mock.patch("gel_release.verify_draft.list_release_assets")
    @mock.patch("gel_release.verify_draft.resolve_tag_commit", return_value="c" * 40)
    @mock.patch("gel_release.verify_draft.get_release")
    def test_preview_record_is_read_back_separately_from_distribution_digests(
        self, get_release, resolve_tag, list_assets, download, attestations
    ):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record, dist, payloads = _preview_candidate(root)
            get_release.return_value = {
                "id": record["draft_release_id"],
                "tag_name": record["tag"],
                "name": record["tag"],
                "draft": True,
                "prerelease": True,
            }
            listed = [
                {
                    "id": entry["id"],
                    "name": entry["name"],
                    "size": len(payloads[entry["id"]]),
                }
                for entry in record["assets"]
            ]
            listed.append(
                {
                    "id": 9000,
                    "name": candidate.PREVIEW_RECORD_NAME,
                    "size": len(payloads[9000]),
                }
            )
            list_assets.return_value = listed

            def readback(asset_id, destination, repo):
                destination.write_bytes(payloads[asset_id])

            download.side_effect = readback
            readback_dir = root / "readback"
            verify_draft.verify(record, readback_dir, verify_attestations_flag=True)
            verify_draft.verify(record, readback_dir, verify_attestations_flag=True)

            self.assertEqual(
                (readback_dir / candidate.PREVIEW_RECORD_NAME).read_bytes(), payloads[9000]
            )
            attested_paths = attestations.call_args.args[0]
            self.assertTrue(
                all(path.name != candidate.PREVIEW_RECORD_NAME for path in attested_paths)
            )
            self.assertEqual(attestations.call_args.args[2], record["build_sha"])

    @mock.patch("gel_release.verify_draft.download_asset")
    @mock.patch("gel_release.verify_draft.list_release_assets")
    @mock.patch("gel_release.verify_draft.resolve_tag_commit", return_value="c" * 40)
    @mock.patch("gel_release.verify_draft.get_release")
    def test_preview_record_changed_bytes_fail_closed(
        self, get_release, resolve_tag, list_assets, download
    ):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            record, _, payloads = _preview_candidate(root)
            get_release.return_value = {
                "id": record["draft_release_id"],
                "tag_name": record["tag"],
                "name": record["tag"],
                "draft": True,
                "prerelease": True,
            }
            listed = [
                {
                    "id": entry["id"],
                    "name": entry["name"],
                    "size": len(payloads[entry["id"]]),
                }
                for entry in record["assets"]
            ]
            listed.append(
                {
                    "id": 9000,
                    "name": candidate.PREVIEW_RECORD_NAME,
                    "size": len(payloads[9000]),
                }
            )
            list_assets.return_value = listed

            def readback(asset_id, destination, repo):
                payload = payloads[asset_id]
                destination.write_bytes(payload + (b" " if asset_id == 9000 else b""))

            download.side_effect = readback
            with self.assertRaisesRegex(verify_draft.DraftVerificationError, "bytes"):
                verify_draft.verify(record, root / "readback", verify_attestations_flag=False)


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
