//! A Debian package install.
//!
//! `dpkg-deb --build` over a hand-built tree is the smallest thing that
//! produces a package `dpkg` will record in its database, which is what
//! detection actually keys on: `/usr/bin/gel` alone is `direct`, and only
//! `dpkg -S /usr/bin/gel` answering makes it `apt`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::scenario::{self, Scenario};

use super::unix_package::{self, PACKAGE, PackageQuery, Privilege};

/// The five fields `dpkg-deb` insists on. `Architecture: all` rather than the
/// host's: the package carries one prebuilt binary that is only ever installed
/// on the machine that built it, so declaring an architecture would add a
/// constraint with nothing to check and one more way for the scenario to fail
/// for reasons unrelated to install-manager detection.
const CONTROL: &str = "\
Package: gel
Version: 0.0.0
Architecture: all
Maintainer: gel-cli install-manager e2e <e2e@example.invalid>
Description: gel CLI install-manager e2e fixture
 Built and installed by tests/install-manager. Not a real package.
";

pub fn run() {
    let package = std::env::var_os("GEL_E2E_PACKAGE").map(PathBuf::from);
    let tools: &[&str] = if package.is_some() {
        &["dpkg", "dpkg-query"]
    } else {
        &["dpkg-deb", "dpkg", "dpkg-query"]
    };
    let privilege = match unix_package::precheck(tools, PackageQuery::Dpkg) {
        Ok(privilege) => privilege,
        Err(reason) => {
            eprintln!("skipping: {reason}");
            return;
        }
    };
    let scenario = AptScenario::new(privilege, package).expect("prepare the apt scenario");
    scenario::assert_managed(&scenario);
}

pub struct AptScenario {
    root: tempfile::TempDir,
    privilege: Privilege,
    /// When set by CI, install this exact release `.deb` instead of building a
    /// fixture package around the extracted candidate binary.
    package: Option<PathBuf>,
    /// Set immediately before `dpkg -i` runs, so cleanup can tell "the install
    /// never got that far" from "the install ran". Without it, every failure
    /// while *building* the package would also print a confusing `dpkg -r gel`
    /// error about a package that was never installed.
    ///
    /// Immediately *before* rather than after, because a `dpkg -i` that fails
    /// part-way through still leaves a database entry to remove. That is only
    /// safe because `precheck` has already established that no `gel` package
    /// exists on this host, so the entry cleanup removes can only be this
    /// scenario's.
    attempted: AtomicBool,
}

impl AptScenario {
    fn new(privilege: Privilege, package: Option<PathBuf>) -> anyhow::Result<AptScenario> {
        Ok(AptScenario {
            root: tempfile::Builder::new().prefix("gel-e2e-apt-").tempdir()?,
            privilege,
            package,
            attempted: AtomicBool::new(false),
        })
    }
}

impl Scenario for AptScenario {
    fn expected_slug(&self) -> &'static str {
        "apt"
    }

    fn install(&self, source: &Path) -> anyhow::Result<PathBuf> {
        if let Some(package) = &self.package {
            anyhow::ensure!(
                package.is_file(),
                "configured release package does not exist: {}",
                package.display(),
            );
            self.attempted.store(true, Ordering::SeqCst);
            scenario::checked(
                self.privilege.command("dpkg").arg("-i").arg(package),
                "`dpkg -i` release package",
            )?;
            let installed = unix_package::installed_system_bin()?;
            anyhow::ensure!(
                scenario::blake2b_hex(&installed) == scenario::blake2b_hex(source),
                "the release .deb installed bytes different from the candidate archive",
            );
            return Ok(installed);
        }

        let pkg = self.root.path().join("pkg");
        // `stage_binary` names the copy `gel` and marks it 0755, which is
        // exactly the file `dpkg-deb` has to find under `usr/bin`.
        scenario::stage_binary(source, &pkg.join("usr").join("bin"))?;
        let control_dir = pkg.join("DEBIAN");
        fs_err::create_dir_all(&control_dir)?;
        fs_err::write(control_dir.join("control"), CONTROL)?;

        let deb = self.root.path().join("gel.deb");
        scenario::checked(
            Command::new("dpkg-deb").arg("--build").arg(&pkg).arg(&deb),
            "`dpkg-deb --build`",
        )?;

        self.attempted.store(true, Ordering::SeqCst);
        scenario::checked(
            self.privilege.command("dpkg").arg("-i").arg(&deb),
            "`dpkg -i`",
        )?;

        unix_package::installed_system_bin()
    }

    fn cleanup(&self) {
        if !self.attempted.load(Ordering::SeqCst) {
            return;
        }
        scenario::report_cleanup(
            self.privilege.command("dpkg").arg("-r").arg(PACKAGE),
            "`dpkg -r gel`",
        );
    }
}
