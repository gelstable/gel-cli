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

The specified `next_preview_version` interface receives
`already_published` as `(phase, meaningful_tree)` pairs but does not receive
the current snapshot separately. It therefore treats any matching phase entry
in the supplied current-identity set as already published; the controller
must pass entries scoped to the current identity when selecting a version.
Suffix allocation itself only inspects published tags, so draft failures do not
create gaps.
