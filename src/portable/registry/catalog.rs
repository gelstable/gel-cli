//! Composed package catalog construction and query helpers.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use url::Url;

use super::config::{Config, RegistrySource};
use super::index::{IndexDocument, InstallRef, PackageData, valid_blake2b};
use super::manifest::Manifest;
use super::source::{Source, SourceError, SourceLoader};
use super::types::{
    Channel, CliPackage, Compression, ExtensionPackage, PackageHash, PackageType, ServerPackage,
};
use crate::portable::ver;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ArtifactKind {
    Server,
    Cli,
    Extension,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ArtifactKey {
    kind: ArtifactKind,
    name: Box<str>,
    version: Box<str>,
    slot: Box<str>,
}

impl fmt::Display for ArtifactKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ArtifactKind::Server => "server",
            ArtifactKind::Cli => "cli",
            ArtifactKind::Extension => "extension",
        };
        write!(
            f,
            "{kind} {}@{} slot {}",
            self.name, self.version, self.slot
        )
    }
}

#[derive(Clone, Debug)]
enum NormalizedArtifact {
    Server(ArtifactKey, ServerPackage),
    Cli(ArtifactKey, CliPackage),
    Extension(ArtifactKey, ExtensionPackage),
}
#[derive(Clone, Debug)]
struct CatalogEntry {
    artifact_index: usize,
    index_source: Source,
}

enum DuplicateRelation {
    EquivalentMirror,
    Conflict { fields: Vec<&'static str> },
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    artifacts: Vec<NormalizedArtifact>,
}

impl Catalog {
    pub async fn load(
        config: &Config,
        loader: &SourceLoader,
        channel: Channel,
        platform: &str,
    ) -> anyhow::Result<Catalog> {
        let mut artifacts = Vec::new();
        let mut seen: HashMap<ArtifactKey, CatalogEntry> = HashMap::new();
        let mut first_source_error: Option<anyhow::Error> = None;

        for registry_source in &config.sources {
            let index_sources = match registry_source {
                RegistrySource::Manifest(manifest_source) => {
                    let manifest_bytes = match loader.load_manifest(manifest_source).await {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            let recoverable = is_recoverable_source_failure(&error);
                            let error = anyhow::Error::new(error).context(format!(
                                "failed to load registry manifest {}",
                                manifest_source.display()
                            ));
                            log::warn!("{error:#}");
                            if recoverable {
                                if first_source_error.is_none() {
                                    first_source_error = Some(error);
                                }
                                continue;
                            }
                            return Err(error);
                        }
                    };
                    let manifest = match Manifest::from_slice(&manifest_bytes) {
                        Ok(manifest) => manifest,
                        Err(error) => {
                            let error = error.context(format!(
                                "failed to parse registry manifest {}",
                                manifest_source.display()
                            ));
                            log::warn!("{error:#}");
                            return Err(error);
                        }
                    };
                    match manifest.select_indexes(manifest_source, channel, platform) {
                        Ok(sources) => sources
                            .into_iter()
                            .map(|source| {
                                let artifact_base = source.clone();
                                (source, artifact_base)
                            })
                            .collect(),
                        Err(error) => {
                            let error = error.context(format!(
                                "failed to select registry indexes from {}",
                                manifest_source.display()
                            ));
                            log::warn!("{error:#}");
                            return Err(error);
                        }
                    }
                }
                RegistrySource::LegacyPackageRoot(root) => {
                    let index_source =
                        match super::source::legacy_index_url(root, platform, channel) {
                            Ok(url) => Source::Http(url),
                            Err(error) => {
                                let error = error.context(format!(
                                    "failed to construct legacy registry index for {}",
                                    root
                                ));
                                log::warn!("{error:#}");
                                return Err(error);
                            }
                        };
                    let artifact_base = Source::Http(root.clone());
                    vec![(index_source, artifact_base)]
                }
            };

            for (index_source, artifact_base) in index_sources {
                let index_bytes = match loader.load_index(&index_source).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let recoverable = is_recoverable_source_failure(&error);
                        let error = anyhow::Error::new(error).context(format!(
                            "failed to load registry index {}",
                            index_source.display()
                        ));
                        log::warn!("{error:#}");
                        if recoverable {
                            if first_source_error.is_none() {
                                first_source_error = Some(error);
                            }
                            continue;
                        }
                        return Err(error);
                    }
                };
                let index = match IndexDocument::from_slice(&index_bytes) {
                    Ok(index) => index,
                    Err(error) => {
                        let error = error.context(format!(
                            "failed to parse registry index {}",
                            index_source.display()
                        ));
                        log::warn!("{error:#}");
                        return Err(error);
                    }
                };

                for pkg in &index.packages {
                    if let Some(artifact) = normalize_artifact(&index_source, &artifact_base, pkg) {
                        let key = artifact.key().clone();
                        if let Some(first_entry) = seen.get(&key) {
                            let first_artifact = &artifacts[first_entry.artifact_index];
                            match duplicate_relation(first_artifact, &artifact) {
                                DuplicateRelation::EquivalentMirror => {
                                    log::info!(
                                        "duplicate registry artifact {key} from {} duplicates {}; using first source {}, ignoring duplicate {}",
                                        index_source.display(),
                                        first_entry.index_source.display(),
                                        artifact_url(first_artifact),
                                        artifact_url(&artifact),
                                    );
                                    continue;
                                }
                                DuplicateRelation::Conflict { fields } => {
                                    anyhow::bail!(
                                        "conflicting registry artifact {key}: {} ({}) disagrees with {} ({}) on {}; refusing to choose between sources",
                                        index_source.display(),
                                        artifact_url(&artifact),
                                        first_entry.index_source.display(),
                                        artifact_url(first_artifact),
                                        fields.join(", "),
                                    );
                                }
                            }
                        }
                        seen.insert(
                            key,
                            CatalogEntry {
                                artifact_index: artifacts.len(),
                                index_source: index_source.clone(),
                            },
                        );
                        artifacts.push(artifact);
                    }
                }
            }
        }

        if artifacts.is_empty() {
            if let Some(error) = first_source_error {
                return Err(error);
            }
        }

        Ok(Catalog { artifacts })
    }

    pub fn server_packages(&self) -> Vec<ServerPackage> {
        let mut packages = self
            .artifacts
            .iter()
            .filter_map(|artifact| match artifact {
                NormalizedArtifact::Server(_, package) => Some(package.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        packages.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.version.cmp(&b.version))
                .then_with(|| a.slot.cmp(&b.slot))
                .then_with(|| a.url.as_str().cmp(b.url.as_str()))
        });
        packages
    }

    pub fn cli_packages(&self) -> Vec<CliPackage> {
        let mut packages = self
            .artifacts
            .iter()
            .filter_map(|artifact| match artifact {
                NormalizedArtifact::Cli(_, package) => Some(package.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        packages.sort_by(|a, b| {
            a.version
                .cmp(&b.version)
                .then_with(|| a.url.as_str().cmp(b.url.as_str()))
                .then_with(|| a.size.cmp(&b.size))
        });
        packages
    }

    pub fn extension_packages(&self, slot: &str) -> Vec<ExtensionPackage> {
        let mut packages = self
            .artifacts
            .iter()
            .filter_map(|artifact| match artifact {
                NormalizedArtifact::Extension(_, package)
                    if package.tags.get("server_slot").map(|s| s.as_str()) == Some(slot) =>
                {
                    Some(package.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        packages.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.version.cmp(&b.version))
                .then_with(|| a.url.as_str().cmp(b.url.as_str()))
                .then_with(|| a.slot.cmp(&b.slot))
        });
        packages
    }
}

pub async fn load_default_async(channel: Channel, platform: &str) -> anyhow::Result<Catalog> {
    let config = Config::load()?;
    let loader = SourceLoader::new()?;
    Catalog::load(&config, &loader, channel, platform).await
}

fn is_recoverable_source_failure(error: &SourceError) -> bool {
    matches!(error, SourceError::Network { .. })
        || is_missing_source_failure(error)
        || matches!(
            error,
            SourceError::Http { status, .. } if is_unavailability_status(*status)
        )
}

fn is_unavailability_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn is_missing_source_failure(error: &SourceError) -> bool {
    match error {
        SourceError::SourceNotFound { .. } => true,
        SourceError::FileIo { error, .. } => error.kind() == std::io::ErrorKind::NotFound,
        SourceError::Http { .. } | SourceError::Network { .. } => false,
    }
}

fn is_missing_source_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<SourceError>()
            .is_some_and(is_missing_source_failure)
    })
}

impl NormalizedArtifact {
    fn key(&self) -> &ArtifactKey {
        match self {
            NormalizedArtifact::Server(key, _)
            | NormalizedArtifact::Cli(key, _)
            | NormalizedArtifact::Extension(key, _) => key,
        }
    }
}
fn duplicate_relation(first: &NormalizedArtifact, later: &NormalizedArtifact) -> DuplicateRelation {
    let mut fields = Vec::new();
    match (first, later) {
        (NormalizedArtifact::Server(_, first), NormalizedArtifact::Server(_, later)) => {
            if first.size != later.size {
                fields.push("size");
            }
            if !package_hash_equal(&first.hash, &later.hash) {
                fields.push("hash");
            }
            if first.tags != later.tags {
                fields.push("tags");
            }
        }
        (NormalizedArtifact::Cli(_, first), NormalizedArtifact::Cli(_, later)) => {
            if first.size != later.size {
                fields.push("size");
            }
            if !package_hash_equal(&first.hash, &later.hash) {
                fields.push("hash");
            }
            if first.compression_variant() != later.compression_variant() {
                fields.push("compression");
            }
        }
        (NormalizedArtifact::Extension(_, first), NormalizedArtifact::Extension(_, later)) => {
            if first.size != later.size {
                fields.push("size");
            }
            if !package_hash_equal(&first.hash, &later.hash) {
                fields.push("hash");
            }
            if first.tags != later.tags {
                fields.push("tags");
            }
        }
        _ => fields.push("kind"),
    }
    if fields.is_empty() {
        DuplicateRelation::EquivalentMirror
    } else {
        DuplicateRelation::Conflict { fields }
    }
}

fn package_hash_equal(first: &PackageHash, later: &PackageHash) -> bool {
    match (first, later) {
        (PackageHash::Blake2b(first), PackageHash::Blake2b(later))
        | (PackageHash::Unknown(first), PackageHash::Unknown(later)) => first == later,
        _ => false,
    }
}

impl CliPackage {
    fn compression_variant(&self) -> Option<&'static str> {
        self.compression.as_ref().map(|_| "zstd")
    }
}

fn artifact_url(artifact: &NormalizedArtifact) -> &Url {
    match artifact {
        NormalizedArtifact::Server(_, package) => &package.url,
        NormalizedArtifact::Cli(_, package) => &package.url,
        NormalizedArtifact::Extension(_, package) => &package.url,
    }
}

fn normalize_artifact(
    index_source: &Source,
    artifact_base: &Source,
    pkg: &PackageData,
) -> Option<NormalizedArtifact> {
    let artifact = normalize_server(index_source, artifact_base, pkg)
        .or_else(|| normalize_cli(index_source, artifact_base, pkg))
        .or_else(|| normalize_extension(index_source, artifact_base, pkg));
    if artifact.is_none() && is_supported_portable_package(pkg) {
        log::info!("Skipping registry package {pkg:?}");
    }
    artifact
}

fn is_supported_portable_package(pkg: &PackageData) -> bool {
    matches!(
        pkg.basename.as_str(),
        "gel-server" | "edgedb-server" | "gel-cli" | "edgedb-cli"
    ) || pkg.tags.contains_key("extension")
}

fn normalize_server(
    _index_source: &Source,
    artifact_base: &Source,
    pkg: &PackageData,
) -> Option<NormalizedArtifact> {
    if !matches!(pkg.basename.as_str(), "gel-server" | "edgedb-server") {
        return None;
    }
    let installref = choose_installref(
        artifact_base,
        pkg,
        &[(Some("application/x-tar"), Some("zstd"))],
    )?;
    let version = parse_build_version(&pkg.version)?;
    let url = resolve_installref_url(artifact_base, &installref.path)?;
    let hash = installref_hash(installref)?;
    let key = ArtifactKey {
        kind: ArtifactKind::Server,
        name: pkg.basename.clone().into_boxed_str(),
        version: pkg.version.clone().into_boxed_str(),
        slot: pkg.slot.clone().into_boxed_str(),
    };
    Some(NormalizedArtifact::Server(
        key,
        ServerPackage {
            name: pkg.basename.clone(),
            version,
            url,
            size: installref.verification.size,
            hash,
            kind: PackageType::TarZst,
            slot: pkg.slot.clone(),
            tags: pkg.tags.clone(),
        },
    ))
}

fn normalize_cli(
    _index_source: &Source,
    artifact_base: &Source,
    pkg: &PackageData,
) -> Option<NormalizedArtifact> {
    if !matches!(pkg.basename.as_str(), "gel-cli" | "edgedb-cli") {
        return None;
    }

    let installref = choose_installref(
        artifact_base,
        pkg,
        &[(None, Some("zstd")), (None, Some("identity"))],
    )?;
    let version = pkg.version.parse::<ver::Semver>().ok()?;
    let url = resolve_installref_url(artifact_base, &installref.path)?;
    let hash = installref_hash(installref)?;
    let compression = if installref.encoding.as_deref() == Some("zstd") {
        Some(Compression::Zstd)
    } else {
        None
    };
    let key = ArtifactKey {
        kind: ArtifactKind::Cli,
        name: pkg.basename.clone().into_boxed_str(),
        version: pkg.version.clone().into_boxed_str(),
        slot: pkg.slot.clone().into_boxed_str(),
    };
    Some(NormalizedArtifact::Cli(
        key,
        CliPackage {
            version,
            url,
            size: installref.verification.size,
            hash,
            compression,
        },
    ))
}

fn normalize_extension(
    _index_source: &Source,
    artifact_base: &Source,
    pkg: &PackageData,
) -> Option<NormalizedArtifact> {
    let extension_name = pkg.tags.get("extension")?.clone();
    let installref = choose_installref(
        artifact_base,
        pkg,
        &[(Some("application/zip"), Some("identity"))],
    )?;
    let version = parse_build_version(&pkg.version)?;
    let url = resolve_installref_url(artifact_base, &installref.path)?;
    let hash = installref_hash(installref)?;
    let key = ArtifactKey {
        kind: ArtifactKind::Extension,
        name: extension_name.clone().into_boxed_str(),
        version: pkg.version.clone().into_boxed_str(),
        slot: pkg.slot.clone().into_boxed_str(),
    };
    Some(NormalizedArtifact::Extension(
        key,
        ExtensionPackage {
            name: extension_name,
            version,
            url,
            size: installref.verification.size,
            hash,
            kind: PackageType::Zip,
            slot: pkg.slot.clone(),
            tags: pkg.tags.clone(),
        },
    ))
}

fn choose_installref<'a>(
    artifact_base: &Source,
    pkg: &'a PackageData,
    candidates: &[(Option<&'static str>, Option<&'static str>)],
) -> Option<&'a InstallRef> {
    for (kind, encoding) in candidates {
        if let Some(installref) = pkg.installrefs.iter().find(|installref| {
            kind.is_none_or(|kind| installref.kind == kind)
                && installref.encoding.as_deref() == *encoding
                && installref
                    .verification
                    .blake2b
                    .as_deref()
                    .map(valid_blake2b)
                    .unwrap_or(false)
                && resolve_installref_url(artifact_base, &installref.path).is_some()
        }) {
            return Some(installref);
        }
    }
    None
}

fn resolve_installref_url(artifact_base: &Source, value: &str) -> Option<Url> {
    if let Ok(url) = Url::parse(value) {
        return match url.scheme() {
            "http" | "https" | "file" => Some(url),
            _ => None,
        };
    }

    match artifact_base {
        Source::Http(base) => base.join(value).ok(),
        Source::File(path) => {
            let path = if Path::new(value).is_absolute() {
                Path::new(value).to_path_buf()
            } else {
                path.parent().unwrap_or_else(|| Path::new(".")).join(value)
            };
            Url::from_file_path(path).ok()
        }
    }
}

fn installref_hash(installref: &InstallRef) -> Option<PackageHash> {
    let hash = installref.verification.blake2b.as_ref()?;
    if !valid_blake2b(hash) {
        return None;
    }
    Some(PackageHash::Blake2b(hash.clone().into_boxed_str()))
}

fn parse_build_version(value: &str) -> Option<ver::Build> {
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::registry::index::Verification;
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

    struct CapturingLogger(Arc<Mutex<Vec<String>>>);

    impl log::Log for CapturingLogger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Info
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                let mut messages = self
                    .0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                messages.push(format!("{}", record.args()));
            }
        }

        fn flush(&self) {}
    }

    static LOG_MESSAGES: LazyLock<Arc<Mutex<Vec<String>>>> = LazyLock::new(|| {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let logger = CapturingLogger(messages.clone());
        log::set_boxed_logger(Box::new(logger)).expect("failed to install capture logger");
        log::set_max_level(log::LevelFilter::Info);
        messages
    });
    static LOG_CAPTURE_LOCK: Mutex<()> = Mutex::new(());

    fn capture_info_logs() -> (Arc<Mutex<Vec<String>>>, MutexGuard<'static, ()>) {
        let guard = LOG_CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let messages = LOG_MESSAGES.clone();
        let mut message_buffer = messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        message_buffer.clear();
        drop(message_buffer);
        (messages, guard)
    }
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn package_data(basename: &str, version: &str, tags: HashMap<String, String>) -> PackageData {
        PackageData {
            basename: basename.to_owned(),
            version: version.to_owned(),
            slot: "7".to_owned(),
            tags,
            installrefs: vec![InstallRef {
                path: "https://example.com/package".to_owned(),
                kind: "application/x-tar".to_owned(),
                encoding: Some("zstd".to_owned()),
                verification: Verification {
                    size: 10,
                    blake2b: Some(HASH.to_owned()),
                },
            }],
        }
    }

    #[test]
    fn supported_portable_package_predicate_is_narrow() {
        for basename in ["gel-server", "edgedb-server", "gel-cli", "edgedb-cli"] {
            assert!(is_supported_portable_package(&package_data(
                basename,
                "7.0",
                HashMap::new(),
            )));
        }
        assert!(!is_supported_portable_package(&package_data(
            "apt-package",
            "7.0",
            HashMap::new(),
        )));
        assert!(is_supported_portable_package(&package_data(
            "custom-package",
            "7.0",
            HashMap::from([("extension".to_owned(), "custom".to_owned())]),
        )));
    }

    #[test]
    fn bare_supported_package_is_skipped_without_build_metadata() {
        let (_messages, _capture_guard) = capture_info_logs();
        let pkg = package_data("gel-server", "7.0", HashMap::new());
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());

        assert!(normalize_artifact(&source, &source, &pkg).is_none());
    }
    #[test]
    fn bare_supported_package_skip_emits_info_diagnostic() {
        let (messages, _capture_guard) = capture_info_logs();
        let pkg = package_data("gel-server", "7.0", HashMap::new());
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());

        assert!(normalize_artifact(&source, &source, &pkg).is_none());
        let messages = messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            messages
                .iter()
                .any(|message| message.contains("Skipping registry package"))
        );
    }

    #[test]
    fn unrelated_malformed_package_stays_quiet() {
        let (messages, _capture_guard) = capture_info_logs();
        let pkg = package_data("apt-package", "not-a-build", HashMap::new());
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        assert!(normalize_artifact(&source, &source, &pkg).is_none());
        let messages = messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            !messages
                .iter()
                .any(|message| message.contains("Skipping registry package"))
        );
    }

    #[tokio::test]
    async fn legacy_root_absolute_installref_resolves_against_root_host() {
        let server = wiremock::MockServer::start().await;
        let platform = "x86_64-unknown-linux-musl";
        let artifact_path = format!("/archive/{platform}/gel-cli-7.10.2.zst");
        let index_path = format!("/archive/.jsonindexes/{platform}.json");
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(index_path))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(format!(
                r#"{{"packages":[{{"basename":"gel-cli","version":"7.10.2","slot":"7","tags":{{}},"installrefs":[{{"ref":"{artifact_path}","type":"application/octet-stream","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )))
            .mount(&server)
            .await;

        let root = url::Url::parse(&format!("{}/packages/", server.uri())).unwrap();
        let config = Config {
            sources: vec![RegistrySource::LegacyPackageRoot(root.clone())],
        };
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, platform)
            .await
            .unwrap();

        assert_eq!(catalog.cli_packages().len(), 1);
        assert_eq!(
            catalog.cli_packages()[0].url,
            url::Url::parse(&format!("{}{}", server.uri(), artifact_path)).unwrap()
        );
    }
    #[tokio::test]
    async fn equivalent_duplicate_artifacts_keep_first_source() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let one = tmp.path().join("one.json");
        let two = tmp.path().join("two.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"one.json"},{"channel":"stable","platform":"linux","url":"two.json"}]}"#,
        )
        .unwrap();
        let body = format!(
            r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
        );
        fs_err::write(&one, &body).unwrap();
        fs_err::write(&two, &body).unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        let packages = catalog.server_packages();
        assert_eq!(packages.len(), 1);
        assert_eq!(
            packages[0].url,
            url::Url::parse("https://example.com/gel-server.tar.zst").unwrap()
        );
    }

    #[tokio::test]
    async fn mirror_duplicate_artifacts_keep_first_source() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let one = tmp.path().join("one.json");
        let two = tmp.path().join("two.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"one.json"},{"channel":"stable","platform":"linux","url":"two.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &one,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://mirror-one.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            ),
        )
        .unwrap();
        fs_err::write(
            &two,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://mirror-two.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        let packages = catalog.server_packages();
        assert_eq!(packages.len(), 1);
        assert_eq!(
            packages[0].url,
            url::Url::parse("https://mirror-one.example.com/gel-server.tar.zst").unwrap()
        );
    }

    #[tokio::test]
    async fn mirror_duplicate_diagnostic_names_ignored_artifact_url() {
        let (messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let one = tmp.path().join("one.json");
        let two = tmp.path().join("two.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"one.json"},{"channel":"stable","platform":"linux","url":"two.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &one,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://first.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            ),
        )
        .unwrap();
        fs_err::write(
            &two,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://ignored.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();
        Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        let messages = messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let diagnostic = messages
            .iter()
            .find(|message| message.contains("https://ignored.example.com/gel-server.tar.zst"))
            .expect("duplicate diagnostic");
        assert!(diagnostic.contains("https://first.example.com/gel-server.tar.zst"));
        assert!(diagnostic.contains("using first source"));
    }

    #[tokio::test]
    async fn conflicting_duplicate_artifact_hash_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let one = tmp.path().join("one.json");
        let two = tmp.path().join("two.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"one.json"},{"channel":"stable","platform":"linux","url":"two.json"}]}"#,
        )
        .unwrap();
        let other_hash = format!("b{}", &HASH[1..]);
        fs_err::write(
            &one,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://mirror-one.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            ),
        )
        .unwrap();
        fs_err::write(
            &two,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://mirror-two.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{other_hash}"}}}}]}}]}}"#
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();
        let error = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("conflicting registry artifact"));
        assert!(message.contains("hash"));
        assert!(message.contains("mirror-one.example.com"));
        assert!(message.contains("mirror-two.example.com"));
    }

    #[tokio::test]
    async fn conflicting_duplicate_artifact_size_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let one = tmp.path().join("one.json");
        let two = tmp.path().join("two.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"one.json"},{"channel":"stable","platform":"linux","url":"two.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &one,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://size-one.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            ),
        )
        .unwrap();
        fs_err::write(
            &two,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://size-two.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":11,"blake2b":"{HASH}"}}}}]}}]}}"#
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();
        let error = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("conflicting registry artifact"));
        assert!(message.contains("size"));
        assert!(message.contains("size-one.example.com"));
        assert!(message.contains("size-two.example.com"));
    }

    #[tokio::test]
    async fn cli_and_extension_mirror_duplicates_keep_first_source() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let one = tmp.path().join("one.json");
        let two = tmp.path().join("two.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"one.json"},{"channel":"stable","platform":"linux","url":"two.json"}]}"#,
        )
        .unwrap();
        let index = |cli_url: &str, extension_url: &str| {
            format!(
                r#"{{"packages":[
                    {{"basename":"gel-cli","version":"7.0.0","slot":"7","tags":{{}},"installrefs":[{{"ref":"{cli_url}","type":"application/octet-stream","encoding":"zstd","verification":{{"size":11,"blake2b":"{HASH}"}}}}]}},
                    {{"basename":"ignored-package","version":"7.0+abcdef0","slot":"7","tags":{{"extension":"local-example","server_slot":"7"}},"installrefs":[{{"ref":"{extension_url}","type":"application/zip","encoding":"identity","verification":{{"size":12,"blake2b":"{HASH}"}}}}]}}
                ]}}"#
            )
        };
        fs_err::write(
            &one,
            index(
                "https://cli-one.example.com/gel-cli.zst",
                "https://extension-one.example.com/local-example.zip",
            ),
        )
        .unwrap();
        fs_err::write(
            &two,
            index(
                "https://cli-two.example.com/gel-cli.zst",
                "https://extension-two.example.com/local-example.zip",
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(catalog.cli_packages().len(), 1);
        assert_eq!(
            catalog.cli_packages()[0].url,
            url::Url::parse("https://cli-one.example.com/gel-cli.zst").unwrap()
        );
        assert_eq!(catalog.extension_packages("7").len(), 1);
        assert_eq!(
            catalog.extension_packages("7")[0].url,
            url::Url::parse("https://extension-one.example.com/local-example.zip").unwrap()
        );
    }
    async fn catalog_with_http_source_status(status: u16) -> anyhow::Result<Catalog> {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/unavailable.json"))
            .respond_with(wiremock::ResponseTemplate::new(status))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir()?;
        let manifest = tmp.path().join("registry.json");
        let index = tmp.path().join("stable-linux.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"stable-linux.json"}]}"#,
        )?;
        fs_err::write(
            &index,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )?;

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::Http(
                    format!("{}/unavailable.json", server.uri()).parse()?,
                )),
                RegistrySource::Manifest(Source::File(manifest)),
            ],
        };
        let loader = SourceLoader::new()?;
        Catalog::load(&config, &loader, Channel::Stable, "linux").await
    }

    #[tokio::test]
    async fn unavailable_http_sources_allow_later_source_artifacts() {
        for status in [429, 500, 502, 503, 599] {
            let catalog = catalog_with_http_source_status(status).await.unwrap();
            assert_eq!(catalog.server_packages().len(), 1, "HTTP {status}");
        }
    }

    #[tokio::test]
    async fn non_unavailability_http_source_errors_are_hard() {
        let error = catalog_with_http_source_status(403).await.unwrap_err();
        assert!(format!("{error:#}").contains("HTTP 403"));
    }

    #[tokio::test]
    async fn later_sources_load_after_earlier_source_failure() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let missing_manifest = tmp.path().join("missing-registry.json");
        let manifest = tmp.path().join("registry.json");
        let index = tmp.path().join("stable-linux.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"stable-linux.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &index,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(missing_manifest)),
                RegistrySource::Manifest(Source::File(manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(catalog.server_packages().len(), 1);
    }

    #[tokio::test]
    async fn loads_local_registry_packages_for_all_package_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let index = tmp.path().join("stable-linux.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"stable-linux.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &index,
            format!(
                r#"{{
                    "packages": [
                        {{
                            "basename": "gel-server",
                            "version": "7.0+abcdef0",
                            "slot": "7",
                            "tags": {{}},
                            "installrefs": [
                                {{
                                    "ref": "https://example.com/gel-server.tar.zst",
                                    "type": "application/x-tar",
                                    "encoding": "zstd",
                                    "verification": {{
                                        "size": 10,
                                        "blake2b": "{HASH}"
                                    }}
                                }}
                            ]
                        }},
                        {{
                            "basename": "gel-cli",
                            "version": "7.0.0",
                            "slot": "7",
                            "tags": {{}},
                            "installrefs": [
                                {{
                                    "ref": "https://example.com/gel-cli.zst",
                                    "type": "application/octet-stream",
                                    "encoding": "zstd",
                                    "verification": {{
                                        "size": 11,
                                        "blake2b": "{HASH}"
                                    }}
                                }}
                            ]
                        }},
                        {{
                            "basename": "ignored-package",
                            "version": "7.0+abcdef0",
                            "slot": "7",
                            "tags": {{
                                "extension": "local-example",
                                "server_slot": "7"
                            }},
                            "installrefs": [
                                {{
                                    "ref": "https://example.com/local-example.zip",
                                    "type": "application/zip",
                                    "encoding": "identity",
                                    "verification": {{
                                        "size": 12,
                                        "blake2b": "{HASH}"
                                    }}
                                }}
                            ]
                        }}
                    ]
                }}"#
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(catalog.server_packages().len(), 1);
        assert_eq!(catalog.cli_packages().len(), 1);
        assert_eq!(catalog.extension_packages("7").len(), 1);
        assert!(catalog.extension_packages("8").is_empty());
    }

    #[tokio::test]
    async fn non_missing_source_errors_are_not_masked_by_earlier_missing_sources() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let missing_manifest = tmp.path().join("missing-registry.json");
        let manifest = tmp.path().join("registry.json");
        let broken_index = tmp.path().join("broken-index.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"broken-index.json"}]}"#,
        )
        .unwrap();
        fs_err::write(&broken_index, b"{not-json").unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(missing_manifest)),
                RegistrySource::Manifest(Source::File(manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let err = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("failed to parse registry index"));
        assert!(!is_missing_source_error(&err));
    }

    #[tokio::test]
    async fn missing_manifest_source_returns_error() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let missing_manifest = tmp.path().join("missing-registry.json");
        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(
                missing_manifest.clone(),
            ))],
        };
        let loader = SourceLoader::new().unwrap();

        let error = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to load registry manifest")
        );
        assert!(
            error
                .to_string()
                .contains(&missing_manifest.display().to_string())
        );
    }

    #[tokio::test]
    async fn loaded_manifest_with_no_matching_index_returns_empty_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"darwin","url":"stable-darwin.json"}]}"#,
        )
        .unwrap();
        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();

        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert!(catalog.server_packages().is_empty());
        assert!(catalog.cli_packages().is_empty());
        assert!(catalog.extension_packages("7").is_empty());
    }

    #[tokio::test]
    async fn network_manifest_failure_continues_when_later_source_has_artifacts() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let index = tmp.path().join("stable-linux.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"stable-linux.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &index,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();
        let unreachable = Source::Http(Url::parse("http://127.0.0.1:1/registry.json").unwrap());
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(unreachable),
                RegistrySource::Manifest(Source::File(manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();

        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(catalog.server_packages().len(), 1);
    }

    #[tokio::test]
    async fn malformed_second_source_fails_after_valid_artifacts() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let valid_manifest = tmp.path().join("valid-registry.json");
        let valid_index = tmp.path().join("stable-linux.json");
        let malformed_manifest = tmp.path().join("malformed-registry.json");
        fs_err::write(
            &valid_manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"stable-linux.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &valid_index,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();
        fs_err::write(&malformed_manifest, b"{not-json").unwrap();
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(valid_manifest)),
                RegistrySource::Manifest(Source::File(malformed_manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();

        let error = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to parse registry manifest")
        );
    }
    #[tokio::test]
    async fn missing_selected_index_continues_when_later_source_has_artifacts() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let missing_manifest = tmp.path().join("missing-index-registry.json");
        let manifest = tmp.path().join("registry.json");
        let index = tmp.path().join("stable-linux.json");
        fs_err::write(
            &missing_manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"missing-index.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"stable-linux.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &index,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(missing_manifest)),
                RegistrySource::Manifest(Source::File(manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(catalog.server_packages().len(), 1);
    }

    #[tokio::test]
    async fn network_selected_index_failure_continues_when_later_source_has_artifacts() {
        let (_messages, _capture_guard) = capture_info_logs();
        let tmp = tempfile::tempdir().unwrap();
        let unreachable_index_manifest = tmp.path().join("network-index-registry.json");
        let manifest = tmp.path().join("registry.json");
        let index = tmp.path().join("stable-linux.json");
        fs_err::write(
            &unreachable_index_manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"http://127.0.0.1:1/missing-index.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"linux","url":"stable-linux.json"}]}"#,
        )
        .unwrap();
        fs_err::write(
            &index,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(unreachable_index_manifest)),
                RegistrySource::Manifest(Source::File(manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(catalog.server_packages().len(), 1);
    }
}
