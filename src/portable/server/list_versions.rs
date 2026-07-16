use std::collections::BTreeMap;
use std::path::PathBuf;

use color_print::cprintln;
use gel_cli_derive::IntoArgs;
use serde::Serialize;

use crate::branding::BRANDING;
use crate::portable::local::{self, InstallInfo};
use crate::portable::registry::{self, Channel, ServerPackage};
use crate::portable::ver::{self};
use crate::table::{self, Cell, Row, Table};

pub fn run(cmd: &Command) -> Result<(), anyhow::Error> {
    let mut channel = cmd.channel;
    if channel.is_none() && !cmd.installed_only {
        channel = Some(Channel::Stable);
    }

    let no_opts = cmd.version.is_none() && cmd.channel.is_none() && !cmd.installed_only && !cmd.all;

    // Combine the channel, version and installed_only into a single string of plain english:
    if no_opts {
        cprintln!("Listing <bold>installed and stable</bold> versions of <bold>{BRANDING}</bold>:");
    } else if cmd.all {
        cprintln!(
            "Listing <bold>all available</bold> versions and channels of <bold>{BRANDING}</bold>:"
        );
    } else {
        let filter = if cmd.installed_only {
            let mut filter = "installed".to_string();
            if cmd.version.is_some() {
                filter.push_str(", matching");
            }
            if let Some(channel) = channel {
                filter.push_str(&format!(", {channel:?} channel").to_lowercase());
            }
            filter
        } else {
            let mut filter = if cmd.version.is_some() {
                "matching, "
            } else if channel == Some(Channel::Stable) {
                "current, "
            } else {
                ""
            }
            .to_string();
            if let Some(channel) = channel {
                filter.push_str(&format!("{channel:?} channel").to_lowercase());
            }
            filter
        };

        cprintln!("Listing <bold>{filter}</bold> versions of <bold>{BRANDING}</bold>:");
    }

    let installed = local::get_installed()?;
    let installed_count = installed.len();
    let all_packages = all_packages()?;
    let all_package_count = all_packages.len();

    // Determine the latest stable version
    let latest_stable = all_packages
        .iter()
        .filter(|p| {
            let channel = Channel::from_version(&p.version.specific()).unwrap_or(Channel::Nightly);
            channel == Channel::Stable
        })
        .max_by_key(|p| p.version.specific())
        .map(|p| p.version.specific());

    let mut version = cmd.version.clone();
    if version.is_none() && !cmd.installed_only {
        if let Some(latest_stable) = latest_stable {
            version = Some(ver::Filter {
                major: latest_stable.major,
                minor: None,
                exact: false,
            })
        }
    }

    log::debug!(
        "channel: {:?}, version: {:?}, installed_only: {:?}",
        channel,
        version,
        cmd.installed_only
    );

    let mut packages = if cmd.installed_only {
        installed
            .into_iter()
            .map(|v| JsonVersionInfo {
                channel: Channel::from_version(&v.version.specific()).unwrap_or(Channel::Nightly),
                version: v.version.clone(),
                installed: true,
                debug_info: DebugInfo {
                    package: None,
                    install: Some(DebugInstall::from(v)),
                },
            })
            .collect::<Vec<_>>()
    } else {
        let mut installed = installed
            .into_iter()
            .map(|v| (v.version.specific(), v))
            .collect::<BTreeMap<_, _>>();
        all_packages
            .into_iter()
            .map(|p| {
                let installed = installed.remove(&p.version.specific());
                JsonVersionInfo {
                    channel: Channel::from_version(&p.version.specific())
                        .unwrap_or(Channel::Nightly),
                    version: p.version.clone(),
                    installed: installed.is_some(),
                    debug_info: DebugInfo {
                        package: Some(p),
                        install: installed.map(DebugInstall::from),
                    },
                }
            })
            .collect::<Vec<_>>()
    };

    if !cmd.all {
        packages = packages
            .into_iter()
            .filter(|p| {
                // DX: This is most useful for users: if no options, union the
                // stable/current-version results with the installed versions.
                if no_opts && p.installed {
                    return true;
                }
                if let Some(channel) = channel {
                    let package_channel =
                        Channel::from_version(&p.version.specific()).unwrap_or(Channel::Nightly);
                    if package_channel != channel {
                        return false;
                    }
                }
                if let Some(version) = &version {
                    if channel == Some(Channel::Stable) || cmd.version.is_some() {
                        version.matches_loose(&p.version.specific())
                    } else {
                        // Dx: For testing/nightly without a specific version,
                        // show the next version as well.
                        version.matches_loose(&p.version.specific())
                            || ver::Filter {
                                major: version.major + 1,
                                minor: None,
                                exact: false,
                            }
                            .matches_loose(&p.version.specific())
                    }
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();
    }
    packages.sort_by(|a, b| a.version.specific().cmp(&b.version.specific()));

    if cmd.json {
        print!("{}", serde_json::to_string_pretty(&packages)?);
    } else {
        let count = packages.len();
        let count_installed = packages.iter().filter(|p| p.installed).count();
        print_table(packages.into_iter().map(|p| (p.version, p.installed)));

        if no_opts {
            let hidden = all_package_count - count;
            cprintln!(
                "HINT: {hidden} versions were hidden. To list all available versions, use the <bold>--all</bold> flag."
            );
        }
        if count_installed < installed_count {
            let hidden = installed_count - count_installed;
            cprintln!(
                "HINT: {hidden} installed versions were hidden. To list all installed versions, use the <bold>--installed-only</bold> flag."
            );
        }
    }
    Ok(())
}

#[derive(clap::Args, IntoArgs, Debug, Clone)]
pub struct Command {
    #[arg(long, conflicts_with_all=&["all"])]
    pub installed_only: bool,

    /// Show all versions including older and non-stable channels.
    #[arg(long)]
    pub all: bool,

    /// Show only versions for the specified channel.
    #[arg(long, conflicts_with_all=&["all"])]
    #[arg(value_enum)]
    pub channel: Option<Channel>,

    /// Show only versions matching the specified version.
    #[arg(long, conflicts_with_all=&["all"])]
    pub version: Option<ver::Filter>,

    /// Single column output.
    #[arg(long, value_parser=[
        "major-version", "installed", "available",
    ])]
    pub column: Option<String>,

    /// Output in JSON format.
    #[arg(long)]
    pub json: bool,
}

#[derive(serde::Serialize)]
pub struct DebugInstall {
    path: Option<PathBuf>,
    server_path: Option<PathBuf>,
    #[serde(flatten)]
    install: InstallInfo,
}

#[derive(serde::Serialize)]
struct JsonServerPackage<'a> {
    name: &'a str,
    version: &'a ver::Build,
    url: &'a url::Url,
    size: u64,
    hash: String,
    kind: &'a registry::types::PackageType,
    slot: &'a str,
    tags: &'a std::collections::HashMap<String, String>,
}

fn serialize_server_package<S>(
    package: &Option<ServerPackage>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match package {
        Some(package) => JsonServerPackage {
            name: &package.name,
            version: &package.version,
            url: &package.url,
            size: package.size,
            hash: format!("blake2b:{}", package.hash),
            kind: &package.kind,
            slot: &package.slot,
            tags: &package.tags,
        }
        .serialize(serializer),
        None => serializer.serialize_none(),
    }
}

#[derive(serde::Serialize)]
pub struct DebugInfo {
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_server_package"
    )]
    package: Option<ServerPackage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install: Option<DebugInstall>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct JsonVersionInfo {
    channel: Channel,
    version: ver::Build,
    installed: bool,
    debug_info: DebugInfo,
}

#[tokio::main(flavor = "current_thread")]
pub async fn all_packages() -> anyhow::Result<Vec<ServerPackage>> {
    let platform = crate::portable::platform::get_server()?;
    let config = registry::Config::load()?;
    let loader = registry::SourceLoader::new()?;
    all_packages_with_async(&config, &loader, platform).await
}

fn report_source(report: &registry::catalog::SourceReport) -> &registry::config::RegistrySource {
    match report {
        registry::catalog::SourceReport::Healthy { source, .. }
        | registry::catalog::SourceReport::Unavailable { source, .. }
        | registry::catalog::SourceReport::Rejected { source, .. } => source,
    }
}

fn is_degraded_report(report: &registry::catalog::SourceReport) -> bool {
    !matches!(report, registry::catalog::SourceReport::Healthy { .. })
}

async fn all_packages_with_reporter<F>(
    config: &registry::Config,
    loader: &registry::SourceLoader,
    platform: &str,
    mut on_report: F,
) -> anyhow::Result<Vec<ServerPackage>>
where
    F: FnMut(&registry::catalog::SourceReport),
{
    let mut pkgs = Vec::with_capacity(16);
    let mut first_degraded_reports = Vec::new();
    for channel in [Channel::Stable, Channel::Testing, Channel::Nightly] {
        let load: registry::CatalogLoad =
            registry::Catalog::load(config, loader, channel, platform).await?;
        pkgs.extend(load.catalog.server_packages());
        for report in load.source_reports {
            if !is_degraded_report(&report) {
                continue;
            }
            if first_degraded_reports
                .iter()
                .any(|first| report_source(first) == report_source(&report))
            {
                continue;
            }
            first_degraded_reports.push(report);
        }
    }
    let mut emitted_sources = Vec::<registry::config::RegistrySource>::new();
    for configured_source in &config.sources {
        if emitted_sources
            .iter()
            .any(|emitted_source| emitted_source == configured_source)
        {
            continue;
        }
        if let Some(report) = first_degraded_reports
            .iter()
            .find(|report| report_source(report) == configured_source)
        {
            on_report(report);
            emitted_sources.push(configured_source.clone());
        }
    }
    Ok(pkgs)
}

pub(crate) async fn all_packages_with_async(
    config: &registry::Config,
    loader: &registry::SourceLoader,
    platform: &str,
) -> anyhow::Result<Vec<ServerPackage>> {
    all_packages_with_reporter(config, loader, platform, |report| {
        if let Some(warning) = report.warning() {
            log::warn!("{warning}");
        }
    })
    .await
}

fn print_table(items: impl Iterator<Item = (ver::Build, bool)>) {
    let mut table = Table::new();
    table.set_format(*table::FORMAT);
    table.add_row(Row::new(vec![
        table::header_cell("Channel"),
        table::header_cell("Version"),
        table::header_cell("Installed"),
    ]));
    for (ver, installed) in items {
        let channel = Channel::from_version(&ver.specific());
        table.add_row(Row::new(vec![
            Cell::new(channel.as_ref().map_or("nightly", |x| x.as_str())),
            Cell::new(&ver.to_string()),
            Cell::new(if installed { "✓" } else { "" }),
        ]));
    }
    table.printstd();
}

impl DebugInstall {
    fn from(install: InstallInfo) -> DebugInstall {
        DebugInstall {
            path: install.base_path().ok(),
            server_path: install.server_path().ok(),
            install,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::registry::catalog::{NoHealthySources, SourceReport};
    use crate::portable::registry::config::{Config, RegistrySource};
    use crate::portable::registry::source::{Source, SourceLoader};
    use crate::portable::registry::types::PackageType;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn server_index_body(version: &str) -> String {
        format!(
            r#"{{"packages":[{{"basename":"gel-server","version":"{version}","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-server-{version}.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
        )
    }

    #[tokio::test]
    async fn all_packages_with_propagates_registry_source_error() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let stable_index = tmp.path().join("stable-linux.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"stable-linux.json"},
              {"channel":"testing","platform":"linux","url":"malformed-index.json"},
              {"channel":"nightly","platform":"linux","url":"nightly-linux.json"}
            ]}"#,
        )
        .unwrap();
        fs_err::write(&stable_index, server_index_body("7.0+abcdef0")).unwrap();
        // malformed-index.json: not valid JSON, must abort the whole load.
        fs_err::write(tmp.path().join("malformed-index.json"), b"{not-json").unwrap();
        fs_err::write(
            tmp.path().join("nightly-linux.json"),
            server_index_body("8.0+abcdef0"),
        )
        .unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();

        let error = all_packages_with_async(&config, &loader, "linux")
            .await
            .unwrap_err();

        let aggregate = error
            .downcast_ref::<NoHealthySources>()
            .expect("expected no-healthy-sources aggregate");
        assert_eq!(aggregate.reports().len(), 1);
        assert!(matches!(
            &aggregate.reports()[0],
            SourceReport::Rejected { .. }
        ));
    }
    #[tokio::test]
    async fn all_packages_with_returns_packages_when_all_channels_load() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"stable.json"},
              {"channel":"testing","platform":"linux","url":"testing.json"},
              {"channel":"nightly","platform":"linux","url":"nightly.json"}
            ]}"#,
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("stable.json"),
            server_index_body("7.0+abcdef0"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("testing.json"),
            server_index_body("7.1+abcdef0"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("nightly.json"),
            server_index_body("8.0+abcdef0"),
        )
        .unwrap();

        let config = Config {
            sources: vec![RegistrySource::Manifest(Source::File(manifest))],
        };
        let loader = SourceLoader::new().unwrap();

        let packages = all_packages_with_async(&config, &loader, "linux")
            .await
            .unwrap();

        assert_eq!(packages.len(), 3);
    }

    #[tokio::test]
    async fn all_packages_with_reports_degraded_source_through_callback() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let missing_manifest = tmp.path().join("missing-registry.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"stable.json"},
              {"channel":"testing","platform":"linux","url":"testing.json"},
              {"channel":"nightly","platform":"linux","url":"nightly.json"}
            ]}"#,
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("stable.json"),
            server_index_body("7.0+abcdef0"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("testing.json"),
            server_index_body("7.1+abcdef0"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("nightly.json"),
            server_index_body("8.0+abcdef0"),
        )
        .unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(manifest)),
                RegistrySource::Manifest(Source::File(missing_manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let mut warnings = Vec::new();

        let packages = all_packages_with_reporter(&config, &loader, "linux", |report| {
            if let Some(warning) = report.warning() {
                warnings.push(warning);
            }
        })
        .await
        .unwrap();

        assert_eq!(packages.len(), 3);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings
                .iter()
                .all(|warning| warning.contains("is unavailable"))
        );
    }

    #[tokio::test]
    async fn all_packages_with_reports_warns_once_per_distinct_degraded_source() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("registry.json");
        let missing_manifest = tmp.path().join("missing-registry.json");
        let second_missing_manifest = tmp.path().join("second-missing-registry.json");
        fs_err::write(
            &manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"stable.json"},
              {"channel":"testing","platform":"linux","url":"testing.json"},
              {"channel":"nightly","platform":"linux","url":"nightly.json"}
            ]}"#,
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("stable.json"),
            server_index_body("7.0+abcdef0"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("testing.json"),
            server_index_body("7.1+abcdef0"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("nightly.json"),
            server_index_body("8.0+abcdef0"),
        )
        .unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(manifest)),
                RegistrySource::Manifest(Source::File(missing_manifest.clone())),
                RegistrySource::Manifest(Source::File(second_missing_manifest.clone())),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let mut warnings = Vec::new();

        all_packages_with_reporter(&config, &loader, "linux", |report| {
            if let Some(warning) = report.warning() {
                warnings.push(warning);
            }
        })
        .await
        .unwrap();

        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains(&missing_manifest.display().to_string()));
        assert!(warnings[1].contains(&second_missing_manifest.display().to_string()));
    }

    #[tokio::test]
    async fn all_packages_with_reports_warns_once_for_duplicate_equal_source() {
        let tmp = tempfile::tempdir().unwrap();
        let healthy_manifest = tmp.path().join("healthy.json");
        let stable_index = tmp.path().join("stable.json");
        let missing_manifest = tmp.path().join("missing.json");
        fs_err::write(
            &healthy_manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"stable.json"}
            ]}"#,
        )
        .unwrap();
        fs_err::write(&stable_index, server_index_body("7.0+abcdef0")).unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(healthy_manifest)),
                RegistrySource::Manifest(Source::File(missing_manifest.clone())),
                RegistrySource::Manifest(Source::File(missing_manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let mut warnings = Vec::new();

        all_packages_with_reporter(&config, &loader, "linux", |report| {
            if let Some(warning) = report.warning() {
                warnings.push(warning);
            }
        })
        .await
        .unwrap();

        assert_eq!(warnings.len(), 1);
    }

    #[tokio::test]
    async fn all_packages_with_reports_keeps_first_degraded_outcome_per_source() {
        let tmp = tempfile::tempdir().unwrap();
        let first_manifest = tmp.path().join("first.json");
        let fallback_manifest = tmp.path().join("fallback.json");
        fs_err::write(
            &first_manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"first-missing.json"},
              {"channel":"testing","platform":"linux","url":"first-malformed.json"},
              {"channel":"nightly","platform":"linux","url":"first-nightly.json"}
            ]}"#,
        )
        .unwrap();
        fs_err::write(tmp.path().join("first-malformed.json"), b"{not-json").unwrap();
        fs_err::write(
            tmp.path().join("first-nightly.json"),
            server_index_body("8.0+aaaaaaa"),
        )
        .unwrap();
        fs_err::write(
            &fallback_manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"fallback-stable.json"},
              {"channel":"testing","platform":"linux","url":"fallback-testing.json"},
              {"channel":"nightly","platform":"linux","url":"fallback-nightly.json"}
            ]}"#,
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("fallback-stable.json"),
            server_index_body("7.0+bbbbbbb"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("fallback-testing.json"),
            server_index_body("7.1+bbbbbbb"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("fallback-nightly.json"),
            server_index_body("8.1+bbbbbbb"),
        )
        .unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(first_manifest)),
                RegistrySource::Manifest(Source::File(fallback_manifest)),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let mut warnings = Vec::new();

        all_packages_with_reporter(&config, &loader, "linux", |report| {
            if let Some(warning) = report.warning() {
                warnings.push(warning);
            }
        })
        .await
        .unwrap();

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("is unavailable"));
    }

    #[tokio::test]
    async fn all_packages_with_reports_emits_degraded_sources_in_config_order() {
        let tmp = tempfile::tempdir().unwrap();
        let first_manifest = tmp.path().join("first.json");
        let second_manifest = tmp.path().join("second.json");
        fs_err::write(
            &first_manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"first-stable.json"},
              {"channel":"testing","platform":"linux","url":"first-testing-missing.json"},
              {"channel":"nightly","platform":"linux","url":"first-nightly.json"}
            ]}"#,
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("first-stable.json"),
            server_index_body("7.0+aaaaaaa"),
        )
        .unwrap();
        fs_err::write(
            tmp.path().join("first-nightly.json"),
            server_index_body("8.0+aaaaaaa"),
        )
        .unwrap();
        fs_err::write(
            &second_manifest,
            br#"{"schema_version":1,"indexes":[
              {"channel":"stable","platform":"linux","url":"second-stable-missing.json"}
            ]}"#,
        )
        .unwrap();

        let config = Config {
            sources: vec![
                RegistrySource::Manifest(Source::File(first_manifest.clone())),
                RegistrySource::Manifest(Source::File(second_manifest.clone())),
            ],
        };
        let loader = SourceLoader::new().unwrap();
        let mut warnings = Vec::new();

        all_packages_with_reporter(&config, &loader, "linux", |report| {
            if let Some(warning) = report.warning() {
                warnings.push(warning);
            }
        })
        .await
        .unwrap();

        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains(&first_manifest.display().to_string()));
        assert!(warnings[1].contains(&second_manifest.display().to_string()));
    }

    #[test]
    fn list_versions_json_preserves_prefixed_server_digest() {
        let package = ServerPackage {
            name: "gel-server".to_string(),
            version: "7.0+abcdef0".parse().unwrap(),
            url: url::Url::parse("https://example.com/server.tar.zst").unwrap(),
            size: 10,
            hash: HASH.parse().unwrap(),
            kind: PackageType::TarZst,
            slot: "7".to_string(),
            tags: std::collections::HashMap::new(),
        };
        let json = serde_json::to_string(&JsonVersionInfo {
            channel: Channel::Stable,
            version: package.version.clone(),
            installed: false,
            debug_info: DebugInfo {
                package: Some(package),
                install: None,
            },
        })
        .unwrap();

        assert_eq!(
            json,
            format!(
                r#"{{"channel":"stable","version":"7.0+abcdef0","installed":false,"debug-info":{{"package":{{"name":"gel-server","version":"7.0+abcdef0","url":"https://example.com/server.tar.zst","size":10,"hash":"blake2b:{HASH}","kind":"TarZst","slot":"7","tags":{{}}}}}}}}"#
            )
        );
    }
}
