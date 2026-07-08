//! Composed package catalog construction and query helpers.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Context;
use url::Url;

use super::config::Config;
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
    slot: Option<Box<str>>,
}

#[derive(Clone, Debug)]
enum NormalizedArtifact {
    Server(ArtifactKey, ServerPackage),
    Cli(ArtifactKey, CliPackage),
    Extension(ArtifactKey, ExtensionPackage),
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
        let mut seen = HashSet::new();
        let mut first_missing_error: Option<anyhow::Error> = None;
        let mut first_fatal_error: Option<anyhow::Error> = None;

        for manifest_source in &config.sources {
            let manifest_bytes = match loader.load_manifest(manifest_source).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    let missing = is_missing_source_failure(&error);
                    let error = Err::<Vec<u8>, _>(error)
                        .context(format!(
                            "failed to load registry manifest {}",
                            manifest_source.display()
                        ))
                        .unwrap_err();
                    log::warn!("{error:#}");
                    if missing {
                        if first_missing_error.is_none() {
                            first_missing_error = Some(error);
                        }
                    } else if first_fatal_error.is_none() {
                        first_fatal_error = Some(error);
                    }
                    continue;
                }
            };
            let manifest = match Manifest::from_slice(&manifest_bytes) {
                Ok(manifest) => manifest,
                Err(error) => {
                    let error = Err::<Manifest, _>(error)
                        .context(format!(
                            "failed to parse registry manifest {}",
                            manifest_source.display()
                        ))
                        .unwrap_err();
                    log::warn!("{error:#}");
                    if first_fatal_error.is_none() {
                        first_fatal_error = Some(error);
                    }
                    continue;
                }
            };

            let index_sources = match manifest.select_indexes(manifest_source, channel, platform) {
                Ok(index_sources) => index_sources,
                Err(error) => {
                    let error = Err::<Vec<Source>, _>(error)
                        .context(format!(
                            "failed to select registry indexes from {}",
                            manifest_source.display()
                        ))
                        .unwrap_err();
                    log::warn!("{error:#}");
                    if first_fatal_error.is_none() {
                        first_fatal_error = Some(error);
                    }
                    continue;
                }
            };

            for index_source in index_sources {
                let index_bytes = match loader.load_index(&index_source).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let missing = is_missing_source_failure(&error);
                        let error = Err::<Vec<u8>, _>(error)
                            .context(format!(
                                "failed to load registry index {}",
                                index_source.display()
                            ))
                            .unwrap_err();
                        log::warn!("{error:#}");
                        if missing {
                            if first_missing_error.is_none() {
                                first_missing_error = Some(error);
                            }
                        } else if first_fatal_error.is_none() {
                            first_fatal_error = Some(error);
                        }
                        continue;
                    }
                };
                let index = match IndexDocument::from_slice(&index_bytes) {
                    Ok(index) => index,
                    Err(error) => {
                        let error = Err::<IndexDocument, _>(error)
                            .context(format!(
                                "failed to parse registry index {}",
                                index_source.display()
                            ))
                            .unwrap_err();
                        log::warn!("{error:#}");
                        if first_fatal_error.is_none() {
                            first_fatal_error = Some(error);
                        }
                        continue;
                    }
                };

                for pkg in &index.packages {
                    if let Some(artifact) = normalize_artifact(&index_source, pkg) {
                        let key = artifact.key().clone();
                        if !seen.insert(key.clone()) {
                            anyhow::bail!("duplicate package artifact: {:?}", key);
                        }
                        artifacts.push(artifact);
                    }
                }
            }
        }

        if artifacts.is_empty() {
            if let Some(error) = first_fatal_error {
                return Err(error);
            }
            if let Some(error) = first_missing_error {
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

pub async fn load_default(channel: Channel, platform: &str) -> anyhow::Result<Catalog> {
    let config = crate::portable::registry::Config::load()?;
    let loader = crate::portable::registry::SourceLoader::new(
        crate::portable::registry::config::registry_http_cache_dir()?,
    )?;
    Catalog::load(&config, &loader, channel, platform).await
}

pub async fn load_default_or_empty(channel: Channel, platform: &str) -> anyhow::Result<Catalog> {
    match load_default(channel, platform).await {
        Ok(catalog) => Ok(catalog),
        Err(error) if is_missing_source_error(&error) => Ok(Catalog::default()),
        Err(error) => Err(error),
    }
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

fn normalize_artifact(index_source: &Source, pkg: &PackageData) -> Option<NormalizedArtifact> {
    if let Some(package) = normalize_server(index_source, pkg) {
        return Some(package);
    }
    if let Some(package) = normalize_cli(index_source, pkg) {
        return Some(package);
    }
    normalize_extension(index_source, pkg)
}

fn normalize_server(index_source: &Source, pkg: &PackageData) -> Option<NormalizedArtifact> {
    if !matches!(pkg.basename.as_str(), "gel-server" | "edgedb-server") {
        return None;
    }
    let installref = choose_installref(
        index_source,
        pkg,
        &[(Some("application/x-tar"), Some("zstd"))],
    )?;
    let version = parse_build_version(&pkg.version)?;
    let url = resolve_installref_url(index_source, &installref.path)?;
    let hash = installref_hash(installref)?;
    let key = ArtifactKey {
        kind: ArtifactKind::Server,
        name: pkg.basename.clone().into_boxed_str(),
        version: pkg.version.clone().into_boxed_str(),
        slot: Some(pkg.slot.clone().into_boxed_str()),
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

fn normalize_cli(index_source: &Source, pkg: &PackageData) -> Option<NormalizedArtifact> {
    if !matches!(pkg.basename.as_str(), "gel-cli" | "edgedb-cli") {
        return None;
    }

    let installref = choose_installref(
        index_source,
        pkg,
        &[(None, Some("zstd")), (None, Some("identity"))],
    )?;
    let version = pkg.version.parse::<ver::Semver>().ok()?;
    let url = resolve_installref_url(index_source, &installref.path)?;
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
        slot: Some(pkg.slot.clone().into_boxed_str()),
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

fn normalize_extension(index_source: &Source, pkg: &PackageData) -> Option<NormalizedArtifact> {
    let extension_name = pkg.tags.get("extension")?.clone();
    let installref = choose_installref(
        index_source,
        pkg,
        &[(Some("application/zip"), Some("identity"))],
    )?;
    let version = parse_build_version(&pkg.version)?;
    let url = resolve_installref_url(index_source, &installref.path)?;
    let hash = installref_hash(installref)?;
    let key = ArtifactKey {
        kind: ArtifactKind::Extension,
        name: extension_name.clone().into_boxed_str(),
        version: pkg.version.clone().into_boxed_str(),
        slot: Some(pkg.slot.clone().into_boxed_str()),
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
    index_source: &Source,
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
                && resolve_installref_url(index_source, &installref.path).is_some()
        }) {
            return Some(installref);
        }
    }
    None
}

fn resolve_installref_url(index_source: &Source, value: &str) -> Option<Url> {
    if let Ok(url) = Url::parse(value) {
        return match url.scheme() {
            "http" | "https" | "file" => Some(url),
            _ => None,
        };
    }

    match index_source {
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
    value.parse().ok().or_else(|| {
        value
            .parse::<ver::Specific>()
            .ok()
            .and_then(|specific| format!("{specific}+local").parse().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::registry::{Channel, Config, Source, SourceLoader};

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[tokio::test]
    async fn loads_server_package_from_local_manifest_and_index() {
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
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![Source::File(manifest)],
        };
        let loader = SourceLoader::new(tmp.path().join("cache")).unwrap();
        let catalog = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(catalog.server_packages().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_artifact_keys_are_conflicts() {
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
            r#"{{"packages":[{{"basename":"gel-server","version":"7.0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
        );
        fs_err::write(&one, &body).unwrap();
        fs_err::write(&two, &body).unwrap();

        let config = Config {
            sources: vec![Source::File(manifest)],
        };
        let loader = SourceLoader::new(tmp.path().join("cache")).unwrap();
        let err = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("duplicate package artifact"));
    }

    #[tokio::test]
    async fn duplicate_artifact_identity_conflicts_across_different_urls() {
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
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://mirror-one.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();
        fs_err::write(
            &two,
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://mirror-two.example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![Source::File(manifest)],
        };
        let loader = SourceLoader::new(tmp.path().join("cache")).unwrap();
        let err = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("duplicate package artifact"));
    }

    #[tokio::test]
    async fn later_sources_load_after_earlier_source_failure() {
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
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#,
            ),
        )
        .unwrap();

        let config = Config {
            sources: vec![Source::File(missing_manifest), Source::File(manifest)],
        };
        let loader = SourceLoader::new(tmp.path().join("cache")).unwrap();
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
                            "version": "7.0",
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
                            "version": "7.0",
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
            sources: vec![Source::File(manifest)],
        };
        let loader = SourceLoader::new(tmp.path().join("cache")).unwrap();
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
            sources: vec![Source::File(missing_manifest), Source::File(manifest)],
        };
        let loader = SourceLoader::new(tmp.path().join("cache")).unwrap();
        let err = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("failed to parse registry index"));
        assert!(!is_missing_source_error(&err));
    }
}
