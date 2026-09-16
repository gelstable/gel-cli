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
