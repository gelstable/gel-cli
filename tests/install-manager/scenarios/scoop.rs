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
        if let Some(reason) = layout.undetectable() {
            eprintln!("skipping: {reason}");
            return;
        }
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
    ///
    /// A relocated `$env:SCOOP` is honoured here but not necessarily *usable* —
    /// see [`Layout::undetectable`], which refuses a root the production
    /// detection rule cannot recognise.
    struct Layout {
        root: PathBuf,
        /// The resolved `scoop` entry point — see [`Layout::scoop`].
        exe: PathBuf,
    }

    impl Layout {
        fn probe() -> anyhow::Result<Layout> {
            // Resolved once, here, rather than at each call site: `which` walks
            // PATH and PATHEXT, and `cleanup` has nowhere to report a failure.
            let exe = which::which("scoop").context("cannot resolve `scoop` on PATH")?;
            if let Some(value) = std::env::var_os("SCOOP").filter(|value| !value.is_empty()) {
                return Ok(Layout {
                    root: PathBuf::from(value),
                    exe,
                });
            }
            let home = dirs::home_dir().context("cannot determine the home directory")?;
            Ok(Layout {
                root: home.join("scoop"),
                exe,
            })
        }

        /// A `scoop` command, spawned through the *resolved* shim path.
        ///
        /// Spawning the bare name `scoop` does not work on Windows and this
        /// indirection is the fix — do not "simplify" it away.
        ///
        /// Scoop ships no `scoop.exe`: what sits on `PATH` is
        /// `<root>\shims\scoop.cmd` (plus a `.ps1`). Rust's
        /// `std::process::Command` resolves a bare program name by appending
        /// `.exe` and nothing else — it deliberately does not consult
        /// `PATHEXT` — so the bare name finds nothing and fails to spawn,
        /// which `scenario::run` turns into a panic. `which::which` *does*
        /// honour `PATHEXT`, which is why `scenario::have("scoop")` answers
        /// `true` for a host a bare spawn cannot launch on at all.
        ///
        /// Handing `Command` the resolved `.cmd` path is what closes the gap:
        /// std recognises a `.bat`/`.cmd` target and runs it through
        /// `cmd.exe`, applying the batch-specific argument quoting added in
        /// the CVE-2024-24576 fix. That is also why this beats writing
        /// `cmd /C scoop ...` by hand — the quoting of the manifest path would
        /// otherwise be ours to get right.
        fn scoop(&self) -> Command {
            Command::new(&self.exe)
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

        /// The executable `scoop install` actually produced, if any.
        ///
        /// Originally this scenario asserted the one path a user runs,
        /// `<root>\apps\gel\current\gel.exe`. CI showed `scoop install`
        /// exiting 0 without producing it, and a hard-coded path cannot say
        /// what it *did* produce — so the executable is now located rather
        /// than assumed. `current` is still preferred when it is there,
        /// because that is the path a user's `PATH` resolves to and the one
        /// the junction branch of detection is about; any other copy under
        /// `apps\gel` is a good enough answer for the contract, since every
        /// path under that directory contains `scoop/apps/`.
        ///
        /// The walk is depth-limited: `current` is a junction back into a
        /// sibling version directory, so an unbounded walk would revisit the
        /// same files, and a future Scoop layout could in principle make that
        /// a cycle.
        fn locate_installed(&self) -> Option<PathBuf> {
            let canonical = self.installed_bin();
            if canonical.is_file() {
                return Some(canonical);
            }
            let mut found = Vec::new();
            collect_exes(&self.app_dir(), 4, &mut found);
            found.sort();
            found.into_iter().next()
        }

        /// Everything a human needs to work out where the payload went.
        ///
        /// Built only on the failure path, and deliberately verbose: this runs
        /// on a CI runner that is destroyed minutes later, so whatever is not
        /// in the log is gone. Scoop's own output is included because the
        /// scenario captures it — throwing it away was what made the first CI
        /// failure impossible to diagnose.
        fn diagnose_missing_install(&self, out: &scenario::Run) -> String {
            let mut report = format!(
                "`scoop install` exited 0 but produced no {} anywhere under {}.\n\
                 --- scoop stdout ---\n{}\n--- scoop stderr ---\n{}\n",
                scenario::EXE_NAME,
                self.app_dir().display(),
                out.stdout,
                out.stderr,
            );
            for (label, dir, depth) in [
                ("apps/gel (recursive)", self.app_dir(), 6),
                ("apps", self.root.join("apps"), 1),
                ("shims", self.root.join("shims"), 1),
                ("cache", self.root.join("cache"), 1),
            ] {
                report.push_str(&format!("--- {label}: {} ---\n", dir.display()));
                list_tree(&dir, depth, &mut report);
            }
            report
        }

        /// Why this scenario cannot prove anything here: the detection rule
        /// would not recognise an install into *this* Scoop root.
        ///
        /// The rule is `path.contains("scoop/apps/")`
        /// (`detect_from_path`, `src/cli/install_manager.rs`), and those are two
        /// adjacent literal segments — so the parent of `apps` has to be named
        /// `scoop`. That holds for the default root and for any relocated root
        /// whose last component is still `scoop` (`D:\scoop`, `C:\opt\scoop`),
        /// but not for one that is renamed: with `$env:SCOOP=D:\tools`, Scoop
        /// installs to `D:\tools\apps\gel\current\gel.exe`, which normalises to
        /// `d:/tools/apps/gel/current/gel.exe` and matches nothing, so
        /// `detect()` returns `Direct`. The canonicalisation fallback in
        /// `detect()` does not rescue it either: resolving the `current`
        /// junction only yields `d:/tools/apps/gel/0.0.0/gel.exe`.
        ///
        /// That is a real, pre-existing gap in the production heuristic — a
        /// `cli upgrade` on such an install would overwrite Scoop's app
        /// directory — but it is a gap in `detect_from_path`, not in this test,
        /// and loosening the rule to accommodate the test would be exactly
        /// backwards. So the scenario refuses to run rather than failing red,
        /// and says why.
        fn undetectable(&self) -> Option<String> {
            let named_scoop = self
                .root
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("scoop"));
            if named_scoop {
                return None;
            }
            Some(format!(
                "this host's Scoop root is {}, whose last path component is not \
                 `scoop`; `detect_from_path` matches the two adjacent segments \
                 `scoop/apps/`, so an install under this root would be reported \
                 as `direct` and the scenario would fail rather than prove \
                 anything. That is a gap in the production heuristic, not in \
                 this test — do not loosen the rule to make this pass",
                self.root.display(),
            ))
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

    /// Collect every `gel.exe` under `dir`, to a bounded depth.
    fn collect_exes(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = fs_err::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_exes(&path, depth - 1, found);
            } else if path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case(scenario::EXE_NAME))
            {
                found.push(path);
            }
        }
    }

    /// Append a `find`-style listing of `dir` to `report`, to a bounded depth.
    fn list_tree(dir: &Path, depth: usize, report: &mut String) {
        let entries = match fs_err::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                report.push_str(&format!("  <cannot read {}: {error}>\n", dir.display()));
                return;
            }
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            let is_dir = path.is_dir();
            let size = fs_err::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            report.push_str(&format!(
                "  {} {size:>12}  {}\n",
                if is_dir { "d" } else { "f" },
                path.display(),
            ));
            if is_dir && depth > 1 {
                list_tree(&path, depth - 1, report);
            }
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
            let out = scenario::checked(
                self.layout.scoop().arg("install").arg(&manifest_path),
                "`scoop install`",
            )?;

            let installed = self
                .layout
                .locate_installed()
                .ok_or_else(|| anyhow::anyhow!("{}", self.layout.diagnose_missing_install(&out)))?;

            // A mirror of `detect_from_path`'s Scoop rule
            // (`src/cli/install_manager.rs`), asserted here so that a payload
            // landing somewhere unexpected fails with *that* sentence rather
            // than as a puzzling `expected "scoop", got "direct"` later on.
            // `assert_managed` still makes the real assertion by asking the
            // installed binary what owns it; this only sharpens the message.
            let normalised = installed
                .to_string_lossy()
                .replace('\\', "/")
                .to_lowercase();
            anyhow::ensure!(
                normalised.contains("scoop/apps/"),
                "`scoop install` put the binary at {}, which does not contain the \
                 two adjacent segments `scoop/apps/` that `detect_from_path` \
                 matches, so this install would be classified `direct`. Do not \
                 loosen the rule to accommodate it",
                installed.display(),
            );
            Ok(installed)
        }

        fn cleanup(&self) {
            if !self.attempted.load(Ordering::SeqCst) {
                return;
            }
            scenario::report_cleanup(
                self.layout.scoop().arg("uninstall").arg(APP),
                "`scoop uninstall gel`",
            );
            // The cached download is keyed by app, version *and* URL, so a
            // leftover entry can never be reused by a later run — but it is a
            // copy of a CLI binary sitting in the developer's Scoop cache, and
            // leaving it there is rude rather than harmless.
            scenario::report_cleanup(
                self.layout.scoop().arg("cache").arg("rm").arg(APP),
                "`scoop cache rm gel`",
            );
        }
    }
}
