//! A Scoop install, on Windows.
//!
//! Scoop is the manager the path rules in `src/cli/install_manager.rs` were
//! written for in the first place: it installs into the *user's home
//! directory* (`~\scoop\apps\<app>\current\<app>.exe`), which the CLI's older
//! "is this under `$HOME`? then it is ours" reasoning happily claimed as a
//! self-managed install. Overwriting it in place would leave Scoop's manifest,
//! its `apps\gel\0.0.0` version directory and its `shims\gel.exe` shim all
//! describing a binary that is no longer there. `scoop_apps_under_home_are_
//! scoop_not_direct` pins that rule as a pure function; this scenario is the
//! only thing that shows a *real* `scoop install` produces a path the rule
//! matches.
//!
//! The packaging is a local manifest plus a local zip, because that is the
//! smallest thing Scoop will accept:
//!
//! * `scoop install <path>\gel.json` installs from a manifest on disk, with no
//!   bucket, no git and no network — Scoop derives the app name from the file
//!   name, which is why the manifest must be called `gel.json`.
//! * The manifest's `url` is a `file://` URL into this scenario's tempdir.
//!   Scoop's downloader is built on `[Net.WebRequest]::Create`, which returns a
//!   `FileWebRequest` for that scheme, so no HTTP server is involved.
//! * `url` has to name an archive for `bin` to resolve after extraction, so the
//!   staged binary is wrapped in a one-entry zip. Stored, not deflated: the
//!   file is thrown away seconds later and compressing a debug build of the CLI
//!   is pure latency. `hash` is the zip's SHA-256 — left in place rather than
//!   passing `--skip-hash-check`, so a truncated or half-written archive fails
//!   as a hash mismatch instead of as a mysterious extraction error.
//!
//! What it touches outside its tempdir is Scoop's own root: `apps\gel`,
//! `shims\gel*` and the download cache, all undone in `cleanup`. As with the
//! distro scenarios, the refusal to run at all when *any* of that already
//! exists happens before the scenario is constructed, so cleanup's
//! `scoop uninstall gel` can only ever remove an install this test made.

/// Windows-only, but the `#[test]` wrapper is not: `main.rs` must list the same
/// eight scenarios on every host so that CI's `--exact e2e_scoop` selection is
/// a property of the target rather than of the runner that compiled it.
#[cfg(not(windows))]
pub fn run() {
    eprintln!("skipping: the scoop scenario only runs on Windows");
}

#[cfg(windows)]
pub use imp::run;

#[cfg(windows)]
mod imp {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};

    use anyhow::Context;
    use sha2::{Digest, Sha256};

    use crate::scenario::{self, Scenario};

    /// Scoop's name for the app, which is also the manifest's file stem and the
    /// argument `scoop uninstall` gets in cleanup.
    const APP: &str = "gel";

    /// Any valid version string works — nothing here upgrades through Scoop —
    /// but it becomes a directory name (`apps\gel\0.0.0`), so it stays boring.
    const MANIFEST_VERSION: &str = "0.0.0";

    pub fn run() {
        if !scenario::have("scoop") {
            eprintln!("skipping: scoop is not installed on this host");
            return;
        }
        let layout = match Layout::probe() {
            Ok(layout) => layout,
            Err(reason) => {
                eprintln!("skipping: {reason:#}");
                return;
            }
        };
        if let Some(reason) = layout.occupied() {
            eprintln!("skipping: {reason}");
            return;
        }
        let scenario = ScoopScenario::new(layout).expect("prepare the scoop scenario");
        scenario::assert_managed(&scenario);
    }

    /// Where this machine's Scoop keeps things.
    ///
    /// Resolved the way Scoop itself resolves it — `$env:SCOOP` when set,
    /// otherwise `<home>\scoop` — rather than by asking `scoop prefix`, which
    /// needs an app that is installed and so cannot answer before the first
    /// install. `dirs::home_dir()` rather than `%USERPROFILE%` because on
    /// Windows `dirs` goes through the Known Folder API, which is the same
    /// thing the CLI under test resolves its own paths with.
    struct Layout {
        root: PathBuf,
    }

    impl Layout {
        fn probe() -> anyhow::Result<Layout> {
            if let Some(value) = std::env::var_os("SCOOP").filter(|value| !value.is_empty()) {
                return Ok(Layout {
                    root: PathBuf::from(value),
                });
            }
            let home = dirs::home_dir().context("cannot determine the home directory")?;
            Ok(Layout {
                root: home.join("scoop"),
            })
        }

        /// `<root>\apps\gel`, the directory `scoop uninstall gel` removes.
        fn app_dir(&self) -> PathBuf {
            self.root.join("apps").join(APP)
        }

        /// What a user actually runs. `current` is a junction Scoop repoints at
        /// the installed version directory; both spellings contain
        /// `scoop/apps/`, so it does not matter which one `current_exe()`
        /// reports.
        fn installed_bin(&self) -> PathBuf {
            self.app_dir().join("current").join(scenario::EXE_NAME)
        }

        /// `<root>\shims\gel.exe`, the launcher Scoop puts on `PATH`.
        fn shim(&self) -> PathBuf {
            self.root.join("shims").join(scenario::EXE_NAME)
        }

        /// Why this scenario must not run here: something is already installed
        /// as `gel` in this Scoop root, and installing over it would end with
        /// cleanup uninstalling an app the developer wanted.
        ///
        /// Checked before the scenario is constructed, so `attempted` can only
        /// ever be set for an install this test made.
        fn occupied(&self) -> Option<String> {
            for path in [self.app_dir(), self.shim()] {
                if path.exists() {
                    return Some(format!(
                        "{} already exists; refusing to install over a `{APP}` this \
                         test did not create, because cleanup would then uninstall it",
                        path.display(),
                    ));
                }
            }
            None
        }
    }

    pub struct ScoopScenario {
        root: tempfile::TempDir,
        layout: Layout,
        /// Set immediately before `scoop install` runs, so cleanup can tell "the
        /// zip never got built" from "Scoop was asked to install something".
        /// Immediately *before*, because an install that fails part-way through
        /// still leaves an `apps\gel` directory to remove.
        attempted: AtomicBool,
    }

    impl ScoopScenario {
        fn new(layout: Layout) -> anyhow::Result<ScoopScenario> {
            Ok(ScoopScenario {
                root: tempfile::Builder::new()
                    .prefix("gel-e2e-scoop-")
                    .tempdir()?,
                layout,
                attempted: AtomicBool::new(false),
            })
        }

        /// A one-entry zip for the manifest to point at, and its SHA-256.
        fn archive(&self, source: &Path) -> anyhow::Result<(PathBuf, String)> {
            let staged = scenario::stage_binary(source, &self.root.path().join("stage"))?;
            let path = self.root.path().join("gel.zip");

            let file = fs_err::File::create(&path)?;
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer
                .start_file(scenario::EXE_NAME, options)
                .context("starting the zip entry")?;
            writer
                .write_all(&fs_err::read(&staged)?)
                .context("writing the staged binary into the zip")?;
            writer.finish().context("finishing the zip")?;

            let digest = Sha256::digest(fs_err::read(&path)?);
            let sha256 = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            Ok((path, sha256))
        }
    }

    impl Scenario for ScoopScenario {
        fn expected_slug(&self) -> &'static str {
            "scoop"
        }

        fn install(&self, source: &Path) -> anyhow::Result<PathBuf> {
            let (archive, sha256) = self.archive(source)?;
            let url = url::Url::from_file_path(&archive).map_err(|()| {
                anyhow::anyhow!("cannot build a file URL for {}", archive.display())
            })?;

            // `serde_json` rather than a format string: the URL and the tempdir
            // path inside it are full of backslashes and drive letters, and a
            // hand-written JSON string literal would have to escape them
            // correctly on pain of an unreadable Scoop parse error.
            let manifest = serde_json::json!({
                "version": MANIFEST_VERSION,
                "url": url.as_str(),
                "hash": sha256,
                "bin": scenario::EXE_NAME,
            });
            // The file *stem* is the app name Scoop installs under, so this
            // must stay `gel.json` for `apps\gel` and cleanup to line up.
            let manifest_path = self.root.path().join(format!("{APP}.json"));
            fs_err::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

            self.attempted.store(true, Ordering::SeqCst);
            scenario::checked(
                Command::new("scoop").arg("install").arg(&manifest_path),
                "`scoop install`",
            )?;

            let installed = self.layout.installed_bin();
            anyhow::ensure!(
                installed.is_file(),
                "`scoop install` succeeded but {} is not a file",
                installed.display(),
            );
            Ok(installed)
        }

        fn cleanup(&self) {
            if !self.attempted.load(Ordering::SeqCst) {
                return;
            }
            scenario::report_cleanup(
                Command::new("scoop").arg("uninstall").arg(APP),
                "`scoop uninstall gel`",
            );
            // The cached download is keyed by app, version *and* URL, so a
            // leftover entry can never be reused by a later run — but it is a
            // copy of a CLI binary sitting in the developer's Scoop cache, and
            // leaving it there is rude rather than harmless.
            scenario::report_cleanup(
                Command::new("scoop").arg("cache").arg("rm").arg(APP),
                "`scoop cache rm gel`",
            );
        }
    }
}
