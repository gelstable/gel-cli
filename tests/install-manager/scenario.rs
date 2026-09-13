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
pub trait Scenario {
    /// What `gel info --get install-manager` must print afterwards.
    fn expected_slug(&self) -> &'static str;

    /// Package `source` for this manager, install it, and return the path of the
    /// *installed* executable — not `source`, which stays untouched.
    fn install(&self, source: &Path) -> anyhow::Result<PathBuf>;

    /// Undo everything `install` did. Must tolerate being called after a partial
    /// or failed install, and must never panic.
    fn cleanup(&self);
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
/// Unused until the managed scenarios land (Tasks 3-4); kept here so those tasks
/// add a scenario file and nothing else.
#[allow(dead_code)]
pub fn assert_managed(scenario: &dyn Scenario) {
    let source = binary_under_test();
    // Constructed before `install` so a failure mid-install still cleans up.
    let _guard = CleanupGuard::new(scenario);
    let installed = scenario
        .install(&source)
        .unwrap_or_else(|error| panic!("install failed: {error:#}"));

    run(gel(&installed)
        .arg("--no-cli-update-check")
        .arg("--version"))
    .expect_success("installed `gel --version`");

    let detected = run(gel(&installed)
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
        let mut cmd = gel(&installed);
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
