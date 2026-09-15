#!/usr/bin/env bash
#
# Run exactly one install-manager e2e scenario and prove that it really ran.
#
# The whole point of the install-e2e workflow is that its green ticks mean
# something, and there are two ways a scenario job can pass while testing
# nothing at all:
#
#   1. The package manager is missing, so the scenario prints `skipping:` on
#      stderr and returns. Every scenario job exists *because* its manager is
#      present on that runner, so a skip there means the tool went missing and
#      the job proved nothing. A skip is a failure (except for winget, which is
#      genuinely absent from the Windows image and is run with
#      GEL_E2E_ALLOW_SKIP=1).
#   2. The `--exact` filter matches no test — a renamed or mistyped scenario.
#      libtest exits 0 when a filter selects nothing, so this is a silent green.
#      The libtest summary line is parsed and exactly one test must have run.
#
# Usage: scripts/ci/run-install-scenario.sh <test-name>
#
# Environment:
#   GEL_E2E_DIST        directory holding the downloaded binaries (default: dist)
#   GEL_E2E_ALLOW_SKIP  set to 1 to permit a `skipping:` line (winget only)

set -euo pipefail

scenario="${1:?usage: run-install-scenario.sh <test-name>}"
dist="${GEL_E2E_DIST:-dist}"
allow_skip="${GEL_E2E_ALLOW_SKIP:-0}"

if [[ -f "$dist/gel.exe" ]]; then
  suffix=".exe"
else
  suffix=""
fi

gel_bin="$dist/gel$suffix"
test_bin="$dist/install-manager$suffix"

for path in "$gel_bin" "$test_bin"; do
  if [[ ! -f "$path" ]]; then
    echo "run-install-scenario: missing $path; did the artifact download run?" >&2
    exit 1
  fi
  # upload-artifact does not preserve the executable bit.
  chmod +x "$path"
done

# The scenario harness resolves GEL_E2E_BIN with std::path, so on Windows it has
# to be a native path rather than the Git Bash view of one.
abs_gel="$(cd "$(dirname "$gel_bin")" && pwd)/$(basename "$gel_bin")"
if command -v cygpath >/dev/null 2>&1; then
  abs_gel="$(cygpath -w "$abs_gel")"
fi
export GEL_E2E_BIN="$abs_gel"

log="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/install-e2e-$scenario.log"

echo "run-install-scenario: $scenario"
echo "run-install-scenario: GEL_E2E_BIN=$GEL_E2E_BIN"

set +e
"$test_bin" --ignored --exact "$scenario" --nocapture 2>&1 | tee "$log"
status="${PIPESTATUS[0]}"
set -e

failed=0

if [[ "$status" -ne 0 ]]; then
  echo "run-install-scenario: $scenario exited with status $status" >&2
  failed=1
fi

# Guard 1: a skip means the manager was missing and nothing was tested.
if [[ "$allow_skip" != "1" ]] && LC_ALL=C grep -qF 'skipping:' "$log"; then
  echo "run-install-scenario: $scenario printed a 'skipping:' line, so its" \
       "package manager was missing on this runner. This job exists to test" \
       "that manager, so a skip is a failure. Offending lines:" >&2
  LC_ALL=C grep -F 'skipping:' "$log" >&2
  failed=1
fi

# Guard 2: libtest exits 0 when the filter matches nothing, so confirm that
# exactly one test actually ran.
summary="$(LC_ALL=C grep -E '^test result:' "$log" | tail -n 1)"
if [[ -z "$summary" ]]; then
  echo "run-install-scenario: no libtest summary line in the output of" \
       "$scenario; the test binary did not run to completion." >&2
  failed=1
else
  ran="$(LC_ALL=C awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^passed;?$/) print $(i - 1) }' <<<"$summary")"
  if [[ "$ran" != "1" ]]; then
    echo "run-install-scenario: expected exactly 1 test to run for --exact" \
         "$scenario, but libtest reported '${ran:-?}' passed. Has the test been" \
         "renamed? Summary line: $summary" >&2
    failed=1
  fi
fi

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

echo "run-install-scenario: $scenario ran and passed"
