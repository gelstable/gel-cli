#!/usr/bin/env bash
set -euo pipefail

cargo test --workspace --all-targets --no-run
cargo test --workspace --lib --bins
cargo test -p shared-client-tests --test shared_client_tests

if [[ -f tests/registry_sources.rs ]]; then
  cargo test --test registry_sources
fi
