#!/usr/bin/env bash
set -euo pipefail

workflow_dir="${1:-.github/workflows}"
invalid="$({ rg -n '^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]+' "$workflow_dir" || true; } \
  | rg -v 'uses:[[:space:]]+(\./[^[:space:]]+|[^@[:space:]]+@[0-9a-f]{40}[[:space:]]+#[[:space:]]+.+)$' || true)"

if [[ -n "$invalid" ]]; then
  printf '%s\n' "$invalid" >&2
  exit 1
fi
