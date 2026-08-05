//! Registry manifest parsing and channel/platform index selection.

use std::path::Path;

use anyhow::bail;
use serde::Deserialize;

use crate::portable::registry::source::{ReferenceKind, classify_reference, relative_http_ref};
use crate::portable::registry::{Channel, Source};

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    schema_version: u32,
    indexes: Vec<ManifestIndex>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestIndex {
    channel: Channel,
    platform: String,
    url: String,
}

impl Manifest {
    pub fn from_slice(bytes: &[u8]) -> anyhow::Result<Manifest> {
        let mut deserializer = serde_json::Deserializer::from_slice(bytes);
        let manifest: Manifest = serde_path_to_error::deserialize(&mut deserializer)?;

        if manifest.schema_version != 1 {
            bail!(
                "unsupported registry manifest schema_version {}",
                manifest.schema_version
            );
        }

        Ok(manifest)
    }

    pub fn select_indexes(
        &self,
        manifest_source: &Source,
        channel: Channel,
        platform: &str,
    ) -> anyhow::Result<Vec<Source>> {
        self.indexes
            .iter()
            .filter(|index| index.channel == channel && index.platform == platform)
            .map(|index| resolve_index_source(manifest_source, &index.url))
            .collect()
    }
}

fn resolve_index_source(manifest_source: &Source, value: &str) -> anyhow::Result<Source> {
    match classify_reference(value) {
        ReferenceKind::HttpUrl(url) => Ok(Source::Http(url)),
        ReferenceKind::FileUrl(url) => Source::from_file_url(url),
        ReferenceKind::UnsupportedScheme(scheme) => {
            bail!("unsupported registry index source scheme {scheme}: {value}")
        }
        ReferenceKind::Relative => match manifest_source {
            Source::Http(base) => Ok(Source::Http(base.join(&relative_http_ref(value))?)),
            Source::File(path) => {
                let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
                Ok(Source::File(base_dir.join(value)))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use url::Url;

    use crate::portable::registry::{Channel, Source};

    use super::Manifest;

    #[test]
    fn selects_matching_channel_and_platform() {
        let manifest = Manifest::from_slice(
            br#"{
                "schema_version": 1,
                "indexes": [
                    {
                        "channel": "stable",
                        "platform": "x86_64-unknown-linux-gnu",
                        "url": "https://example.com/stable-linux.json"
                    },
                    {
                        "channel": "testing",
                        "platform": "x86_64-unknown-linux-gnu",
                        "url": "https://example.com/testing-linux.json"
                    },
                    {
                        "channel": "stable",
                        "platform": "aarch64-apple-darwin",
                        "url": "https://example.com/stable-macos.json"
                    }
                ]
            }"#,
        )
        .unwrap();
        let manifest_source =
            Source::Http(Url::parse("https://example.com/registry.json").unwrap());

        let indexes = manifest
            .select_indexes(
                &manifest_source,
                Channel::Stable,
                "x86_64-unknown-linux-gnu",
            )
            .unwrap();

        assert_eq!(
            indexes,
            vec![Source::Http(
                Url::parse("https://example.com/stable-linux.json").unwrap()
            )]
        );
    }

    #[test]
    fn relative_file_indexes_resolve_from_manifest_path() {
        let tmp = tempdir().unwrap();
        let manifest_source = Source::File(tmp.path().join("manifests/registry.json"));
        let manifest = Manifest::from_slice(
            br#"{
                "schema_version": 1,
                "indexes": [
                    {
                        "channel": "stable",
                        "platform": "x86_64-unknown-linux-gnu",
                        "url": "indexes/stable.json"
                    }
                ]
            }"#,
        )
        .unwrap();

        let indexes = manifest
            .select_indexes(
                &manifest_source,
                Channel::Stable,
                "x86_64-unknown-linux-gnu",
            )
            .unwrap();

        assert_eq!(
            indexes,
            vec![Source::File(
                tmp.path().join("manifests/indexes/stable.json")
            )]
        );
    }

    #[test]
    fn unsupported_schema_version_fails() {
        let err = Manifest::from_slice(
            br#"{
                "schema_version": 2,
                "indexes": []
            }"#,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("unsupported registry manifest schema_version")
        );
    }

    #[test]
    fn colon_containing_file_paths_resolve_relative_to_manifest_path() {
        let tmp = tempdir().unwrap();
        let manifest_source = Source::File(tmp.path().join("registry.json"));
        let manifest = Manifest::from_slice(
            br#"{
                "schema_version": 1,
                "indexes": [
                    {
                        "channel": "stable",
                        "platform": "x86_64-unknown-linux-gnu",
                        "url": "release:2026/stable.json"
                    }
                ]
            }"#,
        )
        .unwrap();

        let indexes = manifest
            .select_indexes(
                &manifest_source,
                Channel::Stable,
                "x86_64-unknown-linux-gnu",
            )
            .unwrap();

        assert_eq!(
            indexes,
            vec![Source::File(tmp.path().join("release:2026/stable.json"))]
        );
    }

    #[test]
    fn colon_containing_http_paths_resolve_relative_to_manifest_url() {
        let manifest_source =
            Source::Http(Url::parse("https://example.com/registry.json").unwrap());
        let manifest = Manifest::from_slice(
            br#"{
                "schema_version": 1,
                "indexes": [
                    {
                        "channel": "stable",
                        "platform": "x86_64-unknown-linux-gnu",
                        "url": "release:2026/stable.json"
                    }
                ]
            }"#,
        )
        .unwrap();

        let indexes = manifest
            .select_indexes(
                &manifest_source,
                Channel::Stable,
                "x86_64-unknown-linux-gnu",
            )
            .unwrap();

        assert_eq!(
            indexes,
            vec![Source::Http(
                Url::parse("https://example.com/release%3A2026/stable.json").unwrap()
            )]
        );
    }

    #[test]
    fn unsupported_index_url_scheme_fails() {
        let manifest_source =
            Source::Http(Url::parse("https://example.com/registry.json").unwrap());
        let manifest = Manifest::from_slice(
            br#"{
                "schema_version": 1,
                "indexes": [
                    {
                        "channel": "stable",
                        "platform": "x86_64-unknown-linux-gnu",
                        "url": "ssh://example.com/stable.json"
                    }
                ]
            }"#,
        )
        .unwrap();

        let err = manifest
            .select_indexes(
                &manifest_source,
                Channel::Stable,
                "x86_64-unknown-linux-gnu",
            )
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("unsupported registry index source scheme")
        );
    }
}
