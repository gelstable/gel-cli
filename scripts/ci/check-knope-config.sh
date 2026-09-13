#!/usr/bin/env bash
set -euo pipefail

knope --validate

dry_run="$(knope prepare-release --dry-run)"
printf '%s\n' "$dry_run"

for expected in \
  "Would add the following to Cargo.toml: version =" \
  "Would add the following to Cargo.lock: gel-cli =" \
  "Would delete .changeset/registry-sources.md" \
  "Would add the following to CHANGELOG.md:" \
  "Would create or update a pull request"
do
  if ! printf '%s' "$dry_run" | grep -qF "$expected"; then
    printf 'missing from knope dry run: %s\n' "$expected" >&2
    exit 1
  fi
done

if printf '%s' "$dry_run" | grep -qiE "would (create a tag|create a release|release)"; then
  printf 'knope must not create tags or releases\n' >&2
  exit 1
fi
