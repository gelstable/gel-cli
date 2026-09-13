import re
import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))


WORKFLOWS = REPO_ROOT / ".github" / "workflows"
RELEASE_WORKFLOWS = (
    "release-pr.yml",
    "release-candidate.yml",
    "release-candidate-check.yml",
    "release-publish.yml",
)


def _text(name: str) -> str:
    return (WORKFLOWS / name).read_text()


class MatrixContractTests(unittest.TestCase):
    def test_staging_matrix_is_derived_from_cli(self):
        text = _text("release-candidate.yml")
        self.assertIn("gel-release matrix build", text)

    def test_smoke_matrix_covers_every_registry_target(self):
        text = _text("release-candidate.yml")
        self.assertIn("gel-release matrix smoke", text)

    def test_repository_python_release_behavior_uses_only_cli(self):
        for name in RELEASE_WORKFLOWS:
            text = _text(name)
            self.assertNotIn("python3 -m scripts.release", text, name)
            self.assertNotIn("python3 -c", text, name)
            self.assertNotIn("pip install --break-system-packages", text, name)

    def test_release_config_installs_uv_in_its_own_job(self):
        release_config = _text("ci.yml").split("  release-config:", 1)[1]
        self.assertIn("astral-sh/setup-uv@", release_config)
        self.assertIn("uv run --frozen pytest", release_config)


class PermissionContractTests(unittest.TestCase):
    def test_every_release_workflow_defaults_to_read(self):
        for name in RELEASE_WORKFLOWS:
            text = _text(name)
            head = text.split("jobs:", 1)[0]
            self.assertIn("permissions:\n  contents: read", head, name)

    def test_staging_is_dispatch_only(self):
        text = _text("release-candidate.yml")
        trigger = text.split("on:", 1)[1].split("permissions:", 1)[0]
        self.assertIn("workflow_dispatch", trigger)
        self.assertNotIn("pull_request", trigger)
        self.assertNotIn("push", trigger)


class PublicationContractTests(unittest.TestCase):
    def test_publication_never_builds_or_uploads(self):
        text = _text("release-publish.yml")
        for forbidden in (
            "cargo build",
            "gh release upload",
            "gh release create",
            "--clobber",
            "attest-build-provenance",
        ):
            self.assertNotIn(forbidden, text, f"publication must not {forbidden}")

    def test_publication_never_force_moves_a_tag(self):
        text = _text("release-publish.yml")
        self.assertNotIn("git push --force", text)
        self.assertNotIn("git tag -f", text)


class PinContractTests(unittest.TestCase):
    def test_every_uses_is_a_full_sha_with_a_version_comment(self):
        pattern = re.compile(r"uses:\s+(\S+)")
        for path in WORKFLOWS.glob("*.yml"):
            for line in path.read_text().splitlines():
                match = pattern.search(line)
                if not match or match.group(1).startswith("./"):
                    continue
                self.assertRegex(
                    line.strip(),
                    r"uses: [^@\s]+@[0-9a-f]{40} # .+$",
                    f"{path.name}: {line.strip()}",
                )


if __name__ == "__main__":
    unittest.main()
