# Task 4 report: stable and preview candidate identity

## Result

Candidate records now use schema version 2 and carry the release line, PR,
phase, original source SHA, source snapshot, build SHA, and line base SHA.
Stable records require a plain version, no phase, and `build_sha == source_sha`.
Preview records require a supported phase and a derived build SHA, and their
release record is verified as the separate `gel-candidate.json` asset. The
stable record path remains `packaging/release-candidate.json`.

Draft verification now checks release metadata, exact distribution inventory,
asset IDs and sizes, downloaded SHA-256 and BLAKE2b digests, manifest sums and
URLs, attestations pinned to the build SHA, and preview record readback. The
preview record is compared byte-for-byte to the expected record and is excluded
from the candidate digest inventory and attestation subjects.

## RED

Added v2 identity, release metadata, preview inventory, source snapshot, and
record readback tests before implementing the new behavior. The required
focused command was:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_candidate.py scripts/release/tests/test_verify_draft.py -q'
```

Before implementation it failed with 30 failures and 22 passes. The failures
were the expected missing v2 arguments and validation/readback behavior, plus
the existing fixtures that still described schema version 1.

## GREEN

Focused candidate, draft, and source snapshot tests:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_candidate.py scripts/release/tests/test_verify_draft.py scripts/release/tests/test_source_equivalence.py -q'
```

Result: 69 passed.

The complete release test suite was then run with:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests -q'
```

Result: 150 passed.

Static checks:

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff check gel_release scripts/release/tests'
```

Result: all checks passed.

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff format --check gel_release scripts/release/tests'
```

Result: 24 files already formatted.

`git diff --check` also completed without output.

## Files

- `gel_release/models.py`
- `gel_release/candidate.py`
- `gel_release/verify_draft.py`
- `gel_release/source_equivalence.py`
- `gel_release/cli.py`
- `scripts/release/tests/test_candidate.py`
- `scripts/release/tests/test_verify_draft.py`
- `scripts/release/tests/test_source_equivalence.py`
- `scripts/release/tests/test_unified_cli.py`
- This report

## Concerns

- Source snapshots are validated as lowercase SHA-256 digests. Draft
  verification can compare against an expected snapshot supplied by the
  controller or CLI; the GitHub API does not independently expose a meaningful
  source tree digest.
- Preview record assets are intentionally read back after distribution
  verification and are not included in provenance subjects, because the record
  cannot attest itself without recursive digests.

## Review fix round 1

The review identified two regressions: malformed `--asset-ids` JSON could raise
an uncaught `TypeError`, and a second preview verification using the same
download directory treated the already downloaded `gel-candidate.json` as an
extra distribution. Added focused CLI, candidate inventory, and preview
readback tests before applying the fixes.

### RED

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_candidate.py scripts/release/tests/test_verify_draft.py scripts/release/tests/test_unified_cli.py -q'
```

```text
...................F....F...F..............................F..........   [100%]
4 failed, 66 passed in 1.08s
```

The failing tests were the new preview readback inventory test, the new
unknown-extra preview inventory test, the new CLI malformed asset ID test, and
the second-run preview verification test.

### GREEN

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests/test_candidate.py scripts/release/tests/test_verify_draft.py scripts/release/tests/test_unified_cli.py -q'
```

```text
.......................................................................  [100%]
71 passed in 1.15s
```

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen pytest scripts/release/tests -q'
```

```text
........................................................................ [ 46%]
........................................................................ [ 93%]
..........                                                               [100%]
154 passed in 4.94s
```

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff check gel_release scripts/release/tests'
```

```text
All checks passed!
```

```text
direnv exec . bash -lc 'PYTHONPATH= uv run --frozen ruff format --check gel_release scripts/release/tests'
```

```text
24 files already formatted
```

`git diff --check` completed without output.

### Fix files

- `gel_release/candidate.py`: validate the asset ID JSON as a list of unique
  `{name, id}` objects and report malformed values as `CandidateMismatch`; let
  preview verification explicitly ignore only its known readback asset while
  retaining exact distribution inventory checks.
- `gel_release/verify_draft.py`: pass the preview record asset name to staged
  distribution verification.
- `scripts/release/tests/test_candidate.py`: cover malformed CLI asset IDs and
  preview inventory filtering while retaining unknown-extra rejection.
- `scripts/release/tests/test_verify_draft.py`: run preview verification twice
  against the same directory.
- `scripts/release/tests/test_unified_cli.py`: exercise malformed asset ID
  inputs through the unified subprocess CLI and assert no traceback.

### Fix concern

The inventory exception is limited to the exact `gel-candidate.json` name and
only for preview records. Stable verification and all other unexpected files
remain errors.
