# Release pipeline rollout plan

Finishing plan for the major release lines pipeline: from the current branch
state to a first real published release. Written 2026-09-19 as a handoff, so a
fresh session can pick this up without the conversation that produced it.

**Spec:** `docs/superpowers/specs/2026-09-16-major-release-lines-design.md`
**Implementation plan:** `docs/superpowers/plans/2026-09-16-major-release-lines.md`
**Operator docs:** `.github/branch-protection.md`, `.github/release-app.md`

## How to read this

Each phase lists **Agent** work (a coding agent can do it unattended) and
**Checkpoint** items (the responsible human must look, decide, or click). An
agent working this plan must stop at every checkpoint and prompt, rather than
assuming approval. Phases are ordered by dependency; do not reorder without
understanding why each one precedes the next.

## Current state

Branch `lucky-harbor-e424-followup-glm-53`, worktree
`.worktrees/release-branch-pipeline`, three commits on top of `23ce45c8`:

```
1dc998d3 fix: validate generated manifests strictly beside the legacy shape
90a0b5c9 test: consolidate the release suite onto behaviour-level cases
dc9f9cd1 fix: attach the stable merge gate to the commit branch protection reads
```

Verified at the tip: 292 release tests pass, `actionlint` and
`scripts/ci/check-action-pins.sh` pass across all eight workflows, `ruff check`,
`ruff format --check`, and `cargo fmt --check` are clean.

Uncommitted: `.github/release-app.md` (new). Untracked and deliberately ignored:
`FINDINGS.md`, `SIMPLIFY.md` (scratch review notes, not for commit).

The release GitHub App exists and is verified working:

```
app_slug: gelstable-releaser   app_id: 5001219   installation: 163010360
permissions: actions:write, contents:write, issues:read,
             metadata:read, pull_requests:write
GEL_RELEASER_KEY     (org secret)   → gel, gel-cli, gel-postgis
GEL_RELEASER_APP_ID  (org variable) → gel, gel-cli, gel-postgis
```

Nothing in this pipeline has ever run on a hosted runner apart from one
permission probe. That is the dominant risk in everything below.

`master` carries no release workflow. This pipeline is the only release path.

---

## Phase 1: Make CI run the release tooling

Nothing currently runs the Python suite or lints the release workflows. Until
this lands, every later phase is unverified by CI.

**Agent**

1. Add a `release-tooling` job to `.github/workflows/ci.yml`: checkout (no
   submodules), `actions/setup-python` pinned to the SHA already used in
   `release-controller.yml`, install `uv` at `UV_VERSION`, `uv sync --frozen`,
   then `ruff check`, `ruff format --check`, `pytest scripts/release/tests -q`.
   Add `UV_VERSION: "0.11.23"` to the workflow `env` to match the release
   workflows. No `paths:` filter.
2. Widen the `quality` job's lint scope: drop the `ci.yml` argument from both
   `check-action-pins.sh` and `actionlint` so they cover the whole directory.
   Both already pass repo-wide; this should not surface new failures.
3. Add contract tests in `scripts/release/tests/test_workflow_contract.py`
   asserting `ci.yml` invokes `pytest scripts/release/tests` and that the lint
   steps pass no per-file argument, so re-narrowing fails the suite.

**Checkpoint**

- Open a PR and confirm a job named `release tooling` actually appears in the
  checks list. Local green proves nothing here; the entire point of this phase
  is that the checks run in CI.
- Decide whether `release tooling` should be a required status check on
  `master`. Recommended yes.

**Exit criteria:** CI shows the new job passing on a real PR, and the whole
`.github/workflows` directory is linted and pin-checked.

---

## Phase 2: Correctness fixes

**Agent**

1. `release-controller.yml`: add `branches: [release/v*.x]` to the
   `pull_request` trigger. This alone is insufficient — a legitimate backport PR
   targeting a release line still fires the controller and still fails in
   `pr-identity`. Also add a classify step after `Resolve the requested pull
   request` that computes the expected generated head via `gel-release
   release-head --line` and sets a `should_run` output. When the live head does
   not match and the event is `pull_request`, skip the remaining steps
   cleanly. When it does not match on `workflow_dispatch` or
   `repository_dispatch`, fail loudly: an operator named that PR explicitly.
2. `source_equivalence.py`: wrap `tree_entries`' `subprocess.run` and raise
   `SourceDrift` on `CalledProcessError`. `SourceDrift` subclasses `ValueError`,
   so `_stable_record_staged` then catches it and correctly reports "not the
   fixed point" instead of crashing the controller.
3. Tests for both: a contract test for the trigger filter, unit tests for the
   classify boundary (generated head runs, backport head skips, dispatch with a
   mismatch fails), and a unit test that `tree_entries` on an unknown rev raises
   `SourceDrift`.

Optional in the same phase, both verified dead:

4. Give the controller its own `repository_dispatch` type (for example
   `release-candidate`) so it no longer collides with `release-pr.yml`'s
   `release-line`; document both in the README recovery section.
5. Delete the unreachable refusal in `release-pr.yml` (the `old_record` /
   `invalidated` pair; the guard can never fire because the first always sets
   the second). Keep `old_head_sha` — the push step's force-with-lease uses it.

**Checkpoint**

- Confirm the controller's neutral skip is acceptable. It is safe today because
  the controller is not a required status check — only `Release candidate check
  / stable merge gate` is. If the controller ever becomes required, a skip
  becomes a permanently pending PR.

**Exit criteria:** a backport PR to a release line no longer produces a failing
controller run; both new tests fail if their fix is reverted.

---

## Phase 3: Migrate from PAT to the GitHub App

**Agent**

1. Replace all 28 `secrets.RELEASE_BOT_TOKEN` references with per-job
   `actions/create-github-app-token` mint steps, pinned to
   `bcd2ba49218906704ab6c1aa796996da409d3eb1` (v3.2.0), reading
   `vars.GEL_RELEASER_APP_ID` and `secrets.GEL_RELEASER_KEY`.
2. Mint in every job that needs a token. Never mint once and pass it between
   jobs: installation tokens last one hour and cannot be refreshed, several
   release jobs run longer, and passing one through job outputs widens where it
   can leak.
3. Down-scope each mint to the permissions that job actually uses rather than
   the installation's full set. Verified working on 2026-09-19.
4. Rewrite the `Require the release bot token` preflight steps. The check is no
   longer "is the secret non-empty" but "did the mint succeed". Preserve the
   comment explaining why `GITHUB_TOKEN` cannot substitute — that rationale is
   unchanged and load-bearing.
5. Rename the concept away from `RELEASE_BOT`. It reads as "the token Knope
   uses"; Knope never calls the GitHub API in this pipeline (`knope.toml`
   defines only local `prepare-release` and `document-change` workflows), and a
   separate third-party `knope-bot` App is installed on this org. Confusing the
   two is a live hazard.
6. Update `.github/branch-protection.md` and `README.md` where they describe the
   token as a PAT.

**Checkpoint**

- Review the per-job permission sets. This is the security-relevant diff in the
  whole rollout: it decides what each job can do if compromised.
- Confirm no minted token is written to a job output, artifact, or step summary.

**Exit criteria:** no `RELEASE_BOT_TOKEN` references remain; every job holding a
token has an explicit, minimal, reviewed permission set.

---

## Phase 4: Make the pipeline repository-agnostic

Required for Phase 5 — the rehearsal runs in a different repository.

**Agent**

1. `gel_release/assets.py`: derive `REPOSITORY` from the `GITHUB_REPOSITORY`
   environment variable, falling back to `gelstable/gel-cli`. This preserves the
   same-repo invariant at `github_release.py:1317` exactly; it only stops
   hardcoding which repo that is. `GITHUB_REPOSITORY` is set by the runner and
   is not workflow-input controllable.
2. Replace the four `if: github.repository == 'gelstable/gel-cli'` guards with
   `github.repository == (vars.RELEASE_REPOSITORY || 'gelstable/gel-cli')`.
3. Tests covering both the default and the override.

**Checkpoint**

- Satisfy yourself the same-repo enforcement is genuinely unchanged. The check
  should still reject a PR whose head repository differs from the operating
  repository; only the source of "operating repository" moves.

---

## Phase 5: Stand up the rehearsal repository

**Checkpoint — human actions, agent cannot do these**

1. Create `gelstable/gel-cli-release-rehearsal` (private is fine).
2. Install the `gelstable-releaser` App on it: App settings → Install App →
   Only select repositories → add the new repo.
3. Tell the agent when both are done.

**Agent**

4. Extend `GEL_RELEASER_KEY` and `GEL_RELEASER_APP_ID` to the new repository by
   ID. This does not require the private key:
   `gh api -X PUT /orgs/gelstable/actions/secrets/GEL_RELEASER_KEY/repositories -F "selected_repository_ids[]=<id>"`.
5. Set `RELEASE_REPOSITORY` for it.
6. Push the branch, then seed the mid-lifecycle state that the whole v7 adoption
   question depends on: `Cargo.toml` at `7.10.2`, a real `v7.10.2` tag and
   published release, and two or three pre-pipeline prereleases such as
   `v1.0.0-rc.2` and `v2.3.0-alpha.1`. Those exist to exercise
   `published_snapshots`' record-absent skip against real data rather than
   fixtures — failing on absence instead of skipping would brick the controller.
7. Create `release/v7.x` from the seeded commit.
8. Apply branch protection per `.github/branch-protection.md`: require
   `Release candidate check / stable merge gate`, require branches up to date,
   no bypass.

**Checkpoint**

- If the rehearsal repository is private, re-probe the collaborator-permission
  endpoint. It was verified against a public repository; visibility rules differ
  and that endpoint gates all preview publication.

---

## Phase 6: Run the lifecycle

The payoff. Each step proves one thing; run them in order and stop on the first
failure rather than pressing on.

| # | Action | Who | Must observe |
| --- | --- | --- | --- |
| 1 | Push a changeset to `release/v7.x` | Agent | PR opens from `knope/release-v7.x` at **7.11.0**, not a v7 prerelease |
| 2 | Controller stages the stable candidate | Automatic | Draft release and record commit appear; **the gate turns green on the new head** |
| 3 | Add `prerelease:rc` to the PR | **Human** | Derived commit, tag `v7.11.0-rc.1`, testing release, `latest` untouched |
| 4 | Push a generated-only commit | Agent | **No** new preview is produced |
| 5 | Remove the label | **Human** | Gate returns green on the stable candidate |
| 6 | Merge the PR with **squash** | **Human** | Publication succeeds; tag `v7.11.0`; `latest` selected |
| 7 | Re-run publication | Agent | Idempotent; no second tag, no duplicate release |
| 8 | Open an ordinary backport PR | Agent | Gate takes the safe path and passes; controller skips neutrally |

Why each matters: step 2 is the check-run attachment fix, reasoned about but
never executed. Step 3 exercises label authorization through App auth — the
first real use of the permission probed in setup. Step 4 is the meaningful
source snapshot invariant. Step 6 uses squash deliberately: it is the merge
method the first-parent binding was rewritten to support and the one least
covered by unit tests. Step 8 validates Phase 2.

Steps 3, 5, and 6 are human actions by design. The label transition *is* the
authorization, and merging *is* what authorizes publication. An agent performing
them would invalidate the rehearsal.

**Checkpoint**

- Record every step's run URL and outcome in `docs/release-rehearsal-<date>.md`.
- Expect failures. Nothing here has run on hosted infrastructure. Budget for
  several fix-and-retry cycles rather than one clean pass.

**Not covered:** attestation identity is repository-scoped, so rehearsal
attestations do not prove production ones. `latest` selection across multiple
lines needs a second line — add `release/v6.x` and repeat steps 1, 2, and 6 if
you want that covered before production.

---

## Phase 7: Production cutover

**Agent**

1. Merge the branch into `master` through the normal review process.

**Checkpoint — human**

2. Create `release/v7.x` on `gelstable/gel-cli` from `master`, per the README's
   mid-lifecycle instructions. Do not bump the version by hand: `master` stays
   at the current plain version and the first release PR moves `7.10.2` to
   `7.11.0` through Knope.
3. Apply branch protection to `master` and `release/v7.x`.
4. Verify the required check appears on a throwaway PR *before* accepting real
   release changes, per the line setup checklist.
5. Merge the first real release PR yourself.

---

## Deferred, with reasons

- **Move the candidate record out of the PR tree** into a draft-release asset
  keyed by head SHA. The single biggest structural simplification available:
  roughly 350-400 Python lines, 130 YAML lines, and one full build cycle per
  release. Deliberately deferred until after the rehearsal — rewriting the
  provenance core before watching the current one complete a lifecycle means
  debugging two unknowns at once.
- **Dead version and snapshot checks** in `check_stable_merge`
  (`github_release.py:281-298`). The gate passes raw PR JSON, so both are always
  skipped. Decide after the rehearsal whether to enrich the gate with a
  `source_snapshot` (cheap, pure git) or delete them; in either case replace the
  three-name field sniffing with one explicit parameter.
- **`release-install-e2e.yml` duplicates `install-e2e.yml`** scenario for
  scenario. Parameterize one as `workflow_call`. Lower priority than it looks:
  this layer is the most likely to need runner-specific divergence once it
  actually runs.
- **Three remaining shape-sniffing sites** in `github_release.py`:
  `_merged_pr_from_release`, `_matching_merge_pr`, and
  `_published_stable_versions`.
- **The `schema_version: 1` ambiguity** — the vendored schema blesses both the
  generated and legacy manifest shapes under one version. A contract
  renegotiation with `gel-registry`, not a local fix.
- **Whether scripted private-key rotation exists.** `.github/release-app.md`
  asserts it does not. That assertion is unverified; check before writing a
  rotation runbook against it.
- **Whether `gel_release/` should be vendored per repo or published as a pinned
  package** before `gel` and `gel-postgis` adopt the pipeline. Porting once is
  cheap; porting to two drifted copies is not.

## Honest assessment of remaining effort

Phases 1 through 4 are mechanical: roughly a day of agent work plus review, low
risk, each verifiable locally and in CI.

Phase 6 is the unknown. Every prior phase has been verified by tests and
reasoning; none has been verified by execution. First contact with hosted
runners typically surfaces GitHub API permission surprises, event-timing races,
and workflow syntax that passes `actionlint` but behaves differently live. Plan
for iteration, not a single pass.

Because `master` has no release workflow, this pipeline is the only path to a
release. If a release is needed before Phase 6 converges, the alternative is a
hand-cut release outside the pipeline — which forfeits attestations and the
registry's verification path, and should be a deliberate decision rather than a
fallback stumbled into under time pressure.
