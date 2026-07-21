//! Loading registry manifests and package indexes from HTTP(S), file URLs, and local paths.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use thiserror::Error;
use url::Url;

use super::types::Channel;
use crate::portable::registry::download::{DEFAULT_TIMEOUT, USER_AGENT};

pub(crate) fn legacy_channel_suffix(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "",
        Channel::Testing => ".testing",
        Channel::Nightly => ".nightly",
    }
}

pub(crate) fn legacy_index_url(
    root: &Url,
    platform: &str,
    channel: Channel,
) -> anyhow::Result<Url> {
    root.join(&format!(
        "/archive/.jsonindexes/{platform}{}.json",
        legacy_channel_suffix(channel),
    ))
    .map_err(Into::into)
}

/// Classification of a reference string found in a manifest or package index.
pub(crate) enum ReferenceKind {
    HttpUrl(Url),
    FileUrl(Url),
    Relative,
    UnsupportedScheme(String),
}

pub(crate) fn classify_reference(value: &str) -> ReferenceKind {
    if let Ok(url) = Url::parse(value) {
        match url.scheme() {
            "http" | "https" => return ReferenceKind::HttpUrl(url),
            "file" => return ReferenceKind::FileUrl(url),
            scheme if value.contains("://") => {
                return ReferenceKind::UnsupportedScheme(scheme.to_string());
            }
            _ => {} // colon-in-first-segment relative ref, e.g. "release:2026/x"
        }
    }
    ReferenceKind::Relative
}

pub(crate) fn relative_http_ref(value: &str) -> String {
    value.replacen(':', "%3A", 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Http(Url),
    File(PathBuf),
}

impl Source {
    pub fn from_file_url(url: Url) -> anyhow::Result<Source> {
        let path = url
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("invalid file registry source URL {url}"))?;
        Ok(Source::File(path))
    }

    pub fn display(&self) -> String {
        match self {
            Source::Http(url) => url.to_string(),
            Source::File(path) => path.display().to_string(),
        }
    }
}

#[derive(Error, Debug)]
pub enum SourceError {
    #[error("registry source not found: {location}")]
    SourceNotFound { location: String },
    #[error("registry source returned HTTP {status}: {location}")]
    Http {
        location: String,
        status: StatusCode,
    },
    #[error("failed to fetch registry source {location}: {error}")]
    Network {
        location: String,
        error: reqwest::Error,
    },
    #[error("failed to read registry file {path}: {error}")]
    FileIo {
        path: PathBuf,
        error: std::io::Error,
    },
}

#[derive(Clone, Debug)]
pub struct SourceLoader {
    client: reqwest::Client,
}

impl SourceLoader {
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(DEFAULT_TIMEOUT)
            .build()?;
        Ok(SourceLoader { client })
    }

    pub async fn load_manifest(&self, source: &Source) -> Result<Vec<u8>, SourceError> {
        self.load(source).await
    }

    pub async fn load_index(&self, source: &Source) -> Result<Vec<u8>, SourceError> {
        self.load(source).await
    }

    async fn load(&self, source: &Source) -> Result<Vec<u8>, SourceError> {
        match source {
            Source::Http(url) => self.load_http(url).await,
            Source::File(path) => self.load_file(path).await,
        }
    }

    async fn load_http(&self, url: &Url) -> Result<Vec<u8>, SourceError> {
        let fetch = async {
            let response = self.client.get(url.clone()).send().await.map_err(|error| {
                SourceError::Network {
                    location: url.to_string(),
                    error,
                }
            })?;
            self.read_http_response(url, response).await
        };
        tokio::pin!(fetch);
        match tokio::time::timeout(SLOW_HTTP_DIAGNOSTIC_DELAY, &mut fetch).await {
            Ok(result) => result,
            Err(_) => {
                if std::io::stderr().is_terminal() {
                    eprintln!(
                        "{}",
                        slow_http_diagnostic(crate::portable::windows::is_in_wsl())
                    );
                }
                fetch.await
            }
        }
    }

    async fn read_http_response(
        &self,
        url: &Url,
        response: reqwest::Response,
    ) -> Result<Vec<u8>, SourceError> {
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Err(SourceError::SourceNotFound {
                location: url.to_string(),
            });
        }
        if !status.is_success() {
            return Err(SourceError::Http {
                location: url.to_string(),
                status,
            });
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| SourceError::Network {
                location: url.to_string(),
                error,
            })
    }

    async fn load_file(&self, path: &Path) -> Result<Vec<u8>, SourceError> {
        tokio::fs::read(path)
            .await
            .map_err(|error| SourceError::FileIo {
                path: path.to_path_buf(),
                error,
            })
    }
}

const SLOW_HTTP_DIAGNOSTIC_DELAY: Duration = Duration::from_secs(2);

fn slow_http_diagnostic(is_wsl: bool) -> String {
    let mut message = String::from(
        "The registry source request is taking longer than expected; \
         check your firewall and network connectivity.",
    );
    if is_wsl {
        message.push_str(" If you are using WSL, check Windows Defender Firewall settings.");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err::write;
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_loader() -> anyhow::Result<SourceLoader> {
        SourceLoader::new()
    }

    #[test]
    fn slow_http_diagnostic_for_non_wsl_mentions_connectivity() {
        let message = slow_http_diagnostic(false);

        assert!(message.contains("firewall"));
        assert!(message.contains("connectivity"));
        assert!(!message.contains("Windows Defender"));
    }

    #[test]
    fn slow_http_diagnostic_for_wsl_mentions_windows_defender() {
        let message = slow_http_diagnostic(true);

        assert!(message.contains("firewall"));
        assert!(message.contains("connectivity"));
        assert!(message.contains("Windows Defender"));
    }

    #[test]
    fn stable_legacy_index_url_has_no_channel_suffix() {
        let root = Url::parse("https://packages.example.test/releases/").unwrap();

        let url = legacy_index_url(&root, "x86_64-unknown-linux-musl", Channel::Stable).unwrap();

        assert_eq!(
            url.as_str(),
            "https://packages.example.test/archive/.jsonindexes/x86_64-unknown-linux-musl.json"
        );
    }

    #[test]
    fn testing_and_nightly_legacy_index_urls_use_suffixes() {
        let root = Url::parse("https://packages.example.test/").unwrap();

        assert_eq!(
            legacy_index_url(&root, "linux", Channel::Testing)
                .unwrap()
                .as_str(),
            "https://packages.example.test/archive/.jsonindexes/linux.testing.json"
        );
        assert_eq!(
            legacy_index_url(&root, "linux", Channel::Nightly)
                .unwrap()
                .as_str(),
            "https://packages.example.test/archive/.jsonindexes/linux.nightly.json"
        );
    }

    #[tokio::test]
    async fn loads_local_file_source() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("registry.json");
        write(&path, br#"{"schema_version":1,"indexes":[]}"#).unwrap();
        let loader = test_loader().unwrap();

        let body = loader.load_manifest(&Source::File(path)).await.unwrap();

        assert_eq!(body, br#"{"schema_version":1,"indexes":[]}"#);
    }

    #[tokio::test]
    async fn http_404_is_source_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/registry.json"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let loader = test_loader().unwrap();
        let source = Source::Http(format!("{}/registry.json", server.uri()).parse().unwrap());

        let err = loader.load_manifest(&source).await.unwrap_err();

        assert!(matches!(err, SourceError::SourceNotFound { .. }));
    }

    #[tokio::test]
    async fn missing_local_file_is_file_not_found() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("missing.json");
        let loader = test_loader().unwrap();

        let err = loader
            .load_manifest(&Source::File(path.clone()))
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            SourceError::FileIo { path: error_path, error }
                if error_path == path && error.kind() == std::io::ErrorKind::NotFound
        ));
    }
}
