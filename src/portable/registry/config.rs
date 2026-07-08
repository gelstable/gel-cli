//! Global CLI registry source configuration.

use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;
use url::Url;

use crate::portable::registry::source::Source;

pub const DEFAULT_REGISTRY_SOURCE: &str =
    "https://raw.githubusercontent.com/community/gel-registry/main/registry.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub sources: Vec<Source>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    registry: Option<RawRegistry>,
}

#[derive(Debug, Deserialize)]
struct RawRegistry {
    #[serde(default)]
    sources: Vec<String>,
}

impl Config {
    pub fn load() -> anyhow::Result<Config> {
        let path = crate::platform::config_dir()?.join("gel.toml");
        if !path.exists() {
            return Self::default_with(DEFAULT_REGISTRY_SOURCE);
        }

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read registry config {}", path.display()))?;
        Self::from_toml_str(&text, &path, DEFAULT_REGISTRY_SOURCE)
    }

    pub fn from_toml_str(
        input: &str,
        config_path: &Path,
        default_source: &str,
    ) -> anyhow::Result<Config> {
        let raw: RawConfig = if input.trim().is_empty() {
            RawConfig { registry: None }
        } else {
            toml::from_str(input).context("failed to parse registry config TOML")?
        };
        let source_strings = raw.registry.map(|r| r.sources).unwrap_or_default();
        if source_strings.is_empty() {
            return Self::default_with(default_source);
        }

        let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
        let sources = source_strings
            .iter()
            .map(|s| parse_source(s, base_dir))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Config { sources })
    }

    fn default_with(default_source: &str) -> anyhow::Result<Config> {
        Ok(Config {
            sources: vec![parse_source(default_source, Path::new("."))?],
        })
    }
}

pub fn registry_http_cache_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::platform::cache_dir()?.join("registry").join("http"))
}

fn parse_source(value: &str, base_dir: &Path) -> anyhow::Result<Source> {
    if !looks_like_windows_absolute_path(value) {
        if let Ok(url) = Url::parse(value) {
            return match url.scheme() {
                "http" | "https" => Ok(Source::Http(url)),
                "file" => Source::from_file_url(url),
                scheme => anyhow::bail!("unsupported registry source scheme {scheme:?}"),
            };
        }
        if looks_like_url_source(value) {
            anyhow::bail!("invalid registry source URL {value:?}");
        }
    }

    let path = PathBuf::from(value);
    Ok(Source::File(
        if path.is_absolute() || looks_like_windows_absolute_path(value) {
            path
        } else {
            normalize_path(base_dir.join(path))
        },
    ))
}

fn looks_like_windows_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn looks_like_url_source(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://") || value.starts_with("file://")
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_SOURCE: &str =
        "https://raw.githubusercontent.com/community/gel-registry/main/registry.json";

    fn parse(input: &str) -> anyhow::Result<Config> {
        Config::from_toml_str(
            input,
            Path::new("/home/me/.config/gel/gel.toml"),
            DEFAULT_SOURCE,
        )
    }

    #[test]
    fn missing_registry_table_uses_default_source() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg.sources.len(), 1);
        assert_eq!(cfg.sources[0].display(), DEFAULT_SOURCE);
    }

    #[test]
    fn empty_sources_use_default_source() {
        let cfg = parse("[registry]\nsources = []\n").unwrap();
        assert_eq!(cfg.sources.len(), 1);
        assert_eq!(cfg.sources[0].display(), DEFAULT_SOURCE);
    }

    #[test]
    fn parses_http_file_and_relative_sources() {
        let cfg = parse(
            r#"
            [registry]
            sources = [
              "https://example.com/registry.json",
              "file:///tmp/registry.json",
              "./fixtures/registry.json",
            ]
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.sources[0].display(),
            "https://example.com/registry.json"
        );
        assert_eq!(cfg.sources[1].display(), "/tmp/registry.json");
        assert_eq!(
            cfg.sources[2].display(),
            "/home/me/.config/gel/fixtures/registry.json"
        );
    }

    #[test]
    fn rejects_unsupported_url_scheme() {
        let err = parse(
            r#"[registry]
            sources = ["ssh://example.com/registry.json"]
        "#,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported registry source scheme")
        );
    }

    #[test]
    fn rejects_malformed_url_like_source() {
        let err = parse(
            r#"[registry]
            sources = ["https://"]
        "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid registry source URL"));
    }

    #[test]
    fn windows_absolute_paths_are_local_sources() {
        let cfg = parse(
            r#"[registry]
            sources = ['C:\gel-registry\registry.json']
        "#,
        )
        .unwrap();
        assert_eq!(cfg.sources.len(), 1);
        assert_eq!(cfg.sources[0].display(), r"C:\gel-registry\registry.json");
    }
}
