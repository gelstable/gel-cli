//! Package artifact downloading and blake2b verification.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use super::types::{Blake2bDigest, CliPackage, ExtensionPackage, ServerPackage};
use crate::branding::BRANDING_CLI;
use anyhow::Context;
use fn_error_context::context;
use indicatif::{ProgressBar, ProgressStyle};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

pub const USER_AGENT: &str = BRANDING_CLI;
pub const DEFAULT_TIMEOUT: Duration = Duration::new(60, 0);
pub const ARTIFACT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::main(flavor = "current_thread")]
pub async fn download_sync(
    dest: impl AsRef<Path>,
    url: &Url,
    quiet: bool,
) -> Result<blake2b_simd::Hash, anyhow::Error> {
    download(dest, url, quiet).await
}

#[tokio::main(flavor = "current_thread")]
pub async fn download_cli_package_verified_sync(
    dest: impl AsRef<Path>,
    package: &CliPackage,
    quiet: bool,
) -> anyhow::Result<()> {
    download_package_verified(dest, &package.url, &package.hash, quiet).await
}

pub async fn download_server_package_verified(
    dest: impl AsRef<Path>,
    package: &ServerPackage,
    quiet: bool,
) -> anyhow::Result<()> {
    download_package_verified(dest, &package.url, &package.hash, quiet).await
}

pub async fn download_extension_package_verified(
    dest: impl AsRef<Path>,
    package: &ExtensionPackage,
    quiet: bool,
) -> anyhow::Result<()> {
    download_package_verified(dest, &package.url, &package.hash, quiet).await
}

#[context("failed to download file at URL: {}", url)]
pub async fn download(
    dest: impl AsRef<Path>,
    url: &Url,
    quiet: bool,
) -> Result<blake2b_simd::Hash, anyhow::Error> {
    download_with_idle_timeout(dest, url, quiet, ARTIFACT_IDLE_TIMEOUT).await
}

async fn download_with_idle_timeout(
    dest: impl AsRef<Path>,
    url: &Url,
    quiet: bool,
    idle_timeout: Duration,
) -> Result<blake2b_simd::Hash, anyhow::Error> {
    let dest = dest.as_ref();
    log::info!("Downloading {} -> {}", url, dest.display());
    if url.scheme() == "file" {
        return download_file(dest, url, quiet).await;
    }

    let send = reqwest::Client::new()
        .get(url.clone())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send();
    let mut response = tokio::time::timeout(idle_timeout, send)
        .await
        .map_err(|_| ArtifactIdleTimeout {
            url: url.clone(),
            phase: ArtifactTimeoutPhase::ResponseHeaders,
            idle_timeout,
        })??
        .error_for_status()?;
    let mut out = fs::File::create(dest)
        .await
        .with_context(|| format!("writing {:?}", dest.display()))?;

    let bar = if quiet {
        ProgressBar::hidden()
    } else if let Some(len) = response.content_length() {
        ProgressBar::new(len)
    } else {
        ProgressBar::new_spinner()
    };
    bar.set_style(
        ProgressStyle::default_bar()
            .template(
                "{elapsed_precise} [{bar}] \
            {bytes:>7.dim}/{total_bytes:7} \
            {binary_bytes_per_sec:.dim} | ETA: {eta}",
            )
            .expect("template is ok")
            .progress_chars("=> "),
    );
    let mut hasher = blake2b_simd::State::new();
    loop {
        let chunk = tokio::time::timeout(idle_timeout, response.chunk())
            .await
            .map_err(|_| ArtifactIdleTimeout {
                url: url.clone(),
                phase: ArtifactTimeoutPhase::ResponseBody,
                idle_timeout,
            })??;
        let Some(chunk) = chunk else {
            break;
        };
        out.write_all(&chunk).await?;
        hasher.update(&chunk);
        bar.inc(chunk.len() as u64);
    }
    out.flush().await?;
    bar.finish();

    Ok(hasher.finalize())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArtifactTimeoutPhase {
    ResponseHeaders,
    ResponseBody,
}

#[derive(Debug, thiserror::Error)]
#[error("artifact download from {url} was idle for {idle_timeout:?} while waiting for {phase}")]
struct ArtifactIdleTimeout {
    url: Url,
    phase: ArtifactTimeoutPhase,
    idle_timeout: Duration,
}

impl fmt::Display for ArtifactTimeoutPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResponseHeaders => formatter.write_str("response headers"),
            Self::ResponseBody => formatter.write_str("response body data"),
        }
    }
}

async fn download_file(
    dest: &Path,
    url: &Url,
    quiet: bool,
) -> Result<blake2b_simd::Hash, anyhow::Error> {
    let src = url
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("invalid file URL: {url}"))?;
    let mut input = fs::File::open(&src)
        .await
        .with_context(|| format!("reading {:?}", src.display()))?;
    let mut out = fs::File::create(dest)
        .await
        .with_context(|| format!("writing {:?}", dest.display()))?;

    let metadata = input.metadata().await?;
    let bar = if quiet {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(metadata.len())
    };
    bar.set_style(
        ProgressStyle::default_bar()
            .template(
                "{elapsed_precise} [{bar}] \
            {bytes:>7.dim}/{total_bytes:7} \
            {binary_bytes_per_sec:.dim} | ETA: {eta}",
            )
            .expect("template is ok")
            .progress_chars("=> "),
    );

    let mut hasher = blake2b_simd::State::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = input.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).await?;
        hasher.update(&buf[..n]);
        bar.inc(n as u64);
    }
    out.flush().await?;
    bar.finish();

    Ok(hasher.finalize())
}

async fn download_package_verified(
    dest: impl AsRef<Path>,
    url: &Url,
    digest: &Blake2bDigest,
    quiet: bool,
) -> anyhow::Result<()> {
    let actual = download(dest, url, quiet).await?;
    if actual.as_bytes() != digest.as_bytes() {
        anyhow::bail!("hash mismatch {} != {}", actual.to_hex(), digest);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::registry::types::Blake2bDigest;
    use tempfile::tempdir;

    fn hash_hex(bytes: &[u8]) -> String {
        blake2b_simd::Params::new().hash(bytes).to_hex().to_string()
    }
    #[test]
    fn typed_cli_download_uses_selected_package_url_and_digest() {
        use crate::portable::registry::types::{CliPackage, Compression};

        let tmp = tempdir().unwrap();
        let source = tmp.path().join("source");
        let destination = tmp.path().join("destination");
        let contents = b"cli bytes";
        fs_err::write(&source, contents).unwrap();
        let package = CliPackage {
            version: "7.10.2".parse().unwrap(),
            url: Url::from_file_path(source).unwrap(),
            size: contents.len() as u64,
            hash: hash_hex(contents).parse().unwrap(),
            compression: Some(Compression::Zstd),
        };

        download_cli_package_verified_sync(&destination, &package, true).unwrap();

        assert_eq!(fs_err::read(destination).unwrap(), contents);
    }

    #[tokio::test]
    async fn typed_server_download_uses_selected_package_url_and_digest() {
        use crate::portable::registry::types::{PackageType, ServerPackage};
        use std::collections::HashMap;

        let tmp = tempdir().unwrap();
        let source = tmp.path().join("source");
        let destination = tmp.path().join("destination");
        let contents = b"server bytes";
        fs_err::write(&source, contents).unwrap();
        let package = ServerPackage {
            name: "gel-server".to_string(),
            version: "7.0+abcdef0".parse().unwrap(),
            url: Url::from_file_path(source).unwrap(),
            size: contents.len() as u64,
            hash: hash_hex(contents).parse().unwrap(),
            kind: PackageType::TarZst,
            slot: "7".to_string(),
            tags: HashMap::new(),
        };

        download_server_package_verified(&destination, &package, true)
            .await
            .unwrap();

        assert_eq!(fs_err::read(destination).unwrap(), contents);
    }

    #[tokio::test]
    async fn typed_extension_download_uses_selected_package_url_and_digest() {
        use crate::portable::registry::types::{ExtensionPackage, PackageType};
        use std::collections::HashMap;

        let tmp = tempdir().unwrap();
        let source = tmp.path().join("source");
        let destination = tmp.path().join("destination");
        let contents = b"extension bytes";
        fs_err::write(&source, contents).unwrap();
        let mut tags = HashMap::new();
        tags.insert("extension".to_string(), "pgvector".to_string());
        tags.insert("server_slot".to_string(), "7".to_string());
        let package = ExtensionPackage {
            name: "pgvector".to_string(),
            version: "7.0+abcdef0".parse().unwrap(),
            url: Url::from_file_path(source).unwrap(),
            size: contents.len() as u64,
            hash: hash_hex(contents).parse().unwrap(),
            kind: PackageType::Zip,
            slot: "7".to_string(),
            tags,
        };

        download_extension_package_verified(&destination, &package, true)
            .await
            .unwrap();

        assert_eq!(fs_err::read(destination).unwrap(), contents);
    }

    async fn chunked_server(delays: Vec<Duration>) -> (Url, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            for delay in delays {
                tokio::time::sleep(delay).await;
                if stream.write_all(b"1\r\nx\r\n").await.is_err() {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        });
        (
            Url::parse(&format!("http://{address}/artifact")).unwrap(),
            handle,
        )
    }

    #[tokio::test]
    async fn artifact_response_header_idle_timeout_is_classified() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_bytes(b"artifact"),
            )
            .mount(&server)
            .await;
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("artifact");
        let url = Url::parse(&server.uri()).unwrap();
        let error = download_with_idle_timeout(&dest, &url, true, Duration::from_millis(25))
            .await
            .unwrap_err();
        let timeout = error.downcast_ref::<ArtifactIdleTimeout>().unwrap();
        assert_eq!(timeout.phase, ArtifactTimeoutPhase::ResponseHeaders);
        assert_eq!(timeout.url, url);
    }

    #[tokio::test]
    async fn artifact_body_idle_timeout_is_classified() {
        let (url, server) = chunked_server(vec![Duration::ZERO, Duration::from_millis(100)]).await;
        let tmp = tempdir().unwrap();
        let error = download_with_idle_timeout(
            tmp.path().join("artifact"),
            &url,
            true,
            Duration::from_millis(25),
        )
        .await
        .unwrap_err();
        let timeout = error.downcast_ref::<ArtifactIdleTimeout>().unwrap();
        assert_eq!(timeout.phase, ArtifactTimeoutPhase::ResponseBody);
        assert_eq!(timeout.url, url);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn active_slow_artifact_download_does_not_time_out() {
        let (url, server) = chunked_server(vec![Duration::from_millis(10); 8]).await;
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("artifact");
        download_with_idle_timeout(&dest, &url, true, Duration::from_millis(25))
            .await
            .unwrap();
        assert_eq!(fs_err::read(dest).unwrap(), b"xxxxxxxx");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn downloads_file_url_and_hashes_contents() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("artifact.bin");
        let dest = tmp.path().join("output.bin");
        let contents = b"local registry artifact contents";
        fs_err::write(&src, contents).unwrap();
        let url = Url::from_file_path(&src).unwrap();

        let hash = download(&dest, &url, true).await.unwrap();

        assert_eq!(fs_err::read(&dest).unwrap(), contents);
        assert_eq!(hash.to_hex().to_string(), hash_hex(contents));
    }

    #[tokio::test]
    async fn download_package_verified_compares_digest_bytes() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("source");
        let dest = tmp.path().join("destination");
        let contents = b"verified bytes";
        fs_err::write(&src, contents).unwrap();
        let url = Url::from_file_path(&src).unwrap();
        let uppercase = hash_hex(contents).to_uppercase();
        let digest = uppercase.parse::<Blake2bDigest>().unwrap();

        download_package_verified(&dest, &url, &digest, true)
            .await
            .unwrap();

        assert_eq!(fs_err::read(dest).unwrap(), contents);
    }

    #[tokio::test]
    async fn download_package_verified_rejects_hash_mismatch() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("artifact.bin");
        let dest = tmp.path().join("output.bin");
        let contents = b"local registry artifact contents";
        fs_err::write(&src, contents).unwrap();
        let url = Url::from_file_path(&src).unwrap();
        let expected = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let digest = expected.parse().unwrap();

        let err = download_package_verified(&dest, &url, &digest, true)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            format!("hash mismatch {} != {expected}", hash_hex(contents))
        );
    }
}
