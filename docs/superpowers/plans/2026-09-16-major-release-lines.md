# Major Release Lines Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish independently maintained major release lines, with label-driven preview releases and reviewed stable candidates whose published bytes are never rebuilt.

**Architecture:** Work in a fresh worktree from `master`, selectively porting the native release modules from `feat/native-release-pipeline`. A small Python release controller resolves the current same-repository PR, line, phase, meaningful source tree, and version; a reusable workflow builds and stages that immutable identity; separate preview and stable publication gates re-read live state immediately before side effects. GitHub drafts and candidate records are the durable state, while published tags allocate preview suffixes.

**Tech Stack:** Rust 1.88, Python 3.11 with `pydantic`, `jsonschema`, `pytest` and `gh`, Knope 0.23.0, GitHub Actions, `cargo-deb` 3.7.0, `cargo-generate-rpm` 0.21.0.

**Spec:** `docs/superpowers/specs/2026-09-16-major-release-lines-design.md`

## Global Constraints

- Run repository commands through `direnv exec . bash -lc '<command>'`; use the repo shell for Rust tests, formatting, and clippy.
- Start implementation in a separate clean worktree at `master`; preserve `feat/native-release-pipeline` and its uncommitted work as reference. Do not develop in this reference worktree.
- Release branches are `release/v<major>.x`, generated heads are `knope/release-v<major>.x`, and `master` publishes no Gel CLI releases.
- Phase labels are exactly `prerelease:alpha`, `prerelease:beta`, and `prerelease:rc`; zero or one may be active. `-dev.N` and nightly are excluded.
- Preview versions are `X.Y.Z-{alpha,beta,rc}.N`, beginning at `.1` for each base version and phase. Published tags are immutable and allocate the next suffix without gaps caused by failed drafts.
- Stable versions are plain `X.Y.Z` in their release line's major. A phase-labeled PR cannot pass the stable merge gate.
- A preview tag points to a derived versioned commit; its record also names the original PR head. A stable tag points to the actual merge commit.
- Shared candidate packaging, native install tests, digest/manifest generation, attestation, API readback, and source equivalence remain gates before publication.
- The manifest explicitly uses `stable` for plain versions and `testing` for supported prerelease versions. Previews are GitHub prereleases and never latest; stable is latest only when it is the highest published stable SemVer across all lines.
- The registry discovers only published, non-draft allowlisted releases and updates its root through a separate reviewable snapshot PR. Keep the legacy CLI package root, `edgedb` alias, and nightly behavior unchanged.
- Use pinned action SHAs, narrow workflow permissions, a per-line serialization lock, and a repository-wide publication lock for the latest-setting API call.

## File map and task order

The reference branch has `gel_release/{assets,package_target,linux_packages,digests,registry_manifest,candidate,models,verify_draft,source_equivalence,cli}.py`, their tests under `scripts/release/tests/`, and four release workflows. None of those Python modules is present on `master`. Port the mechanics, then replace branch-specific control flow. The reference worktree has uncommitted edits; inspect their diffs and copy selected content without modifying or cleaning that worktree.

| Responsibility | Files on new worktree |
| --- | --- |
| Shared target inventory, deterministic assets, packages, manifests, verification | `gel_release/` modules above, `pyproject.toml`, `uv.lock`, `scripts/release/tests/`, `scripts/ci/run-install-scenario.sh`, `tests/fixtures/registry/release-manifest/gel-registry.json` |
| Release-line, phase, version, snapshot decisions | `gel_release/release_state.py`, `gel_release/preview.py`, `gel_release/github_release.py`, `gel_release/cli.py` and focused tests |
| Immutable candidate identity and source comparison | `gel_release/models.py`, `candidate.py`, `verify_draft.py`, `source_equivalence.py` and focused tests |
| Preparation, staging, approval, publication | `knope.toml`, `.github/workflows/release-pr.yml`, `release-controller.yml`, `release-candidate.yml`, `release-candidate-check.yml`, `release-publish.yml`, `release-install-e2e.yml` |
| Operator instructions and workflow contracts | `README.md`, `scripts/release/tests/test_workflow_contract.py`, `.github/branch-protection.md` |

The existing `.github/workflows.src/*.targets.yml` on `master` belongs to the older generated release build. Remove or disconnect it only after the native workflow covers its supported targets. Keep `install-e2e.yml` for ordinary CI; the release matrix gets its own reusable workflow.

### Task 1: Establish the clean implementation worktree and port native mechanics

**Files:** Create `gel_release/{__init__,assets,package_target,linux_packages,digests,registry_manifest,models,candidate,verify_draft,source_equivalence,cli}.py`, `pyproject.toml`, `uv.lock`, `scripts/release/tests/test_{assets,package_target,linux_packages,digests,registry_manifest,candidate,verify_draft,source_equivalence,unified_cli}.py`; modify `Cargo.toml`, `scripts/ci/run-install-scenario.sh`; create registry fixture. These are selective copies from the reference, not a merge of its workflow files.

**Interfaces:** Preserve `gel-release matrix`, `channel`, `completions`, `package-target`, `linux-packages`, `assemble-stage`, `candidate`, `verify-draft`, `verify-public`, and `source-equivalence` as the common CLI. Later tasks extend these commands.

- [ ] **Step 1: Create a clean worktree from `master`.** From the repository's main checkout, use `direnv exec . bash -lc 'git worktree add -b feat/major-release-lines .worktrees/major-release-lines master'`; run `direnv allow` once there if required. Confirm `git status --short --branch` is clean and that `git rev-parse HEAD` equals the selected `master` commit. If that branch or path exists, choose a fresh name without overwriting anything.
- [ ] **Step 2: Inventory the reference before copying.** In the reference worktree run `direnv exec . bash -lc 'git diff -- gel_release scripts/release scripts/ci/run-install-scenario.sh README.md .github/workflows'` and record which working-tree edits are needed for prerelease package ordering, explicit manifest channels, and the install matrix. Copy the modules, lockfile, focused tests, fixture, and Cargo package metadata into the new worktree; do not copy the reference `knope.toml` or its branch-specific workflows yet.
- [ ] **Step 3: Run the imported tests.** Run `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_assets.py scripts/release/tests/test_package_target.py scripts/release/tests/test_linux_packages.py scripts/release/tests/test_digests.py scripts/release/tests/test_registry_manifest.py scripts/release/tests/test_candidate.py scripts/release/tests/test_verify_draft.py scripts/release/tests/test_source_equivalence.py scripts/release/tests/test_unified_cli.py'`. Expected: all pass; fix only master-port mismatches, especially Cargo metadata and fixture hashes.
- [ ] **Step 4: Check Rust compatibility.** Run `direnv exec . bash -lc 'cargo test'` and `direnv exec . bash -lc 'cargo fmt --check'`. Expected: no new failures. Review `git diff --check` and commit the port as one implementation commit, leaving this plan file in the reference worktree uncommitted.

### Task 2: Make release-line and PR identity explicit

**Files:** Create `gel_release/release_state.py`, `scripts/release/tests/test_release_state.py`; modify `gel_release/cli.py`.

**Interfaces:** `parse_line(base: str) -> int`, `expected_head(major: int) -> str`, `validate_pr(pr: dict, repo: str) -> ReleasePr`, and `phase_from_labels(labels: list[str]) -> str | None`. `ReleasePr` contains PR number, base ref/SHA, head ref/SHA, repository, and major.

- [ ] **Step 1: Write failing identity tests.** Cover `release/v7.x` → `7` and `knope/release-v7.x`, rejection of `master` and malformed line names, a same-repository PR with matching head/base, forked head, mismatched major, closed PR, and two active phase labels. Include `phase_from_labels([]) is None` and each of the three allowed labels.
- [ ] **Step 2: Run `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_release_state.py -q'`.** Expected: import or assertion failures for the new behavior.
- [ ] **Step 3: Implement strict parsing and validation.** Use anchored patterns `^release/v([1-9][0-9]*)\.x$` and `^knope/release-v([1-9][0-9]*)\.x$`; compare `head.repo.full_name` and `base.repo.full_name` to the trusted repository; reject non-open PRs. Read labels from a freshly fetched PR object, not the event payload. Return an immutable `ReleasePr` data object and raise `ValueError` with the rejected line/PR identity.
- [ ] **Step 4: Expose a `gel-release pr-identity --pr-json PATH --repo OWNER/REPO` command** that prints machine-readable validated identity for workflows. Run the focused tests and CLI test; expected: pass. Commit the identity layer.

### Task 3: Select versions and meaningful source snapshots

**Files:** Create `gel_release/preview.py`, `scripts/release/tests/test_preview.py`; modify `gel_release/source_equivalence.py`, `scripts/release/tests/test_source_equivalence.py`, `gel_release/cli.py`.

**Interfaces:** `stable_version(cargo_version: str, major: int) -> str`; `meaningful_tree(rev: str, repo: Path = Path('.')) -> str`; `next_preview_version(base: str, phase: str, published_tags: list[str], already_published: set[tuple[str, str]]) -> str | None`. `meaningful_tree` hashes sorted mode/path/blob entries after excluding only the generated allowlist.

- [ ] **Step 1: Test version decisions and snapshots.** Include `7.1.1` on v7 accepted, `8.0.0` on v7 rejected, prerelease Cargo version rejected, phase `alpha` selecting `7.1.1-alpha.1` then `.2`, switch to beta selecting `.1` for unchanged source, base-version change resetting to `.1`, and a published `(phase, meaningful_tree)` returning no work. Test tag inputs from other bases/phases and failed draft versions do not advance the suffix. Test generated candidate/distribution-only commits preserve `meaningful_tree`, while source and prepared metadata changes alter it.
- [ ] **Step 2: Run focused tests; expect failures.** `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_preview.py scripts/release/tests/test_source_equivalence.py -q'`.
- [ ] **Step 3: Implement deterministic snapshot and SemVer parsing.** Reuse `source_equivalence.tree_entries`; exclude exactly `Formula/gel.rb`, `bucket/gel.json`, `packaging/aur/PKGBUILD`, and `packaging/release-candidate.json`. Hash canonical lines containing mode, path, and blob OID. Parse only plain base versions and supported phase tags, and reject unsupported suffixes before returning a build identity.
- [ ] **Step 4: Add CLI commands `snapshot --rev SHA` and `preview-version --base VERSION --phase PHASE --tags-json PATH --published-json PATH`.** Run the focused tests and commit. The controller will use published Git tags, not draft names, for suffix allocation.

### Task 4: Extend candidate records for stable and preview identity

**Files:** Modify `gel_release/models.py`, `candidate.py`, `verify_draft.py`, `source_equivalence.py`, `cli.py`; modify `scripts/release/tests/test_{candidate,verify_draft,unified_cli}.py`.

**Interfaces:** `CandidateRecord` adds `line`, `pr_number`, `phase: Literal['alpha','beta','rc'] | None`, `source_snapshot`, `build_sha`, and `base_sha` while retaining version, tag, draft ID, original `source_sha`, asset IDs/digests, build date, runs, and attestation. For stable, `build_sha == source_sha` and `phase is None`; for preview, `build_sha` is the derived commit and `source_sha` is the original PR head. The release asset is named `gel-candidate.json` for preview; the stable record path stays `packaging/release-candidate.json`.

- [ ] **Step 1: Write failing record tests.** Assert valid stable/preview shapes, wrong line major, phase/version mismatch, draft ID mismatch, changed source snapshot, duplicate assets, and a preview record whose asset inventory does not include `gel-candidate.json`. Verify a record loaded back from a GitHub release asset has the same bytes and identity as the one written before upload.
- [ ] **Step 2: Run focused tests; expect failures.** `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_candidate.py scripts/release/tests/test_verify_draft.py -q'`.
- [ ] **Step 3: Add strict record validation and readback.** Increment `schema_version` to 2 for the new required identity fields. In `verify_draft`, check release ID, tag/name, draft status, prerelease flag, exact distribution inventory (plus the record asset for preview), uploaded asset IDs, sizes, SHA-256 and BLAKE2b bytes, and attestations. Record asset verification must compare its downloaded bytes to the expected record and must not include itself in its digest list.
- [ ] **Step 4: Extend `gel-release candidate write` and `verify-draft` flags** with line, PR, phase, original source SHA, build SHA, snapshot, and base SHA. Run focused tests and commit. Keep failures as clear validation errors, without traceback in ordinary CLI output.

### Task 5: Prepare one release PR per line

**Files:** Create `knope.toml`, `.github/workflows/release-pr.yml`, `scripts/release/tests/test_release_pr.py`; modify `gel_release/cli.py` and `README.md`.

**Interfaces:** `gel-release prepare-line --base-ref release/vN.x` validates the line, pending `.changeset/*.md`, prepared plain version, and major before the workflow pushes `knope/release-vN.x` and opens/refreshes its PR. The workflow passes line/PR identity to `release-controller.yml` after refresh.

- [ ] **Step 1: Write failing preparation tests.** Cover no pending change files (no PR), pending patch/minor change files, first release of a new major with deliberately set starting Cargo version, wrong-major prepared version, stale line base, and two independent major lines selecting separate PR heads. Assert a subsequent release creates a new PR after the previous PR merges.
- [ ] **Step 2: Run focused tests; expect failures.** `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_release_pr.py -q'`.
- [ ] **Step 3: Configure Knope and workflow.** Adapt the reference Knope prepare steps, but make the PR base and head line-specific. If Knope cannot parameterize its `CreatePullRequest` base safely, let Knope prepare version/changelog locally and use `gh pr create/edit` after validating line and prepared version. Use a bot token for the branch push and direct `gh workflow run release-controller.yml --ref <controller-ref> -f pr=<number>` after refresh. Avoid force-pushing over a PR that contains a stable candidate record without first re-preparing from the latest line and making the old record invalid.
- [ ] **Step 4: Run focused tests and `direnv exec . bash -lc 'knope --version'`** (or the pinned install in the repo shell). Confirm workflow only triggers on `release/v*.x` and repository dispatch, uses narrow permissions, and skips when no changeset exists. Commit the preparation task.

### Task 6: Resolve current PR state and derive a preview commit

**Files:** Create `gel_release/github_release.py`, `scripts/release/tests/test_github_release.py`, `.github/workflows/release-controller.yml`; modify `gel_release/cli.py`.

**Interfaces:** `resolve_candidate(pr: ReleasePr, live_pr: dict, tags: list[str], releases: list[dict]) -> CandidateIdentity | None`, with line, PR, base SHA, original head SHA, meaningful snapshot, phase, selected version, and channel. `derive_preview_commit(source_sha: str, version: str, repo: Path) -> str` creates a Git commit tree with only `Cargo.toml`/`Cargo.lock` version changes; the PR branch stays at its prepared stable version.

- [ ] **Step 1: Write failing controller tests.** Simulate label addition/removal, bot PR refresh, explicit retry, conflicting labels, fork and head/base mismatch, phase switch with unchanged source, generated-record-only update, source update, changed prepared base, published same snapshot, and a stale event whose payload labels differ from the live PR. Assert unsupported `-dev.N` stops before a build matrix is requested.
- [ ] **Step 2: Run focused tests; expect failures.** `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_github_release.py -q'`.
- [ ] **Step 3: Implement the controller.** Fetch the PR from GitHub by number at run time; validate repository/head/base; fetch published tags and existing candidate records; select current identity; output all identity fields as JSON. The workflow listens to `pull_request` labeled/unlabeled/synchronize/reopened, `workflow_dispatch`, and the preparation dispatch. Use a per-line non-canceling concurrency group and check live PR state again before dispatching stage. For an active phase, fetch the PR's issue timeline, identify the most recent addition of that currently active label after its most recent removal, and confirm that actor's repository permission was `write`, `maintain`, or `admin`. Apply the same check on bot refresh and retry; never treat the bot event actor as phase authorization.
- [ ] **Step 4: Derive and preserve the preview commit.** Update only Cargo version fields using the locked toolchain; verify `cargo metadata` returns the selected prerelease and `Cargo.lock` is consistent; create a commit with `source_sha` as parent, and push it to a temporary private ref such as `refs/heads/gel-preview-build/<line>/<tag>` until the tag is created. Make retries reuse an identical derived tree/commit. The stage workflow checks out this exact SHA for every job. Run tests and commit.

### Task 7: Stage and verify immutable candidate bytes

**Files:** Create `.github/workflows/release-candidate.yml`, `.github/workflows/release-install-e2e.yml`; modify `gel_release/github_release.py`, `verify_draft.py`, `scripts/release/tests/test_workflow_contract.py`.

**Interfaces:** Reusable candidate workflow accepts a JSON `CandidateIdentity` or explicit immutable fields (line, PR, base SHA, source SHA, build SHA, snapshot, phase, version, channel). It outputs draft release ID and verified candidate record path/asset. All matrix jobs check out `build_sha`; install jobs consume the built candidate binaries/packages.

- [ ] **Step 1: Add failing workflow-contract tests.** Parse YAML to assert stage accepts immutable inputs, target and smoke matrices derive from `gel-release matrix`, all build jobs checkout `build_sha`, the install matrix gates draft staging, stage uploads exact inventory and verifies API readback, stable records go to the PR while preview records become `gel-candidate.json`, and no publication job contains build/package/upload commands. Assert full SHA action pins and minimum permissions.
- [ ] **Step 2: Run `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_workflow_contract.py -q'`; expect failures.**
- [ ] **Step 3: Port the reference build and install jobs.** Keep target inventory, deterministic archives, `.deb`/`.rpm`, completions, registry manifest, sums, archive-layout and binary-version checks, and native package install scenarios. Compare each job's checked-out commit to `build_sha`; fail if missing outputs or skipped install scenarios. Stage draft only after all required jobs pass; attest assets, upload, and read them back through the API before marking candidate verified.
- [ ] **Step 4: Make draft retry safe.** Key drafts by exact tag and line. If a draft exists, compare its record to the selected identity before reusing; replace assets only while still draft and still unpublished. A published release or tag mismatch fails closed. For preview, upload `gel-candidate.json` after distribution assets and verify it separately; for stable, commit the record to the exact generated PR head only if the live PR/head/base/phase/snapshot still match. Run focused tests and `direnv exec . bash -lc 'actionlint'`, then commit.

### Task 8: Enforce the stable merge gate

**Files:** Create `.github/workflows/release-candidate-check.yml`; modify `gel_release/github_release.py`, `source_equivalence.py`, `verify_draft.py`, `scripts/release/tests/test_workflow_contract.py`, `scripts/release/tests/test_github_release.py`.

**Interfaces:** `check_stable_merge(record: CandidateRecord, live_pr: dict, merge_sha: str, repo: Path) -> None` rejects phase labels, wrong branch/major/version, stale base/head, draft mismatch, and source differences outside the generated allowlist.

- [ ] **Step 1: Write failing gate tests.** Cover valid same-repository generated PR, fork, wrong head/base names, moved base, a label swap whose intermediate no-label event staged a stale stable record, version major mismatch, missing record, changed draft assets, and prospective merge tree with changed source. Test generated metadata-only differences accepted.
- [ ] **Step 2: Run focused tests; expect failures.** `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_github_release.py scripts/release/tests/test_source_equivalence.py -q'`.
- [ ] **Step 3: Implement the gate and workflow.** For `pull_request` opened/synchronize/reopened/labeled/unlabeled on release lines, fetch the live PR and its current labels from GitHub; require zero phase labels, current line base, exact generated head, and record identity. Fetch `refs/pull/<number>/merge`, compare its complete tree to recorded tested source outside the four generated paths, verify the draft by API readback, and check `Cargo.toml` plus `Cargo.lock` resolve to the record's plain stable version. Make this check required in branch protection on each line.
- [ ] **Step 4: Run tests and `direnv exec . bash -lc 'actionlint'`, then commit.** Confirm the workflow runs on label changes as well as PR synchronize so a phase-labeled PR cannot retain a stale green gate.

### Task 9: Publish preview and stable releases with idempotent gates

**Files:** Create `.github/workflows/release-publish.yml`; modify `gel_release/github_release.py`, `cli.py`, `scripts/release/tests/test_github_release.py`, `scripts/release/tests/test_workflow_contract.py`.

**Interfaces:** `publish_preview(identity, record, live_pr, release) -> None` and `publish_stable(record, line_push_sha, release) -> None` verify live authorization and identity, tag target, candidate bytes, and release state before API mutation. `should_make_latest(version: str, published_stable_versions: list[str]) -> bool` compares numeric SemVer components across all lines.

- [ ] **Step 1: Write failing publication tests.** Cover preview phase removed, changed snapshot, changed line/PR, stale draft, same published snapshot retry, immutable mismatched existing tag, and success with a derived commit tag. For stable cover ordinary backport push with no merge-matching record, valid merge commit, mismatched tag target, changed draft bytes, already published matching retry, v7 patch after v8 stable (not latest), and highest stable (latest).
- [ ] **Step 2: Run focused tests; expect failures.** `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_github_release.py -q'`.
- [ ] **Step 3: Implement publication with fresh checks.** Preview re-fetches live PR and label authorization, compares meaningful source snapshot, reads the candidate asset and all distribution assets, creates `vX.Y.Z-phase.N` at `build_sha`, and publishes the existing draft as a prerelease with `make_latest=false`. Stable listens to pushes on `release/v*.x`; require the record to be newly introduced by the merge push, require the merged PR number and head to match the record, and compare the actual pushed SHA to the tested source outside generated paths. Recheck draft bytes, create `vX.Y.Z` at that merge commit, and publish the existing draft. Never rebuild, upload, replace, or move published assets/tags. Emit line, PR, phase, source SHA, version, draft ID, and rejection reason.
- [ ] **Step 4: Serialize the latest update repository-wide.** Under a single publication concurrency group, list currently published non-draft stable releases across all lines immediately before the PATCH and set `make_latest=true` only for the numeric maximum. Treat an existing matching tag and published release as success after verifying bytes and tag target. Run tests and `direnv exec . bash -lc 'actionlint'`, then commit.

### Task 10: Validate channels, package ordering, registry isolation, and migration

**Files:** Modify `gel_release/registry_manifest.py`, `linux_packages.py`, `README.md`, `scripts/release/tests/test_{registry_manifest,linux_packages,workflow_contract}.py`; create `.github/branch-protection.md`.

**Interfaces:** `release_channel(version: str) -> Literal['stable','testing']` accepts only plain SemVer and supported phase prereleases; `package_manager_version(version: str) -> str` sorts a prerelease below the matching stable package. The registry manifest's explicit `channel` remains authoritative.

- [ ] **Step 1: Write failing tests for channels and ordering.** Validate `7.1.0`, `7.1.0-alpha.1`, `-beta.1`, `-rc.1`, reject `-dev.1` and arbitrary suffixes, check generated manifest schema/channel, and compare `.deb`/`.rpm` version ordering against matching stable. Assert registry discovery documentation and configuration remain limited to published, non-draft allowlisted releases with a separate snapshot PR.
- [ ] **Step 2: Run focused tests; expect failures where the port lacks these rules.** `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests/test_registry_manifest.py scripts/release/tests/test_linux_packages.py scripts/release/tests/test_workflow_contract.py -q'`.
- [ ] **Step 3: Finish the migration instructions.** Document cutting `release/vN.x` from `master`, setting its starting version, protected branches and required check, change files and cherry-picks, label transitions/retry, stale draft recovery, stable merge/publish, latest across majors, and registry snapshot promotion. Include how to retire the old master-based generated release workflow only after the new line workflow passes end-to-end checks. Do not change the CLI's legacy default package root.
- [ ] **Step 4: Run `direnv exec . bash -lc 'uv run --frozen pytest scripts/release/tests -q'`, `direnv exec . bash -lc 'cargo test'`, `direnv exec . bash -lc 'cargo fmt --check'`, `direnv exec . bash -lc 'cargo clippy --all-features --workspace --all-targets'`, `direnv exec . bash -lc 'actionlint'`, and `direnv exec . bash -lc 'scripts/ci/check-action-pins.sh'`.** Fix concrete failures, inspect `direnv exec . bash -lc 'git diff --check'`, and commit implementation files. Do not add or commit this plan file.

## Acceptance review

- [ ] A line with no pending change files produces no release PR; v7 and v8 can have simultaneous PRs and candidates, and both keep independent version/changelog state.
- [ ] A maintainer label transition produces the expected preview suffix and testing manifest; identical phase/snapshot retries do nothing, phase changes start at `.1`, and failed drafts create no published suffix gap.
- [ ] Generated-only PR commits cannot trigger another preview; the derived preview tag names the versioned commit and the preview record names the original PR head.
- [ ] A phase-labeled PR fails the stable required check; an unlabelled, current, verified candidate can merge and publishes its existing draft at the actual line merge SHA.
- [ ] Published tags/assets are immutable; stable backports without a matching candidate record do nothing; latest remains the greatest published stable SemVer across all major lines.
- [ ] Workflow tests prove the install matrix and API readback are gates, all actions are pinned, and publication performs no build or upload.
