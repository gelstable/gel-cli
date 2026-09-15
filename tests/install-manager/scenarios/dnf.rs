//! An RPM install.
//!
//! `dnf` is the slug but `rpm` is what detection asks (`rpm -qf` in
//! `SystemProbe`), and dnf installs nothing an `rpm -i` does not. So the
//! scenario builds a package with `rpmbuild` and installs it with `rpm`,
//! skipping the repository metadata a real `dnf install` would need.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::scenario::{self, Scenario};

use super::unix_package::{self, PACKAGE, PackageQuery, Privilege};

/// `%global debug_package %{nil}` and `%define __os_install_post %{nil}` between
/// them switch off the whole `brp-*` post-install pipeline. Those scripts strip
/// binaries and split out debuginfo, and neither is welcome here: the package
/// exists to carry *these exact bytes* to `/usr/bin/gel`, and `assert_managed`
/// then hashes them to prove `cli upgrade` left them alone.
///
/// `BuildArch` has to be the host's. `noarch` is what a fixture package would
/// normally want, but rpmbuild rejects a `noarch` package containing an ELF
/// executable outright ("arch dependent binaries in noarch package").
fn spec(arch: &str) -> String {
    format!(
        "\
%global debug_package %{{nil}}
%define __os_install_post %{{nil}}

Name:           {PACKAGE}
Version:        0.0.0
Release:        1
Summary:        gel CLI install-manager e2e fixture
License:        Apache-2.0
BuildArch:      {arch}
AutoReqProv:    no

%description
Built and installed by tests/install-manager. Not a real package.

%install
mkdir -p %{{buildroot}}/usr/bin
install -m 0755 %{{_sourcedir}}/gel %{{buildroot}}/usr/bin/gel

%files
/usr/bin/gel
"
    )
}

/// The host architecture under the name rpm knows it by.
///
/// `std::env::consts::ARCH` agrees with rpm for the two architectures the CLI
/// ships Linux builds for; anything else is a skip rather than a guess, because
/// a wrong `BuildArch` produces a package rpm refuses to install.
fn rpm_arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64"),
        "aarch64" => Some("aarch64"),
        _ => None,
    }
}

pub fn run() {
    let privilege = match unix_package::precheck(&["rpmbuild", "rpm"], PackageQuery::Rpm) {
        Ok(privilege) => privilege,
        Err(reason) => {
            eprintln!("skipping: {reason}");
            return;
        }
    };
    let Some(arch) = rpm_arch() else {
        eprintln!(
            "skipping: no rpm architecture name for {}",
            std::env::consts::ARCH,
        );
        return;
    };
    let scenario = DnfScenario::new(privilege, arch).expect("prepare the dnf scenario");
    scenario::assert_managed(&scenario);
}

pub struct DnfScenario {
    root: tempfile::TempDir,
    privilege: Privilege,
    arch: &'static str,
    /// See the same field on the apt scenario.
    attempted: AtomicBool,
}

impl DnfScenario {
    fn new(privilege: Privilege, arch: &'static str) -> anyhow::Result<DnfScenario> {
        Ok(DnfScenario {
            root: tempfile::Builder::new().prefix("gel-e2e-dnf-").tempdir()?,
            privilege,
            arch,
            attempted: AtomicBool::new(false),
        })
    }
}

/// The one `.rpm` under `dir`, found by walking rather than by reconstructing
/// the file name. rpmbuild's output name depends on macros (`%{?dist}` in
/// particular) that the host's `rpm` configuration can change underneath us.
fn find_rpm(dir: &Path) -> anyhow::Result<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs_err::read_dir(&current)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rpm") {
                found.push(path);
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => anyhow::bail!("rpmbuild produced no .rpm under {}", dir.display()),
        n => anyhow::bail!(
            "rpmbuild produced {n} .rpm files under {}: {found:?}",
            dir.display(),
        ),
    }
}

impl Scenario for DnfScenario {
    fn expected_slug(&self) -> &'static str {
        "dnf"
    }

    fn install(&self, source: &Path) -> anyhow::Result<PathBuf> {
        let top = self.root.path().join("rpmbuild");
        for sub in ["SOURCES", "SPECS", "BUILD", "BUILDROOT", "RPMS", "SRPMS"] {
            fs_err::create_dir_all(top.join(sub))?;
        }
        scenario::stage_binary(source, &top.join("SOURCES"))?;
        let spec_path = top.join("SPECS").join("gel.spec");
        fs_err::write(&spec_path, spec(self.arch))?;

        scenario::checked(
            Command::new("rpmbuild")
                .arg("-bb")
                // `_topdir` keeps every intermediate inside the tempdir instead
                // of in the invoking user's `~/rpmbuild`.
                .arg("--define")
                .arg(format!("_topdir {}", top.display()))
                .arg(&spec_path),
            "`rpmbuild -bb`",
        )?;
        let rpm = find_rpm(&top.join("RPMS"))?;

        self.attempted.store(true, Ordering::SeqCst);
        scenario::checked(
            self.privilege.command("rpm").arg("-i").arg(&rpm),
            "`rpm -i`",
        )?;

        unix_package::installed_system_bin()
    }

    fn cleanup(&self) {
        if !self.attempted.load(Ordering::SeqCst) {
            return;
        }
        scenario::report_cleanup(
            self.privilege.command("rpm").arg("-e").arg(PACKAGE),
            "`rpm -e gel`",
        );
    }
}
