#!/usr/bin/env bash
set -euo pipefail

uv run --frozen pytest -q scripts/release/tests/test_workflow_contract.py

for workflow in .github/workflows/*.yml; do
  scripts/ci/check-action-pins.sh "$workflow"
done

actionlint .github/workflows/*.yml
