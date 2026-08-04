$ErrorActionPreference = "Stop"

cargo test --workspace --all-targets --no-run
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo test --workspace --lib --bins
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo test -p shared-client-tests --test shared_client_tests
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if (Test-Path "tests/registry_sources.rs") {
    cargo test --test registry_sources
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
