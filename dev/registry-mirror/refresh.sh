#!/usr/bin/env bash
# Regenerate committed registry-mirror fixtures from packages.geldata.com.
#
# Fetches the real upstream index files the CLI fetches in legacy package-root
# mode (see src/portable/registry/source.rs:22-32):
#   https://packages.geldata.com/archive/.jsonindexes/{platform}{suffix}.json
# where suffix is "" (stable), ".testing", or ".nightly".
#
# Also regenerates the authored manifest registry.json from the same pinned
# matrix (upstream is a package root, not a manifest endpoint, so we author it).
#
# Run via:  direnv exec . bash dev/registry-mirror/refresh.sh
# Then:     git commit tests/fixtures/registry/mirror
#
# Re-run after upstream schema or version changes. This is the single manual
# step that keeps committed fixtures from calcifying.
set -euo pipefail

ROOT="https://packages.geldata.com/archive/.jsonindexes"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT="$SCRIPT_DIR/../../tests/fixtures/registry/mirror"
mkdir -p "$OUT"

PLATFORMS=(
  "x86_64-unknown-linux-gnu"
  "x86_64-unknown-linux-musl"
  "aarch64-unknown-linux-gnu"
  "aarch64-unknown-linux-musl"
  "x86_64-apple-darwin"
  "aarch64-apple-darwin"
)
CHANNELS=("stable" "testing" "nightly")

suffix_for() {
  case "$1" in
    stable) echo "" ;;
    testing) echo ".testing" ;;
    nightly) echo ".nightly" ;;
  esac
}

for platform in "${PLATFORMS[@]}"; do
  for channel in "${CHANNELS[@]}"; do
    suffix="$(suffix_for "$channel")"
    url="${ROOT}/${platform}${suffix}.json"
    out="${OUT}/${channel}-${platform}.json"
    echo "fetching $url"
    curl --fail --proto '=https' --tlsv1.2 -sSfL "$url" -o "$out"
  done
done

# Author the manifest from the same matrix (not fetched; we define the structure).
{
  echo '{'
  echo '  "schema_version": 1,'
  echo '  "indexes": ['
  first=1
  for platform in "${PLATFORMS[@]}"; do
    for channel in "${CHANNELS[@]}"; do
      url="${channel}-${platform}.json"
      if [ "$first" -eq 1 ]; then first=0; else echo ','; fi
      printf '    {"channel": "%s", "platform": "%s", "url": "%s"}' \
        "$channel" "$platform" "$url"
    done
  done
  echo ''
  echo '  ]'
  echo '}'
} > "$OUT/registry.json"

echo "wrote $(ls -1 "$OUT" | wc -l | tr -d ' ') files to $OUT"
