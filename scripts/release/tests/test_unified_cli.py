import importlib
import json
import subprocess
import sys
import unittest
from pathlib import Path

from gel_release import assets

REPO_ROOT = Path(__file__).resolve().parents[3]


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

    def test_internal_modules_do_not_expose_standalone_clis(self):
        for name in (
            "linux_packages",
            "package_target",
            "registry_manifest",
        ):
            module = importlib.import_module(f"gel_release.{name}")
            self.assertFalse(hasattr(module, "main"), name)


if __name__ == "__main__":
    unittest.main()
