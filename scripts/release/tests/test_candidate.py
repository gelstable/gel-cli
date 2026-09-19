import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from gel_release import assets, candidate, cli


def _dist(root: Path, version: str) -> tuple[Path, dict[str, int]]:
    dist = root / "dist"
    dist.mkdir()
    ids = {}
    for index, name in enumerate(assets.expected_assets(version)):
        (dist / name).write_bytes(f"payload for {name}".encode())
        ids[name] = 1000 + index
    return dist, ids


def _record(root: Path, version: str = "7.11.0") -> tuple[dict, Path]:
    dist, ids = _dist(root, version)
    phase = "alpha" if "-alpha." in version else None
    source_sha = "a" * 40
    build_sha = source_sha if phase is None else "c" * 40
    record = candidate.build_record(
        line="release/v7.x",
        pr_number=321,
        phase=phase,
        version=version,
        tag=f"v{version}",
        draft_release_id=123456789,
        source_sha=source_sha,
        build_sha=build_sha,
        source_snapshot="d" * 64,
        base_sha="b" * 40,
        build_date="2026-09-12T00:00:00+00:00",
        workflow_runs=[{"workflow": "release-candidate.yml", "run_id": 42, "run_attempt": 1}],
        attestation={
            "predicate_type": "https://slsa.dev/provenance/v1",
            "subject_count": 23,
        },
        dist_dir=dist,
        asset_ids=ids,
    )
    return record, dist


class RecordShapeTests(unittest.TestCase):
    def test_inventory_matches_expected_assets(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp))
            names = [entry["name"] for entry in record["assets"]]
            self.assertEqual(names, assets.expected_assets("7.11.0"))
            self.assertEqual(names, sorted(names))

    def test_record_does_not_list_itself(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp))
            names = {entry["name"] for entry in record["assets"]}
            self.assertNotIn("release-candidate.json", names)
            self.assertNotIn("packaging/release-candidate.json", names)

    def test_identity_fields_for_stable_and_preview(self):
        # A preview record carries the same identity as a stable one except for
        # its phase and the build SHA derived from the frozen source commit.
        for version, phase, build_sha in (
            ("7.11.0", None, "a" * 40),
            ("7.11.0-alpha.1", "alpha", "c" * 40),
        ):
            with self.subTest(version=version):
                with tempfile.TemporaryDirectory() as tmp:
                    record, _ = _record(Path(tmp), version)
                    self.assertEqual(record["schema_version"], 2)
                    self.assertEqual(record["line"], "release/v7.x")
                    self.assertEqual(record["pr_number"], 321)
                    self.assertEqual(record["phase"], phase)
                    self.assertEqual(record["version"], version)
                    self.assertEqual(record["tag"], f"v{version}")
                    self.assertEqual(record["draft_release_id"], 123456789)
                    self.assertEqual(record["source_sha"], "a" * 40)
                    self.assertEqual(record["build_sha"], build_sha)
                    self.assertEqual(record["base_sha"], "b" * 40)
                    self.assertEqual(record["source_snapshot"], "d" * 64)
                    self.assertEqual(record["workflow_runs"][0]["run_attempt"], 1)
                    self.assertEqual(record["attestation"]["subject_count"], 23)

    def test_dump_is_byte_stable_and_newline_terminated(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp))
            first = candidate.dump(record)
            self.assertEqual(first, candidate.dump(json.loads(first)))
            self.assertTrue(first.endswith(b"\n"))

    def test_load(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp))
            path = Path(tmp) / "record.json"
            path.write_bytes(candidate.dump(record))
            loaded = candidate.load(path)
            self.assertEqual(loaded, record)

    def test_build_record_missing_asset(self):
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp) / "dist"
            dist.mkdir()
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.build_record(
                    line="release/v7.x",
                    pr_number=1,
                    phase=None,
                    version="7.11.0",
                    tag="v7.11.0",
                    draft_release_id=1,
                    source_sha="a" * 40,
                    build_sha="a" * 40,
                    source_snapshot="d" * 64,
                    base_sha="b" * 40,
                    build_date="2026-09-12T00:00:00+00:00",
                    workflow_runs=[],
                    attestation={},
                    dist_dir=dist,
                    asset_ids={},
                )

    def test_wrong_line_major_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp))
            record["line"] = "release/v8.x"
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.validate_record(record)

    def test_phase_and_version_must_match(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp))
            record["phase"] = "alpha"
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.validate_record(record)

    def test_preview_build_sha_must_be_derived_from_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp), "7.11.0-alpha.1")
            record["build_sha"] = record["source_sha"]
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.validate_record(record)

    def test_changed_source_snapshot_is_rejected_when_expected_snapshot_is_given(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist, expected_source_snapshot="e" * 64)

    def test_duplicate_asset_names_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp))
            record["assets"][1]["name"] = record["assets"][0]["name"]
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.validate_record(record)

    def test_preview_record_asset_name_is_separate_from_digest_inventory(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp), "7.11.0-alpha.1")
            self.assertEqual(candidate.record_asset_name(record), "gel-candidate.json")
            self.assertNotIn("gel-candidate.json", [item["name"] for item in record["assets"]])

    def test_uploaded_record_readback_preserves_bytes_and_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp), "7.11.0-alpha.1")
            expected = candidate.dump(record)
            loaded = candidate.load_bytes(expected)
            self.assertEqual(candidate.dump(loaded), expected)
            self.assertEqual(loaded, record)

    def test_uploaded_record_readback_rejects_changed_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp), "7.11.0-alpha.1")
            path = Path(tmp) / candidate.PREVIEW_RECORD_NAME
            path.write_bytes(candidate.dump(record) + b" ")
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record_asset(record, path)


class VerifyTests(unittest.TestCase):
    def test_matching_tree_verifies(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            candidate.verify_record(record, "7.11.0", dist)

    # Each mutation makes exactly one change to the staged distribution
    # directory or to the recorded inventory; every one must fail verification.
    MUTATIONS = (
        ("tampered asset bytes", lambda record, dist: (dist / "SHA256SUMS").write_bytes(b"x")),
        ("missing asset file", lambda record, dist: (dist / "gel-registry.json").unlink()),
        ("extra asset file", lambda record, dist: (dist / "surprise.bin").write_bytes(b"x")),
        ("unsupported schema version", lambda record, dist: record.update(schema_version=99)),
        ("tag mismatch", lambda record, dist: record.update(tag="v7.11.1")),
        ("truncated inventory", lambda record, dist: record.update(assets=record["assets"][:-1])),
        (
            "size mismatch",
            lambda record, dist: record["assets"][0].update(size=record["assets"][0]["size"] + 1),
        ),
        (
            "blake2b mismatch",
            lambda record, dist: record["assets"][0].update(blake2b512="0" * 128),
        ),
    )

    def test_mutated_candidate_is_rejected(self):
        for label, mutate in self.MUTATIONS:
            with self.subTest(mutation=label):
                with tempfile.TemporaryDirectory() as tmp:
                    record, dist = _record(Path(tmp))
                    mutate(record, dist)
                    with self.assertRaises(candidate.CandidateMismatch):
                        candidate.verify_record(record, "7.11.0", dist)

    def test_version_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.1", dist)

    def test_preview_readback_asset_is_ignored_when_verifying_staged_distributions(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp), "7.11.0-alpha.1")
            (dist / candidate.PREVIEW_RECORD_NAME).write_bytes(candidate.dump(record))
            candidate.verify_record(
                record,
                "7.11.0-alpha.1",
                dist,
                record_asset_name=candidate.PREVIEW_RECORD_NAME,
            )

    def test_unknown_extra_asset_is_still_rejected_with_preview_readback(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp), "7.11.0-alpha.1")
            (dist / candidate.PREVIEW_RECORD_NAME).write_bytes(candidate.dump(record))
            (dist / "surprise.bin").write_bytes(b"unexpected")
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(
                    record,
                    "7.11.0-alpha.1",
                    dist,
                    record_asset_name=candidate.PREVIEW_RECORD_NAME,
                )


class CliTests(unittest.TestCase):
    def test_cli_write_and_verify(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            dist, ids = _dist(root, "7.11.0")
            asset_ids_path = root / "asset-ids.json"
            asset_ids_path.write_text(json.dumps([{"name": k, "id": v} for k, v in ids.items()]))
            out_record = root / "release-candidate.json"

            write_args = [
                "candidate",
                "write",
                "--line",
                "release/v7.x",
                "--pr-number",
                "321",
                "--version",
                "7.11.0",
                "--draft-release-id",
                "123456789",
                "--source-sha",
                "b" * 40,
                "--build-sha",
                "b" * 40,
                "--source-snapshot",
                "d" * 64,
                "--base-sha",
                "a" * 40,
                "--build-date",
                "2026-09-12T00:00:00+00:00",
                "--run-id",
                "42",
                "--run-attempt",
                "1",
                "--dist-dir",
                str(dist),
                "--asset-ids",
                str(asset_ids_path),
                "--out",
                str(out_record),
            ]
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                self.assertEqual(cli.main(write_args), 0)
            self.assertIn("wrote", buf.getvalue())
            self.assertTrue(out_record.is_file())

            verify_args = [
                "candidate",
                "verify",
                "--version",
                "7.11.0",
                "--dist-dir",
                str(dist),
                "--record",
                str(out_record),
            ]
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                self.assertEqual(cli.main(verify_args), 0)
            self.assertIn("matches", buf.getvalue())

    def test_cli_write_rejects_malformed_asset_ids_without_traceback(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            dist, _ = _dist(root, "7.11.0")
            out_record = root / "release-candidate.json"
            for malformed in (["name"], None):
                with self.subTest(malformed=malformed):
                    asset_ids_path = root / "asset-ids.json"
                    asset_ids_path.write_text(json.dumps(malformed))
                    stderr = io.StringIO()
                    with contextlib.redirect_stderr(stderr):
                        result = cli.main(
                            [
                                "candidate",
                                "write",
                                "--line",
                                "release/v7.x",
                                "--pr-number",
                                "321",
                                "--version",
                                "7.11.0",
                                "--draft-release-id",
                                "123456789",
                                "--source-sha",
                                "b" * 40,
                                "--build-sha",
                                "b" * 40,
                                "--source-snapshot",
                                "d" * 64,
                                "--base-sha",
                                "a" * 40,
                                "--build-date",
                                "2026-09-12T00:00:00+00:00",
                                "--run-id",
                                "42",
                                "--run-attempt",
                                "1",
                                "--dist-dir",
                                str(dist),
                                "--asset-ids",
                                str(asset_ids_path),
                                "--out",
                                str(out_record),
                            ]
                        )
                    self.assertEqual(result, 2)
                    self.assertIn("validation error", stderr.getvalue().lower())
                    self.assertNotIn("traceback", stderr.getvalue().lower())


if __name__ == "__main__":
    unittest.main()
