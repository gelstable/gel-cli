"""Rewrite the committed release-manifest fixture from the generator."""

from pathlib import Path

from scripts.release import registry_manifest
from scripts.release.tests.test_registry_manifest import FIXTURE, _entries

FIXTURE.parent.mkdir(parents=True, exist_ok=True)
FIXTURE.write_bytes(
    registry_manifest.dump(
        registry_manifest.build_manifest(
            "1.2.3", "2026-09-12T00:00:00+00:00", _entries("1.2.3")
        )
    )
)
print(f"wrote {FIXTURE}")
