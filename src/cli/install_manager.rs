//! Detect whether this executable is owned by a system package manager.
//!
//! A managed install must never be overwritten in place: replacing a Homebrew
//! cellar file or a Scoop app directory corrupts the manager's own state. The
//! path rules are a pure function and every question only the host can answer
//! — the three Linux package databases, and the environment that tells Scoop
//! where it keeps its apps — goes through a trait, so the whole decision table
//! is testable on any host.

use std::path::{Path, PathBuf};
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
    /// Complete list of all InstallManager variants.
    ///
    /// When adding a new variant to the enum, you must also:
    /// 1. Add the variant to this array
    /// 2. Add a match arm in the `slug()` method with its lowercase slug
    ///
    /// The compiler ensures `slug()` is exhaustive; this array must be updated
    /// manually.
    ///
    /// Only the slug contract test below reads it — production code always has
    /// one concrete manager in hand, never the set — so it is compiled out of a
    /// normal build rather than carried there as dead code.
    #[cfg(test)]
    pub const ALL: [InstallManager; 8] = [
        InstallManager::Homebrew,
        InstallManager::Scoop,
        InstallManager::WinGet,
        InstallManager::Nix,
        InstallManager::Apt,
        InstallManager::Dnf,
        InstallManager::Pacman,
        InstallManager::Direct,
    ];

    /// The stable, lowercase name for this manager.
    ///
    /// This is a contract, not a display string: the e2e install matrix asserts
    /// on these exact values, and `gel info --get install-manager` prints them
    /// into bug reports. Renaming one is a breaking change.
    pub fn slug(self) -> &'static str {
        match self {
            InstallManager::Homebrew => "homebrew",
            InstallManager::Scoop => "scoop",
            InstallManager::WinGet => "winget",
            InstallManager::Nix => "nix",
            InstallManager::Apt => "apt",
            InstallManager::Dnf => "dnf",
            InstallManager::Pacman => "pacman",
            InstallManager::Direct => "direct",
        }
    }

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

/// Asks the host what it knows about ownership.
///
/// Split out from the path rules so detection can be tested without a dpkg,
/// rpm, or pacman on the machine running the tests — and without a Scoop
/// install on it either.
pub trait HostProbe {
    fn dpkg_owns(&self, exe: &Path) -> bool;
    fn rpm_owns(&self, exe: &Path) -> bool;
    fn pacman_owns(&self, exe: &Path) -> bool;

    /// The Scoop install roots this host is configured with, if any.
    ///
    /// Resolved the way Scoop resolves them: `$env:SCOOP` for the per-user
    /// root, `$env:SCOOP_GLOBAL` for the machine-wide one. Their defaults
    /// (`~\scoop`, `%ProgramData%\scoop`) are deliberately absent — a default
    /// root's last component is `scoop`, which the literal path rule already
    /// matches, so there is nothing for them to add here.
    fn scoop_roots(&self) -> Vec<PathBuf>;
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

impl HostProbe for SystemProbe {
    fn dpkg_owns(&self, exe: &Path) -> bool {
        Self::owns("dpkg", &["-S"], exe)
    }
    fn rpm_owns(&self, exe: &Path) -> bool {
        Self::owns("rpm", &["-qf"], exe)
    }
    fn pacman_owns(&self, exe: &Path) -> bool {
        Self::owns("pacman", &["-Qo"], exe)
    }
    fn scoop_roots(&self) -> Vec<PathBuf> {
        ["SCOOP", "SCOOP_GLOBAL"]
            .iter()
            .filter_map(std::env::var_os)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .collect()
    }
}

fn normalized(exe: &Path) -> String {
    exe.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn is_system_bin(exe: &Path) -> bool {
    let path = exe.to_string_lossy().replace('\\', "/");
    path.starts_with("/usr/bin/") || path.starts_with("/bin/")
}

/// True when `path` — already [`normalized`] — is a file Scoop installed
/// under `root`.
///
/// Scoop lays every install out as `<root>\apps\<app>\<version>\...`, with a
/// `current` junction beside the version directories, so an `apps` directory
/// directly under the configured root is what marks a file as Scoop's.
/// Claiming the whole root would be too broad: `cache`, `buckets` and
/// `persist` are Scoop's too but hold nothing the CLI can be running from, and
/// `<root>\shims\gel.exe` is a launcher that starts the real binary under
/// `apps` as a child process — so it is that child's path, not the shim's,
/// that reaches detection.
fn is_under_scoop_root(path: &str, root: &Path) -> bool {
    let root = normalized(root);
    let root = root.trim_end_matches('/');
    !root.is_empty() && path.starts_with(&format!("{root}/apps/"))
}

pub fn detect_from_path(exe: &Path, probe: &dyn HostProbe) -> InstallManager {
    let path = normalized(exe);

    if path.contains("/nix/store/") {
        return InstallManager::Nix;
    }
    // Two halves, because a Scoop root can be moved and it can be renamed. The
    // literal segments catch the default root and any relocated one whose last
    // component is still `scoop` (`D:\scoop`, `C:\opt\scoop`). A renamed root
    // leaves nothing in the path to recognise — with `$env:SCOOP=D:\tools` the
    // binary sits at `D:\tools\apps\gel\current\gel.exe` — so the configured
    // roots are consulted too. Missing that case would classify a
    // Scoop-managed install `Direct` and let `cli upgrade` overwrite it.
    if path.contains("scoop/apps/")
        || probe
            .scoop_roots()
            .iter()
            .any(|root| is_under_scoop_root(&path, root))
    {
        return InstallManager::Scoop;
    }
    if path.contains("winget/packages/") {
        return InstallManager::WinGet;
    }
    if path.contains("/opt/homebrew/")
        || path.contains("/usr/local/cellar/")
        || path.contains("/home/linuxbrew/.linuxbrew/")
        || path.contains("/.linuxbrew/")
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
        Ok(exe) => {
            // `current_exe()` returns a fully-resolved path on Linux (it reads
            // `/proc/self/exe`) but the *symlink* path on macOS (it uses
            // `_NSGetExecutablePath`). A Homebrew or Nix install on macOS is
            // invoked through such a symlink, so the raw path alone would
            // misclassify it as `Direct`. Only pay for a second classification
            // when the raw path didn't already match a manager and resolving
            // it actually changes anything.
            let resolved = std::fs::canonicalize(&exe).unwrap_or_else(|_| exe.clone());
            match detect_from_path(&exe, &SystemProbe) {
                InstallManager::Direct if resolved != exe => {
                    detect_from_path(&resolved, &SystemProbe)
                }
                managed => managed,
            }
        }
        Err(error) => {
            log::debug!("cannot determine the running executable: {error:#}");
            InstallManager::Direct
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{HostProbe, InstallManager, detect_from_path};

    #[derive(Default)]
    struct StubProbe {
        dpkg: bool,
        rpm: bool,
        pacman: bool,
        scoop_roots: Vec<PathBuf>,
    }

    impl HostProbe for StubProbe {
        fn dpkg_owns(&self, _exe: &Path) -> bool {
            self.dpkg
        }
        fn rpm_owns(&self, _exe: &Path) -> bool {
            self.rpm
        }
        fn pacman_owns(&self, _exe: &Path) -> bool {
            self.pacman
        }
        fn scoop_roots(&self) -> Vec<PathBuf> {
            self.scoop_roots.clone()
        }
    }

    /// Detection on a host with no Scoop root configured, which is every host
    /// that has not set `$env:SCOOP`.
    fn detect(path: &str) -> InstallManager {
        detect_from_path(Path::new(path), &StubProbe::default())
    }

    /// Detection on a host whose Scoop is configured with these roots.
    fn detect_under_roots(path: &str, roots: &[&str]) -> InstallManager {
        let probe = StubProbe {
            scoop_roots: roots.iter().map(PathBuf::from).collect(),
            ..StubProbe::default()
        };
        detect_from_path(Path::new(path), &probe)
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

    /// A Scoop root that has been *renamed*, not merely relocated: with
    /// `$env:SCOOP=D:\tools` no `scoop` segment survives in the path, so the
    /// literal rule has nothing to match. Calling such an install `Direct`
    /// would let `cli upgrade` overwrite Scoop's app directory — the very
    /// hazard this module exists to prevent.
    #[test]
    fn renamed_scoop_root_from_the_environment_is_scoop() {
        assert_eq!(
            detect_under_roots(r"D:\tools\apps\gel\current\gel.exe", &[r"D:\tools"]),
            InstallManager::Scoop
        );
        // What `detect()` retries with after resolving the `current` junction.
        assert_eq!(
            detect_under_roots(r"D:\tools\apps\gel\0.0.0\gel.exe", &[r"D:\tools"]),
            InstallManager::Scoop
        );
    }

    /// `scoop install -g` installs under a second, machine-wide root, which is
    /// configured by its own variable.
    #[test]
    fn global_scoop_root_is_scoop() {
        assert_eq!(
            detect_under_roots(
                r"D:\shared\apps\gel\current\gel.exe",
                &[r"D:\tools", r"D:\shared"],
            ),
            InstallManager::Scoop
        );
    }

    /// A trailing separator on `$env:SCOOP` is a plausible thing for a user to
    /// have, and must not leave the match hunting for `d:/tools//apps/`.
    #[test]
    fn scoop_root_with_a_trailing_separator_still_matches() {
        assert_eq!(
            detect_under_roots(r"D:\tools\apps\gel\current\gel.exe", &[r"D:\tools\"]),
            InstallManager::Scoop
        );
    }

    /// The root names a directory, not a string prefix: a sibling whose name
    /// merely starts with it is nobody's Scoop install.
    #[test]
    fn sibling_of_the_scoop_root_is_not_scoop() {
        assert_eq!(
            detect_under_roots(r"D:\toolsmith\apps\gel\current\gel.exe", &[r"D:\tools"]),
            InstallManager::Direct
        );
    }

    /// `apps` has to sit directly under the configured root. Anything deeper is
    /// some other tree that happens to have an `apps` directory in it.
    #[test]
    fn nested_apps_directory_under_the_scoop_root_is_not_scoop() {
        assert_eq!(
            detect_under_roots(r"D:\tools\vendor\apps\gel\gel.exe", &[r"D:\tools"]),
            InstallManager::Direct
        );
    }

    /// An empty `$env:SCOOP` is not a root at `/apps/`: the variable is
    /// filtered out before it ever reaches the path rules, and a stub that
    /// hands one over anyway must still be refused.
    #[test]
    fn an_empty_scoop_root_claims_nothing() {
        assert_eq!(
            detect_under_roots("/home/alice/apps/gel/gel", &[""]),
            InstallManager::Direct
        );
    }

    /// `current_exe()` returns the unresolved symlink path on macOS. The
    /// classification of the *canonical* target of a Homebrew or Nix symlink
    /// (what `detect()` falls back to when the raw path is `Direct`) has to
    /// be correct, even though `detect()` itself isn't unit-testable.
    #[test]
    fn resolved_homebrew_and_nix_targets_are_classified_correctly() {
        // Intel macOS Homebrew: /usr/local/bin/gel resolves into the Cellar.
        assert_eq!(
            detect("/usr/local/Cellar/gel/7.11.0/bin/gel"),
            InstallManager::Homebrew
        );
        // Nix profile on macOS: ~/.nix-profile/bin/gel resolves into the store.
        assert_eq!(
            detect("/nix/store/abc123-gel-cli-7.11.0/bin/gel"),
            InstallManager::Nix
        );
    }

    /// Homebrew on Linux also supports a sudo-less `~/.linuxbrew` prefix,
    /// distinct from the shared `/home/linuxbrew/.linuxbrew` prefix.
    #[test]
    fn linuxbrew_under_home_is_homebrew() {
        assert_eq!(
            detect("/home/alice/.linuxbrew/Cellar/gel/1.2/bin/gel"),
            InstallManager::Homebrew
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

    /// `is_system_bin` must gate on the directory `/usr/bin/` or `/bin/`, not
    /// merely a string prefix, or a lookalike directory such as
    /// `/usr/binary-thing/` would spawn dpkg/rpm/pacman needlessly.
    #[test]
    fn lookalike_directory_is_not_a_system_bin() {
        let probe = StubProbe {
            dpkg: true,
            rpm: true,
            pacman: true,
            ..StubProbe::default()
        };
        assert_eq!(
            detect_from_path(Path::new("/usr/binary-thing/gel"), &probe),
            InstallManager::Direct
        );
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
            ..StubProbe::default()
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
            ..StubProbe::default()
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

    #[test]
    fn slug_is_non_empty_lowercase_and_unique() {
        let mut slugs = Vec::new();
        for manager in &InstallManager::ALL {
            let slug = manager.slug();
            assert!(!slug.is_empty(), "{manager:?} has empty slug");
            assert_eq!(
                slug,
                slug.to_lowercase(),
                "{manager:?} slug '{slug}' is not lowercase"
            );
            slugs.push(slug);
        }

        // Check uniqueness by comparing lengths
        let unique_slugs: std::collections::HashSet<_> = slugs.iter().collect();
        assert_eq!(
            unique_slugs.len(),
            slugs.len(),
            "duplicate slugs found: {slugs:?}"
        );
    }

    #[test]
    fn slug_contract_values_are_exact() {
        // The e2e test matrix depends on these exact slug values.
        // Any typo here breaks the contract with the test suite.
        assert_eq!(InstallManager::Homebrew.slug(), "homebrew");
        assert_eq!(InstallManager::Scoop.slug(), "scoop");
        assert_eq!(InstallManager::WinGet.slug(), "winget");
        assert_eq!(InstallManager::Nix.slug(), "nix");
        assert_eq!(InstallManager::Apt.slug(), "apt");
        assert_eq!(InstallManager::Dnf.slug(), "dnf");
        assert_eq!(InstallManager::Pacman.slug(), "pacman");
        assert_eq!(InstallManager::Direct.slug(), "direct");
    }
}
