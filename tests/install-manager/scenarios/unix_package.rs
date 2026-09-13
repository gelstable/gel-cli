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

/// Decide whether this host can run a distro scenario at all.
///
/// `Err` is a skip reason, not a failure: a laptop without `dpkg` is not a
/// broken build. The `/usr/bin/gel` check is the one that is about safety
/// rather than capability — if something is already installed there, this
/// scenario would package over a real install and then remove it in cleanup,
/// so it refuses instead.
pub fn precheck(tools: &[&str]) -> Result<Privilege, String> {
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
