# Major release lines: implementation status

**Status:** Implementation complete. All findings from the five-review synthesis are addressed and committed; the branch awaits a final human review before merge. Do not enable branch protection or run a hosted lifecycle until that review passes.

**Worktree:** `/Users/scotttrinh/github.com/scotttrinh/gel-cli/.worktrees/release-branch-pipeline`

**Branch:** `lucky-harbor-e424-followup-glm-53`

**Implementation commit:** `63b8d19e` (`refactor: prune vestigial release pipeline paths and duplicated rules`)

**Plan:** `docs/superpowers/plans/2026-09-16-major-release-lines.md` (tracked)
**Authoritative spec:** `docs/superpowers/specs/2026-09-16-major-release-lines-design.md` (tracked)

## Correction to the previous handoff

The earlier revision of this file declared the branch **"Not ready to merge"** over the
`_published_tag_inventory` blocker. That blocker, and the failed-run ref retention item, were
fixed in commit `225009ca` (verified at `gel_release/github_release.py` with a regression test).
Anyone resuming from the previous handoff was holding a branch blocked on nothing.

The spec and plan are now tracked in the repository instead of living only in a gitignored
directory; the agent reports under `.superpowers/` remain the disposable artifacts.

## Follow-up session: five-review synthesis burn-down

All findings from the synthesized review report were corrected commit by commit:

1. `e4dd19bd` — **Blocker: `make_latest` encoding.** The release PATCH now sends the documented
   string enum `"true"|"false"` instead of a JSON boolean; preview and stable PATCH payloads are
   pinned exactly in tests, including latest-across-majors through the payload.
2. `84811f54` — **Blocker: squash/rebase merges.** Publication bound the recorded base to the
   pushed merge's immediate first parent, which rejected rebase merges (rewritten preparation
   commit) and was undocumented for squashes. The base is now bound through the first-parent
   chain (base within two generated commits); merge commit, squash, and rebase shapes are pinned
   end to end with real git fixtures, and README/branch-protection.md document that no merge
   method needs to be disabled.
3. `f107c9d0` — **Blocker: stable staging had no fixed point.** `resolve_candidate` returned a
   fresh stable identity unconditionally, so the staging record commit either re-triggered the
   controller forever or left branch protection waiting for a gate that never ran. A staged
   record successor with matching identity now resolves to no work; the controller and
   `commit-stable` dispatch `release-candidate-check` explicitly, and the gate is
   `workflow_dispatch`-able and resolves every boundary through the live PR.
4. `961c4973` — **Blocker: README migration procedure.** The v7 instructions no longer say to
   set an already-tagged starting version; the existing-major procedure (leave `Cargo.toml` at
   the master version, let Knope bump) is documented, and the controller rejects a prepared
   version whose tag already belongs to a published release before any build, stage, gate, or
   merge.
5. `bb7b8052` — **Registry boundary.** `ReleaseManifest` now accepts the published v7.10.x
   `replacements` shape (verified against the live manifest) with the same `schema_version`, so
   the JSON schema and the Pydantic model bless the same documents.
6. `f3a40ee2` — **Converged finding: caller-supplied verdicts.** Removed the
   `phase_authorized`/`record_introduced`/`source_equivalent` escape hatches and the workflow's
   `phase_authorized: true` injection. Publication tests run inside real merged-line checkouts,
   the preview authorization rejection path is covered (unauthorized actor, bot actor, missing
   proof), and backport no-ops are pinned beyond PR-number mismatch.
7. `63b8d19e` — **Cleanup.** Allowlist trimmed to the one generated path the pipeline produces;
   `assert_snapshot` argument swap removed; dead `workflows.src` symlinks deleted; the
   timeline/permission rule consolidated into `gel-release phase-permissions` (the live-state
   rechecks remain); lowest-value YAML/prose grep tests removed; duplicated git test helpers
   deduplicated.

## Verification

After each commit: the focused suite, then the full `scripts/release/tests` suite (291 tests,
all passing), `actionlint`, Ruff lint/format, and `git diff --check` through
`direnv exec . bash -lc '<command>'`. Rust suites were not touched by this follow-up (no Rust
changes); the last full Rust run is recorded below.

Hosted behavior remains unverified: GitHub API permissions, attestation identity, native
install scenarios, publication mutations, and branch rules have not been exercised end to end
on hosted runners. Run a hosted preview-to-stable lifecycle before enabling required branch
rules.

## Other open items

- **Deferred test coverage:** Public candidate readback does not compare recorded BLAKE2b when
  SHA-256 and size already match; package ordering tests assert the emitted tilde form without a
  native package-manager comparison.
- **Trust boundary:** Candidate-ref workflow YAML and candidate-owned Python run with release
  write privileges. The docs state this trust assumption; protect generated branches/tooling or
  move privileged orchestration to a trusted revision if contributors must be excluded from
  release authority.
- **Transition:** The new gate dispatch (`release-candidate-check.yml` `workflow_dispatch`) and
  the `phase-permissions` command must exist on the dispatched ref (`master`); the pipeline
  activates only after this branch merges.

## Last full verification

At `63b8d19e`, the release Python suite passed **291 tests**. Actionlint, action pin checks,
Ruff lint/format, and diff checks passed. No Rust code changed in this follow-up; at
`dfe38faf` the Rust suites passed 277 unit and 15 functional tests with four baseline
`schema::Index.build_concurrently` migration failures that also fail on the clean `master`
baseline.

Repository commands must use the project wrapper, for example:

```sh
direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen pytest scripts/release/tests -q'
direnv exec . bash -lc 'actionlint'
direnv exec . bash -lc 'scripts/ci/check-action-pins.sh'
```

## Decisions already made

- Used the supplied clean linked worktree rather than creating another.
- Kept preview `gel-candidate.json` as a release asset outside its own digest inventory to avoid recursive self-digests.
- Added an explicit current snapshot argument to preview selection because the sketched interface could not distinguish a current snapshot from an older published one.
- Permitted replacement of an obsolete **unpublished** draft for the same authorized line and PR; published releases and tags remain immutable.
- Permitted same-identity retry after tag creation and failed publication. An obsolete unpublished tag may be removed only after proving no published release owns it.
- Allowed ordinary backport PRs through a safe gate only when they cannot add or change candidate records; generated release PRs still face the full stable gate.
- Stable publication supports every GitHub merge method: the recorded base is bound through the pushed merge's first-parent chain, and the generated PR's two commits bound the distance.
- The staged stable record is the controller's fixed point; the required stable merge gate is dispatched explicitly because bot pushes may not fire pull_request events.

This status file is in the plan's gitignored `.superpowers/sdd/2026-09-16-major-release-lines/` workspace (tracked via `git add -f` alongside the spec and plan), alongside the detailed ledger, task reports, review packages, and final review findings. Keep this worktree for the next session; removing it or running `git clean -fdx` would remove the handoff.
