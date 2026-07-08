//! Loading registry manifests and package indexes from HTTP(S), file URLs, and local paths.

use std::path::{Path, PathBuf};

use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest::StatusCode;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use thiserror::Error;
use url::Url;

use crate::portable::registry::download::{DEFAULT_TIMEOUT, USER_AGENT};

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
        error: reqwest_middleware::Error,
    },
    #[error("failed to read registry file {path}: {error}")]
    FileIo {
        path: PathBuf,
        error: std::io::Error,
    },
}

#[derive(Clone, Debug)]
pub struct SourceLoader {
    cached_client: ClientWithMiddleware,
    uncached_client: reqwest::Client,
}

impl SourceLoader {
    pub fn new(cache_dir: PathBuf) -> anyhow::Result<SourceLoader> {
        let uncached_client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(DEFAULT_TIMEOUT)
            .build()?;
        let cached_client = ClientBuilder::new(uncached_client.clone())
            .with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager: CACacheManager::new(cache_dir, false),
                options: HttpCacheOptions::default(),
            }))
            .build();
        Ok(SourceLoader {
            cached_client,
            uncached_client,
        })
    }

    pub async fn load_manifest(&self, source: &Source) -> Result<Vec<u8>, SourceError> {
        match source {
            Source::Http(url) => self.load_http_cached(url).await,
            Source::File(path) => self.load_file(path).await,
        }
    }

    pub async fn load_index(&self, source: &Source) -> Result<Vec<u8>, SourceError> {
        match source {
            Source::Http(url) => self.load_http_uncached(url).await,
            Source::File(path) => self.load_file(path).await,
        }
    }

    async fn load_http_cached(&self, url: &Url) -> Result<Vec<u8>, SourceError> {
        let response = self
            .cached_client
            .get(url.clone())
            .send()
            .await
            .map_err(|error| SourceError::Network {
                location: url.to_string(),
                error,
            })?;
        self.read_http_response(url, response).await
    }

    async fn load_http_uncached(&self, url: &Url) -> Result<Vec<u8>, SourceError> {
        let response = self
            .uncached_client
            .get(url.clone())
            .send()
            .await
            .map_err(|error| SourceError::Network {
                location: url.to_string(),
                error: error.into(),
            })?;
        self.read_http_response(url, response).await
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
                error: error.into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err::write;
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_loader(cache_dir: PathBuf) -> anyhow::Result<SourceLoader> {
        SourceLoader::new(cache_dir)
    }

    #[tokio::test]
    async fn loads_local_file_source() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("registry.json");
        write(&path, br#"{"schema_version":1,"indexes":[]}"#).unwrap();
        let loader = test_loader(tmp.path().join("cache")).unwrap();

        let body = loader.load_manifest(&Source::File(path)).await.unwrap();

        assert_eq!(body, br#"{"schema_version":1,"indexes":[]}"#);
    }

    #[tokio::test]
    async fn http_404_is_source_not_found() {
        let server = MockServer::start().await;
        let tmp = tempdir().unwrap();
        Mock::given(method("GET"))
            .and(path("/registry.json"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let loader = test_loader(tmp.path().join("cache")).unwrap();
        let source = Source::Http(format!("{}/registry.json", server.uri()).parse().unwrap());

        let err = loader.load_manifest(&source).await.unwrap_err();

        assert!(matches!(err, SourceError::SourceNotFound { .. }));
    }

    #[tokio::test]
    async fn fresh_http_source_uses_cache() {
        let server = MockServer::start().await;
        let tmp = tempdir().unwrap();
        Mock::given(method("GET"))
            .and(path("/registry.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "max-age=3600")
                    .insert_header("etag", "\"v1\"")
                    .set_body_raw(br#"{"schema_version":1,"indexes":[]}"#, "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let loader = test_loader(tmp.path().join("cache")).unwrap();
        let source = Source::Http(format!("{}/registry.json", server.uri()).parse().unwrap());

        let first = loader.load_manifest(&source).await.unwrap();
        let second = loader.load_manifest(&source).await.unwrap();

        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn index_http_source_bypasses_cache() {
        let server = MockServer::start().await;
        let tmp = tempdir().unwrap();
        Mock::given(method("GET"))
            .and(path("/stable-aarch64-apple-darwin.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "max-age=3600")
                    .insert_header("etag", "\"v1\"")
                    .set_body_raw(br#"{"packages":[]}"#, "application/json"),
            )
            .expect(2)
            .mount(&server)
            .await;
        let loader = test_loader(tmp.path().join("cache")).unwrap();
        let source = Source::Http(
            format!("{}/stable-aarch64-apple-darwin.json", server.uri())
                .parse()
                .unwrap(),
        );

        let first = loader.load_index(&source).await.unwrap();
        let second = loader.load_index(&source).await.unwrap();

        assert_eq!(first, br#"{"packages":[]}"#);
        assert_eq!(second, br#"{"packages":[]}"#);
    }
}
