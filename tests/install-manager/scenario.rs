//! Shared machinery for every install-manager scenario.
//!
//! A scenario owns one question: "after installing the CLI *this* way, what does
//! the CLI think owns it, and does `cli upgrade` respect that?" Everything that
//! is identical between scenarios — locating the binary, spawning it with a
//! predictable environment, hashing the installed file, and the managed-install
//! assertion body — lives here.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

/// The `gel` binary a scenario installs and then exercises.
///
/// `GEL_E2E_BIN` wins when it is set. CI compiles this test target once on a
/// GitHub runner and then runs it inside per-manager containers, where the
/// compile-time `CARGO_BIN_EXE_gel` path does not exist; the workflow mounts the
/// binary somewhere else and points this variable at it. Locally the
/// compile-time path is right and nothing needs to be set.
pub fn binary_under_test() -> PathBuf {
    let (path, origin) = match std::env::var_os("GEL_E2E_BIN") {
        Some(value) => (PathBuf::from(value), "GEL_E2E_BIN"),
        None => (
            PathBuf::from(env!("CARGO_BIN_EXE_gel")),
            "CARGO_BIN_EXE_gel",
        ),
    };
    assert!(
        path.is_file(),
        "binary under test ({origin}) does not exist: {}\n\
         Set GEL_E2E_BIN to the `gel` executable this run should install.",
        path.display(),
    );
    path
}

/// `BRANDING_CLI_CMD_FILE` in `src/branding.rs`. Integration tests cannot reach
/// into the crate (gel-cli is binary-only), so the name is repeated here.
pub const EXE_NAME: &str = if cfg!(windows) { "gel.exe" } else { "gel" };

/// Copy the binary under test into `dir` and return the copy.
///
/// Scenarios must install from a copy, never from `binary_under_test()` itself:
/// `copy_to_installation_path` (`src/cli/install.rs`) *renames* the running
/// executable into the install directory whenever source and destination share
/// a filesystem, so installing straight from cargo's `target/debug/gel` moves
/// the build artifact away and every later `cargo test` has to relink it. A
/// staged copy is also the more faithful reproduction: the real install script
/// downloads the binary to a scratch directory and runs it from there.
pub fn stage_binary(source: &Path, dir: &Path) -> anyhow::Result<PathBuf> {
    fs_err::create_dir_all(dir)?;
    let staged = dir.join(EXE_NAME);
    fs_err::copy(source, &staged)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs_err::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(staged)
}

/// One finished `gel` invocation.
pub struct Run {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    /// Panic unless the process exited 0, quoting both streams.
    pub fn expect_success(&self, what: &str) -> &Run {
        assert!(
            self.success,
            "{what} failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout, self.stderr,
        );
        self
    }

    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }
}

/// Start a `gel` command with the environment scrubbed of things that would
/// quietly change which registry the CLI talks to or how loud it is.
///
/// * `RUST_LOG` — the libtest harness exports it empty, which `env_logger`
///   reads as "off" rather than as "apply the built-in default".
/// * `GEL_PKG_ROOT` / `EDGEDB_PKG_ROOT` — a set (even empty) package root wins
///   over `[registry].sources`, so a fixture registry would be ignored.
/// * `GEL_E2E_*` — scenario knobs that must not leak into the child.
pub fn gel(bin: &Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.env_remove("RUST_LOG")
        .env_remove("GEL_PKG_ROOT")
        .env_remove("EDGEDB_PKG_ROOT")
        .env_remove("GEL_E2E_BIN")
        .env_remove("GEL_E2E_ALLOW_GLOBAL_CONFIG");
    cmd
}

/// Run a prepared command to completion and capture both streams as text.
pub fn run(cmd: &mut Command) -> Run {
    let output = cmd
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn {:?}: {error}", cmd.get_program()));
    Run {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Whether `tool` can be found on `PATH`.
///
/// Every scenario in this target drives a package manager that may simply not
/// exist on the host, and the contract is that it then *skips* rather than
/// fails. `run` panics when a spawn fails, so availability has to be settled
/// before the first spawn, not discovered from its error.
pub fn have(tool: &str) -> bool {
    which::which(tool).is_ok()
}

/// Run an external tool and turn a non-zero exit into an error.
///
/// [`Run::expect_success`] panics, which is right for an assertion about the
/// CLI but wrong inside [`Scenario::install`]: a `dpkg-deb` that refuses to
/// build should surface as "install failed: ..." with both streams attached,
/// through the same `Result` as every other packaging step.
pub fn checked(cmd: &mut Command, what: &str) -> anyhow::Result<Run> {
    let out = run(cmd);
    anyhow::ensure!(
        out.success,
        "{what} failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.stdout,
        out.stderr,
    );
    Ok(out)
}

/// Spawn without panicking, for [`Scenario::cleanup`].
///
/// Cleanup runs from a `Drop` guard, including while a panic is unwinding, and
/// a second panic there would abort the process and take the original failure's
/// message with it. `None` means the tool could not be spawned at all.
pub fn run_quietly(cmd: &mut Command) -> Option<Run> {
    let output = cmd.output().ok()?;
    Some(Run {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Report a cleanup command that did not work, and never panic doing it.
pub fn report_cleanup(cmd: &mut Command, what: &str) {
    match run_quietly(cmd) {
        Some(out) if out.success => {}
        Some(out) => eprintln!(
            "cleanup: {what} failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr,
        ),
        None => eprintln!("cleanup: could not spawn {what}"),
    }
}

/// Ask a `gel` binary for one `info --get` value, with the scenario's env applied.
///
/// Querying the binary beats recomputing `dirs` logic in the test: the answer is
/// by definition the directory the CLI will actually use, including the parts
/// (`XDG_BIN_HOME` on Linux, `~/Library/Application Support` on macOS) that
/// differ per host.
pub fn info_get<I, K, V>(bin: &Path, key: &str, env: I) -> PathBuf
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut cmd = gel(bin);
    cmd.arg("--no-cli-update-check")
        .arg("info")
        .arg("--get")
        .arg(key);
    cmd.envs(env);
    let out = run(&mut cmd);
    out.expect_success(&format!("`gel info --get {key}`"));
    // `info --get` appends a trailing path separator to directory answers.
    let value = out.stdout_trimmed().trim_end_matches(['/', '\\']);
    assert!(!value.is_empty(), "`gel info --get {key}` printed nothing");
    PathBuf::from(value)
}

/// Lowercase hex Blake2b-512 of a file's contents.
///
/// Same digest the registry verifies downloads against
/// (`src/portable/registry/download.rs`), so one helper serves both the fixture
/// index this harness writes and the "did the bytes change?" assertion.
pub fn blake2b_hex(path: &Path) -> String {
    let bytes = fs_err::read(path).expect("read file to hash");
    blake2b_simd::blake2b(&bytes).to_hex().to_string()
}

/// Mirror of `portable::platform::get_cli()` for the host running this test.
///
/// Returns `None` on a host the CLI has no build for, so a scenario can skip
/// rather than write a fixture index nothing will ever match. Note Linux CLI
/// builds are MUSL regardless of the host libc — unlike the *server* platform
/// mirrored in `tests/registry_sources.rs`.
pub fn cli_platform() -> Option<&'static str> {
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        return None;
    };
    let os = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(target_os = "linux") {
        "unknown-linux-musl"
    } else if cfg!(windows) {
        "pc-windows-msvc"
    } else {
        return None;
    };
    Some(match (arch, os) {
        ("x86_64", "apple-darwin") => "x86_64-apple-darwin",
        ("aarch64", "apple-darwin") => "aarch64-apple-darwin",
        ("x86_64", "unknown-linux-musl") => "x86_64-unknown-linux-musl",
        ("aarch64", "unknown-linux-musl") => "aarch64-unknown-linux-musl",
        ("x86_64", "pc-windows-msvc") => "x86_64-pc-windows-msvc",
        ("aarch64", "pc-windows-msvc") => "aarch64-pc-windows-msvc",
        _ => return None,
    })
}

/// Mirror of `cli_media_type()` in `src/portable/registry/index.rs`.
///
/// `validate_cli` drops any installref whose media type does not match the
/// index's platform, so a fixture that hardcodes one value builds a catalog the
/// CLI silently reports as empty ("cannot find new version") on the other two
/// platforms.
pub fn cli_media_type(platform: &str) -> &'static str {
    if platform.ends_with("-apple-darwin") {
        "application/x-mach-binary"
    } else if platform.contains("-windows-") {
        "application/x-dosexec"
    } else {
        "application/x-pie-executable"
    }
}

/// One way of getting the CLI onto a machine, plus how to take it back off.
///
/// Concurrency: libtest runs test functions on parallel threads, and a scenario
/// is otherwise free to run alongside others. The one piece of shared state is
/// the CLI's global `cli.toml`, which a scenario must only touch through
/// [`ConfigFile`] — that type holds a process-wide lock for the whole
/// replace/restore window, so implementors get the serialisation for free and
/// must not hand-roll it.
pub trait Scenario {
    /// What `gel info --get install-manager` must print afterwards.
    fn expected_slug(&self) -> &'static str;

    /// Environment every `gel` invocation for this scenario must carry —
    /// redirected config/data/cache directories, manager-specific paths, and so
    /// on. The default is "nothing extra".
    ///
    /// This lives on the trait rather than on each implementor because the
    /// shared bodies below ([`assert_managed`]) spawn the CLI on a scenario's
    /// behalf, and would otherwise run it with the ambient environment, quietly
    /// reading and writing the developer's real directories.
    fn env(&self) -> Vec<(&'static str, PathBuf)> {
        Vec::new()
    }

    /// A `gel` command carrying this scenario's environment.
    ///
    /// Every spawn in a scenario or in a shared body goes through here; the bare
    /// [`gel`] function is the plumbing it is built from, not an entry point.
    fn command(&self, bin: &Path) -> Command {
        let mut cmd = gel(bin);
        cmd.envs(self.env());
        cmd
    }

    /// Package `source` for this manager, install it, and return the path of the
    /// *installed* executable — not `source`, which stays untouched.
    fn install(&self, source: &Path) -> anyhow::Result<PathBuf>;

    /// Undo everything `install` did. Must tolerate being called after a partial
    /// or failed install, and must never panic.
    fn cleanup(&self);
}

/// Serialises every scenario that has to write the CLI's global `cli.toml`.
///
/// Scenarios whose platform cannot redirect the config directory all write the
/// *same* file, and libtest would happily run two of them at once.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

fn lock_config() -> MutexGuard<'static, ()> {
    // A scenario that panicked mid-write poisons the lock, but the next one
    // still has to run — and `ConfigFile::replace` repairs whatever the dead run
    // left behind — so take the guard back instead of propagating the poison.
    CONFIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub const CONFIG_FILE: &str = "cli.toml";
/// Sits beside `cli.toml` while a scenario's fixture config is in place.
pub const CONFIG_BACKUP_FILE: &str = "cli.toml.e2e-bak";

/// Custody of one `cli.toml` for the duration of a scenario.
///
/// The displaced file is moved to a sibling on disk rather than held in memory:
/// a `Vec<u8>` in the test process is lost to Ctrl-C, SIGTERM, a libtest
/// timeout or `panic = "abort"`, and with it the developer's registry
/// configuration, leaving nothing to recover from. A file on disk survives all
/// of those, and the next run puts it back.
///
/// Holds [`CONFIG_LOCK`] for its whole lifetime, so two scenarios cannot have
/// custody of the same file at once.
pub struct ConfigFile {
    path: PathBuf,
    backup: PathBuf,
    /// Whether a `cli.toml` was there before us and has to come back.
    had_previous: bool,
    _lock: MutexGuard<'static, ()>,
}

impl ConfigFile {
    /// Whether this directory holds configuration belonging to whoever owns the
    /// machine — either a live `cli.toml` or a backup an interrupted run left.
    ///
    /// Callers use this to decide whether replacing the file needs an explicit
    /// opt-in.
    pub fn is_occupied(config_dir: &Path) -> bool {
        config_dir.join(CONFIG_FILE).exists() || config_dir.join(CONFIG_BACKUP_FILE).exists()
    }

    /// Put `body` in `<config_dir>/cli.toml`, preserving whatever was there.
    pub fn replace(config_dir: &Path, body: &str) -> anyhow::Result<ConfigFile> {
        let lock = lock_config();
        fs_err::create_dir_all(config_dir)?;
        let path = config_dir.join(CONFIG_FILE);
        let backup = config_dir.join(CONFIG_BACKUP_FILE);

        // A leftover backup means an earlier run died between replacing the file
        // and restoring it. The backup is the real config and whatever sits at
        // `cli.toml` is that run's fixture, so recover before doing anything
        // else — otherwise this run would "back up" the fixture and the real
        // config would be gone for good.
        if backup.exists() {
            fs_err::remove_file(&path).ok();
            fs_err::rename(&backup, &path)?;
        }

        let had_previous = path.exists();
        if had_previous {
            // Rename rather than read-then-write: the bytes are never in flight,
            // so there is no instant at which they exist only in this process.
            fs_err::rename(&path, &backup)?;
        }
        fs_err::write(&path, body)?;
        Ok(ConfigFile {
            path,
            backup,
            had_previous,
            _lock: lock,
        })
    }

    /// Undo [`ConfigFile::replace`], releasing the lock.
    pub fn restore(self) -> anyhow::Result<()> {
        fs_err::remove_file(&self.path).ok();
        if self.had_previous {
            fs_err::rename(&self.backup, &self.path)?;
        }
        Ok(())
    }
}

/// Runs `Scenario::cleanup` on the way out, including when an assertion panics.
///
/// A `Drop` guard rather than `catch_unwind`: `catch_unwind` would need the
/// scenario to be `UnwindSafe` (it owns a `TempDir` and, on Windows, a config
/// backup), and it swallows the panic location, so a failure would be reported
/// at the re-raise site instead of at the assertion that actually broke. `Drop`
/// keeps the original panic message and backtrace intact. This relies on
/// unwinding, which is the profile every test target in this repo uses.
pub struct CleanupGuard<'a> {
    scenario: &'a dyn Scenario,
}

impl<'a> CleanupGuard<'a> {
    pub fn new(scenario: &'a dyn Scenario) -> Self {
        CleanupGuard { scenario }
    }
}

impl Drop for CleanupGuard<'_> {
    fn drop(&mut self) {
        self.scenario.cleanup();
    }
}

/// The assertion body shared by every *managed* scenario (Homebrew, Scoop,
/// WinGet, Nix, apt, dnf, pacman).
///
/// The last step is the whole point of this target: `cli upgrade --force` must
/// leave the manager-owned file byte-for-byte unchanged. Overwriting it would
/// desynchronise the package manager's own record of the install, and a unit
/// test on the decision table cannot show that the bytes survived.
///
pub fn assert_managed(scenario: &dyn Scenario) {
    let source = binary_under_test();
    // Constructed before `install` so a failure mid-install still cleans up.
    let _guard = CleanupGuard::new(scenario);
    let installed = scenario
        .install(&source)
        .unwrap_or_else(|error| panic!("install failed: {error:#}"));

    run(scenario
        .command(&installed)
        .arg("--no-cli-update-check")
        .arg("--version"))
    .expect_success("installed `gel --version`");

    let detected = run(scenario
        .command(&installed)
        .arg("--no-cli-update-check")
        .arg("info")
        .arg("--get")
        .arg("install-manager"));
    detected.expect_success("`gel info --get install-manager`");
    assert_eq!(
        detected.stdout_trimmed(),
        scenario.expected_slug(),
        "wrong install manager detected\n--- stderr ---\n{}",
        detected.stderr,
    );

    let before = blake2b_hex(&installed);

    for extra in [None, Some("--force")] {
        let mut cmd = scenario.command(&installed);
        // No `--no-cli-update-check`: `cli upgrade` is exactly the path under
        // test, and `main.rs` skips the background version check for it anyway.
        cmd.arg("cli").arg("upgrade");
        if let Some(flag) = extra {
            cmd.arg(flag);
        }
        let label = match extra {
            Some(flag) => format!("`gel cli upgrade {flag}`"),
            None => "`gel cli upgrade`".to_string(),
        };
        let upgrade = run(&mut cmd);
        upgrade.expect_success(&label);
        // `msg!` is `eprintln!` (src/print/color.rs), so both the defer hint and
        // the "Upgraded to version" line land on stderr, never stdout.
        assert!(
            upgrade.stderr.contains("was installed via"),
            "{label} should defer to the package manager\n--- stderr ---\n{}",
            upgrade.stderr,
        );
        assert!(
            !upgrade.stderr.contains("Upgraded to version"),
            "{label} must not replace a managed install\n--- stderr ---\n{}",
            upgrade.stderr,
        );
    }

    assert_eq!(
        blake2b_hex(&installed),
        before,
        "`cli upgrade` rewrote the {} install at {}",
        scenario.expected_slug(),
        installed.display(),
    );
}

/// Hermetic coverage for the one piece of the harness that can destroy
/// something the developer cares about.
///
/// Not `#[ignore]`d and not a scenario: these install nothing and touch only a
/// tempdir, so they run under `cargo test --all-features` and guard the
/// backup/restore contract on every host — including the situations no scenario
/// reaches here (Windows, and a machine that already has a global `cli.toml`).
mod config_file_tests {
    use super::{CONFIG_BACKUP_FILE, CONFIG_FILE, ConfigFile};

    const FIXTURE: &str = "[registry]\nsources = [\"fixture\"]\n";
    const REAL: &str = "[registry]\nsources = [\"https://packages.example.com/registry.json\"]\n";

    fn read(dir: &std::path::Path, name: &str) -> String {
        fs_err::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn restores_the_file_it_displaced() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join(CONFIG_FILE), REAL).unwrap();

        let held = ConfigFile::replace(dir.path(), FIXTURE).unwrap();
        assert_eq!(read(dir.path(), CONFIG_FILE), FIXTURE);
        assert_eq!(
            read(dir.path(), CONFIG_BACKUP_FILE),
            REAL,
            "the displaced config must be on disk, not only in memory",
        );

        held.restore().unwrap();
        assert_eq!(read(dir.path(), CONFIG_FILE), REAL);
        assert!(!dir.path().join(CONFIG_BACKUP_FILE).exists());
    }

    #[test]
    fn removes_a_file_that_was_not_there_before() {
        let dir = tempfile::tempdir().unwrap();

        let held = ConfigFile::replace(dir.path(), FIXTURE).unwrap();
        assert_eq!(read(dir.path(), CONFIG_FILE), FIXTURE);

        held.restore().unwrap();
        assert!(!dir.path().join(CONFIG_FILE).exists());
        assert!(!dir.path().join(CONFIG_BACKUP_FILE).exists());
    }

    /// A run killed between replace and restore leaves the real config in the
    /// backup and a fixture at `cli.toml`. The next run must recover it instead
    /// of backing the fixture up over it.
    #[test]
    fn recovers_a_backup_an_interrupted_run_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join(CONFIG_BACKUP_FILE), REAL).unwrap();
        fs_err::write(dir.path().join(CONFIG_FILE), "stale fixture").unwrap();

        let held = ConfigFile::replace(dir.path(), FIXTURE).unwrap();
        assert_eq!(read(dir.path(), CONFIG_FILE), FIXTURE);
        assert_eq!(read(dir.path(), CONFIG_BACKUP_FILE), REAL);

        held.restore().unwrap();
        assert_eq!(read(dir.path(), CONFIG_FILE), REAL);
    }

    #[test]
    fn occupied_covers_both_a_live_config_and_a_stale_backup() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!ConfigFile::is_occupied(dir.path()));

        fs_err::write(dir.path().join(CONFIG_BACKUP_FILE), REAL).unwrap();
        assert!(ConfigFile::is_occupied(dir.path()));

        fs_err::remove_file(dir.path().join(CONFIG_BACKUP_FILE)).unwrap();
        fs_err::write(dir.path().join(CONFIG_FILE), REAL).unwrap();
        assert!(ConfigFile::is_occupied(dir.path()));
    }
}
