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
