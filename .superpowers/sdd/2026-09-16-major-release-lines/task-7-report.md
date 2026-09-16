# Task 7 report: stage and verify immutable candidate bytes

## Result

Task 7 is implemented. The candidate workflow accepts a JSON `CandidateIdentity`
or explicit immutable fields, checks out the resolved `build_sha` in each build,
smoke, staging, verification, and stable-record job, and gates staging on the
native install matrix. Staging assembles and checks the exact distribution
inventory, creates or safely reuses an unpublished draft, uploads distribution
assets, creates provenance attestations, and reads the release inventory back
through the GitHub API. Verification downloads every recorded asset through the
API and checks identity, digests, registry metadata, and attestations. Stable
records are pushed to the live PR head only after a final identity check;
preview records are uploaded as `gel-candidate.json`.

The Task 6 controller dispatch interface was checked and is wired to
`release-candidate.yml` with `-f identity="$identity"`. It creates a
run-scoped branch at the finalized `build_sha` before dispatching, and the
candidate workflow checks that the dispatch source SHA equals that identity.

## RED

The required workflow contract was run before the workflows and implementation
were present:

```text
direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen pytest scripts/release/tests/test_workflow_contract.py -q'
9 failed, 2 passed
```

The identity boundary tests and draft verification identity tests also failed
before their helpers existed with `AttributeError` for the missing identity
functions. These failures were resolved by adding the identity boundary helpers
and the contract assertions.

## GREEN

```text
direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen pytest scripts/release/tests/test_workflow_contract.py -q'
11 passed in 0.18s
```

```text
direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen pytest scripts/release/tests/test_github_release.py::CandidateIdentityBoundaryTests scripts/release/tests/test_verify_draft.py::ReleaseIdentityTests -q'
26 passed in 0.11s
```

```text
direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen pytest scripts/release/tests -q'
213 passed in 11.65s
```

```text
direnv exec . bash -lc 'actionlint .github/workflows/*.yml'
# no output; exit 0

direnv exec . bash -lc 'scripts/ci/check-action-pins.sh .github/workflows'
# no output; exit 0

direnv exec . bash -lc 'git diff --check'
# no output; exit 0

direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen ruff check gel_release/github_release.py gel_release/verify_draft.py scripts/release/tests/test_github_release.py scripts/release/tests/test_verify_draft.py scripts/release/tests/test_workflow_contract.py'
All checks passed!

direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen ruff format --check gel_release/github_release.py gel_release/verify_draft.py scripts/release/tests/test_github_release.py scripts/release/tests/test_verify_draft.py scripts/release/tests/test_workflow_contract.py'
5 files already formatted
```

## Files

- `.github/workflows/release-candidate.yml`: immutable candidate build,
  inventory staging, attestation, API readback, verification, and stable/preview
  record handling.
- `.github/workflows/release-install-e2e.yml`: reusable native install harness
  and eight required package-manager/direct-install scenarios using candidate
  artifacts.
- `.github/workflows/release-controller.yml`: immutable dispatch ref creation
  so workflow provenance and candidate checkouts share `build_sha`.
- `gel_release/github_release.py`: live PR identity checks, exact draft retry
  matching, and candidate identity serialization.
- `gel_release/verify_draft.py`: persisted candidate identity verification.
- `scripts/release/tests/test_workflow_contract.py`: workflow graph, input,
  permission, pin, and publication-boundary contracts.
- `scripts/release/tests/test_github_release.py`: live identity and draft retry
  boundary tests.
- `scripts/release/tests/test_verify_draft.py`: candidate record identity tests.
- `tests/install-manager/scenarios/apt.rs` and
  `tests/install-manager/scenarios/dnf.rs`: install and verify the actual
  candidate `.deb` and `.rpm` when supplied by the release workflow.
- `.superpowers/sdd/2026-09-16-major-release-lines/task-7-report.md`: this
  report.

## Concerns

- Native package-manager install jobs require GitHub-hosted Linux, macOS, and
  Windows runners and were contract-tested locally but not executed in this
  environment.
- `actions/attest-build-provenance` derives the attestation predicate's source
  commit from the workflow event's OIDC claims. The controller now dispatches
  from a run-scoped branch whose tip is the immutable `build_sha`, and the
  candidate workflow fails before staging if `GITHUB_SHA` differs. A hosted run
  is still needed to exercise the GitHub attestation service end to end.
- `ty check` still reports two pre-existing type errors in `verify_draft.py`
  (the `expected_build_sha` narrowing and phase assignment); no new errors were
  introduced by Task 7.

## Review fix round 1

### RED

The added regression contracts initially reproduced the three blocking gaps:

```text
direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen pytest scripts/release/tests/test_workflow_contract.py -q'
..FF..F......                                                            [100%]
3 failed, 10 passed in 0.15s
```

The first actionlint run after adding the dispatch ref reported:

```text
direnv exec . bash -lc 'actionlint .github/workflows/*.yml'
.github/workflows/release-controller.yml:309:9: shellcheck reported issue in this script: SC2153:info:2:16: Possible misspelling: CANDIDATE_REF may not be assigned. Did you mean candidate_ref?
```

### GREEN

```text
direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen pytest scripts/release/tests/test_workflow_contract.py -q'
..............                                                           [100%]
14 passed in 0.16s
```

```text
direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen pytest scripts/release/tests -q'
........................................................................ [ 33%]
........................................................................ [ 66%]
........................................................................ [100%]
216 passed in 11.69s
```

```text
direnv exec . bash -lc 'cargo test --features install_manager_e2e --test install-manager --no-run'
Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
```

```text
direnv exec . bash -lc 'actionlint .github/workflows/*.yml'
# no output; exit 0

direnv exec . bash -lc 'scripts/ci/check-action-pins.sh .github/workflows'
# no output; exit 0

direnv exec . bash -lc 'git diff --check'
# no output; exit 0

direnv exec . bash -lc 'cargo fmt --all -- --check'
# no output; exit 0

direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen ruff check gel_release/github_release.py gel_release/verify_draft.py scripts/release/tests/test_github_release.py scripts/release/tests/test_verify_draft.py scripts/release/tests/test_workflow_contract.py'
All checks passed!

direnv exec . bash -lc 'PYTHONPATH=. uv run --frozen ruff format --check gel_release/github_release.py gel_release/verify_draft.py scripts/release/tests/test_github_release.py scripts/release/tests/test_verify_draft.py scripts/release/tests/test_workflow_contract.py'
5 files already formatted
```
