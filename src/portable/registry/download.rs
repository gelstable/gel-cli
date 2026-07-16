//! Package artifact downloading and blake2b verification.

use std::path::Path;
use std::time::Duration;

use super::types::PackageHash;
use crate::branding::BRANDING_CLI;
use anyhow::Context;
use fn_error_context::context;
use indicatif::{ProgressBar, ProgressStyle};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

pub const USER_AGENT: &str = BRANDING_CLI;
pub const DEFAULT_TIMEOUT: Duration = Duration::new(60, 0);

#[tokio::main(flavor = "current_thread")]
pub async fn download_sync(
    dest: impl AsRef<Path>,
    url: &Url,
    quiet: bool,
) -> Result<blake2b_simd::Hash, anyhow::Error> {
    download(dest, url, quiet).await
}

#[tokio::main(flavor = "current_thread")]
pub async fn download_package_verified_sync(
    dest: impl AsRef<Path>,
    url: &Url,
    hash: &PackageHash,
    quiet: bool,
) -> anyhow::Result<()> {
    download_package_verified(dest, url, hash, quiet).await
}

#[context("failed to download file at URL: {}", url)]
pub async fn download(
    dest: impl AsRef<Path>,
    url: &Url,
    quiet: bool,
) -> Result<blake2b_simd::Hash, anyhow::Error> {
    let dest = dest.as_ref();
    log::info!("Downloading {} -> {}", url, dest.display());
    if url.scheme() == "file" {
        return download_file(dest, url, quiet).await;
    }

    let mut req = reqwest::Client::new()
        .get(url.clone())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await?
        .error_for_status()?;
    let mut out = fs::File::create(dest)
        .await
        .with_context(|| format!("writing {:?}", dest.display()))?;

    let bar = if quiet {
        ProgressBar::hidden()
    } else if let Some(len) = req.content_length() {
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
    while let Some(chunk) = req.chunk().await? {
        out.write_all(&chunk[..]).await?;
        hasher.update(&chunk[..]);
        bar.inc(chunk.len() as u64);
    }
    bar.finish();

    Ok(hasher.finalize())
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
    bar.finish();

    Ok(hasher.finalize())
}

pub async fn download_verified(
    dest: impl AsRef<Path>,
    url: &Url,
    expected_blake2b: &str,
    quiet: bool,
) -> anyhow::Result<()> {
    let hash = download(dest, url, quiet).await?;
    let hash_hex = hash.to_hex().to_string();
    if hash_hex != expected_blake2b {
        anyhow::bail!("hash mismatch {} != {}", hash_hex, expected_blake2b);
    }
    Ok(())
}

pub async fn download_package_verified(
    dest: impl AsRef<Path>,
    url: &Url,
    hash: &PackageHash,
    quiet: bool,
) -> anyhow::Result<()> {
    match hash {
        PackageHash::Blake2b(hex) => download_verified(dest, url, hex, quiet).await,
        PackageHash::Unknown(value) => {
            anyhow::bail!("cannot verify hash, unknown hash format {value:?}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::registry::PackageHash;
    use tempfile::tempdir;

    fn hash_hex(bytes: &[u8]) -> String {
        blake2b_simd::Params::new().hash(bytes).to_hex().to_string()
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
    async fn download_package_verified_accepts_matching_blake2b_for_file_url() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("artifact.bin");
        let dest = tmp.path().join("output.bin");
        let contents = b"local registry artifact contents";
        fs_err::write(&src, contents).unwrap();
        let url = Url::from_file_path(&src).unwrap();
        let hash = PackageHash::Blake2b(hash_hex(contents).into_boxed_str());

        download_package_verified(&dest, &url, &hash, true)
            .await
            .unwrap();

        assert_eq!(fs_err::read(&dest).unwrap(), contents);
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
        let hash = PackageHash::Blake2b(expected.into());

        let err = download_package_verified(&dest, &url, &hash, true)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            format!("hash mismatch {} != {expected}", hash_hex(contents))
        );
    }

    #[tokio::test]
    async fn download_package_verified_rejects_unknown_hash_format() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("artifact.bin");
        let dest = tmp.path().join("output.bin");
        fs_err::write(&src, b"local registry artifact contents").unwrap();
        let url = Url::from_file_path(&src).unwrap();
        let hash = PackageHash::Unknown("sha256:deadbeef".into());

        let err = download_package_verified(&dest, &url, &hash, true)
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "cannot verify hash, unknown hash format \"sha256:deadbeef\""
        );
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn download_verified_rejects_hash_mismatch_for_file_url() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("artifact.bin");
        let dest = tmp.path().join("output.bin");
        fs_err::write(&src, b"local registry artifact contents").unwrap();
        let url = Url::from_file_path(&src).unwrap();

        let err = download_verified(&dest, &url, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", true)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("hash mismatch"));
    }
}
