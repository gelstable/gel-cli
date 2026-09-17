"""Contracts for the immutable release candidate workflows.

GitHub evaluates workflow YAML outside the Python test process, so these tests
keep the important graph and permission boundaries reviewable locally. They
deliberately inspect parsed job configuration and the commands at each
boundary rather than trying to execute GitHub Actions on a developer machine.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = REPO_ROOT / ".github" / "workflows"
README = REPO_ROOT / "README.md"
BRANCH_PROTECTION = REPO_ROOT / ".github" / "branch-protection.md"
SHA = re.compile(r"^[0-9a-f]{40}$")


def _workflow(name: str) -> dict:
    path = WORKFLOWS / name
    assert path.is_file(), f"missing workflow {path}"
    value = yaml.safe_load(path.read_text())
    assert isinstance(value, dict)
    # PyYAML 1.1 resolves the YAML 1.2 key `on` to boolean True.  Normalize
    # that parser quirk so the contract remains independent of the PyYAML
    # version used by the release tooling.
    if True in value and "on" not in value:
        value["on"] = value.pop(True)
    return value


def _steps(job: dict) -> list[dict]:
    steps = job.get("steps", [])
    assert isinstance(steps, list)
    return [step for step in steps if isinstance(step, dict)]


def _run_text(job: dict) -> str:
    return "\n".join(str(step.get("run", "")) for step in _steps(job))


class CandidateInputContractTests(unittest.TestCase):
    def test_candidate_is_dispatchable_and_reusable_with_immutable_identity(self):
        workflow = _workflow("release-candidate.yml")
        triggers = workflow["on"]
        assert "workflow_dispatch" in triggers
        assert "workflow_call" in triggers

        dispatch_inputs = triggers["workflow_dispatch"]["inputs"]
        call_inputs = triggers["workflow_call"]["inputs"]
        expected = {
            "identity",
            "line",
            "pr_number",
            "base_sha",
            "source_sha",
            "build_sha",
            "source_snapshot",
            "phase",
            "version",
            "channel",
        }
        assert expected <= set(dispatch_inputs)
        assert expected <= set(call_inputs)
        for name in expected - {"identity"}:
            assert call_inputs[name]["type"] == "string"

    def test_candidate_outputs_draft_and_verified_record(self):
        workflow = _workflow("release-candidate.yml")
        outputs = workflow["on"]["workflow_call"]["outputs"]
        assert {"draft_release_id", "verified_candidate"} <= set(outputs)
        assert "jobs" in workflow
        assert "stage" in workflow["jobs"]
        assert "verify" in workflow["jobs"]

    def test_controller_dispatch_matches_candidate_identity_input(self):
        controller = (WORKFLOWS / "release-controller.yml").read_text()
        assert "gh workflow run release-candidate.yml" in controller
        assert '-f identity="$identity"' in controller
        assert 'build_sha="$(jq -r \'.build_sha\' "$RUNNER_TEMP/identity.json")"' in controller
        assert 'git push origin "$build_sha:refs/heads/$candidate_ref"' in controller
        assert 'jq -c . "$RUNNER_TEMP/identity.json"' in controller
        assert '--ref "$candidate_ref"' in controller

    def test_dispatch_source_is_bound_to_the_immutable_build_sha(self):
        workflow = _workflow("release-candidate.yml")
        identity_text = _run_text(workflow["jobs"]["identity"])
        assert any(
            step.get("if") == "github.event_name == 'workflow_dispatch'"
            for step in _steps(workflow["jobs"]["identity"])
        )
        assert 'test "$GITHUB_SHA" = "$BUILD_SHA"' in identity_text

    def test_json_identity_normalization_keeps_the_object(self):
        identity_text = _run_text(_workflow("release-candidate.yml")["jobs"]["identity"])
        assert 'if type == "object" then . else error(' in identity_text
        assert ".build_sha = (.build_sha // .source_sha)" in identity_text

    def test_stage_uploads_use_authenticated_cli_and_share_mutation_lock(self):
        workflow = _workflow("release-candidate.yml")
        stage = workflow["jobs"]["stage"]
        assert stage["concurrency"] == {
            "group": "release-mutation",
            "cancel-in-progress": False,
        }
        upload = next(
            step for step in _steps(stage) if step.get("name") == "Upload every distribution asset"
        )
        assert upload["env"]["GH_TOKEN"] == "${{ github.token }}"
        record = next(
            step
            for step in _steps(stage)
            if step.get("name") == "Write the immutable candidate record"
        )
        assert record["env"]["GH_TOKEN"] == "${{ github.token }}"
        assert "find_replaceable_draft" in _run_text(stage)
        assert "git/ref/tags/$TAG" in _run_text(stage)
        assert "git/refs/tags/$TAG" in _run_text(stage)
        assert "git/tags/$tag_target" in _run_text(stage)

    def test_stage_revalidation_python_block_is_executable_after_yaml_folding(self):
        workflow = _workflow("release-candidate.yml")
        text = _run_text(workflow["jobs"]["stage"])
        match = re.search(r"python -c '(?P<source>.*?)\n\s*' \"\$RUNNER_TEMP", text, re.S)
        assert match, "missing inline draft revalidation script"
        compile(match.group("source"), "release-candidate.yml: draft revalidation", "exec")

    def test_candidate_cleanup_keeps_successful_preview_ref_until_publication(self):
        workflow = _workflow("release-candidate.yml")
        cleanup = workflow["jobs"]["cleanup-temporary-refs"]
        assert "always()" in cleanup["if"]
        text = _run_text(cleanup)
        assert "STAGE_RESULT" in text
        assert "VERIFY_RESULT" in text
        assert "preview_ref" in text
        assert "v${{ needs.identity.outputs.version }}" in str(cleanup["steps"])

    def test_controller_rejects_a_requested_line_that_does_not_match_live_pr(self):
        workflow = (WORKFLOWS / "release-controller.yml").read_text()
        assert "REQUESTED_LINE" in workflow
        assert ".base.ref" in workflow
        assert "requested release line" in workflow


class CandidateGraphContractTests(unittest.TestCase):
    def test_target_and_smoke_matrices_are_derived_from_release_cli(self):
        workflow = _workflow("release-candidate.yml")
        plan = workflow["jobs"]["plan"]
        plan_run = _run_text(plan)
        assert "gel-release matrix build" in plan_run
        assert "gel-release matrix smoke" in plan_run

        build_matrix = workflow["jobs"]["build"]["strategy"]["matrix"]
        smoke_matrix = workflow["jobs"]["smoke"]["strategy"]["matrix"]
        assert "fromJSON(needs.plan.outputs.build_matrix)" in str(build_matrix)
        assert "fromJSON(needs.plan.outputs.smoke_matrix)" in str(smoke_matrix)

    def test_every_build_job_checks_out_the_immutable_build_sha(self):
        workflow = _workflow("release-candidate.yml")
        jobs = workflow["jobs"]
        for name in ("plan", "completions", "build"):
            steps = _steps(jobs[name])
            checkouts = [
                step for step in steps if step.get("uses", "").startswith("actions/checkout@")
            ]
            assert checkouts, f"{name} has no checkout"
            assert any("build_sha" in str(step.get("with", {}).get("ref")) for step in checkouts)

    def test_install_matrix_consumes_candidate_artifacts_and_gates_staging(self):
        workflow = _workflow("release-candidate.yml")
        install = workflow["jobs"]["install"]
        assert install["uses"] == "./.github/workflows/release-install-e2e.yml"
        assert "build_sha" in install["with"]
        assert "version" in install["with"]
        assert "install" in workflow["jobs"]["stage"]["needs"]

        install_workflow = _workflow("release-install-e2e.yml")
        assert "workflow_call" in install_workflow["on"]
        install_text = "\n".join(
            _run_text(job)
            for name, job in install_workflow["jobs"].items()
            if isinstance(job, dict)
        )
        assert "actions/download-artifact@" in "\n".join(
            str(step.get("uses", ""))
            for job in install_workflow["jobs"].values()
            if isinstance(job, dict)
            for step in _steps(job)
        )
        assert "run-install-scenario.sh" in install_text
        for scenario in (
            "e2e_direct",
            "e2e_apt",
            "e2e_dnf",
            "e2e_pacman",
            "e2e_homebrew",
            "e2e_nix",
            "e2e_scoop",
            "e2e_winget",
        ):
            assert scenario in install_text

    def test_linux_package_scenarios_install_the_release_packages(self):
        workflow = _workflow("release-install-e2e.yml")
        prepare_text = _run_text(workflow["jobs"]["prepare"])
        assert "release-dist/gel_${VERSION}_amd64.deb" in prepare_text
        assert "release-dist/gel-${VERSION}-1.x86_64.rpm" in prepare_text

        apt = workflow["jobs"]["apt"]
        dnf = workflow["jobs"]["dnf"]
        assert ".deb" in str(apt.get("env", {}))
        assert "GEL_E2E_PACKAGE" in apt.get("env", {})
        assert ".rpm" in str(dnf.get("env", {}))
        assert "GEL_E2E_PACKAGE" in dnf.get("env", {})

        scenarios = WORKFLOWS.parents[1] / "tests" / "install-manager" / "scenarios"
        apt_source = (scenarios / "apt.rs").read_text()
        dnf_source = (scenarios / "dnf.rs").read_text()
        for source in (apt_source, dnf_source):
            assert "GEL_E2E_PACKAGE" in source
            assert "if let Some(package)" in source
            assert '.arg("-i")' in source
            assert ".arg(package)" in source

    def test_stage_uploads_exact_inventory_attests_and_reads_api_back(self):
        workflow = _workflow("release-candidate.yml")
        stage = workflow["jobs"]["stage"]
        stage_text = _run_text(stage)
        assert "assemble-stage" in stage_text
        assert "expected_assets" in stage_text
        assert "gh release upload" in stage_text
        assert "attest-build-provenance@" in "\n".join(
            str(step.get("uses", "")) for step in _steps(stage)
        )
        assert "verify-draft" in _run_text(workflow["jobs"]["verify"])
        verify_text = _run_text(workflow["jobs"]["verify"])
        assert "--download-dir readback" in verify_text
        assert "releases/" in verify_text

    def test_stable_record_is_committed_to_pr_and_preview_is_release_asset(self):
        workflow = _workflow("release-candidate.yml")
        jobs = workflow["jobs"]
        stable = _run_text(jobs["commit-stable"])
        assert "packaging/release-candidate.json" in stable
        assert "pulls/$PR_NUMBER" in stable
        assert "git push" in stable
        assert "assert_live_identity" in stable

        preview = _run_text(jobs["stage"])
        assert "gel-candidate.json" in preview
        assert "phase" in preview
        assert "gh release upload" in preview

    def test_published_release_workflow_has_no_build_package_or_upload_steps(self):
        path = WORKFLOWS / "release-publish.yml"
        if not path.exists():
            return
        text = path.read_text()
        for forbidden in ("cargo build", "cargo deb", "generate-rpm", "gh release upload"):
            assert forbidden not in text


class WorkflowSafetyContractTests(unittest.TestCase):
    def test_new_workflows_have_read_defaults_and_full_action_pins(self):
        for name in ("release-candidate.yml", "release-install-e2e.yml"):
            text = (WORKFLOWS / name).read_text()
            head = text.split("jobs:", 1)[0]
            assert "permissions:\n  contents: read" in head
            for line in text.splitlines():
                if "uses:" not in line or "./" in line:
                    continue
                assert re.search(r"uses:\s+[^@\s]+@[0-9a-f]{40}\s+#\s+.+$", line)

    def test_candidate_minimizes_elevated_permissions_to_stage_and_commit(self):
        workflow = _workflow("release-candidate.yml")
        assert workflow["permissions"] == {"contents": "read"}
        assert workflow["jobs"]["stage"]["permissions"] == {
            "contents": "write",
            "id-token": "write",
            "attestations": "write",
        }
        assert workflow["jobs"]["verify"]["permissions"] == {
            "contents": "read",
            "attestations": "read",
        }

    def test_publication_is_serialized_across_lines_and_uses_read_defaults(self):
        workflow = _workflow("release-publish.yml")
        assert workflow["on"]["push"]["branches"] == ["release/v*.x"]
        assert workflow["on"]["workflow_run"] == {
            "workflows": ["Release candidate"],
            "types": ["completed"],
        }
        assert workflow["permissions"] == {"contents": "read"}
        assert workflow["concurrency"] == {
            "group": "release-publish",
            "cancel-in-progress": False,
        }
        assert workflow["jobs"]["publish"]["concurrency"] == {
            "group": "release-mutation",
            "cancel-in-progress": False,
        }

    def test_publication_rechecks_and_only_patches_existing_releases(self):
        workflow = _workflow("release-publish.yml")
        text = _run_text(workflow["jobs"]["publish"])
        assert "publish_preview" in text
        assert "publish_stable" in text
        assert "should_make_latest" in text
        assert "make_latest" in text
        assert "verify-draft" in text
        assert "source-equivalence" in text
        preview_steps = [
            step
            for step in _steps(workflow["jobs"]["publish"])
            if step.get("name") == "Publish an authorized preview"
        ]
        assert preview_steps
        assert "github.event_name == 'workflow_run'" in str(preview_steps[0].get("if"))
        for forbidden in (
            "cargo build",
            "cargo deb",
            "generate-rpm",
            "gh release upload",
            "gh api -X DELETE",
        ):
            assert forbidden not in text

    def test_preview_publication_refreshes_live_state_and_cleans_refs(self):
        workflow = _workflow("release-publish.yml")
        preview = _run_text(workflow["jobs"]["publish"])
        assert "fetch_live_preview_pr" in preview
        assert "release-candidate.json" in preview
        assert "gel-candidate.json" in preview
        cleanup = workflow["jobs"]["cleanup-temporary-refs"]
        assert "always()" in cleanup["if"]
        assert "workflow_run.head_branch" in str(cleanup)
        assert "preview_ref" in _run_text(cleanup)


class StableMergeWorkflowContractTests(unittest.TestCase):
    def test_stable_gate_runs_for_all_release_pr_state_changes(self):
        workflow = _workflow("release-candidate-check.yml")
        triggers = workflow["on"]
        assert triggers["pull_request"]["types"] == [
            "opened",
            "synchronize",
            "reopened",
            "labeled",
            "unlabeled",
        ]
        assert triggers["pull_request"]["branches"] == ["release/v*.x"]
        assert "candidate" in workflow["jobs"]
        assert workflow["jobs"]["candidate"]["name"] == "stable merge gate"

    def test_stable_gate_reads_live_pr_merge_ref_and_candidate_record(self):
        workflow = _workflow("release-candidate-check.yml")
        text = _run_text(workflow["jobs"]["candidate"])
        assert "pulls/$PR_NUMBER" in text
        assert "refs/pull/$PR_NUMBER/merge" in text
        assert "packaging/release-candidate.json" in text
        assert "check_stable_merge" in text
        assert "Cargo.toml" in text
        assert "Cargo.lock" in text

    def test_stable_gate_uses_read_only_permissions_and_pinned_actions(self):
        workflow = _workflow("release-candidate-check.yml")
        assert workflow["permissions"] == {"contents": "read"}
        assert workflow["jobs"]["candidate"]["permissions"] == {
            "contents": "read",
            "pull-requests": "read",
            "attestations": "read",
        }
        text = (WORKFLOWS / "release-candidate-check.yml").read_text()
        for line in text.splitlines():
            if "uses:" not in line or "./" in line:
                continue
            assert re.search(r"uses:\s+[^@\s]+@[0-9a-f]{40}\s+#\s+.+$", line)

    def test_ordinary_backport_has_safe_path_and_generated_pr_keeps_full_gate(self):
        workflow = _workflow("release-candidate-check.yml")
        text = _run_text(workflow["jobs"]["candidate"])
        assert "release-head" in text
        assert "packaging/release-candidate.json" in text
        assert "packaging/gel-candidate.json" in text
        assert "generated" in text
        assert "candidate record" in text


class ReleaseMigrationDocumentationContractTests(unittest.TestCase):
    def test_readme_covers_line_migration_and_recovery(self):
        text = README.read_text()
        for required in (
            "release/vN.x",
            "master",
            "Cargo.toml",
            "Cargo.lock",
            ".changeset/",
            "cherry-pick",
            "prerelease:alpha",
            "prerelease:beta",
            "prerelease:rc",
            "retry",
            "stale draft",
            "stable merge gate",
            "Release publish",
            "latest",
            "separate snapshot pull request",
            "published, non-draft",
            "allowlist",
            "master-based generated release workflow",
        ):
            with self.subTest(required=required):
                self.assertIn(required, text)

    def test_readme_keeps_legacy_package_root_and_channel_paths_documented(self):
        text = README.read_text()
        for required in (
            "GEL_PKG_ROOT",
            "EDGEDB_PKG_ROOT",
            "https://packages.geldata.com",
            "nightly",
            "[registry]",
            "sources",
            '"channel": "stable"',
            '"channel": "testing"',
        ):
            with self.subTest(required=required):
                self.assertIn(required, text)

    def test_branch_protection_guidance_requires_the_stable_merge_gate_on_each_line(self):
        self.assertTrue(BRANCH_PROTECTION.is_file())
        text = BRANCH_PROTECTION.read_text()
        for required in (
            "release/v*.x",
            "stable merge gate",
            "opened",
            "synchronize",
            "reopened",
            "labeled",
            "unlabeled",
            "required status check",
            "Do not allow bypassing",
        ):
            with self.subTest(required=required):
                self.assertIn(required, text)

    def test_documents_backport_and_workflow_trust_boundaries(self):
        branch_text = BRANCH_PROTECTION.read_text()
        readme_text = README.read_text()
        for text in (branch_text, readme_text):
            for required in (
                "ordinary backport",
                "candidate record",
                "generated release PR",
                "trust boundary",
                "candidate ref",
            ):
                with self.subTest(text=text[:20], required=required):
                    self.assertIn(required, text)
