# Final review fix wave

Review base: `4548e8eb17372c99b72e9a4b6d65db67f749dcde`

The seven Important findings from `final-review-findings.md` are resolved in
the final fix wave. The report is included in the fix wave commit.

## Important findings

1. **Exact draft selection — fixed.** `find_reusable_draft` now selects by the
   exact candidate tag before inspecting the body identity. Older drafts and
   published releases on the same line do not block a later candidate. Tests
   cover sequential tags, same-tag identity mismatches, and published drafts.

2. **Superseded unpublished drafts — fixed.** `find_replaceable_draft` accepts
   only a still-draft candidate with the same tag, line, PR, base, and
   prerelease kind. The staging job holds the shared mutation lock, rechecks
   the release before deleting assets, removes an unpublished stale tag only
   after resolving its commit target, refreshes the release body, and then
   uploads the new bytes. Published releases and tags fail closed. Tests cover
   source refreshes, wrong line/PR/kind, and retry behavior.

3. **GitHub CLI authentication — fixed.** The distribution upload and preview
   record upload paths now set `GH_TOKEN` explicitly. The workflow contract
   test asserts the wiring.

4. **Restage/publication race — fixed.** Candidate staging and publication
   share the `release-mutation` non-canceling job lock. Publication retains the
   repository-wide `release-publish` lock for latest selection. Draft identity
   and state are revalidated immediately before asset deletion, and the
   controller rejects a requested line that differs from the live PR base.
   Workflow contract tests cover the shared lock and revalidation boundary.

5. **Fresh preview authorization — fixed.** Preview publication calls
   `fetch_live_preview_pr` after draft byte verification and immediately before
   tag/PATCH mutation. It re-fetches the PR, prepared version, source snapshot,
   phase timeline, and maintainer permissions, then rechecks identity and
   authorization. Tests cover a source head changing during verification.

6. **Backport merge gate — fixed.** Ordinary backport PRs receive the safe
   passing path only when neither candidate record changes; generated release
   PRs still execute the full stable gate. Branch protection and README now
   document PR-based backports and the candidate-ref trust boundary. Workflow
   tests cover both paths.

7. **Tag/PATCH recovery — fixed.** Published tags remain immutable. Draft tags
   are excluded from published suffix allocation, matching unpublished tags are
   reused after a failed PATCH, and stale source replacements resolve annotated
   or lightweight tag targets before deletion. Preview and stable PATCH failure
   retry tests cover the recovery contract.

The three straightforward minor findings are also covered: stable staging
determines phase before selecting the record filename, line-only controller
dispatch uses the shared release-head helper, and candidate/publication cleanup
removes temporary refs after successful or failed runs.

## Verification

Focused release tests:

```text
PYTHONPATH=. uv run --frozen pytest -q scripts/release/tests/test_github_release.py scripts/release/tests/test_release_state.py scripts/release/tests/test_workflow_contract.py
109 passed in 8.49s
```

Full release suite:

```text
PYTHONPATH=. uv run --frozen pytest scripts/release/tests -q
274 passed in 34.26s
```

Workflow and source checks:

```text
actionlint .github/workflows/release-candidate.yml .github/workflows/release-publish.yml .github/workflows/release-controller.yml .github/workflows/release-candidate-check.yml
PASS (exit 0)

scripts/ci/check-action-pins.sh
PASS (exit 0)

PYTHONPATH=. uv run --frozen ruff check gel_release scripts/release/tests
All checks passed!

PYTHONPATH=. uv run --frozen ruff format --check gel_release scripts/release/tests
28 files already formatted

git diff --check
PASS (exit 0)

cargo fmt --check
PASS (exit 0)

cargo clippy --all-features --workspace --all-targets
PASS (exit 0; existing warnings only)
```

## Residual concerns

- Hosted GitHub API publication, attestation, native install managers, branch
  protection enforcement, and the candidate-ref trust boundary were not
  executable locally. The workflows and documentation are checked, but those
  hosted behaviors need a real repository run.
- `cargo test` retains the known baseline failures from the ledger: 277 Rust
  unit tests passed, 15 of 19 functional tests passed, and four migration
  tests (`migrations::initial`, `migrations::modified1`, `migrations::project`,
  and `migrations::prompt_id`) fail because the server reports that
  `schema::Index.build_concurrently` is missing. This is pre-existing and
  unrelated to the release-line changes.
- Ledger-deferred minor coverage remains for candidate-record BLAKE2b
  readback, broader Knope version/changelog assertions, and package-manager
  ordering tool comparisons.
