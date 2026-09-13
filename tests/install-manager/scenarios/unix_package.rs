//! What the three distro package managers (apt, dnf, pacman) have in common.
//!
//! They differ only in how a package is built and installed. Everything else —
//! where the package puts the binary, how the scenario gets permission to write
//! there, and the refusal to run at all on a machine that already has a
//! `/usr/bin/gel` — is identical, and getting any of it subtly different per
//! manager would mean three subtly different tests of the same rule.

use std::path::Path;
use std::process::Command;

use crate::scenario;

/// Where a distro package puts the CLI.
///
/// This is not an arbitrary choice: `is_system_bin`
/// (`src/cli/install_manager.rs`) only consults a package database for a binary
/// directly under `/usr/bin/` or `/bin/`, so a package that installed to, say,
/// `/usr/local/bin` would be reported as `direct` no matter which manager owns
/// it. These scenarios exist to prove the real rule, so they install where the
/// real distro packages do.
pub const SYSTEM_BIN: &str = "/usr/bin/gel";

/// The package name every distro scenario builds, and the argument its cleanup
/// passes to `dpkg -r` / `rpm -e` / `pacman -R`.
pub const PACKAGE: &str = "gel";

/// How a scenario runs a command that has to write outside its tempdir.
#[derive(Clone, Copy, Debug)]
pub enum Privilege {
    /// Already uid 0 — how the per-manager CI containers run.
    Root,
    /// Not root, but `sudo` will elevate without asking for a password.
    Sudo,
}

impl Privilege {
    /// What this host offers, or `None` when it offers neither.
    ///
    /// `sudo -n` rather than plain `sudo`: a password prompt inside a test run
    /// reads from a captured stdin that will never answer it, so the run would
    /// hang forever with no output rather than fail.
    fn current() -> Option<Privilege> {
        let uid = scenario::run_quietly(Command::new("id").arg("-u"))?;
        if !uid.success {
            return None;
        }
        if uid.stdout_trimmed() == "0" {
            return Some(Privilege::Root);
        }
        if !scenario::have("sudo") {
            return None;
        }
        let probe = scenario::run_quietly(Command::new("sudo").arg("-n").arg("true"))?;
        probe.success.then_some(Privilege::Sudo)
    }

    /// A command that will run `program` with permission to write `/usr/bin`.
    pub fn command(self, program: &str) -> Command {
        match self {
            Privilege::Root => Command::new(program),
            Privilege::Sudo => {
                let mut cmd = Command::new("sudo");
                cmd.arg("-n").arg(program);
                cmd
            }
        }
    }
}

/// How to ask a package database whether it already knows a `gel` package.
///
/// A filesystem check is not enough on its own. `cleanup` removes the package
/// *by name*, so a `gel` package that is already registered — even one owning
/// some path other than `/usr/bin/gel` — is at risk: `dpkg -i` and `pacman -U`
/// would replace it and cleanup would then uninstall it, and `rpm -i` would
/// fail while cleanup uninstalled it anyway. Uninstalling a package the
/// developer actually wanted is the worst thing this harness could do, so the
/// database is consulted before anything is built.
#[derive(Clone, Copy, Debug)]
pub enum PackageQuery {
    Dpkg,
    Rpm,
    Pacman,
}

impl PackageQuery {
    fn is_installed(self) -> bool {
        match self {
            // Exit status alone is not enough here: a package that was removed
            // but not purged still answers 0, with "deinstall ok config-files".
            // Such a package owns no files and `dpkg -r` would refuse it, so it
            // is not something this scenario can destroy — and treating it as
            // "installed" would make every run after the first one on the same
            // container skip.
            PackageQuery::Dpkg => {
                let queried = scenario::run_quietly(
                    Command::new("dpkg-query")
                        .arg("-W")
                        .arg("-f=${Status}")
                        .arg(PACKAGE),
                );
                queried.is_some_and(|out| {
                    out.success
                        && out.stdout_trimmed().split_whitespace().next_back() == Some("installed")
                })
            }
            PackageQuery::Rpm => answers("rpm", &["-q", PACKAGE]),
            PackageQuery::Pacman => answers("pacman", &["-Qi", PACKAGE]),
        }
    }
}

/// Whether a query tool exits 0, which for `rpm -q` and `pacman -Qi` means "yes,
/// that package is installed".
fn answers(tool: &str, args: &[&str]) -> bool {
    let mut cmd = Command::new(tool);
    cmd.args(args);
    scenario::run_quietly(&mut cmd).is_some_and(|out| out.success)
}

/// Decide whether this host can run a distro scenario at all.
///
/// `Err` is a skip reason, not a failure: a laptop without `dpkg` is not a
/// broken build. The last two checks are about safety rather than capability —
/// an existing `/usr/bin/gel`, or an existing `gel` package under any path at
/// all, means this scenario would install over something it did not create and
/// then destroy it in cleanup, so it refuses instead.
///
/// Both refusals happen here, before the scenario is constructed and so before
/// any `attempted` flag can be set. That is what lets each scenario's `cleanup`
/// remove the package unconditionally once `attempted` is true: the only `gel`
/// package that can exist at that point is the one this scenario installed.
pub fn precheck(tools: &[&str], query: PackageQuery) -> Result<Privilege, String> {
    for tool in tools {
        if !scenario::have(tool) {
            return Err(format!("{tool} is not installed on this host"));
        }
    }
    if Path::new(SYSTEM_BIN).exists() {
        return Err(format!(
            "{SYSTEM_BIN} already exists; refusing to package over an install \
             this test did not create"
        ));
    }
    if query.is_installed() {
        return Err(format!(
            "a `{PACKAGE}` package is already installed on this host; refusing to \
             replace a package this test did not create, because cleanup would \
             then uninstall it"
        ));
    }
    Privilege::current().ok_or_else(|| {
        "installing a system package needs uid 0 or a password-less sudo".to_string()
    })
}

/// Confirm the package manager really put the binary where the detection rules
/// expect, rather than trusting that a zero exit status means what we wanted.
pub fn installed_system_bin() -> anyhow::Result<std::path::PathBuf> {
    let installed = std::path::PathBuf::from(SYSTEM_BIN);
    anyhow::ensure!(
        installed.is_file(),
        "the package installed without error but {SYSTEM_BIN} is not a file",
    );
    Ok(installed)
}
