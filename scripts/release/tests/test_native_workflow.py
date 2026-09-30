from pathlib import Path

import yaml


def test_signing_boundary_and_final_asset_handoff():
    workflow = yaml.safe_load(Path(".github/workflows/release.yml").read_text())
    sign = workflow["jobs"]["sign"]
    draft = workflow["jobs"]["draft"]
    assert sign["environment"] == "package-signing"
    assert "github.ref == 'refs/heads/master'" in sign["if"]
    assert "sign" in draft["needs"]
    assert "PACKAGE_SIGNING_KEY" not in yaml.safe_dump(draft)
    downloads = [
        s["with"]
        for s in draft["steps"]
        if s.get("uses", "").startswith("actions/download-artifact@")
    ]
    assert downloads[-1]["name"] == "signed-native-rpms"
    assert downloads[-1]["path"] == "dist"
