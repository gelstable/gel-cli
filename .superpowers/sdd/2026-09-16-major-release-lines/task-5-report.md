# Task 5 report: prepare one release PR per line

## Result

Implemented and committed the line-specific preparation layer. The
`gel-release prepare-line --base-ref release/vN.x` command validates the
release line, its current base, pending changesets, plain Cargo version, lock
file version, and major identity. The preparation workflow creates or refreshes
`knope/release-vN.x`, uses a lease-protected branch update, and dispatches the
controller with the line and PR number after API identity validation.

## RED

Added `scripts/release/tests/test_release_pr.py` before implementing the new
command. The required focused command was:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py -q'
```

It failed as expected with 8 failures because `gel_release.cli` had no
`prepare_line` validator or `prepare-line` parser command. The one existing
line identity test passed.

## GREEN

The focused preparation suite passed after implementation:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py -q'
...........                                                              [100%]
11 passed in 4.93s
```

The preparation, line identity, and CLI boundary tests passed together:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py scripts/release/tests/test_release_state.py scripts/release/tests/test_unified_cli.py -q'
33 passed in 4.79s
```

The full release suite passed:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests -q'
165 passed in 9.88s
```

Additional checks passed:

```text
direnv exec . bash -lc 'knope --version'
knope 0.23.0

direnv exec . bash -lc 'knope --validate'
direnv exec . bash -lc 'actionlint .github/workflows/release-pr.yml'
direnv exec . bash -lc 'scripts/ci/check-action-pins.sh .github/workflows/release-pr.yml'
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff check gel_release/cli.py scripts/release/tests/test_release_pr.py'
All checks passed!
```

The configured `knope prepare-release --dry-run --verbose` was also run in the
repository shell and selected the expected minor bump from the pending minor
changeset, without touching the worktree.

## Files

- `knope.toml`
- `.github/workflows/release-pr.yml`
- `gel_release/cli.py`
- `scripts/release/tests/test_release_pr.py`
- `README.md`
- This report

## Behavior

- No pending `.changeset/*.md` files return a machine-readable `pending=false`
  result, and every mutating workflow step is skipped.
- Patch and minor pending files are reported independently for each line.
- A new major line uses its deliberately committed Cargo starting version;
  versions from another major are rejected.
- A local line whose tracked `origin/release/vN.x` ref moved is rejected as
  stale. The post-Knope validation also compares the original base SHA and
  generated head.
- Generated heads and PR lookup use the exact line-specific names. If no open
  matching PR remains after a merge, the workflow creates a new one.
- A prior stable candidate record on a generated branch is invalidated from a
  fresh latest-line preparation before the guarded branch refresh.

## Concerns

- The workflow installs the pinned `uv` release from PyPI at runtime after the
  pinned setup-python action. GitHub-hosted runners provide the remaining
  `gh`, `jq`, and Git tooling used by the workflow.
- The controller is dispatched at `master` through `CONTROLLER_REF`; this keeps
  the controller workflow available even when a release line was cut before
  that workflow was added.

## Review fix round 1

The review identified two gaps and both are fixed in this round:

- The preparation step now fetches `origin/$BASE_REF` again immediately after
  `knope prepare-release` and before post-preparation validation. The push step
  fetches the line a final time and compares its live SHA with the captured
  base SHA before updating the generated head. A line that moves during
  preparation therefore stops before the branch push, and the controller still
  receives the line and PR identity only after fresh PR API validation.
- Open PR selection is now a real CLI boundary. `pr-operation` consumes the
  exact `gh pr list --state open` JSON, rejects ambiguous or mismatched rows,
  chooses `refresh` for one matching PR, and chooses `create` for an empty list.
  Tests exercise the empty list after a simulated merge, refresh, duplicate
  rejection, and the workflow's create/edit integration points.

### Fix RED

The live-fetch regression test failed before the workflow change:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py::ReleasePrWorkflowTests -q'
1 failed, 2 passed
```

The PR-operation tests then failed before the selector and CLI were added:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py::ReleasePrWorkflowTests -q'
2 failed, 1 passed
```

### Fix GREEN

The complete focused preparation suite passed:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py -q'
................                                                         [100%]
16 passed in 4.93s
```

The full release suite and all required workflow/configuration checks passed:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests -q'
170 passed in 9.83s

direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff check gel_release scripts/release/tests'
All checks passed!

direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff format --check gel_release scripts/release/tests'
25 files already formatted

direnv exec . bash -lc 'actionlint .github/workflows/release-pr.yml'
direnv exec . bash -lc 'scripts/ci/check-action-pins.sh .github/workflows/release-pr.yml'
direnv exec . bash -lc 'knope --version'
knope 0.23.0

direnv exec . bash -lc 'knope --validate'
direnv exec . bash -lc 'git diff --check'
```

## Review fix round 2

The remaining review gap was that the prior selector test passed an empty list
twice and the workflow text assertions did not execute either GitHub PR
operation. The workflow now calls `gel-release pr-sync`, which owns the live
`gh pr list` selection and the matching `gh pr edit` or `gh pr create` call.
The executable regression test supplies a fake `gh`, starts with PR 101 open,
marks it absent to model its merge, and verifies that the next invocation
creates PR 102.

### Fix RED

The executable regression test failed before the `pr-sync` command existed:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py::ReleasePrWorkflowTests::test_pr_sync_invokes_create_after_open_pr_is_marked_merged -q'
F                                                                        [100%]
E       AssertionError: 2 != 0
----------------------------- Captured stderr call -----------------------------
validation error: argument command: invalid choice: 'pr-sync' (choose from ...)
```

### Fix GREEN

The executable PR operation regression passed:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py::ReleasePrWorkflowTests::test_pr_sync_invokes_create_after_open_pr_is_marked_merged -q'
.                                                                        [100%]
1 passed in 0.33s
```

The focused release PR tests passed:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_release_pr.py -q'
.................                                                        [100%]
17 passed in 5.32s
```

The complete release suite passed:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests -q'
........................................................................ [ 42%]
........................................................................ [ 84%]
...........................                                              [100%]
171 passed in 10.10s
```

Workflow and Python checks passed:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff format gel_release/cli.py scripts/release/tests/test_release_pr.py'
1 file reformatted, 1 file left unchanged

direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff check gel_release/cli.py scripts/release/tests/test_release_pr.py'
All checks passed!

direnv exec . bash -lc 'actionlint .github/workflows/release-pr.yml'
direnv exec . bash -lc 'scripts/ci/check-action-pins.sh .github/workflows/release-pr.yml'
direnv exec . bash -lc 'git diff --check'
```
