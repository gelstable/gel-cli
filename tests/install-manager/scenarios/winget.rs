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
//! the community repository actually ship. `InstallerUrl` points at a loopback
//! HTTP server this scenario runs for the duration of the install (see
//! [`FileServer`]): WinGet downloads through WinINet, which refuses a `file://`
//! URL outright, so serving the staged binary over `127.0.0.1` is what lets a
//! purely local fixture work at all. `InstallerSha256` is the hash of those
//! same bytes and is checked, so a bad transfer fails as a hash mismatch rather
//! than as a broken install.
//!
//! Two host prerequisites, neither of which this scenario will arrange for
//! itself:
//!
//! * WinGet has to be present. It *is* on the `windows-2025` GitHub runner
//!   image, even though the image README does not list it — a first CI run
//!   settled that by installing through it for real. It is still absent from
//!   plenty of other Windows hosts (Server SKUs without the App Installer
//!   package), so `run` still checks and skips loudly.
//! * `winget install --manifest` is gated behind the `LocalManifestFiles`
//!   administrator policy. Running elevated is *not* enough — the first CI run
//!   failed on an elevated runner with "This feature needs to be enabled by
//!   administrators" — so the host needs
//!   `winget settings --enable LocalManifestFiles` once, as administrator.
//!   This scenario reads the policy through `winget settings export` and skips
//!   rather than flipping it: it is machine-wide state with no reliable way to
//!   read back a prior value and restore it, so a test must never change it on
//!   a real machine. The CI job does enable it, in a step whose comment says
//!   the only reason that is acceptable is that the runner is ephemeral.
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
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use sha2::{Digest, Sha256};
    use warp::Filter;

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
            // Opens with the literal `skipping:` because
            // `scripts/ci/run-install-scenario.sh` greps for exactly that to
            // decide a job tested nothing, and fails the job on a hit.
            eprintln!(
                "skipping: WINGET IS NOT USABLE ON THIS HOST, SO e2e_winget \
                 PROVED NOTHING.\n\
                 `winget` is not on PATH, or is a WindowsApps execution alias \
                 with no App Installer package behind it to run. Note this is \
                 *not* the expected state on a GitHub `windows-2025` runner: \
                 WinGet is present there (the image README simply does not list \
                 it), so seeing this in CI means something regressed, not that \
                 it was never available.\n\
                 Nothing about WinGet detection was exercised: treat a green \
                 result for this scenario as \"not run\", never as \"passed\"."
            );
            return;
        }
        if let Some(reason) = local_manifests_disabled() {
            eprintln!("skipping: {reason}");
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

    /// Why `winget install --manifest` would refuse before it is attempted.
    ///
    /// `winget settings export` prints the effective settings as JSON,
    /// including the `adminSettings` block, so the policy can be *read* rather
    /// than discovered from a failed install — which is the difference between
    /// a clean skip that names the fix and a panic a developer has to decode.
    ///
    /// `None` both when the policy is on and when the question cannot be
    /// answered at all (no export, unparseable JSON, a WinGet too old to have
    /// the key). An unreadable setting must not silence the scenario: if the
    /// install then fails, it fails with WinGet's own message, which names the
    /// same command this one does.
    ///
    /// Deliberately read-only. Enabling the policy is machine-wide state with
    /// no way to learn what it was before, so the harness will not do it to
    /// someone's machine; the CI job does it to an ephemeral runner instead.
    fn local_manifests_disabled() -> Option<String> {
        let out = scenario::run_quietly(winget().arg("settings").arg("export"))?;
        if !out.success {
            return None;
        }
        // WinGet may emit a UTF-8 BOM, which `serde_json` rejects.
        let body = out.stdout.trim_start_matches('\u{feff}').trim();
        let settings: serde_json::Value = serde_json::from_str(body).ok()?;
        let enabled = settings
            .get("adminSettings")?
            .get("LocalManifestFiles")?
            .as_bool()?;
        if enabled {
            return None;
        }
        Some(format!(
            "`winget install --manifest` is disabled on this host by the \
             LocalManifestFiles administrator policy, so {PACKAGE_ID} cannot be \
             installed from the local manifest this scenario builds. Run \
             `winget settings --enable LocalManifestFiles` in an elevated shell \
             to allow it — this harness will not change a machine-wide setting \
             on your behalf"
        ))
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

    /// A loopback HTTP server that serves exactly one file.
    ///
    /// WinGet's downloader is WinINet, which will not fetch a `file://`
    /// `InstallerUrl` — the first attempt at this scenario died with
    /// `InternetOpenUrl() failed. 0x8007007b : The filename, directory name, or
    /// volume label syntax is incorrect.` after parsing the manifest
    /// successfully. So the staged binary is served over HTTP instead, and the
    /// manifest points at `http://127.0.0.1:<port>/gel.exe`.
    ///
    /// Bound to `127.0.0.1` on an ephemeral port and serving one path: nothing
    /// off this machine can reach it, and nothing on it can fetch anything but
    /// the file this scenario staged. Plain HTTP on purpose — TLS here would
    /// mean a certificate WinGet has no reason to trust, which is a second
    /// problem rather than a safety improvement.
    ///
    /// The listener is bound *synchronously* by `bind_with_graceful_shutdown`,
    /// before the port is returned, so there is no window in which the URL
    /// exists but the socket does not and no sleep is needed before handing the
    /// URL to WinGet.
    struct FileServer {
        runtime: tokio::runtime::Runtime,
        shutdown: tokio::sync::oneshot::Sender<()>,
        port: u16,
    }

    impl FileServer {
        fn start(file: &Path) -> anyhow::Result<FileServer> {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()?;
            let (shutdown, shut_rx) = tokio::sync::oneshot::channel::<()>();
            let route = warp::path(scenario::EXE_NAME)
                .and(warp::path::end())
                .and(warp::fs::file(file.to_path_buf()));

            // `bind_with_graceful_shutdown` needs a reactor in scope to create
            // its listener, and it creates it before returning the address.
            let guard = runtime.enter();
            let (addr, server) =
                warp::serve(route).bind_with_graceful_shutdown(([127, 0, 0, 1], 0), async move {
                    shut_rx.await.ok();
                });
            drop(guard);
            runtime.spawn(server);

            Ok(FileServer {
                runtime,
                shutdown,
                port: addr.port(),
            })
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}/{}", self.port, scenario::EXE_NAME)
        }

        /// Signal the graceful shutdown and wait briefly for the runtime.
        ///
        /// Bounded rather than unbounded: this runs from a `Drop` guard that
        /// may be unwinding a panic, and a cleanup that blocks forever would
        /// swallow the failure it is cleaning up after.
        fn stop(self) {
            self.shutdown.send(()).ok();
            self.runtime.shutdown_timeout(Duration::from_secs(5));
        }
    }

    pub struct WinGetScenario {
        root: tempfile::TempDir,
        /// Set immediately before `winget install` runs. Immediately *before*,
        /// because an install that fails part-way through still leaves a
        /// package directory and a registry entry to remove — which is only
        /// safe because `occupied` has already established that neither exists.
        attempted: AtomicBool,
        /// The fixture download server, alive from `install` until `cleanup`.
        /// Interior mutability because `Scenario::cleanup` takes `&self`.
        server: Mutex<Option<FileServer>>,
    }

    impl WinGetScenario {
        fn new() -> anyhow::Result<WinGetScenario> {
            Ok(WinGetScenario {
                root: tempfile::Builder::new()
                    .prefix("gel-e2e-winget-")
                    .tempdir()?,
                attempted: AtomicBool::new(false),
                server: Mutex::new(None),
            })
        }

        fn manifest_dir(&self) -> PathBuf {
            self.root.path().join("manifest")
        }

        /// Write the three YAML files WinGet reads as one manifest.
        fn write_manifest(&self, installer: &Path, url: &str) -> anyhow::Result<PathBuf> {
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
                    // Quoted even though a loopback URL is tame: a bare
                    // `http://...` is a valid YAML plain scalar only by luck of
                    // the `:` being followed by `/`, and quoting says so.
                    url = yaml_quoted(url),
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
            // The installer WinGet downloads is the staged copy, served over
            // loopback HTTP from the tempdir; WinGet copies it into its own
            // Packages directory, so the tempdir going away afterwards takes
            // nothing the install needs.
            let staged = scenario::stage_binary(source, &self.root.path().join("stage"))?;
            let server = FileServer::start(&staged)?;
            let url = server.url();
            // Parked before the install rather than after, so a failed install
            // still leaves the server for `cleanup` to shut down.
            *self.server.lock().unwrap_or_else(PoisonError::into_inner) = Some(server);
            let manifest_dir = self.write_manifest(&staged, &url)?;

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
            // Before the `attempted` gate: the server is started during
            // `install` and may well outlive a failure that happened before
            // `winget install` was ever reached.
            if let Some(server) = self
                .server
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                server.stop();
            }
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
