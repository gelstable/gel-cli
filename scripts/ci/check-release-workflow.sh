#!/usr/bin/env bash
set -euo pipefail

python3 -m unittest scripts.release.tests.test_workflow_contract

for workflow in .github/workflows/*.yml; do
  scripts/ci/check-action-pins.sh "$workflow"
done

actionlint .github/workflows/*.yml
