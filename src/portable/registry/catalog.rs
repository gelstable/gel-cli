//! Composed package catalog construction and query helpers.

use std::collections::HashMap;
use std::future::Future;

use futures_util::StreamExt;

use super::config::{Config, RegistrySource};
use super::index::{IndexDocument, ValidatedArtifact};
use super::manifest::Manifest;
use super::source::{Source, SourceLoader};
use super::types::{ArtifactIdentity, Channel, CliPackage, ExtensionPackage, ServerPackage};

const MAX_CONCURRENT_SOURCE_LOADS: usize = 4;

async fn collect_bounded_ordered<T, U, F, Fut>(values: Vec<T>, limit: usize, operation: F) -> Vec<U>
where
    F: Fn(T) -> Fut,
    Fut: Future<Output = U>,
{
    let mut completed = futures_util::stream::iter(values.into_iter().enumerate())
        .map(|(index, value)| {
            let future = operation(value);
            async move { (index, future.await) }
        })
        .buffer_unordered(limit)
        .collect::<Vec<_>>()
        .await;
    completed.sort_by_key(|(index, _)| *index);
    completed.into_iter().map(|(_, value)| value).collect()
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
    artifacts: Vec<ValidatedArtifact>,
}
#[derive(Debug)]
pub(crate) struct CatalogLoad {
    pub(crate) catalog: Catalog,
    pub(crate) source_reports: Vec<SourceReport>,
}

#[derive(Debug)]
pub(crate) enum SourceReport {
    Healthy {
        source: RegistrySource,
        selected_indexes: usize,
        artifact_count: usize,
    },
    Unavailable {
        source: RegistrySource,
        error: anyhow::Error,
    },
    Rejected {
        source: RegistrySource,
        error: anyhow::Error,
    },
}

impl SourceReport {
    pub(crate) fn warning(&self) -> Option<String> {
        match self {
            SourceReport::Healthy { .. } => None,
            SourceReport::Unavailable { source, error } => Some(format!(
                "registry source {source} is unavailable: {error:#}",
            )),
            SourceReport::Rejected { source, error } => {
                Some(format!("registry source {source} was rejected: {error:#}",))
            }
        }
    }
}

pub(crate) fn for_each_degraded_report<F>(reports: &[SourceReport], mut on_report: F)
where
    F: FnMut(&SourceReport),
{
    let mut reported_sources = Vec::<RegistrySource>::new();
    for report in reports {
        let source = match report {
            SourceReport::Healthy { .. } => continue,
            SourceReport::Unavailable { source, .. } | SourceReport::Rejected { source, .. } => {
                source
            }
        };
        if reported_sources
            .iter()
            .any(|reported_source| reported_source == source)
        {
            continue;
        }
        reported_sources.push(source.clone());
        on_report(report);
    }
}

#[derive(Debug, thiserror::Error)]
#[error("no healthy registry sources: {summary}")]
pub(crate) struct NoHealthySources {
    reports: Vec<SourceReport>,
    summary: String,
}

impl NoHealthySources {
    fn new(reports: Vec<SourceReport>) -> Self {
        let summary = reports
            .iter()
            .filter_map(SourceReport::warning)
            .collect::<Vec<_>>()
            .join("; ");
        Self { reports, summary }
    }

    #[cfg(test)]
    pub(crate) fn reports(&self) -> &[SourceReport] {
        &self.reports
    }
}

impl Catalog {
    pub async fn load(
        config: &Config,
        loader: &SourceLoader,
        channel: Channel,
        platform: &str,
    ) -> anyhow::Result<CatalogLoad> {
        let source_inputs = config
            .sources
            .iter()
            .cloned()
            .enumerate()
            .collect::<Vec<_>>();
        let resolved = collect_bounded_ordered(
            source_inputs,
            MAX_CONCURRENT_SOURCE_LOADS,
            |(source_ordinal, registry_source)| async move {
                resolve_configured_source(
                    source_ordinal,
                    registry_source,
                    loader,
                    channel,
                    platform,
                )
                .await
            },
        )
        .await;

        let index_inputs = resolved
            .iter()
            .flat_map(|outcome| match outcome {
                ResolvedSourceOutcome::Ready { indexes, .. } => indexes.clone(),
                ResolvedSourceOutcome::Unavailable { .. }
                | ResolvedSourceOutcome::Rejected { .. } => Vec::new(),
            })
            .collect::<Vec<_>>();
        let validated_indexes = collect_bounded_ordered(
            index_inputs,
            MAX_CONCURRENT_SOURCE_LOADS,
            |input| async move { load_and_validate_index(loader, input, platform).await },
        )
        .await;

        let mut accumulators = resolved
            .into_iter()
            .map(SourceAccumulator::from_resolved)
            .collect::<Vec<_>>();
        for outcome in validated_indexes {
            let source_ordinal = outcome.source_ordinal();
            accumulators[source_ordinal].record_index(outcome);
        }

        let mut artifacts = Vec::new();
        let mut seen: HashMap<ArtifactIdentity, CatalogEntry> = HashMap::new();
        let mut healthy_sources = 0usize;
        let mut source_reports = Vec::with_capacity(config.sources.len());

        for outcome in accumulators.into_iter().map(SourceAccumulator::finish) {
            match outcome {
                ConfiguredSourceOutcome::Healthy {
                    source,
                    selected_indexes,
                    artifacts: source_artifacts,
                } => {
                    healthy_sources += 1;
                    let artifact_count = source_artifacts.len();
                    compose_source_artifacts(&mut artifacts, &mut seen, source_artifacts)?;
                    source_reports.push(SourceReport::Healthy {
                        source,
                        selected_indexes,
                        artifact_count,
                    });
                }
                ConfiguredSourceOutcome::Unavailable { source, error } => {
                    source_reports.push(SourceReport::Unavailable { source, error });
                }
                ConfiguredSourceOutcome::Rejected { source, error } => {
                    source_reports.push(SourceReport::Rejected { source, error });
                }
            }
        }

        if healthy_sources == 0 {
            return Err(NoHealthySources::new(source_reports).into());
        }

        Ok(CatalogLoad {
            catalog: Catalog { artifacts },
            source_reports,
        })
    }

    pub fn server_packages(&self) -> Vec<ServerPackage> {
        let mut packages = self
            .artifacts
            .iter()
            .filter_map(|artifact| match artifact {
                ValidatedArtifact::Server(_, package) => Some(package.clone()),
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
                ValidatedArtifact::Cli(_, package) => Some(package.clone()),
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
                ValidatedArtifact::Extension(_, package)
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
    let load = Catalog::load(&config, &loader, channel, platform).await?;
    for report in &load.source_reports {
        if let SourceReport::Healthy {
            source,
            selected_indexes,
            artifact_count,
        } = report
        {
            log::debug!(
                "registry source {source} loaded {selected_indexes} indexes and {artifact_count} artifacts"
            );
        }
    }
    for_each_degraded_report(&load.source_reports, |report| {
        if let Some(warning) = report.warning() {
            log::warn!("{warning}");
        }
    });
    Ok(load.catalog)
}

enum ConfiguredSourceOutcome {
    Healthy {
        source: RegistrySource,
        selected_indexes: usize,
        artifacts: Vec<(Source, ValidatedArtifact)>,
    },
    Unavailable {
        source: RegistrySource,
        error: anyhow::Error,
    },
    Rejected {
        source: RegistrySource,
        error: anyhow::Error,
    },
}

#[derive(Clone)]
struct SelectedIndexInput {
    source_ordinal: usize,
    index_ordinal: usize,
    index_source: Source,
    artifact_base: Source,
}

enum ResolvedSourceOutcome {
    Ready {
        source: RegistrySource,
        indexes: Vec<SelectedIndexInput>,
    },
    Unavailable {
        source: RegistrySource,
        error: anyhow::Error,
    },
    Rejected {
        source: RegistrySource,
        error: anyhow::Error,
    },
}

enum ValidatedIndexOutcome {
    Healthy {
        source_ordinal: usize,
        index_ordinal: usize,
        index_source: Source,
        artifacts: Vec<ValidatedArtifact>,
    },
    Unavailable {
        source_ordinal: usize,
        index_ordinal: usize,
        error: anyhow::Error,
    },
    Rejected {
        source_ordinal: usize,
        index_ordinal: usize,
        error: anyhow::Error,
    },
}

impl ValidatedIndexOutcome {
    fn source_ordinal(&self) -> usize {
        match self {
            Self::Healthy { source_ordinal, .. }
            | Self::Unavailable { source_ordinal, .. }
            | Self::Rejected { source_ordinal, .. } => *source_ordinal,
        }
    }

    fn index_ordinal(&self) -> usize {
        match self {
            Self::Healthy { index_ordinal, .. }
            | Self::Unavailable { index_ordinal, .. }
            | Self::Rejected { index_ordinal, .. } => *index_ordinal,
        }
    }
}

enum IndexFailure {
    Unavailable(anyhow::Error),
    Rejected(anyhow::Error),
}

struct SourceAccumulator {
    source: RegistrySource,
    selected_indexes: usize,
    next_index_ordinal: usize,
    artifacts: Vec<(Source, ValidatedArtifact)>,
    failure: Option<IndexFailure>,
}

impl SourceAccumulator {
    fn from_resolved(outcome: ResolvedSourceOutcome) -> Self {
        match outcome {
            ResolvedSourceOutcome::Ready { source, indexes } => Self {
                source,
                selected_indexes: indexes.len(),
                next_index_ordinal: 0,
                artifacts: Vec::new(),
                failure: None,
            },
            ResolvedSourceOutcome::Unavailable { source, error } => Self {
                source,
                selected_indexes: 0,
                artifacts: Vec::new(),
                next_index_ordinal: 0,
                failure: Some(IndexFailure::Unavailable(error)),
            },
            ResolvedSourceOutcome::Rejected { source, error } => Self {
                source,
                selected_indexes: 0,
                artifacts: Vec::new(),
                next_index_ordinal: 0,
                failure: Some(IndexFailure::Rejected(error)),
            },
        }
    }

    fn record_index(&mut self, outcome: ValidatedIndexOutcome) {
        let index_ordinal = outcome.index_ordinal();
        debug_assert_eq!(self.next_index_ordinal, index_ordinal);
        self.next_index_ordinal += 1;
        match outcome {
            ValidatedIndexOutcome::Healthy {
                index_source,
                artifacts,
                ..
            } => {
                if self.failure.is_none() {
                    self.artifacts.extend(
                        artifacts
                            .into_iter()
                            .map(|artifact| (index_source.clone(), artifact)),
                    );
                }
            }
            ValidatedIndexOutcome::Unavailable { error, .. } => {
                if self.failure.is_none() {
                    self.artifacts.clear();
                    self.failure = Some(IndexFailure::Unavailable(error));
                }
            }
            ValidatedIndexOutcome::Rejected { error, .. } => {
                if self.failure.is_none() {
                    self.artifacts.clear();
                    self.failure = Some(IndexFailure::Rejected(error));
                }
            }
        }
    }

    fn finish(self) -> ConfiguredSourceOutcome {
        match self.failure {
            None => ConfiguredSourceOutcome::Healthy {
                source: self.source,
                selected_indexes: self.selected_indexes,
                artifacts: self.artifacts,
            },
            Some(IndexFailure::Unavailable(error)) => ConfiguredSourceOutcome::Unavailable {
                source: self.source,
                error,
            },
            Some(IndexFailure::Rejected(error)) => ConfiguredSourceOutcome::Rejected {
                source: self.source,
                error,
            },
        }
    }
}

async fn resolve_configured_source(
    source_ordinal: usize,
    registry_source: RegistrySource,
    loader: &SourceLoader,
    channel: Channel,
    platform: &str,
) -> ResolvedSourceOutcome {
    let source = registry_source.clone();
    let indexes = match &registry_source {
        RegistrySource::Manifest(manifest_source) => {
            let manifest_bytes = match loader.load_manifest(manifest_source).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return ResolvedSourceOutcome::Unavailable {
                        source,
                        error: anyhow::Error::new(error).context(format!(
                            "failed to load registry manifest {}",
                            manifest_source.display()
                        )),
                    };
                }
            };
            let manifest = match Manifest::from_slice(&manifest_bytes) {
                Ok(manifest) => manifest,
                Err(error) => {
                    return ResolvedSourceOutcome::Rejected {
                        source,
                        error: error.context(format!(
                            "failed to parse registry manifest {}",
                            manifest_source.display()
                        )),
                    };
                }
            };
            let sources = match manifest.select_indexes(manifest_source, channel, platform) {
                Ok(sources) => sources,
                Err(error) => {
                    return ResolvedSourceOutcome::Rejected {
                        source,
                        error: error.context(format!(
                            "failed to select registry indexes from {}",
                            manifest_source.display()
                        )),
                    };
                }
            };
            sources
                .into_iter()
                .enumerate()
                .map(|(index_ordinal, index_source)| SelectedIndexInput {
                    source_ordinal,
                    index_ordinal,
                    artifact_base: index_source.clone(),
                    index_source,
                })
                .collect::<Vec<_>>()
        }
        RegistrySource::LegacyPackageRoot(root) => {
            let index_source = match super::source::legacy_index_url(root, platform, channel) {
                Ok(url) => Source::Http(url),
                Err(error) => {
                    return ResolvedSourceOutcome::Rejected {
                        source,
                        error: error.context(format!(
                            "failed to construct legacy registry index for {root}"
                        )),
                    };
                }
            };
            let artifact_base = Source::Http(root.clone());
            vec![SelectedIndexInput {
                source_ordinal,
                index_ordinal: 0,
                index_source,
                artifact_base,
            }]
        }
    };

    ResolvedSourceOutcome::Ready { source, indexes }
}

async fn load_and_validate_index(
    loader: &SourceLoader,
    input: SelectedIndexInput,
    platform: &str,
) -> ValidatedIndexOutcome {
    let SelectedIndexInput {
        source_ordinal,
        index_ordinal,
        index_source,
        artifact_base,
    } = input;
    let index_bytes = match loader.load_index(&index_source).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return ValidatedIndexOutcome::Unavailable {
                source_ordinal,
                index_ordinal,
                error: anyhow::Error::new(error).context(format!(
                    "failed to load registry index {}",
                    index_source.display()
                )),
            };
        }
    };
    let index = match IndexDocument::from_slice(&index_bytes) {
        Ok(index) => index,
        Err(error) => {
            return ValidatedIndexOutcome::Rejected {
                source_ordinal,
                index_ordinal,
                error: error.context(format!(
                    "failed to parse registry index {}",
                    index_source.display()
                )),
            };
        }
    };
    let validated = match index.validate(&index_source, &artifact_base, platform) {
        Ok(validated) => validated,
        Err(error) => {
            return ValidatedIndexOutcome::Rejected {
                source_ordinal,
                index_ordinal,
                error: anyhow::Error::new(error).context(format!(
                    "failed to validate registry index {}",
                    index_source.display()
                )),
            };
        }
    };

    ValidatedIndexOutcome::Healthy {
        source_ordinal,
        index_ordinal,
        index_source,
        artifacts: validated.into_artifacts(),
    }
}

fn compose_source_artifacts(
    artifacts: &mut Vec<ValidatedArtifact>,
    seen: &mut HashMap<ArtifactIdentity, CatalogEntry>,
    source_artifacts: Vec<(Source, ValidatedArtifact)>,
) -> anyhow::Result<()> {
    for (index_source, artifact) in source_artifacts {
        let identity = artifact.identity().clone();
        if let Some(first_entry) = seen.get(&identity) {
            let first_artifact = &artifacts[first_entry.artifact_index];
            match duplicate_relation(first_artifact, &artifact) {
                DuplicateRelation::EquivalentMirror => continue,
                DuplicateRelation::Conflict { fields } => {
                    anyhow::bail!(
                        "conflicting registry artifact {identity}: {} ({}) disagrees with {} ({}) on {}; refusing to choose between sources",
                        index_source.display(),
                        artifact.url(),
                        first_entry.index_source.display(),
                        first_artifact.url(),
                        fields.join(", "),
                    );
                }
            }
        }
        seen.insert(
            identity,
            CatalogEntry {
                artifact_index: artifacts.len(),
                index_source,
            },
        );
        artifacts.push(artifact);
    }
    Ok(())
}

fn duplicate_relation(first: &ValidatedArtifact, later: &ValidatedArtifact) -> DuplicateRelation {
    let mut fields = Vec::new();
    match (first, later) {
        (ValidatedArtifact::Server(_, first), ValidatedArtifact::Server(_, later)) => {
            if first.size != later.size {
                fields.push("size");
            }
            if first.hash != later.hash {
                fields.push("hash");
            }
            if first.tags != later.tags {
                fields.push("tags");
            }
        }
        (ValidatedArtifact::Cli(_, first), ValidatedArtifact::Cli(_, later)) => {
            if first.size != later.size {
                fields.push("size");
            }
            if first.hash != later.hash {
                fields.push("hash");
            }
            if first.compression_variant() != later.compression_variant() {
                fields.push("compression");
            }
        }
        (ValidatedArtifact::Extension(_, first), ValidatedArtifact::Extension(_, later)) => {
            if first.size != later.size {
                fields.push("size");
            }
            if first.hash != later.hash {
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

impl CliPackage {
    fn compression_variant(&self) -> Option<&'static str> {
        self.compression.as_ref().map(|_| "zstd")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn write_manifest_with_indexes(
        directory: &std::path::Path,
        manifest_name: &str,
        indexes: &[&str],
    ) -> std::path::PathBuf {
        let manifest = directory.join(manifest_name);
        let indexes = indexes
            .iter()
            .map(|url| {
                serde_json::json!({
                    "channel": "stable",
                    "platform": "linux",
                    "url": url,
                })
            })
            .collect::<Vec<_>>();
        fs_err::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "indexes": indexes,
            }))
            .unwrap(),
        )
        .unwrap();
        manifest
    }

    fn server_index(hash: &str) -> String {
        format!(
            r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{hash}"}}}}]}}]}}"#
        )
    }

    #[test]
    fn degraded_report_callback_deduplicates_equal_sources() {
        let source = RegistrySource::Manifest(Source::File("missing.json".into()));
        let reports = vec![
            SourceReport::Healthy {
                source: source.clone(),
                selected_indexes: 0,
                artifact_count: 0,
            },
            SourceReport::Unavailable {
                source: source.clone(),
                error: anyhow::anyhow!("first failure"),
            },
            SourceReport::Unavailable {
                source,
                error: anyhow::anyhow!("duplicate failure"),
            },
        ];
        let mut warnings = Vec::new();
        for_each_degraded_report(&reports, |report| {
            if let Some(warning) = report.warning() {
                warnings.push(warning);
            }
        });

        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn degraded_report_callback_preserves_distinct_source_order() {
        let first = RegistrySource::Manifest(Source::File("first.json".into()));
        let second = RegistrySource::Manifest(Source::File("second.json".into()));
        let reports = vec![
            SourceReport::Healthy {
                source: first.clone(),
                selected_indexes: 0,
                artifact_count: 0,
            },
            SourceReport::Unavailable {
                source: first,
                error: anyhow::anyhow!("first failure"),
            },
            SourceReport::Rejected {
                source: second,
                error: anyhow::anyhow!("second failure"),
            },
        ];
        let mut warnings = Vec::new();
        for_each_degraded_report(&reports, |report| {
            if let Some(warning) = report.warning() {
                warnings.push(warning);
            }
        });

        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("first.json"));
        assert!(warnings[1].contains("second.json"));
    }

    #[tokio::test]
    async fn bounded_collection_preserves_input_order_and_limit() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let values = (0usize..12).collect::<Vec<_>>();
        let output = collect_bounded_ordered(values, 4, {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            move |value| {
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis((12 - value) as u64)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    value
                }
            }
        })
        .await;

        assert_eq!(output, (0usize..12).collect::<Vec<_>>());
        assert_eq!(maximum.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn concurrent_completion_does_not_change_source_precedence() {
        use std::time::Duration;
        let server = wiremock::MockServer::start().await;
        let first_artifact = "https://first.example.com/server.tar.zst";
        let second_artifact = "https://second.example.com/server.tar.zst";
        let index_body = |artifact_url: &str| {
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[{{"ref":"{artifact_url}","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
        };
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/first.json"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_string(index_body(first_artifact)),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/second.json"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_string(index_body(second_artifact)),
            )
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let write_manifest = |name: &str, index_url: String| {
            let path = tmp.path().join(name);
            fs_err::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "schema_version": 1,
                    "indexes": [{
                        "channel": "stable",
                        "platform": "linux",
                        "url": index_url,
                    }],
                }))
                .unwrap(),
            )
            .unwrap();
            path
        };
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(write_manifest(
                    "first-manifest.json",
                    format!("{}/first.json", server.uri()),
                ))),
                RegistrySource::Manifest(Source::File(write_manifest(
                    "second-manifest.json",
                    format!("{}/second.json", server.uri()),
                ))),
            ],
        };
        let load = Catalog::load(
            &config,
            &SourceLoader::new().unwrap(),
            Channel::Stable,
            "linux",
        )
        .await
        .unwrap();
        assert_eq!(
            load.catalog.server_packages()[0].url.as_str(),
            first_artifact
        );
    }

    #[tokio::test]
    async fn unavailable_source_plus_healthy_empty_index_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let empty_manifest =
            write_manifest_with_indexes(tmp.path(), "empty-manifest.json", &["empty.json"]);
        fs_err::write(tmp.path().join("empty.json"), br#"{"packages":[]}"#).unwrap();
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(tmp.path().join("missing.json"))),
                RegistrySource::Manifest(Source::File(empty_manifest.clone())),
            ],
        };
        let load = Catalog::load(
            &config,
            &SourceLoader::new().unwrap(),
            Channel::Stable,
            "linux",
        )
        .await
        .unwrap();
        assert!(load.catalog.server_packages().is_empty());
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Unavailable { .. }
        ));
        assert!(matches!(
            &load.source_reports[1],
            SourceReport::Healthy {
                source: RegistrySource::Manifest(Source::File(source_path)),
                selected_indexes: 1,
                artifact_count: 0,
            } if source_path == &empty_manifest
        ));
    }

    #[tokio::test]
    async fn rejected_source_plus_manifest_without_match_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = write_manifest_with_indexes(tmp.path(), "bad-manifest.json", &["bad.json"]);
        fs_err::write(tmp.path().join("bad.json"), server_index("bad")).unwrap();
        let no_match = tmp.path().join("no-match.json");
        fs_err::write(&no_match, br#"{"schema_version":1,"indexes":[]}"#).unwrap();
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(bad)),
                RegistrySource::Manifest(Source::File(no_match)),
            ],
        };
        let load = Catalog::load(
            &config,
            &SourceLoader::new().unwrap(),
            Channel::Stable,
            "linux",
        )
        .await
        .unwrap();
        assert!(load.catalog.server_packages().is_empty());
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Rejected { .. }
        ));
        assert!(matches!(
            &load.source_reports[1],
            SourceReport::Healthy { .. }
        ));
    }

    #[tokio::test]
    async fn no_healthy_sources_preserves_configured_order() {
        let tmp = tempfile::tempdir().unwrap();
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(tmp.path().join("first.json"))),
                RegistrySource::Manifest(Source::File(tmp.path().join("second.json"))),
            ],
        };
        let error = Catalog::load(
            &config,
            &SourceLoader::new().unwrap(),
            Channel::Stable,
            "linux",
        )
        .await
        .unwrap_err();
        let error = error.downcast_ref::<NoHealthySources>().unwrap();
        assert_eq!(error.reports().len(), 2);
        assert!(
            error.to_string().find("first.json").unwrap()
                < error.to_string().find("second.json").unwrap()
        );
    }

    #[tokio::test]
    async fn one_failed_selected_index_rejects_the_whole_source() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = write_manifest_with_indexes(
            tmp.path(),
            "registry.json",
            &["good.json", "missing.json"],
        );
        fs_err::write(tmp.path().join("good.json"), server_index(HASH)).unwrap();
        let empty_manifest =
            write_manifest_with_indexes(tmp.path(), "empty-manifest.json", &["empty.json"]);
        fs_err::write(tmp.path().join("empty.json"), br#"{"packages":[]}"#).unwrap();
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(manifest)),
                RegistrySource::Manifest(Source::File(empty_manifest)),
            ],
        };
        let load = Catalog::load(
            &config,
            &SourceLoader::new().unwrap(),
            Channel::Stable,
            "linux",
        )
        .await
        .unwrap();
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Unavailable { .. }
        ));
        assert!(load.catalog.server_packages().is_empty());
    }

    #[tokio::test]
    async fn one_invalid_selected_index_rejects_prior_valid_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest =
            write_manifest_with_indexes(tmp.path(), "registry.json", &["good.json", "bad.json"]);
        fs_err::write(tmp.path().join("good.json"), server_index(HASH)).unwrap();
        fs_err::write(tmp.path().join("bad.json"), server_index("bad")).unwrap();
        let empty_manifest =
            write_manifest_with_indexes(tmp.path(), "empty-manifest.json", &["empty.json"]);
        fs_err::write(tmp.path().join("empty.json"), br#"{"packages":[]}"#).unwrap();
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(manifest)),
                RegistrySource::Manifest(Source::File(empty_manifest)),
            ],
        };
        let load = Catalog::load(
            &config,
            &SourceLoader::new().unwrap(),
            Channel::Stable,
            "linux",
        )
        .await
        .unwrap();
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Rejected { .. }
        ));
        assert!(load.catalog.server_packages().is_empty());
    }
    #[tokio::test]
    async fn earlier_rejected_selected_index_preserves_manifest_ordered_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest =
            write_manifest_with_indexes(tmp.path(), "registry.json", &["bad.json", "missing.json"]);
        fs_err::write(tmp.path().join("bad.json"), server_index("bad")).unwrap();
        let empty_manifest =
            write_manifest_with_indexes(tmp.path(), "empty-manifest.json", &["empty.json"]);
        fs_err::write(tmp.path().join("empty.json"), br#"{"packages":[]}"#).unwrap();
        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(manifest)),
                RegistrySource::Manifest(Source::File(empty_manifest)),
            ],
        };
        let load = Catalog::load(
            &config,
            &SourceLoader::new().unwrap(),
            Channel::Stable,
            "linux",
        )
        .await
        .unwrap();
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Rejected { error, .. }
                if error.to_string().contains("bad.json")
        ));
        assert!(load.catalog.server_packages().is_empty());
    }

    fn duplicate_config(tmp: &tempfile::TempDir, first: &str, second: &str) -> Config {
        let manifest = tmp.path().join("registry.json");
        let one = tmp.path().join("one.json");
        let two = tmp.path().join("two.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"x86_64-unknown-linux-gnu","url":"one.json"},{"channel":"stable","platform":"x86_64-unknown-linux-gnu","url":"two.json"}]}"#,
        )
        .unwrap();
        fs_err::write(one, first).unwrap();
        fs_err::write(two, second).unwrap();
        Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        }
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
                r#"{{"packages":[{{"basename":"gel-cli","version":"7.10.2","slot":"7","tags":{{}},"installrefs":[{{"ref":"{artifact_path}","type":"application/x-pie-executable","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
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
            .unwrap()
            .catalog;

        assert_eq!(catalog.cli_packages().len(), 1);
        assert_eq!(
            catalog.cli_packages()[0].url,
            url::Url::parse(&format!("{}{}", server.uri(), artifact_path)).unwrap()
        );
    }
    #[tokio::test]
    async fn equivalent_duplicate_artifacts_keep_first_source() {
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
            .unwrap()
            .catalog;

        let packages = catalog.server_packages();
        assert_eq!(packages.len(), 1);
        assert_eq!(
            packages[0].url,
            url::Url::parse("https://example.com/gel-server.tar.zst").unwrap()
        );
    }

    #[tokio::test]
    async fn mirror_duplicate_artifacts_keep_first_source() {
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
            .unwrap()
            .catalog;

        let packages = catalog.server_packages();
        assert_eq!(packages.len(), 1);
        assert_eq!(
            packages[0].url,
            url::Url::parse("https://mirror-one.example.com/gel-server.tar.zst").unwrap()
        );
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
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let one = tmp.path().join("one.json");
        let two = tmp.path().join("two.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"x86_64-unknown-linux-gnu","url":"one.json"},{"channel":"stable","platform":"x86_64-unknown-linux-gnu","url":"two.json"}]}"#,
        )
        .unwrap();
        let index = |cli_url: &str, extension_url: &str| {
            format!(
                r#"{{"packages":[
                    {{"basename":"gel-cli","version":"7.0.0","slot":"7","tags":{{}},"installrefs":[{{"ref":"{cli_url}","type":"application/x-pie-executable","encoding":"zstd","verification":{{"size":11,"blake2b":"{HASH}"}}}}]}},
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
        let catalog = Catalog::load(
            &config,
            &loader,
            Channel::Stable,
            "x86_64-unknown-linux-gnu",
        )
        .await
        .unwrap()
        .catalog;

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

    #[tokio::test]
    async fn cli_records_with_different_wire_slots_are_one_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let index = |url: &str, slot: &str| {
            format!(
                r#"{{"packages":[{{"basename":"gel-cli","version":"7.10.2","slot":"{slot}","tags":{{}},"installrefs":[{{"ref":"{url}","type":"application/x-pie-executable","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
        };
        let config = duplicate_config(
            &tmp,
            &index("https://first.example.com/gel-cli.zst", "one"),
            &index("https://second.example.com/gel-cli.zst", "two"),
        );
        let loader = SourceLoader::new().unwrap();
        let catalog = Catalog::load(
            &config,
            &loader,
            Channel::Stable,
            "x86_64-unknown-linux-gnu",
        )
        .await
        .unwrap()
        .catalog;

        assert_eq!(catalog.cli_packages().len(), 1);
        assert_eq!(
            catalog.cli_packages()[0].url,
            Url::parse("https://first.example.com/gel-cli.zst").unwrap()
        );
    }

    #[tokio::test]
    async fn server_tag_difference_is_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let index = |tag: &str| {
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{"channel":"{tag}"}},"installrefs":[{{"ref":"https://example.com/gel-server-{tag}.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
        };
        let config = duplicate_config(&tmp, &index("stable"), &index("testing"));
        let loader = SourceLoader::new().unwrap();
        let error = Catalog::load(
            &config,
            &loader,
            Channel::Stable,
            "x86_64-unknown-linux-gnu",
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("tags"));
    }

    #[tokio::test]
    async fn cli_compression_difference_is_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let index = |encoding: &str, url: &str| {
            format!(
                r#"{{"packages":[{{"basename":"gel-cli","version":"7.10.2","slot":"7","tags":{{}},"installrefs":[{{"ref":"{url}","type":"application/x-pie-executable","encoding":"{encoding}","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
        };
        let config = duplicate_config(
            &tmp,
            &index("zstd", "https://first.example.com/gel-cli.zst"),
            &index("identity", "https://second.example.com/gel-cli"),
        );
        let loader = SourceLoader::new().unwrap();
        let error = Catalog::load(
            &config,
            &loader,
            Channel::Stable,
            "x86_64-unknown-linux-gnu",
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("compression"));
    }

    #[tokio::test]
    async fn extension_tag_difference_is_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let index = |extra_tag: &str, url: &str| {
            format!(
                r#"{{"packages":[{{"basename":"extension-package","version":"7.0+abcdef0","slot":"7","tags":{{"extension":"pgvector","server_slot":"7"{extra_tag}}},"installrefs":[{{"ref":"{url}","type":"application/zip","encoding":"identity","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
        };
        let config = duplicate_config(
            &tmp,
            &index("", "https://first.example.com/pgvector.zip"),
            &index(
                r#","source":"mirror""#,
                "https://second.example.com/pgvector.zip",
            ),
        );
        let loader = SourceLoader::new().unwrap();
        let error = Catalog::load(
            &config,
            &loader,
            Channel::Stable,
            "x86_64-unknown-linux-gnu",
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("tags"));
    }
    async fn catalog_with_http_source_status(status: u16) -> anyhow::Result<CatalogLoad> {
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
            let catalog = catalog_with_http_source_status(status)
                .await
                .unwrap()
                .catalog;
            assert_eq!(catalog.server_packages().len(), 1, "HTTP {status}");
        }
    }

    #[tokio::test]
    async fn non_unavailability_http_source_errors_allow_later_source_artifacts() {
        let load = catalog_with_http_source_status(403).await.unwrap();
        assert_eq!(load.catalog.server_packages().len(), 1);
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Unavailable { .. }
        ));
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
        let load = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(load.catalog.server_packages().len(), 1);
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Unavailable { .. }
        ));
        assert!(matches!(
            &load.source_reports[1],
            SourceReport::Healthy { .. }
        ));
    }

    #[tokio::test]
    async fn loads_local_registry_packages_for_all_package_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let index = tmp.path().join("stable-linux.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[{"channel":"stable","platform":"x86_64-unknown-linux-gnu","url":"stable-linux.json"}]}"#,
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
                                    "type": "application/x-pie-executable",
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
        let catalog = Catalog::load(
            &config,
            &loader,
            Channel::Stable,
            "x86_64-unknown-linux-gnu",
        )
        .await
        .unwrap()
        .catalog;

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
        let aggregate = err.downcast_ref::<NoHealthySources>().unwrap();
        assert!(matches!(
            &aggregate.reports()[0],
            SourceReport::Unavailable { .. }
        ));
        assert!(matches!(
            &aggregate.reports()[1],
            SourceReport::Rejected { .. }
        ));
    }

    #[tokio::test]
    async fn missing_manifest_source_returns_error() {
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
            .unwrap()
            .catalog;

        assert!(catalog.server_packages().is_empty());
        assert!(catalog.cli_packages().is_empty());
        assert!(catalog.extension_packages("7").is_empty());
    }

    #[tokio::test]
    async fn network_manifest_failure_continues_when_later_source_has_artifacts() {
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

        let load = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(load.catalog.server_packages().len(), 1);
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Unavailable { .. }
        ));
        assert!(matches!(
            &load.source_reports[1],
            SourceReport::Healthy { .. }
        ));
    }

    #[tokio::test]
    async fn malformed_second_source_is_rejected_after_valid_artifacts() {
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

        let load = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(load.catalog.server_packages().len(), 1);
        assert!(matches!(
            &load.source_reports[1],
            SourceReport::Rejected { .. }
        ));
    }
    #[tokio::test]
    async fn missing_selected_index_continues_when_later_source_has_artifacts() {
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
        let load = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(load.catalog.server_packages().len(), 1);
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Unavailable { .. }
        ));
        assert!(matches!(
            &load.source_reports[1],
            SourceReport::Healthy { .. }
        ));
    }

    #[tokio::test]
    async fn network_selected_index_failure_continues_when_later_source_has_artifacts() {
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
        let load = Catalog::load(&config, &loader, Channel::Stable, "linux")
            .await
            .unwrap();

        assert_eq!(load.catalog.server_packages().len(), 1);
        assert!(matches!(
            &load.source_reports[0],
            SourceReport::Unavailable { .. }
        ));
        assert!(matches!(
            &load.source_reports[1],
            SourceReport::Healthy { .. }
        ));
    }
}
