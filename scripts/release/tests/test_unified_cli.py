import importlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from pydantic import ValidationError

from gel_release import assets
from gel_release.models import CandidateRecord

REPO_ROOT = Path(__file__).resolve().parents[3]


class ModelBoundaryTests(unittest.TestCase):
    def test_candidate_rejects_extra_fields(self):
        with self.assertRaises(ValidationError):
            CandidateRecord.model_validate(
                {
                    "schema_version": 2,
                    "line": "release/v1.x",
                    "pr_number": 1,
                    "phase": None,
                    "version": "1.2.3",
                    "tag": "v1.2.3",
                    "draft_release_id": 1,
                    "source_sha": "a" * 40,
                    "build_sha": "a" * 40,
                    "source_snapshot": "d" * 64,
                    "base_sha": "b" * 40,
                    "build_date": "2026-09-12T00:00:00+00:00",
                    "workflow_runs": [],
                    "attestation": {
                        "predicate_type": "https://slsa.dev/provenance/v1",
                        "subject_count": 1,
                    },
                    "assets": [],
                    "unexpected": True,
                }
            )

    def test_candidate_rejects_tag_version_disagreement(self):
        with self.assertRaisesRegex(ValidationError, "tag must be v1.2.3"):
            CandidateRecord.model_validate(
                {
                    "schema_version": 2,
                    "line": "release/v1.x",
                    "pr_number": 1,
                    "phase": None,
                    "version": "1.2.3",
                    "tag": "v1.2.4",
                    "draft_release_id": 1,
                    "source_sha": "a" * 40,
                    "build_sha": "a" * 40,
                    "source_snapshot": "d" * 64,
                    "base_sha": "b" * 40,
                    "build_date": "2026-09-12T00:00:00+00:00",
                    "workflow_runs": [],
                    "attestation": {
                        "predicate_type": "https://slsa.dev/provenance/v1",
                        "subject_count": 1,
                    },
                    "assets": [],
                }
            )


class CliBoundaryTests(unittest.TestCase):
    def _run(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-m", "gel_release.cli", *args],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
        )

    def test_build_matrix_comes_from_canonical_targets(self):
        completed = self._run("matrix", "build")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            json.loads(completed.stdout)["include"],
            [
                {
                    "target": target.triple,
                    "runner": target.runner,
                    "linux_packages": target.deb_arch is not None,
                }
                for target in assets.TARGETS
            ],
        )

    def test_release_channel_follows_version(self):
        for version, channel in (
            ("7.11.0", "stable"),
            ("7.11.0-rc.1", "testing"),
        ):
            with self.subTest(version=version):
                completed = self._run("channel", "--version", version)
                self.assertEqual(completed.returncode, 0, completed.stderr)
                self.assertEqual(completed.stdout.strip(), channel)

    def test_release_channel_rejects_unsupported_version(self):
        for version in ("7.11.0-preview.1", "7.11.0-dev.4121"):
            with self.subTest(version=version):
                completed = self._run("channel", "--version", version)
                self.assertEqual(completed.returncode, 2)
                self.assertIn("unsupported release version", completed.stderr)

    def test_smoke_matrix_excludes_distribution_only_target(self):
        completed = self._run("matrix", "smoke")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        triples = {entry["target"] for entry in json.loads(completed.stdout)["include"]}
        self.assertEqual(triples, {target.triple for target in assets.REGISTRY_TARGETS})
        self.assertNotIn("x86_64-apple-darwin", triples)

    def test_candidate_verify_rejects_malformed_json_cleanly(self):
        with tempfile.TemporaryDirectory() as tmp:
            record = Path(tmp) / "record.json"
            record.write_text("{}")
            completed = self._run(
                "candidate",
                "verify",
                "--version",
                "1.2.3",
                "--dist-dir",
                tmp,
                "--record",
                str(record),
            )
        self.assertEqual(completed.returncode, 2)
        self.assertIn("validation", completed.stderr.lower())

    def test_candidate_mismatch_is_reported_without_a_traceback(self):
        with tempfile.TemporaryDirectory() as tmp:
            record = Path(tmp) / "record.json"
            record.write_text(
                json.dumps(
                    {
                        "schema_version": 2,
                        "line": "release/v1.x",
                        "pr_number": 1,
                        "phase": None,
                        "version": "1.2.3",
                        "tag": "v1.2.3",
                        "draft_release_id": 1,
                        "source_sha": "a" * 40,
                        "build_sha": "a" * 40,
                        "source_snapshot": "d" * 64,
                        "base_sha": "b" * 40,
                        "build_date": "2026-09-12T00:00:00+00:00",
                        "workflow_runs": [],
                        "attestation": {
                            "predicate_type": "https://slsa.dev/provenance/v1",
                            "subject_count": 1,
                        },
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
                )
            )
            completed = self._run(
                "candidate",
                "verify",
                "--version",
                "1.2.4",
                "--dist-dir",
                tmp,
                "--record",
                str(record),
            )
        self.assertEqual(completed.returncode, 2)
        self.assertNotIn("Traceback", completed.stderr)

    def test_internal_modules_do_not_expose_standalone_clis(self):
        for name in (
            "candidate",
            "linux_packages",
            "package_target",
            "registry_manifest",
            "source_equivalence",
            "verify_draft",
        ):
            module = importlib.import_module(f"gel_release.{name}")
            self.assertFalse(hasattr(module, "main"), name)


if __name__ == "__main__":
    unittest.main()
