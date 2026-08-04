//! Real-fixture composition tests against committed upstream index snapshots.
//!
//! Distinct from the synthetic-fixtured tests in catalog.rs: these load the
//! genuine index bytes captured from packages.geldata.com (see
//! dev/registry-mirror/refresh.sh) to catch upstream schema drift, and drive
//! the full config-load + catalog path on real data.

use std::path::PathBuf;

use fs_err as fs;

use super::catalog::{Catalog, CatalogLoad, SourceReport};
use super::config::{Config, RegistrySource};
use super::source::{Source, SourceLoader};
use super::types::{ArtifactIdentity, Channel};

/// Fixed platform so assertions are host-independent.
const PINNED_PLATFORM: &str = "x86_64-unknown-linux-gnu";

fn mirror_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/registry/mirror")
}

fn committed_manifest() -> PathBuf {
    mirror_dir().join("registry.json")
}

fn stable_index_name() -> String {
    format!("stable-{PINNED_PLATFORM}.json")
}

/// Copy every file in the committed mirror into a fresh tempdir so a scenario
/// can mutate its own copy without touching the committed fixtures.
fn copy_mirror_to_tempdir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create tempdir");
    for entry in fs::read_dir(mirror_dir()).expect("read mirror dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_file() {
            fs::copy(&path, tmp.path().join(entry.file_name())).expect("copy fixture");
        }
    }
    tmp
}

async fn load(config: &Config) -> CatalogLoad {
    let loader = SourceLoader::new().expect("source loader");
    Catalog::load(config, &loader, Channel::Stable, PINNED_PLATFORM)
        .await
        .expect("catalog loads")
}

#[tokio::test]
async fn faithful_mirror_yields_server_packages() {
    let config = Config {
        sources: vec![RegistrySource::Manifest(Source::File(committed_manifest()))],
    };
    let load = load(&config).await;
    assert!(
        matches!(&load.source_reports[0], SourceReport::Healthy { .. }),
        "single committed source should be healthy"
    );
    assert!(
        !load.catalog.server_packages().is_empty(),
        "real upstream index should yield server packages"
    );
    assert!(load.conflicts.is_empty());
}

#[tokio::test]
async fn conflicting_mirror_excludes_contested_identity() {
    let tmp = copy_mirror_to_tempdir();
    let index_path = tmp.path().join(stable_index_name());
    let mut doc: serde_json::Value =
        serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
    // Target a package that validates as Server (gel-server; the real stable
    // index has 14) and bump the size of EVERY installref, so whichever
    // installref validate_server selects (application/x-tar + zstd) carries
    // the changed size. Same identity, differing size → a "size" conflict.
    let packages = doc["packages"].as_array_mut().expect("packages array");
    let target = packages
        .iter_mut()
        .find(|p| p["basename"] == "gel-server")
        .expect("real stable index contains gel-server packages");
    let contested_version = target["version"]
        .as_str()
        .expect("version present")
        .to_owned();
    let contested_slot = target["slot"].as_str().expect("slot present").to_owned();
    for installref in target["installrefs"]
        .as_array_mut()
        .expect("installrefs array")
    {
        let size = installref["verification"]["size"]
            .as_u64()
            .expect("size present");
        installref["verification"]["size"] = serde_json::Value::from(size + 1);
    }
    fs::write(&index_path, serde_json::to_vec(&doc).unwrap()).unwrap();

    let config = Config {
        sources: vec![
            RegistrySource::Manifest(Source::File(committed_manifest())),
            RegistrySource::Manifest(Source::File(tmp.path().join("registry.json"))),
        ],
    };
    let load = load(&config).await;

    assert!(matches!(
        &load.source_reports[0],
        SourceReport::Healthy { .. }
    ));
    assert!(matches!(
        &load.source_reports[1],
        SourceReport::Healthy { .. }
    ));
    assert!(
        !load.conflicts.is_empty(),
        "mutated identity should produce at least one conflict"
    );
    for conflict in &load.conflicts {
        let warning = conflict.warning();
        assert!(
            warning.contains("contested"),
            "conflict warning should mention contested: {warning}"
        );
    }
    // The defining outcome: a conflict is recorded for the EXACT contested
    // identity (gel-server at the mutated version/slot), naming the mutated
    // `size` field.
    let has_conflict = load.conflicts.iter().any(|c| {
        matches!(
            c.identity(),
            ArtifactIdentity::Server { name, version, slot }
                if name.as_ref() == "gel-server"
                    && version.as_ref() == contested_version
                    && slot.as_ref() == contested_slot
        ) && c.warning().contains("size")
    });
    assert!(
        has_conflict,
        "expected a size conflict on gel-server {contested_version}"
    );
    // And that contested identity is EXCLUDED from the catalog — a regression
    // that records the conflict but leaves the artifact present must fail here.
    let still_present = load.catalog.server_packages().iter().any(|p| {
        p.name == "gel-server"
            && p.version.to_string() == contested_version.as_str()
            && p.slot == contested_slot.as_str()
    });
    assert!(
        !still_present,
        "contested gel-server {contested_version} must be excluded from the catalog"
    );
}

#[tokio::test]
async fn unavailable_source_falls_back_to_faithful() {
    let tmp = copy_mirror_to_tempdir();
    // First source's manifest points at an index file that does not exist.
    let bad_manifest = tmp.path().join("bad-registry.json");
    fs::write(
        &bad_manifest,
        format!(
            r#"{{"schema_version":1,"indexes":[{{"channel":"stable","platform":"{PINNED_PLATFORM}","url":"does-not-exist.json"}}]}}"#,
        ),
    )
    .unwrap();

    let config = Config {
        sources: vec![
            RegistrySource::Manifest(Source::File(bad_manifest)),
            RegistrySource::Manifest(Source::File(committed_manifest())),
        ],
    };
    let load = load(&config).await;

    assert!(
        matches!(&load.source_reports[0], SourceReport::Unavailable { .. }),
        "first source should be unavailable"
    );
    assert!(
        matches!(&load.source_reports[1], SourceReport::Healthy { .. }),
        "second source should be healthy"
    );
    assert!(
        !load.catalog.server_packages().is_empty(),
        "fallback source should still contribute packages"
    );
}

#[tokio::test]
async fn rejected_source_falls_back_to_faithful() {
    let tmp = copy_mirror_to_tempdir();
    // Truncate the copied index to invalid JSON → parse failure rejects the
    // whole source atomically.
    fs::write(tmp.path().join(stable_index_name()), b"{not json").unwrap();

    let config = Config {
        sources: vec![
            RegistrySource::Manifest(Source::File(tmp.path().join("registry.json"))),
            RegistrySource::Manifest(Source::File(committed_manifest())),
        ],
    };
    let load = load(&config).await;

    assert!(
        matches!(&load.source_reports[0], SourceReport::Rejected { .. }),
        "first source should be rejected"
    );
    assert!(
        matches!(&load.source_reports[1], SourceReport::Healthy { .. }),
        "second source should be healthy"
    );
    assert!(
        !load.catalog.server_packages().is_empty(),
        "fallback source should still contribute packages"
    );
}
