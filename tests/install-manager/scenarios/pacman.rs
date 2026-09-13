//! An Arch package install.
//!
//! The awkward part is that `makepkg` refuses to run as root, while everything
//! else about this scenario — writing `/usr/bin/gel`, `pacman -U` — requires
//! it, and the CI container is root. So the build step drops privileges with
//! `setpriv` and only the install step keeps them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::scenario::{self, Scenario};

use super::unix_package::{self, PACKAGE, PackageQuery, Privilege};

/// `arch=('any')` is a lie about a prebuilt ELF binary, but a harmless one:
/// unlike rpm, makepkg does not inspect the payload, and pacman only compares
/// the field against its own architecture, which `any` always satisfies. It
/// saves mapping the host architecture onto Arch's names for it.
///
/// `!strip` and `!debug` matter for the same reason as the rpm spec's disabled
/// `brp` pipeline: `assert_managed` hashes the installed file, so the package
/// must deliver the staged bytes unmodified.
const PKGBUILD: &str = "\
pkgname=gel
pkgver=0.0.0
pkgrel=1
pkgdesc='gel CLI install-manager e2e fixture'
arch=('any')
license=('Apache-2.0')
source=('gel')
sha256sums=('SKIP')
options=('!strip' '!debug')

package() {
  install -Dm755 \"$srcdir/gel\" \"$pkgdir/usr/bin/gel\"
}
";

/// The unprivileged account the build drops to when this process is root.
/// Present on every Arch install; `id` is asked rather than assuming 65534.
const BUILD_USER: &str = "nobody";

pub fn run() {
    let privilege = match unix_package::precheck(
        &["makepkg", "pacman", "fakeroot", "chmod"],
        PackageQuery::Pacman,
    ) {
        Ok(privilege) => privilege,
        Err(reason) => {
            eprintln!("skipping: {reason}");
            return;
        }
    };
    let build = match Builder::for_privilege(privilege) {
        Ok(build) => build,
        Err(reason) => {
            eprintln!("skipping: {reason}");
            return;
        }
    };
    let scenario = PacmanScenario::new(privilege, build).expect("prepare the pacman scenario");
    scenario::assert_managed(&scenario);
}

/// How to invoke `makepkg` so that it does not refuse to start.
#[derive(Debug)]
enum Builder {
    /// This process is not root, so `makepkg` is happy as-is.
    AsIs,
    /// This process is root. `makepkg` exits immediately for uid 0 (it would
    /// run `package()` under `fakeroot` with real root privileges), so the
    /// build runs as `nobody` instead.
    AsUser { uid: String, gid: String },
}

impl Builder {
    fn for_privilege(privilege: Privilege) -> Result<Builder, String> {
        match privilege {
            // `Privilege::Sudo` is only ever chosen for a non-root process.
            Privilege::Sudo => Ok(Builder::AsIs),
            Privilege::Root => {
                if !scenario::have("setpriv") {
                    return Err("running as root and `setpriv` is missing, so `makepkg` \
                         cannot be dropped to an unprivileged user"
                        .to_string());
                }
                let uid = id_of("-u")?;
                let gid = id_of("-g")?;
                Ok(Builder::AsUser { uid, gid })
            }
        }
    }

    fn command(&self) -> Command {
        match self {
            Builder::AsIs => Command::new("makepkg"),
            Builder::AsUser { uid, gid } => {
                let mut cmd = Command::new("setpriv");
                cmd.arg(format!("--reuid={uid}"))
                    .arg(format!("--regid={gid}"))
                    // Without this, `setpriv` keeps root's supplementary
                    // groups and refuses to start.
                    .arg("--clear-groups")
                    .arg("makepkg");
                cmd
            }
        }
    }

    /// Whether the build directory has to be readable and writable by someone
    /// other than this process.
    fn needs_open_permissions(&self) -> bool {
        matches!(self, Builder::AsUser { .. })
    }
}

fn id_of(flag: &str) -> Result<String, String> {
    let out = scenario::run_quietly(Command::new("id").arg(flag).arg(BUILD_USER))
        .ok_or_else(|| "could not run `id`".to_string())?;
    if !out.success {
        return Err(format!("no `{BUILD_USER}` account to build the package as"));
    }
    Ok(out.stdout_trimmed().to_string())
}

pub struct PacmanScenario {
    root: tempfile::TempDir,
    privilege: Privilege,
    build: Builder,
    /// See the same field on the apt scenario.
    attempted: AtomicBool,
}

impl PacmanScenario {
    fn new(privilege: Privilege, build: Builder) -> anyhow::Result<PacmanScenario> {
        Ok(PacmanScenario {
            root: tempfile::Builder::new()
                .prefix("gel-e2e-pacman-")
                .tempdir()?,
            privilege,
            build,
            attempted: AtomicBool::new(false),
        })
    }
}

/// The single built package in `dir`. makepkg's file name carries the
/// compression extension configured in `/etc/makepkg.conf`, so it is found by
/// its `.pkg.tar` infix rather than spelled out.
fn find_package(dir: &Path) -> anyhow::Result<PathBuf> {
    let mut found = Vec::new();
    for entry in fs_err::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if name.contains(".pkg.tar") && !name.ends_with(".sig") {
            found.push(path);
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => anyhow::bail!("makepkg produced no package in {}", dir.display()),
        n => anyhow::bail!(
            "makepkg produced {n} packages in {}: {found:?}",
            dir.display()
        ),
    }
}

impl Scenario for PacmanScenario {
    fn expected_slug(&self) -> &'static str {
        "pacman"
    }

    fn install(&self, source: &Path) -> anyhow::Result<PathBuf> {
        let build = self.root.path().join("build");
        scenario::stage_binary(source, &build)?;
        fs_err::write(build.join("PKGBUILD"), PKGBUILD)?;

        if self.build.needs_open_permissions() {
            // `tempdir()` creates a 0700 directory owned by root, which the
            // build user can neither enter nor write. Widening both levels is
            // safe here because the tree holds nothing secret and is removed
            // with the tempdir.
            scenario::checked(
                Command::new("chmod").arg("0755").arg(self.root.path()),
                "`chmod` on the scenario root",
            )?;
            scenario::checked(
                Command::new("chmod").arg("-R").arg("0777").arg(&build),
                "`chmod` on the build directory",
            )?;
        }

        scenario::checked(
            self.build
                .command()
                .arg("--force")
                // Nothing to resolve: the package has no dependencies and
                // `pacman -Sy` inside a test would rewrite the container's
                // package database.
                .arg("--nodeps")
                .current_dir(&build)
                // makepkg reads and writes `$HOME`; the build user's real home
                // may not exist or may not be writable by it.
                .env("HOME", &build)
                .env("PKGDEST", &build)
                .env("SRCDEST", &build)
                .env("BUILDDIR", &build),
            "`makepkg`",
        )?;
        let package = find_package(&build)?;

        self.attempted.store(true, Ordering::SeqCst);
        scenario::checked(
            self.privilege
                .command("pacman")
                .arg("-U")
                .arg("--noconfirm")
                .arg(&package),
            "`pacman -U`",
        )?;

        unix_package::installed_system_bin()
    }

    fn cleanup(&self) {
        if !self.attempted.load(Ordering::SeqCst) {
            return;
        }
        scenario::report_cleanup(
            self.privilege
                .command("pacman")
                .arg("-R")
                .arg("--noconfirm")
                .arg(PACKAGE),
            "`pacman -R gel`",
        );
    }
}
