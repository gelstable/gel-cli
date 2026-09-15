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
    ///
    /// A relocated or renamed `$env:SCOOP` is honoured here and is usable:
    /// `detect_from_path` reads the same variable, and `scenario::run` does not
    /// strip it from the child's environment, so the binary under test resolves
    /// the same root this does.
    struct Layout {
        root: PathBuf,
        /// Scoop's PowerShell entry point — see [`Layout::scoop`].
        script: PathBuf,
    }

    impl Layout {
        fn probe() -> anyhow::Result<Layout> {
            // Presence is still settled through `which`, which is the only
            // thing that answers "is Scoop on this machine at all". What is
            // *launched* is the PowerShell entry point below, not what `which`
            // returns.
            which::which("scoop").context("cannot resolve `scoop` on PATH")?;

            let root = match std::env::var_os("SCOOP").filter(|value| !value.is_empty()) {
                Some(value) => PathBuf::from(value),
                None => dirs::home_dir()
                    .context("cannot determine the home directory")?
                    .join("scoop"),
            };

            // Scoop's real entry point first, its `shims\scoop.ps1` second.
            // That order is deliberate and the reverse of what "what does PATH
            // resolve to" would suggest: the shim's body is
            //
            //     if ($MyInvocation.ExpectingInput) { $input | & $path @args }
            //     else { & $path @args }
            //
            // and the `$input` branch blocks forever on a stdin that never
            // reaches EOF. Verified locally against PowerShell 7: with stdin an
            // open pipe the shim hangs, while the direct script returns
            // immediately. `scenario::run` uses `Command::output()`, which
            // gives the child a null stdin, so the shim would in fact be safe
            // here — but a test that hangs produces no output at all, and one
            // fewer hop is one fewer thing between the argument and Scoop.
            let candidates = [
                root.join("apps")
                    .join("scoop")
                    .join("current")
                    .join("bin")
                    .join("scoop.ps1"),
                root.join("shims").join("scoop.ps1"),
            ];
            let script = candidates
                .iter()
                .find(|path| path.is_file())
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "scoop is on PATH but no scoop.ps1 was found at {} or {}; \
                         this scenario drives Scoop through PowerShell rather \
                         than its .cmd shim",
                        candidates[0].display(),
                        candidates[1].display(),
                    )
                })?;

            Ok(Layout { root, script })
        }

        /// A `scoop` command, driven through PowerShell rather than through
        /// Scoop's `.cmd` shim. Both halves of that are load-bearing — do not
        /// "simplify" either away.
        ///
        /// **Why not the bare name.** Scoop ships no `scoop.exe`: what sits on
        /// `PATH` is `<root>\shims\scoop.cmd` and `scoop.ps1`. Rust's
        /// `std::process::Command` resolves a bare program name by appending
        /// `.exe` and nothing else — it deliberately does not consult
        /// `PATHEXT` — so `Command::new("scoop")` finds nothing and fails to
        /// spawn. `which::which` *does* honour `PATHEXT`, which is why
        /// `scenario::have("scoop")` answers `true` for a host a bare spawn
        /// cannot launch on at all.
        ///
        /// **Why not the `.cmd` either.** Handing `Command` the resolved
        /// `scoop.cmd` does spawn — std recognises a `.bat`/`.cmd` target and
        /// runs it through `cmd.exe` — but the batch-specific argument
        /// escaping added in the CVE-2024-24576 fix is then re-parsed by the
        /// `cmd.exe` → `scoop.cmd` → PowerShell hop, and the argument does not
        /// survive it. CI showed exactly that, and the doubled quote is the
        /// fingerprint:
        ///
        /// ```text
        /// Couldn't find manifest for 'gel.json''.
        /// ```
        ///
        /// An absolute manifest path went in; Scoop saw a bare `gel.json` with
        /// a stray trailing quote. (Worse, `scoop install` still exited 0,
        /// which is why the "did a binary appear?" assertion in `install` has
        /// to stay — a zero exit from Scoop does not mean it did anything.)
        ///
        /// **What this does instead.** `powershell.exe` is a real `.exe`, so
        /// std applies its ordinary MSVCRT argument quoting and no batch layer
        /// re-parses it. `-File` is the mode that matters: unlike `-Command`,
        /// PowerShell does not re-interpret what follows as script text, it
        /// binds the remaining arguments to the script positionally, and
        /// Scoop's own shim forwards them with `@args` — array splatting, not
        /// string splicing. So a manifest path containing a space arrives
        /// intact.
        fn scoop(&self) -> Command {
            let mut cmd = Command::new("powershell.exe");
            cmd.arg("-NoProfile")
                .arg("-NonInteractive")
                // Belt and braces against the `$input` hang described on
                // `probe`: with no input format there is nothing for a
                // `$MyInvocation.ExpectingInput` branch to wait on, whichever
                // of the two entry points was resolved.
                .arg("-InputFormat")
                .arg("None")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(&self.script);
            cmd
        }

        /// `<root>\apps\gel`, the directory `scoop uninstall gel` removes.
        fn app_dir(&self) -> PathBuf {
            self.root.join("apps").join(APP)
        }

        /// What a user actually runs. `current` is a junction Scoop repoints at
        /// the installed version directory; both spellings sit under
        /// `<root>\apps`, so it does not matter which one `current_exe()`
        /// reports.
        fn installed_bin(&self) -> PathBuf {
            self.app_dir().join("current").join(scenario::EXE_NAME)
        }

        /// `<root>\shims\gel.exe`, the launcher Scoop puts on `PATH`.
        fn shim(&self) -> PathBuf {
            self.root.join("shims").join(scenario::EXE_NAME)
        }

        /// A mirror of `detect_from_path`'s Scoop rule
        /// (`src/cli/install_manager.rs`), in both its halves: the literal
        /// `scoop/apps/` segments, which catch a default or merely relocated
        /// root, and `<root>\apps`, which is what catches a renamed one.
        ///
        /// Kept faithful to the production rule rather than convenient to this
        /// test: if an install lands somewhere neither half matches, the honest
        /// answer is that detection would call it `direct`, and this scenario
        /// should say so rather than widen until it passes.
        fn detection_would_match(&self, installed: &Path) -> bool {
            let normalise = |path: &Path| path.to_string_lossy().replace('\\', "/").to_lowercase();
            let installed = normalise(installed);
            let root = normalise(&self.root);
            installed.contains("scoop/apps/")
                || installed.starts_with(&format!("{}/apps/", root.trim_end_matches('/')))
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
        /// path under that directory is one detection recognises.
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
            anyhow::ensure!(
                self.layout.detection_would_match(&installed),
                "`scoop install` put the binary at {}, which neither contains the \
                 two adjacent segments `scoop/apps/` nor sits under {}, so this \
                 install would be classified `direct`. Do not loosen the rule to \
                 accommodate it",
                installed.display(),
                self.layout.root.join("apps").display(),
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
