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

#[allow(unused_imports)]
pub(crate) use catalog::{Catalog, load_default, load_default_or_empty};
pub(crate) use config::Config;
#[allow(unused_imports)]
pub(crate) use download::{
    DEFAULT_TIMEOUT, USER_AGENT, download, download_sync, download_verified,
};
pub(crate) use source::{Source, SourceLoader};
#[allow(unused_imports)]
pub(crate) use types::{
    Channel, CliPackage, Compression, ExtensionPackage, PackageHash, PackageType, Query,
    QueryDisplay, QuerySelector, ServerPackage,
};

fn server_platform(query: &Query) -> anyhow::Result<&'static str> {
    if cfg!(all(target_arch = "aarch64", target_os = "macos"))
        && query
            .version
            .as_ref()
            .map(|v| v.major == 1)
            .unwrap_or(false)
    {
        Ok("x86_64-apple-darwin")
    } else {
        platform::get_server()
    }
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
    let catalog = timeout(timeo, load_default_or_empty(channel, platform)).await?;
    Ok(catalog.cli_packages())
}

#[tokio::main(flavor = "current_thread")]
pub async fn get_server_packages(channel: Channel) -> anyhow::Result<Vec<ServerPackage>> {
    let plat = platform::get_server()?;
    load_platform_server_packages(channel, plat).await
}

#[tokio::main(flavor = "current_thread")]
pub async fn get_platform_server_packages(
    channel: Channel,
    platform: &str,
) -> anyhow::Result<Vec<ServerPackage>> {
    load_platform_server_packages(channel, platform).await
}

pub(crate) async fn load_platform_server_packages(
    channel: Channel,
    platform: &str,
) -> anyhow::Result<Vec<ServerPackage>> {
    let catalog = load_default_or_empty(channel, platform).await?;
    Ok(catalog.server_packages())
}

#[tokio::main(flavor = "current_thread")]
#[allow(dead_code)]
pub async fn get_platform_extension_packages(
    channel: Channel,
    slot: &str,
    platform: &str,
) -> anyhow::Result<Vec<ExtensionPackage>> {
    get_platform_extension_packages_async(channel, slot, platform).await
}

async fn get_platform_extension_packages_async(
    channel: Channel,
    slot: &str,
    platform: &str,
) -> anyhow::Result<Vec<ExtensionPackage>> {
    let catalog = load_default_or_empty(channel, platform).await?;
    Ok(catalog.extension_packages(slot))
}

#[tokio::main(flavor = "current_thread")]
pub async fn get_server_package(query: &Query) -> anyhow::Result<Option<ServerPackage>> {
    let plat = server_platform(query)?;
    if cfg!(all(target_arch = "aarch64", target_os = "macos"))
        && query
            .version
            .as_ref()
            .map(|v| v.major == 1)
            .unwrap_or(false)
    {
        return get_platform_server_package(query, "x86_64-apple-darwin").await;
    }
    get_platform_server_package(query, plat).await
}

async fn get_platform_server_package(
    query: &Query,
    platform: &str,
) -> anyhow::Result<Option<ServerPackage>> {
    let filter = query.version.as_ref();
    let pkg = load_platform_server_packages(query.channel, platform)
        .await?
        .into_iter()
        .filter(|pkg| filter.map(|q| q.matches(&pkg.version)).unwrap_or(true))
        .max_by_key(|pkg| pkg.version.specific());
    Ok(pkg)
}

#[tokio::main(flavor = "current_thread")]
#[allow(dead_code)]
pub async fn get_specific_package(
    version: &ver::Specific,
) -> anyhow::Result<Option<ServerPackage>> {
    let channel = Channel::from_version(version)?;
    let all = if cfg!(all(target_arch = "aarch64", target_os = "macos")) && version.major == 1 {
        load_platform_server_packages(channel, "x86_64-apple-darwin").await?
    } else {
        let plat = platform::get_server()?;
        load_platform_server_packages(channel, plat).await?
    };
    let pkg = all
        .into_iter()
        .find(|pkg| &pkg.version.specific() == version);
    Ok(pkg)
}
