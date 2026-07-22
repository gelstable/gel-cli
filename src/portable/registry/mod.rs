//! Portable package registry configuration, loading, querying, and downloading.

use std::time::Duration;

use crate::async_util::timeout;
use crate::portable::platform;
use crate::portable::ver;

pub(crate) mod catalog;
pub(crate) mod config;
pub(crate) mod download;
pub(crate) mod index;
pub(crate) mod manifest;
pub(crate) mod source;
pub(crate) mod types;

#[cfg(test)]
mod fixture_tests;

pub(crate) use catalog::{Catalog, CatalogLoad, load_default_async};
pub(crate) use config::Config;
pub(crate) use download::{
    USER_AGENT, download_cli_package_verified_sync, download_extension_package_verified,
    download_server_package_verified, download_sync,
};
pub(crate) use source::{Source, SourceLoader};
pub(crate) use types::{
    Channel, CliPackage, Compression, ExtensionPackage, PackageHash, Query, QuerySelector,
    ServerPackage,
};

fn server_platform_for_host(
    host_arch: &str,
    host_os: &str,
    native_platform: &'static str,
    requested_major: Option<u32>,
) -> &'static str {
    if host_arch == "aarch64" && host_os == "macos" && requested_major == Some(1) {
        "x86_64-apple-darwin"
    } else {
        native_platform
    }
}

fn server_platform_for_query(query: &Query) -> anyhow::Result<&'static str> {
    let native_platform = platform::get_server()?;
    Ok(server_platform_for_host(
        std::env::consts::ARCH,
        std::env::consts::OS,
        native_platform,
        query.version.as_ref().map(|version| version.major),
    ))
}

fn server_platform_for_specific(version: &ver::Specific) -> anyhow::Result<&'static str> {
    let native_platform = platform::get_server()?;
    Ok(server_platform_for_host(
        std::env::consts::ARCH,
        std::env::consts::OS,
        native_platform,
        Some(version.major),
    ))
}

#[tokio::main(flavor = "current_thread")]
pub async fn get_cli_packages(
    channel: Channel,
    timeout: Duration,
) -> anyhow::Result<Vec<CliPackage>> {
    get_platform_cli_packages_async(channel, platform::get_cli()?, timeout).await
}

#[tokio::main(flavor = "current_thread")]
pub async fn get_platform_cli_packages(
    channel: Channel,
    platform: &str,
    timeo: Duration,
) -> anyhow::Result<Vec<CliPackage>> {
    get_platform_cli_packages_async(channel, platform, timeo).await
}

async fn get_platform_cli_packages_async(
    channel: Channel,
    platform: &str,
    timeo: Duration,
) -> anyhow::Result<Vec<CliPackage>> {
    let catalog = timeout(timeo, load_default_async(channel, platform)).await?;
    Ok(catalog.cli_packages())
}

pub(crate) async fn load_platform_server_packages_async(
    channel: Channel,
    platform: &str,
) -> anyhow::Result<Vec<ServerPackage>> {
    let catalog = load_default_async(channel, platform).await?;
    Ok(catalog.server_packages())
}

#[tokio::main(flavor = "current_thread")]
pub async fn get_server_package(query: &Query) -> anyhow::Result<Option<ServerPackage>> {
    select_server_package_async(query).await
}

async fn select_server_package_async(query: &Query) -> anyhow::Result<Option<ServerPackage>> {
    let platform = server_platform_for_query(query)?;
    let packages = load_platform_server_packages_async(query.channel, platform).await?;
    Ok(select_server_package_from_packages(packages, query))
}

fn select_server_package_from_packages(
    packages: Vec<ServerPackage>,
    query: &Query,
) -> Option<ServerPackage> {
    let filter = query.version.as_ref();
    packages
        .into_iter()
        .filter(|pkg| filter.map(|q| q.matches(&pkg.version)).unwrap_or(true))
        .max_by_key(|pkg| pkg.version.specific())
}

#[tokio::main(flavor = "current_thread")]
pub async fn get_specific_package(
    version: &ver::Specific,
) -> anyhow::Result<Option<ServerPackage>> {
    select_specific_server_package_async(version).await
}

async fn select_specific_server_package_async(
    version: &ver::Specific,
) -> anyhow::Result<Option<ServerPackage>> {
    let channel = Channel::from_version(version)?;
    let platform = server_platform_for_specific(version)?;
    let packages = load_platform_server_packages_async(channel, platform).await?;
    Ok(select_specific_server_package_from_packages(
        packages, version,
    ))
}

fn select_specific_server_package_from_packages(
    packages: Vec<ServerPackage>,
    version: &ver::Specific,
) -> Option<ServerPackage> {
    packages
        .into_iter()
        .find(|pkg| pkg.version.specific() == *version)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::registry::types::PackageType;
    use std::str::FromStr;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn server_platform_falls_back_to_x86_for_aarch64_macos_v1() {
        assert_eq!(
            server_platform_for_host("aarch64", "macos", "aarch64-apple-darwin", Some(1),),
            "x86_64-apple-darwin"
        );
    }

    #[test]
    fn server_platform_uses_native_for_aarch64_macos_v2_and_later() {
        assert_eq!(
            server_platform_for_host("aarch64", "macos", "aarch64-apple-darwin", Some(2),),
            "aarch64-apple-darwin"
        );
    }

    #[test]
    fn server_platform_uses_native_on_other_hosts() {
        assert_eq!(
            server_platform_for_host("x86_64", "macos", "x86_64-apple-darwin", Some(1),),
            "x86_64-apple-darwin"
        );
        assert_eq!(
            server_platform_for_host("aarch64", "linux", "aarch64-unknown-linux-gnu", Some(1),),
            "aarch64-unknown-linux-gnu"
        );
    }

    #[test]
    fn query_selection_uses_highest_matching_fixture_package() {
        let packages = fixture_packages();
        let query = Query {
            channel: Channel::Stable,
            version: Some(ver::Filter::from_str("1").unwrap()),
        };

        let package = select_server_package_from_packages(packages, &query).unwrap();
        assert_eq!(
            package.version.specific(),
            ver::Specific::from_str("1.2").unwrap()
        );
    }

    #[test]
    fn exact_selection_uses_matching_fixture_package() {
        let packages = fixture_packages();
        let version = ver::Specific::from_str("2.0").unwrap();

        let package =
            select_specific_server_package_from_packages(packages.clone(), &version).unwrap();
        assert_eq!(package.version.specific(), version);

        let missing = ver::Specific::from_str("3.0").unwrap();
        assert!(select_specific_server_package_from_packages(packages, &missing).is_none());
    }

    fn fixture_packages() -> Vec<ServerPackage> {
        vec![
            fixture_package("1.1+aaaaaaa"),
            fixture_package("1.2+bbbbbbb"),
            fixture_package("2.0+ccccccc"),
        ]
    }

    fn fixture_package(version: &str) -> ServerPackage {
        ServerPackage {
            name: "gel-server".into(),
            version: ver::Build::from_str(version).unwrap(),
            url: url::Url::parse("https://example.com/server.tar.zst").unwrap(),
            size: 1,
            hash: HASH.parse().unwrap(),
            kind: PackageType::TarZst,
            slot: "default".into(),
            tags: Default::default(),
        }
    }
}
