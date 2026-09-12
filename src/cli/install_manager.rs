//! Detect whether this executable is owned by a system package manager.
//!
//! A managed install must never be overwritten in place: replacing a Homebrew
//! cellar file or a Scoop app directory corrupts the manager's own state. The
//! path rules are a pure function and the three Linux package-database lookups
//! go through a trait, so the whole decision table is testable on any host.

use std::path::Path;
use std::process::Command;

use crate::branding::BRANDING_CLI_CMD;
use crate::platform::current_exe;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallManager {
    Homebrew,
    Scoop,
    WinGet,
    Nix,
    Apt,
    Dnf,
    Pacman,
    Direct,
}

impl InstallManager {
    /// True only when this binary owns itself and may be replaced in place.
    pub fn is_self_managed(self) -> bool {
        matches!(self, InstallManager::Direct)
    }

    /// The native upgrade instruction for a managed install.
    pub fn upgrade_hint(self) -> Option<String> {
        let cmd = BRANDING_CLI_CMD;
        Some(match self {
            InstallManager::Homebrew => format!(
                "{cmd} was installed via Homebrew. \
                 Please run `brew upgrade gel` to update."
            ),
            InstallManager::Scoop => format!(
                "{cmd} was installed via Scoop. \
                 Please run `scoop update gel` to update."
            ),
            InstallManager::WinGet => format!(
                "{cmd} was installed via WinGet. \
                 Please run `winget upgrade Gelstable.Gel` to update."
            ),
            InstallManager::Nix => format!(
                "{cmd} was installed via Nix. \
                 Please run `nix profile upgrade gel-cli` to update."
            ),
            InstallManager::Apt => format!(
                "{cmd} was installed via apt. \
                 Please update using your system package manager."
            ),
            InstallManager::Dnf => format!(
                "{cmd} was installed via dnf. \
                 Please update using your system package manager."
            ),
            InstallManager::Pacman => format!(
                "{cmd} was installed via pacman. \
                 Please update using your system package manager."
            ),
            InstallManager::Direct => return None,
        })
    }
}

/// Queries the host's package databases.
///
/// Split out from the path rules so detection can be tested without a dpkg,
/// rpm, or pacman on the machine running the tests.
pub trait OwnershipProbe {
    fn dpkg_owns(&self, exe: &Path) -> bool;
    fn rpm_owns(&self, exe: &Path) -> bool;
    fn pacman_owns(&self, exe: &Path) -> bool;
}

pub struct SystemProbe;

impl SystemProbe {
    fn owns(tool: &str, args: &[&str], exe: &Path) -> bool {
        if which::which(tool).is_err() {
            return false;
        }
        Command::new(tool)
            .args(args)
            .arg(exe)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}

impl OwnershipProbe for SystemProbe {
    fn dpkg_owns(&self, exe: &Path) -> bool {
        Self::owns("dpkg", &["-S"], exe)
    }
    fn rpm_owns(&self, exe: &Path) -> bool {
        Self::owns("rpm", &["-qf"], exe)
    }
    fn pacman_owns(&self, exe: &Path) -> bool {
        Self::owns("pacman", &["-Qo"], exe)
    }
}

fn normalized(exe: &Path) -> String {
    exe.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn is_system_bin(exe: &Path) -> bool {
    let path = exe.to_string_lossy().replace('\\', "/");
    path.starts_with("/usr/bin") || path.starts_with("/bin")
}

pub fn detect_from_path(exe: &Path, probe: &dyn OwnershipProbe) -> InstallManager {
    let path = normalized(exe);

    if path.contains("/nix/store/") {
        return InstallManager::Nix;
    }
    if path.contains("scoop/apps/") {
        return InstallManager::Scoop;
    }
    if path.contains("winget/packages/") {
        return InstallManager::WinGet;
    }
    if path.contains("/opt/homebrew/")
        || path.contains("/usr/local/cellar/")
        || path.contains("/home/linuxbrew/.linuxbrew/")
    {
        return InstallManager::Homebrew;
    }
    if is_system_bin(exe) {
        if probe.dpkg_owns(exe) {
            return InstallManager::Apt;
        }
        if probe.rpm_owns(exe) {
            return InstallManager::Dnf;
        }
        if probe.pacman_owns(exe) {
            return InstallManager::Pacman;
        }
    }
    InstallManager::Direct
}

pub fn detect() -> InstallManager {
    match current_exe() {
        Ok(exe) => detect_from_path(&exe, &SystemProbe),
        Err(error) => {
            log::debug!("cannot determine the running executable: {error:#}");
            InstallManager::Direct
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{InstallManager, OwnershipProbe, detect_from_path};

    #[derive(Default)]
    struct StubProbe {
        dpkg: bool,
        rpm: bool,
        pacman: bool,
    }

    impl OwnershipProbe for StubProbe {
        fn dpkg_owns(&self, _exe: &Path) -> bool {
            self.dpkg
        }
        fn rpm_owns(&self, _exe: &Path) -> bool {
            self.rpm
        }
        fn pacman_owns(&self, _exe: &Path) -> bool {
            self.pacman
        }
    }

    fn detect(path: &str) -> InstallManager {
        detect_from_path(Path::new(path), &StubProbe::default())
    }

    #[test]
    fn nix_store_paths_are_nix() {
        assert_eq!(
            detect("/nix/store/abc123-gel-cli-7.11.0/bin/gel"),
            InstallManager::Nix
        );
    }

    #[test]
    fn homebrew_prefixes_are_homebrew() {
        assert_eq!(detect("/opt/homebrew/bin/gel"), InstallManager::Homebrew);
        assert_eq!(
            detect("/usr/local/Cellar/gel/7.11.0/bin/gel"),
            InstallManager::Homebrew
        );
        assert_eq!(
            detect("/home/linuxbrew/.linuxbrew/bin/gel"),
            InstallManager::Homebrew
        );
    }

    /// The bug this module exists to fix: a Scoop install lives under the
    /// user's home directory, so the old home-prefix check would have let
    /// `cli upgrade` overwrite Scoop's app directory and break its shims.
    #[test]
    fn scoop_apps_under_home_are_scoop_not_direct() {
        assert_eq!(
            detect(r"C:\Users\alice\scoop\apps\gel\current\gel.exe"),
            InstallManager::Scoop
        );
        assert_eq!(
            detect("/c/Users/alice/scoop/apps/gel/current/gel.exe"),
            InstallManager::Scoop
        );
    }

    #[test]
    fn winget_package_paths_are_winget() {
        assert_eq!(
            detect(
                r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Packages\Gelstable.Gel_abc\gel.exe"
            ),
            InstallManager::WinGet
        );
    }

    #[test]
    fn system_bin_without_an_owning_package_is_direct() {
        assert_eq!(detect("/usr/bin/gel"), InstallManager::Direct);
    }

    #[test]
    fn system_bin_owned_by_dpkg_is_apt() {
        let probe = StubProbe {
            dpkg: true,
            ..StubProbe::default()
        };
        assert_eq!(
            detect_from_path(Path::new("/usr/bin/gel"), &probe),
            InstallManager::Apt
        );
    }

    #[test]
    fn system_bin_owned_by_rpm_is_dnf() {
        let probe = StubProbe {
            rpm: true,
            ..StubProbe::default()
        };
        assert_eq!(
            detect_from_path(Path::new("/bin/gel"), &probe),
            InstallManager::Dnf
        );
    }

    #[test]
    fn system_bin_owned_by_pacman_is_pacman() {
        let probe = StubProbe {
            pacman: true,
            ..StubProbe::default()
        };
        assert_eq!(
            detect_from_path(Path::new("/usr/bin/gel"), &probe),
            InstallManager::Pacman
        );
    }

    #[test]
    fn dpkg_wins_over_rpm_and_pacman() {
        let probe = StubProbe {
            dpkg: true,
            rpm: true,
            pacman: true,
        };
        assert_eq!(
            detect_from_path(Path::new("/usr/bin/gel"), &probe),
            InstallManager::Apt
        );
    }

    /// A package database is only consulted for a system bin path, so an
    /// ordinary self-managed install never spawns dpkg, rpm, or pacman.
    #[test]
    fn package_databases_are_not_consulted_outside_system_bin() {
        let probe = StubProbe {
            dpkg: true,
            rpm: true,
            pacman: true,
        };
        assert_eq!(
            detect_from_path(Path::new("/home/alice/.local/bin/gel"), &probe),
            InstallManager::Direct
        );
    }

    #[test]
    fn self_managed_only_for_direct() {
        assert!(InstallManager::Direct.is_self_managed());
        for manager in [
            InstallManager::Homebrew,
            InstallManager::Scoop,
            InstallManager::WinGet,
            InstallManager::Nix,
            InstallManager::Apt,
            InstallManager::Dnf,
            InstallManager::Pacman,
        ] {
            assert!(!manager.is_self_managed(), "{manager:?}");
        }
    }

    #[test]
    fn every_managed_variant_has_a_hint_and_direct_has_none() {
        assert_eq!(InstallManager::Direct.upgrade_hint(), None);
        for manager in [
            InstallManager::Homebrew,
            InstallManager::Scoop,
            InstallManager::WinGet,
            InstallManager::Nix,
            InstallManager::Apt,
            InstallManager::Dnf,
            InstallManager::Pacman,
        ] {
            let hint = manager.upgrade_hint().expect("managed variants have hints");
            assert!(!hint.is_empty(), "{manager:?}");
        }
    }

    #[test]
    fn hints_name_the_owning_manager_and_its_command() {
        assert!(
            InstallManager::Homebrew
                .upgrade_hint()
                .unwrap()
                .contains("brew upgrade gel")
        );
        assert!(
            InstallManager::Scoop
                .upgrade_hint()
                .unwrap()
                .contains("scoop update gel")
        );
        assert!(
            InstallManager::Nix
                .upgrade_hint()
                .unwrap()
                .contains("nix profile upgrade gel-cli")
        );
        assert!(
            InstallManager::WinGet
                .upgrade_hint()
                .unwrap()
                .contains("winget upgrade Gelstable.Gel")
        );
        assert!(
            InstallManager::Apt
                .upgrade_hint()
                .unwrap()
                .contains("system package manager")
        );
    }
}
