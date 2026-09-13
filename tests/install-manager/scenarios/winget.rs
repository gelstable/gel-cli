//! A WinGet portable install, on Windows.
//!
//! The rule under test is `path.contains("winget/packages/")`. A WinGet
//! *portable* package is unpacked into
//! `%LOCALAPPDATA%\Microsoft\WinGet\Packages\<PackageIdentifier>_<source>\`
//! for a user-scope install, or `%ProgramFiles%\WinGet\Packages\...` for a
//! machine-scope one, and a symlink to it goes into WinGet's `Links` directory.
//! `detect_from_path` lowercases and flips backslashes before matching, so both
//! spellings normalise to `.../winget/packages/...` and both are `WinGet` —
//! which is the whole point, because the user-scope directory sits under the
//! home directory where the CLI would otherwise have assumed it owned itself.
//!
//! Packaging is a three-file "multi-file manifest" (version, installer and
//! default-locale YAML) in one directory, installed with
//! `winget install --manifest <dir>`. The singleton form is deprecated and
//! newer WinGet builds warn on it; the three-file form is what `winget.run` and
//! the community repository actually ship. `InstallerUrl` is a `file://` URL
//! into this scenario's tempdir — WinGet's downloader copies local files rather
//! than fetching them — and `InstallerSha256` is checked, so a bad copy fails
//! as a hash mismatch rather than as a broken install.
//!
//! Two host prerequisites, neither of which this scenario will arrange for
//! itself:
//!
//! * WinGet has to be present. It is *not* preinstalled on the `windows-2025`
//!   GitHub runner image, and bootstrapping `Microsoft.DesktopAppInstaller`
//!   onto Windows Server is exactly the flakiness this harness is supposed to
//!   avoid, so in CI this scenario skips. See `run` — the skip is deliberately
//!   loud, because a quiet one under `continue-on-error: true` reads as a pass.
//! * `winget install --manifest` is gated on the `LocalManifestFiles` setting
//!   for a non-elevated user. An elevated shell (which is what a GitHub Windows
//!   runner gives you) needs nothing; otherwise the host needs
//!   `winget settings --enable LocalManifestFiles` once, as administrator. A
//!   host without it fails with WinGet's own message naming that command.
//!
//! The package identifier is deliberately *not* the real `Gelstable.Gel` that
//! `InstallManager::WinGet.upgrade_hint()` names. Cleanup uninstalls by
//! identifier, and a fixture that shares an identifier with a package someone
//! might genuinely have installed is a fixture that can uninstall it.

/// Windows-only, but the `#[test]` wrapper is not: `main.rs` must list the same
/// eight scenarios on every host so that CI's `--exact e2e_winget` selection is
/// a property of the target rather than of the runner that compiled it.
#[cfg(not(windows))]
pub fn run() {
    eprintln!("skipping: the winget scenario only runs on Windows");
}

#[cfg(windows)]
pub use imp::run;

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};

    use sha2::{Digest, Sha256};

    use crate::scenario::{self, Scenario};

    /// A `Publisher.Package` identifier nothing real will ever use, so that
    /// `winget uninstall --id` in cleanup cannot reach a package the developer
    /// installed on purpose. Notably *not* `Gelstable.Gel`.
    const PACKAGE_ID: &str = "GelCliE2E.GelInstallManagerFixture";

    const PACKAGE_VERSION: &str = "0.0.0";

    /// Manifest schema version. 1.6.0 is understood by every WinGet that still
    /// ships (1.6 landed in 2023) and, being older than current, is also
    /// accepted by newer ones — manifests are forward-compatible, not backward.
    const MANIFEST_SCHEMA: &str = "1.6.0";

    /// The name of the symlink WinGet puts in its `Links` directory. Without it
    /// WinGet would derive the alias from the installer's file name and put a
    /// `gel.exe` on the developer's `PATH`, shadowing their real CLI for as
    /// long as the scenario ran.
    const COMMAND_ALIAS: &str = "gel-e2e.exe";

    /// WinGet matches `Architecture` against the host and refuses an installer
    /// that claims a different one.
    const ARCHITECTURE: &str = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "x64"
    };

    pub fn run() {
        if !available() {
            eprintln!(
                "SKIPPING e2e_winget: WINGET IS NOT USABLE ON THIS HOST, SO THIS \
                 SCENARIO PROVED NOTHING.\n\
                 `winget` could not be found on PATH, or could not run. WinGet is \
                 not part of the `windows-2025` GitHub runner image (that image \
                 ships Chocolatey, not the Windows Package Manager), and this \
                 harness deliberately does not bootstrap \
                 Microsoft.DesktopAppInstaller onto Windows Server.\n\
                 Nothing about WinGet detection was exercised: treat a green \
                 result for this scenario as \"not run\", never as \"passed\"."
            );
            return;
        }
        if let Some(reason) = occupied() {
            eprintln!("skipping: {reason}");
            return;
        }
        let scenario = WinGetScenario::new().expect("prepare the winget scenario");
        scenario::assert_managed(&scenario);
    }

    /// Whether WinGet is really here, not merely named on `PATH`.
    ///
    /// The `winget.exe` under `%LOCALAPPDATA%\Microsoft\WindowsApps` is an App
    /// Execution Alias — a zero-byte reparse point that `which` happily finds
    /// and that fails at exec time when the App Installer package behind it is
    /// not provisioned. So presence on `PATH` is necessary but not sufficient,
    /// and the real question is whether it answers `--version`.
    fn available() -> bool {
        scenario::have("winget")
            && scenario::run_quietly(winget().arg("--version")).is_some_and(|out| out.success)
    }

    /// A `winget` command with the prompts that would hang a captured stdin
    /// turned off. `--disable-interactivity` rather than piping "y": the
    /// agreements prompts are the ones `--accept-*-agreements` covers, and
    /// anything else asking a question here is a bug we want to see fail.
    fn winget() -> Command {
        Command::new("winget")
    }

    /// The directories a portable install can land in, user scope first.
    ///
    /// `dirs::data_local_dir()` rather than `%LOCALAPPDATA%` because on Windows
    /// `dirs` resolves through the Known Folder API — the same thing WinGet
    /// itself uses, and the reason this harness cannot isolate Windows paths by
    /// setting environment variables. `%ProgramFiles%` has no Known Folder
    /// accessor in `dirs`, so it comes from the environment.
    fn portable_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Some(local) = dirs::data_local_dir() {
            roots.push(local.join("Microsoft").join("WinGet").join("Packages"));
        }
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            roots.push(PathBuf::from(program_files).join("WinGet").join("Packages"));
        }
        roots
    }

    /// Package directories WinGet has already created for this identifier.
    ///
    /// The directory name is `<PackageIdentifier>_<source hash>`, and the
    /// source part is empty for a local manifest, so the prefix is all that can
    /// be matched on.
    fn package_dirs() -> Vec<PathBuf> {
        let mut found = Vec::new();
        for root in portable_roots() {
            let Ok(entries) = fs_err::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(PACKAGE_ID) {
                    found.push(entry.path());
                }
            }
        }
        found
    }

    /// Why this scenario must not run here.
    ///
    /// Same discipline as the distro scenarios in `unix_package::precheck`:
    /// both refusals happen before the scenario is constructed, so the only
    /// package `cleanup` can ever uninstall is one this run installed. The
    /// database is consulted as well as the filesystem, because `winget
    /// uninstall` works by identifier and would remove a registered package
    /// whatever path it owns.
    fn occupied() -> Option<String> {
        if let Some(dir) = package_dirs().first() {
            return Some(format!(
                "{} already exists; refusing to install over a `{PACKAGE_ID}` this \
                 test did not create, because cleanup would then uninstall it",
                dir.display(),
            ));
        }
        let listed = scenario::run_quietly(
            winget()
                .arg("list")
                .arg("--id")
                .arg(PACKAGE_ID)
                .arg("--exact")
                .arg("--disable-interactivity")
                .arg("--accept-source-agreements"),
        );
        match listed {
            // Exit 0 means WinGet found it. A non-zero exit is
            // "no installed package found matching input criteria", which is
            // the answer this scenario needs.
            Some(out) if out.success => Some(format!(
                "`{PACKAGE_ID}` is already installed on this host; refusing to \
                 replace a package this test did not create, because cleanup \
                 would then uninstall it"
            )),
            Some(_) => None,
            None => Some("`winget list` could not be spawned".to_string()),
        }
    }

    pub struct WinGetScenario {
        root: tempfile::TempDir,
        /// Set immediately before `winget install` runs. Immediately *before*,
        /// because an install that fails part-way through still leaves a
        /// package directory and a registry entry to remove — which is only
        /// safe because `occupied` has already established that neither exists.
        attempted: AtomicBool,
    }

    impl WinGetScenario {
        fn new() -> anyhow::Result<WinGetScenario> {
            Ok(WinGetScenario {
                root: tempfile::Builder::new()
                    .prefix("gel-e2e-winget-")
                    .tempdir()?,
                attempted: AtomicBool::new(false),
            })
        }

        fn manifest_dir(&self) -> PathBuf {
            self.root.path().join("manifest")
        }

        /// Write the three YAML files WinGet reads as one manifest.
        fn write_manifest(&self, installer: &Path) -> anyhow::Result<PathBuf> {
            let url = url::Url::from_file_path(installer).map_err(|()| {
                anyhow::anyhow!("cannot build a file URL for {}", installer.display())
            })?;
            let digest = Sha256::digest(fs_err::read(installer)?);
            let sha256: String = digest.iter().map(|byte| format!("{byte:02X}")).collect();

            let dir = self.manifest_dir();
            fs_err::create_dir_all(&dir)?;

            fs_err::write(
                dir.join(format!("{PACKAGE_ID}.yaml")),
                format!(
                    "PackageIdentifier: {PACKAGE_ID}\n\
                     PackageVersion: {PACKAGE_VERSION}\n\
                     DefaultLocale: en-US\n\
                     ManifestType: version\n\
                     ManifestVersion: {MANIFEST_SCHEMA}\n"
                ),
            )?;
            fs_err::write(
                dir.join(format!("{PACKAGE_ID}.installer.yaml")),
                format!(
                    "PackageIdentifier: {PACKAGE_ID}\n\
                     PackageVersion: {PACKAGE_VERSION}\n\
                     InstallerType: portable\n\
                     Installers:\n\
                     \x20 - Architecture: {ARCHITECTURE}\n\
                     \x20   InstallerUrl: {url}\n\
                     \x20   InstallerSha256: {sha256}\n\
                     \x20   PortableCommandAlias: {COMMAND_ALIAS}\n\
                     ManifestType: installer\n\
                     ManifestVersion: {MANIFEST_SCHEMA}\n",
                    // Quoted, because the URL embeds a Windows path that may
                    // contain characters a YAML plain scalar would reinterpret.
                    url = yaml_quoted(url.as_str()),
                ),
            )?;
            fs_err::write(
                dir.join(format!("{PACKAGE_ID}.locale.en-US.yaml")),
                format!(
                    "PackageIdentifier: {PACKAGE_ID}\n\
                     PackageVersion: {PACKAGE_VERSION}\n\
                     PackageLocale: en-US\n\
                     Publisher: gel-cli install-manager e2e\n\
                     PackageName: gel CLI install-manager e2e fixture\n\
                     License: MIT\n\
                     ShortDescription: Built and installed by tests/install-manager. \
                     Not a real package.\n\
                     ManifestType: defaultLocale\n\
                     ManifestVersion: {MANIFEST_SCHEMA}\n"
                ),
            )?;
            Ok(dir)
        }
    }

    /// A YAML single-quoted scalar, which needs no escaping beyond doubling any
    /// apostrophe — and a Windows home directory can genuinely contain one.
    fn yaml_quoted(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    /// Where the portable install actually landed.
    ///
    /// Searched rather than predicted, for two reasons: the package directory's
    /// name ends in a source-dependent suffix, and the scope (and therefore the
    /// root) depends on whether WinGet was run elevated. Both candidate roots
    /// normalise to `winget/packages/`, so either answer satisfies the rule
    /// under test — but the assertion has to run against the file that exists.
    fn installed_bin() -> anyhow::Result<PathBuf> {
        let dirs = package_dirs();
        anyhow::ensure!(
            !dirs.is_empty(),
            "`winget install` succeeded but no `{PACKAGE_ID}*` directory appeared \
             under any of {:?}",
            portable_roots(),
        );
        for dir in &dirs {
            if let Some(exe) = first_exe(dir) {
                return Ok(exe);
            }
        }
        anyhow::bail!("no executable under any of {dirs:?}")
    }

    /// The first `*.exe` directly inside `dir`, in sorted order.
    ///
    /// A portable package holds exactly one file, but WinGet names it after the
    /// installer rather than after the command alias, so the name is not worth
    /// hardcoding.
    fn first_exe(dir: &Path) -> Option<PathBuf> {
        let mut names: Vec<PathBuf> = fs_err::read_dir(dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            })
            .collect();
        names.sort();
        names.into_iter().next()
    }

    impl Scenario for WinGetScenario {
        fn expected_slug(&self) -> &'static str {
            "winget"
        }

        fn install(&self, source: &Path) -> anyhow::Result<PathBuf> {
            // The installer WinGet "downloads" is the staged copy, left in the
            // tempdir; WinGet copies it into its own Packages directory, so the
            // tempdir going away afterwards takes nothing the install needs.
            let staged = scenario::stage_binary(source, &self.root.path().join("stage"))?;
            let manifest_dir = self.write_manifest(&staged)?;

            self.attempted.store(true, Ordering::SeqCst);
            scenario::checked(
                winget()
                    .arg("install")
                    .arg("--manifest")
                    .arg(&manifest_dir)
                    .arg("--accept-package-agreements")
                    .arg("--accept-source-agreements")
                    .arg("--disable-interactivity"),
                "`winget install --manifest`",
            )?;

            installed_bin()
        }

        fn cleanup(&self) {
            if !self.attempted.load(Ordering::SeqCst) {
                return;
            }
            scenario::report_cleanup(
                winget()
                    .arg("uninstall")
                    .arg("--id")
                    .arg(PACKAGE_ID)
                    .arg("--exact")
                    .arg("--disable-interactivity")
                    .arg("--accept-source-agreements"),
                "`winget uninstall`",
            );
            // WinGet removes the package directory itself; a leftover one means
            // the uninstall did not really happen, and saying so beats leaving a
            // stray copy of the CLI in the developer's LocalAppData unremarked.
            for dir in package_dirs() {
                eprintln!(
                    "cleanup: {} still exists after `winget uninstall`",
                    dir.display(),
                );
            }
        }
    }
}
