# Task 3 report

## Result

Implemented and committed deterministic source snapshots, stable-version
validation, preview suffix selection, and the two CLI commands as
`3e448824` (`feat: select preview versions and source snapshots`).

## Changes

- Added `gel_release/preview.py` with:
  - strict plain SemVer validation for a Cargo stable version and release-line
    major;
  - exact `alpha`, `beta`, and `rc` phase validation;
  - preview suffix allocation from matching published `vX.Y.Z-phase.N` tags;
  - filtering of unsupported tags and draft-style names so failed drafts do not
    consume a suffix;
  - idempotent no-work handling for an already published phase snapshot.
- Added `source_equivalence.meaningful_tree`, which hashes sorted canonical
  mode/path/blob entries after excluding exactly the existing four generated
  paths.
- Added `gel-release snapshot --rev SHA [--repo PATH]` and
  `gel-release preview-version --base VERSION --phase PHASE --tags-json PATH
  --published-json PATH`.
- Added version, suffix, snapshot, and CLI tests, including generated-only,
  distribution-only, source, and prepared metadata changes.

## Tests and outputs

### RED

Command:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_preview.py scripts/release/tests/test_source_equivalence.py -q'
```

Result before implementation:

```text
ImportError: cannot import name 'preview' from 'gel_release'
1 error during collection
```

### GREEN

Focused command:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_preview.py scripts/release/tests/test_source_equivalence.py -q'
```

Output:

```text
20 passed in 6.18s
```

Relevant full release suite:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests -q'
```

Output:

```text
125 passed in 8.65s
```

Lint and formatting:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff check gel_release/preview.py gel_release/source_equivalence.py gel_release/cli.py scripts/release/tests/test_preview.py scripts/release/tests/test_source_equivalence.py'
All checks passed!

direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff format gel_release/preview.py gel_release/cli.py scripts/release/tests/test_preview.py'
3 files left unchanged
```

`git diff --check` and `git diff --cached --check` completed without output.

## Files

- `gel_release/preview.py`
- `gel_release/source_equivalence.py`
- `gel_release/cli.py`
- `scripts/release/tests/test_preview.py`
- `scripts/release/tests/test_source_equivalence.py`
- This report

## Concerns

Suffix allocation only inspects published tags, so draft failures do not create
gaps.

## Review fix round 1

Reviewer finding: the original selector compared only the phase and could
mistakenly suppress a new source snapshot when an older snapshot had already
been published. The CLI also loaded published snapshot pairs without supplying
the current snapshot. The selector now requires `current_snapshot` as its
fifth argument and compares the exact `(phase, current_snapshot)` pair. The
CLI requires `--snapshot` (also accepted as `--current-snapshot`) and passes it
through to the selector.

### Fix RED

Exact command:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_preview.py scripts/release/tests/test_unified_cli.py -q'
```

Output before the fix:

```text
9 failed, 13 passed in 2.93s
```

The failures were the new old/current snapshot tests and CLI `--snapshot`
coverage: the selector accepted four positional arguments and the CLI did not
recognize `--snapshot`.

### Fix GREEN

Exact command:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_preview.py scripts/release/tests/test_unified_cli.py -q'
```

Output:

```text
22 passed in 3.05s
```

Additional post-fix release suite:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests -q'
127 passed in 8.72s
```

The fix is committed in `23bbf840` (`fix: distinguish published preview
snapshots`).
