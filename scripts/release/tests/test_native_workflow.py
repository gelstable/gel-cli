from pathlib import Path

import pytest
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


@pytest.mark.parametrize("job", ["sign", "draft"])
@pytest.mark.parametrize(
    "ref,release,expected",
    [
        ("refs/heads/master", "true", True),
        ("refs/heads/release/7.x", "true", True),
        ("refs/heads/package-apt-rpm", "true", False),
        ("refs/tags/v7.1.0", "true", False),
        ("refs/heads/release/7.x", "false", False),
    ],
)
def test_release_job_branch_gate(job, ref, release, expected):
    workflow = yaml.safe_load(Path(".github/workflows/release.yml").read_text())
    condition = workflow["jobs"][job]["if"]
    expression = condition.replace("needs.plan.outputs.release", "release")
    expression = expression.replace("github.ref", "ref")
    expression = expression.replace("&&", "and").replace("||", "or")
    assert (
        eval(
            expression,
            {"__builtins__": {}},
            {
                "release": release,
                "ref": ref,
                "startsWith": lambda value, prefix: value.startswith(prefix),
            },
        )
        is expected
    )
