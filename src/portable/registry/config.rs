//! Global CLI registry source configuration.

use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use url::Url;

use crate::portable::registry::source::Source;

pub const DEFAULT_PACKAGE_ROOT: &str = "https://packages.geldata.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrySource {
    Manifest(Source),
    LegacyPackageRoot(Url),
}

impl std::fmt::Display for RegistrySource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistrySource::Manifest(source) => formatter.write_str(&source.display()),
            RegistrySource::LegacyPackageRoot(root) => root.fmt(formatter),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub sources: Vec<RegistrySource>,
}

impl Config {
    pub fn load() -> anyhow::Result<Config> {
        let pkg_root = crate::cli::env::Env::pkg_root()?;
        let global = crate::config::get_config()?;
        let config_path = match global.file_name.as_deref() {
            Some(path) => path.to_path_buf(),
            None => crate::platform::config_dir()?.join("cli.toml"),
        };
        Self::from_inputs(
            pkg_root.as_deref(),
            &global.registry.sources,
            &config_path,
            DEFAULT_PACKAGE_ROOT,
        )
    }

    pub fn from_inputs(
        pkg_root: Option<&str>,
        registry_sources: &[String],
        config_path: &Path,
        default_package_root: &str,
    ) -> anyhow::Result<Config> {
        if let Some(pkg_root) = pkg_root {
            return Ok(Config {
                sources: vec![RegistrySource::LegacyPackageRoot(parse_package_root(
                    pkg_root,
                )?)],
            });
        }

        if !registry_sources.is_empty() {
            let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
            let sources = registry_sources
                .iter()
                .map(|source| parse_source(source, base_dir).map(RegistrySource::Manifest))
                .collect::<anyhow::Result<Vec<_>>>()?;
            return Ok(Config { sources });
        }

        Ok(Config {
            sources: vec![RegistrySource::LegacyPackageRoot(parse_package_root(
                default_package_root,
            )?)],
        })
    }
}

fn parse_package_root(value: &str) -> anyhow::Result<Url> {
    let url =
        Url::parse(value).with_context(|| format!("invalid legacy package root URL {value:?}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        anyhow::bail!("invalid legacy package root URL {value:?}");
    }
    Ok(url)
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

    const DEFAULT_SOURCE: &str = "https://packages.geldata.com";

    fn parse(input: &str) -> anyhow::Result<Config> {
        let global: crate::config::Config = toml::from_str(input)?;
        Config::from_inputs(
            None,
            &global.registry.sources,
            Path::new("/home/me/.config/gel/cli.toml"),
            DEFAULT_SOURCE,
        )
    }

    #[test]
    fn missing_registry_table_uses_default_source() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg.sources.len(), 1);
        assert!(matches!(
            &cfg.sources[0],
            RegistrySource::LegacyPackageRoot(root)
                if root == &Url::parse(DEFAULT_SOURCE).unwrap()
        ));
    }

    #[test]
    fn empty_sources_use_default_source() {
        let cfg = parse("[registry]\nsources = []\n").unwrap();
        assert_eq!(cfg.sources.len(), 1);
        assert!(matches!(
            &cfg.sources[0],
            RegistrySource::LegacyPackageRoot(root)
                if root == &Url::parse(DEFAULT_SOURCE).unwrap()
        ));
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
            cfg.sources[0].to_string(),
            "https://example.com/registry.json"
        );
        assert_eq!(cfg.sources[1].to_string(), "/tmp/registry.json");
        assert_eq!(
            cfg.sources[2].to_string(),
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
    fn rejects_unknown_registry_fields() {
        let err = parse(
            r#"[registry]
            unknown = true
        "#,
        )
        .unwrap_err();

        assert!(format!("{err:#}").contains("unknown"));
    }

    #[test]
    fn no_package_root_uses_configured_manifest_sources() {
        let configured = vec![
            "https://example.com/registry.json".to_owned(),
            "./fixtures/registry.json".to_owned(),
        ];

        let cfg = Config::from_inputs(
            None,
            &configured,
            Path::new("/home/me/.config/gel/cli.toml"),
            DEFAULT_SOURCE,
        )
        .unwrap();

        assert!(matches!(
            &cfg.sources[0],
            RegistrySource::Manifest(Source::Http(url))
                if url.as_str() == "https://example.com/registry.json"
        ));
        assert!(matches!(
            &cfg.sources[1],
            RegistrySource::Manifest(Source::File(path))
                if path == Path::new("/home/me/.config/gel/fixtures/registry.json")
        ));
    }

    #[test]
    fn package_root_input_replaces_configured_sources() {
        let configured = vec!["https://example.com/registry.json".to_owned()];

        let cfg = Config::from_inputs(
            Some("https://mirror.example.test/packages"),
            &configured,
            Path::new("/home/me/.config/gel/cli.toml"),
            DEFAULT_SOURCE,
        )
        .unwrap();

        assert_eq!(cfg.sources.len(), 1);
        assert!(matches!(
            &cfg.sources[0],
            RegistrySource::LegacyPackageRoot(root)
                if root == &Url::parse("https://mirror.example.test/packages").unwrap()
        ));
    }

    #[test]
    fn malformed_package_root_has_context() {
        let err = Config::from_inputs(
            Some("not a URL"),
            &[],
            Path::new("/home/me/.config/gel/cli.toml"),
            DEFAULT_SOURCE,
        )
        .unwrap_err();

        assert!(format!("{err:#}").contains("invalid legacy package root URL"));
    }

    #[test]
    fn load_reads_registry_sources_from_cli_toml_not_gel_toml() {
        let _lock = crate::cli::env::test_env_lock();
        let env = crate::cli::env::RestoreEnv::new(&[
            "HOME",
            "XDG_CONFIG_HOME",
            "GEL_PKG_ROOT",
            "EDGEDB_PKG_ROOT",
        ]);
        let tmp = tempfile::tempdir().unwrap();
        env.set("HOME", &tmp.path().display().to_string());
        env.set(
            "XDG_CONFIG_HOME",
            &tmp.path().join("config").display().to_string(),
        );
        unsafe {
            std::env::remove_var("GEL_PKG_ROOT");
            std::env::remove_var("EDGEDB_PKG_ROOT");
        }

        let config_dir = crate::platform::config_dir().unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("cli.toml"),
            "[registry]\nsources = [\"./from-cli.json\"]\n",
        )
        .unwrap();
        std::fs::write(
            config_dir.join("gel.toml"),
            "[registry]\nsources = [\"./from-gel.json\"]\n",
        )
        .unwrap();

        let config = Config::load().unwrap();

        assert_eq!(config.sources.len(), 1);
        assert_eq!(
            config.sources[0].to_string(),
            config_dir.join("from-cli.json").display().to_string()
        );
    }

    #[test]
    fn windows_absolute_paths_are_local_sources() {
        let cfg = parse(
            r#"[registry]
            sources = ['C:\gel-registry\registry.json']
        "#,
        )
        .unwrap();
        assert_eq!(cfg.sources[0].to_string(), r"C:\gel-registry\registry.json");
    }
}
