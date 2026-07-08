use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::Context;
use fn_error_context::context;
use gel_cli_derive::IntoArgs;
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::LazyLock;

use crate::branding::BRANDING_CLI_CMD;
use crate::commands::ExitCode;
use crate::platform;
use crate::portable::exit_codes;
use crate::portable::local::{InstallInfo, write_json};
use crate::portable::platform::{get_server, optional_docker_check};
use crate::portable::registry::{
    self, Channel, PackageHash, Query, QuerySelector, ServerPackage, download_verified,
};
use crate::portable::ver::{self, Build};
use crate::print::{self, Highlight};

static INSTALLED_VERSIONS: LazyLock<Mutex<BTreeSet<Build>>> =
    LazyLock::new(|| Mutex::new(BTreeSet::new()));

pub fn run(options: &Command) -> anyhow::Result<()> {
    if optional_docker_check()? {
        print::error!("`{BRANDING_CLI_CMD} server install` not supported in Docker containers.");
        Err(ExitCode::new(exit_codes::DOCKER_CONTAINER))?;
    }
    let selector = if let Some(version) = options.version.as_ref() {
        Some(QuerySelector::Version(version))
    } else if let Some(channel) = options.channel {
        Some(QuerySelector::Channel(channel))
    } else if options.nightly {
        Some(QuerySelector::Channel(Channel::Nightly))
    } else {
        None
    };
    let (query, _) = Query::from_selector(selector, || Ok(Query::stable()))?;
    version(&query)?;
    Ok(())
}

#[derive(clap::Args, IntoArgs, Debug, Clone)]
pub struct Command {
    #[arg(short = 'i', long)]
    pub interactive: bool,
    #[arg(long, conflicts_with_all=&["channel", "version"])]
    pub nightly: bool,
    #[arg(long, conflicts_with_all=&["nightly", "channel"])]
    pub version: Option<ver::Filter>,
    #[arg(long, conflicts_with_all=&["nightly", "version"])]
    #[arg(value_enum)]
    pub channel: Option<Channel>,
}

pub fn version(query: &Query) -> anyhow::Result<InstallInfo> {
    let pkg_info = select_package(query)?
        .with_context(|| format!("no package matching your criteria found: {query:?}"))?;
    ver::print_version_hint(&pkg_info.version.specific(), query);
    package(&pkg_info)
}

pub fn specific(version: &ver::Specific) -> anyhow::Result<InstallInfo> {
    let target_dir = platform::portable_dir()?.join(version.to_string());
    if target_dir.exists() {
        return InstallInfo::read(&target_dir);
    }
    let pkg = select_specific_package(version)?
        .with_context(|| format!("cannot find package {version}"))?;
    package(&pkg)
}

fn server_platform(query: &Query) -> anyhow::Result<&'static str> {
    if cfg!(all(target_arch = "aarch64", target_os = "macos"))
        && query
            .version
            .as_ref()
            .map(|version| version.major == 1)
            .unwrap_or(false)
    {
        Ok("x86_64-apple-darwin")
    } else {
        get_server()
    }
}

fn server_platform_for_specific(version: &ver::Specific) -> anyhow::Result<&'static str> {
    if cfg!(all(target_arch = "aarch64", target_os = "macos")) && version.major == 1 {
        Ok("x86_64-apple-darwin")
    } else {
        get_server()
    }
}

fn select_package(query: &Query) -> anyhow::Result<Option<ServerPackage>> {
    let platform = server_platform(query)?;
    let query = query.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let catalog = registry::load_default_or_empty(query.channel, platform).await?;
            let filter = query.version.as_ref();
            Ok::<_, anyhow::Error>(
                catalog
                    .server_packages()
                    .into_iter()
                    .filter(|pkg| filter.map(|q| q.matches(&pkg.version)).unwrap_or(true))
                    .max_by_key(|pkg| pkg.version.specific()),
            )
        })
}

fn select_specific_package(version: &ver::Specific) -> anyhow::Result<Option<ServerPackage>> {
    let platform = server_platform_for_specific(version)?;
    let channel = Channel::from_version(version)?;
    let version = version.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let catalog = registry::load_default_or_empty(channel, platform).await?;
            Ok::<_, anyhow::Error>(
                catalog
                    .server_packages()
                    .into_iter()
                    .find(|pkg| pkg.version.specific() == version),
            )
        })
}

#[tokio::main(flavor = "current_thread")]
pub async fn package(pkg_info: &ServerPackage) -> anyhow::Result<InstallInfo> {
    let ver_name = pkg_info.version.specific().to_string();
    let target_dir = platform::portable_dir()?.join(ver_name);
    if target_dir.exists() {
        let meta = check_metadata(&target_dir, pkg_info)?;
        if INSTALLED_VERSIONS
            .lock()
            .unwrap()
            .insert(meta.version.clone())
        {
            print::msg!(
                "Version {} is already downloaded",
                meta.version.to_string().emphasized()
            );
        }
        return Ok(meta);
    }

    print::msg!("Downloading package...");
    let cache_path = download_package(pkg_info).await?;
    let tmp_target = platform::tmp_file_path(&target_dir);
    unpack_package(&cache_path, &tmp_target)?;
    let info = InstallInfo {
        version: pkg_info.version.clone(),
        package_url: pkg_info.url.clone(),
        package_hash: pkg_info.hash.clone(),
        installed_at: SystemTime::now(),
        slot: pkg_info.slot.clone(),
    };
    write_json(&tmp_target.join("install_info.json"), "metadata", &info)?;
    fs::rename(&tmp_target, &target_dir)
        .with_context(|| format!("cannot rename {tmp_target:?} -> {target_dir:?}"))?;
    unlink_cache(&cache_path);
    print::msg!(
        "Successfully installed {}",
        pkg_info.version.to_string().emphasized()
    );
    INSTALLED_VERSIONS
        .lock()
        .unwrap()
        .insert(pkg_info.version.clone());

    Ok(info)
}

#[context("metadata error for {:?}", dir)]
fn check_metadata(dir: &Path, pkg_info: &ServerPackage) -> anyhow::Result<InstallInfo> {
    let data = InstallInfo::read(dir)?;
    if data.version != pkg_info.version {
        log::warn!(
            "Remote package has version {},
                    installed package version: {}",
            pkg_info.version,
            data.version
        );
    }
    log::info!(
        "Package {} was installed at {}, location: {:?}",
        data.version,
        humantime::format_rfc3339(data.installed_at),
        dir
    );
    Ok(data)
}

#[context("failed to download {}", pkg_info)]
pub async fn download_package(pkg_info: &ServerPackage) -> anyhow::Result<PathBuf> {
    let cache_dir = platform::cache_dir()?;
    let download_dir = cache_dir.join("downloads");
    fs::create_dir_all(&download_dir)?;
    let cache_path = download_dir.join(pkg_info.cache_file_name());
    match &pkg_info.hash {
        PackageHash::Blake2b(hex) => {
            download_verified(&cache_path, &pkg_info.url, hex, false).await?;
        }
        PackageHash::Unknown(val) => {
            anyhow::bail!("cannot verify hash, unknown hash format {val:?}");
        }
    }
    Ok(cache_path)
}

fn build_path(base: &Path, path: &Path) -> anyhow::Result<Option<PathBuf>> {
    let mut components = path.components().filter_map(|part| {
        match part {
            Component::Normal(part) => Some(Ok(part)),
            // Leading '/' characters, root paths, and '.'
            // components are just ignored and treated as "empty
            // components"
            Component::Prefix(..) | Component::RootDir | Component::CurDir => None,
            // If any part of the filename is '..', then skip over
            // unpacking the file to prevent directory traversal
            // security issues.  See, e.g.: CVE-2001-1267,
            // CVE-2002-0399, CVE-2005-1918, CVE-2007-4131
            Component::ParentDir => Some(Err(anyhow::anyhow!("erroneous path {:?}", path))),
        }
    });
    if let Some(directory_name) = components.next() {
        directory_name?;
    } else {
        return Ok(None); // skipping root
    }

    let mut dest = PathBuf::from(base);
    if let Some(component) = components.next() {
        dest.push(component?);
    } else {
        return Ok(None); // the package directory itself
    }
    for component in components {
        let component = component?;
        match dest.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => {
                anyhow::bail!("cannot unpack {:?} to the symlinked dir {:?}", path, dest);
            }
            Ok(m) if m.file_type().is_file() => {
                anyhow::bail!("{:?} is a file, not a directory for {:?}", dest, path);
            }
            Ok(_) => {}
            Err(_) => {
                fs::create_dir(&dest)?;
            }
        }
        dest.push(component);
    }
    Ok(Some(dest))
}

#[context("failed to unpack {:?} -> {:?}", cache_file, target_dir)]
fn unpack_package(cache_file: &Path, target_dir: &Path) -> anyhow::Result<()> {
    if target_dir.exists() {
        fs::remove_dir_all(target_dir)?;
    }
    fs::create_dir_all(target_dir)?;

    // needed for long paths on windows
    let target_dir = target_dir.canonicalize()?;

    let file = fs::File::open(cache_file)?;
    let bar = ProgressBar::new(file.metadata()?.len());
    bar.set_style(
        ProgressStyle::default_bar()
            .template("Unpacking [{bar}] {bytes:>7.dim}/{total_bytes:7}")
            .expect("template is ok")
            .progress_chars("=> "),
    );
    let file = zstd::Decoder::new(io::BufReader::new(bar.wrap_read(file)))?;
    let mut arch = tar::Archive::new(file);

    for entry in arch.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if let Some(path) = build_path(&target_dir, &path)? {
            entry.unpack(path)?;
        }
    }
    bar.finish_and_clear();
    Ok(())
}

fn unlink_cache(cache_file: &Path) {
    fs::remove_file(cache_file)
        .map_err(|e| {
            log::warn!("Failed to remove cache {cache_file:?}: {e}");
        })
        .ok();
}
