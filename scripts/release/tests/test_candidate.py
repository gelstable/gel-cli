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
    record = candidate.build_record(
        version=version,
        tag=f"v{version}",
        draft_release_id=123456789,
        source_sha="a" * 40,
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

    def test_identity_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, _ = _record(Path(tmp))
            self.assertEqual(record["schema_version"], 1)
            self.assertEqual(record["tag"], "v7.11.0")
            self.assertEqual(record["draft_release_id"], 123456789)
            self.assertEqual(record["source_sha"], "a" * 40)
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
                    version="7.11.0",
                    tag="v7.11.0",
                    draft_release_id=1,
                    source_sha="a" * 40,
                    build_date="2026-09-12T00:00:00+00:00",
                    workflow_runs=[],
                    attestation={},
                    dist_dir=dist,
                    asset_ids={},
                )


class VerifyTests(unittest.TestCase):
    def test_matching_tree_verifies(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            candidate.verify_record(record, "7.11.0", dist)

    def test_tampered_asset_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            (dist / "SHA256SUMS").write_bytes(b"tampered")
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist)

    def test_missing_asset_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            (dist / "gel-registry.json").unlink()
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist)

    def test_extra_asset_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            (dist / "surprise.bin").write_bytes(b"unexpected")
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist)

    def test_version_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.1", dist)

    def test_unsupported_schema_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            record["schema_version"] = 99
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist)

    def test_tag_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            record["tag"] = "v7.11.1"
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist)

    def test_recorded_inventory_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            record["assets"] = record["assets"][:-1]
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist)

    def test_size_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            record["assets"][0]["size"] += 1
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist)

    def test_blake2b_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            record, dist = _record(Path(tmp))
            record["assets"][0]["blake2b512"] = "0" * 128
            with self.assertRaises(candidate.CandidateMismatch):
                candidate.verify_record(record, "7.11.0", dist)


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
                "--version",
                "7.11.0",
                "--draft-release-id",
                "123456789",
                "--source-sha",
                "b" * 40,
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


if __name__ == "__main__":
    unittest.main()
