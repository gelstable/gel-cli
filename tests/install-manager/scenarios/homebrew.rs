//! A Homebrew install, on macOS or on Linux.
//!
//! This is the scenario that covers the symlink branch in `detect()`
//! (`src/cli/install_manager.rs`). Homebrew installs into
//! `<prefix>/Cellar/gel/<version>/bin/gel` and links that into `<prefix>/bin`,
//! so the path a user runs is a symlink — and `current_exe()` on macOS reports
//! the path used to exec, not its target. On an Apple-silicon prefix the raw
//! path (`/opt/homebrew/bin/gel`) happens to match the `/opt/homebrew/` rule on
//! its own, but on an Intel prefix it does not: `/usr/local/bin/gel` matches
//! nothing, and only the canonicalised `/usr/local/Cellar/...` target says
//! Homebrew. That fallback is what keeps `cli upgrade` from overwriting an
//! Intel-mac Homebrew install, and this scenario is the only thing that
//! exercises it end to end.
//!
//! Two things it touches outside its tempdir, both undone in `cleanup`:
//! Homebrew's own prefix (the install), and a throwaway tap under
//! `$(brew --repository)/Library/Taps`. The tap is not optional — current
//! Homebrew refuses `brew install ./gel.rb` outright ("Homebrew requires
//! formulae to be in a tap"), so a local formula has to be reachable as
//! `<user>/<repo>/<formula>`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use sha2::{Digest, Sha256};

use crate::scenario::{self, Scenario};

/// Tap directory `Taps/gel-e2e/homebrew-e2e`, addressed as `gel-e2e/e2e`.
/// Deliberately not a name a person would pick, so cleanup can remove the whole
/// directory without wondering whether it belongs to the developer.
const TAP_USER: &str = "gel-e2e";
const TAP_REPO: &str = "e2e";

const FORMULA_VERSION: &str = "0.0.0";

pub fn run() {
    if !scenario::have("brew") {
        eprintln!("skipping: brew is not installed on this host");
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
    let scenario = HomebrewScenario::new(layout).expect("prepare the homebrew scenario");
    scenario::assert_managed(&scenario);
}

/// Where this machine's Homebrew keeps things. Asked of `brew` rather than
/// guessed: the prefix differs between Apple silicon (`/opt/homebrew`), Intel
/// (`/usr/local`) and Linux (`~/.linuxbrew` or `/home/linuxbrew/.linuxbrew`),
/// and those differences are exactly what the detection rules encode.
struct Layout {
    prefix: PathBuf,
    taps: PathBuf,
}

impl Layout {
    fn probe() -> anyhow::Result<Layout> {
        let prefix = ask("--prefix")?;
        let repository = ask("--repository")?;
        Ok(Layout {
            taps: repository.join("Library").join("Taps"),
            prefix,
        })
    }

    fn installed_bin(&self) -> PathBuf {
        self.prefix.join("bin").join("gel")
    }

    fn cellar(&self) -> PathBuf {
        self.prefix.join("Cellar").join("gel")
    }

    fn tap_user_dir(&self) -> PathBuf {
        self.taps.join(TAP_USER)
    }

    fn tap_dir(&self) -> PathBuf {
        self.tap_user_dir().join(format!("homebrew-{TAP_REPO}"))
    }

    /// Why this scenario must not run here: something is already called `gel`
    /// in Homebrew's prefix, and installing over it would end with cleanup
    /// uninstalling a package the developer wanted.
    fn occupied(&self) -> Option<String> {
        for path in [self.installed_bin(), self.cellar()] {
            if path.exists() {
                return Some(format!(
                    "{} already exists; refusing to install over a `gel` this test \
                     did not create",
                    path.display(),
                ));
            }
        }
        None
    }
}

fn ask(flag: &str) -> anyhow::Result<PathBuf> {
    let out = scenario::checked(brew().arg(flag), &format!("`brew {flag}`"))?;
    let value = out.stdout_trimmed();
    anyhow::ensure!(!value.is_empty(), "`brew {flag}` printed nothing");
    Ok(PathBuf::from(value))
}

/// A `brew` command with the interactive and network-touching conveniences off.
/// An auto-update in the middle of a test is minutes of unrelated work and one
/// more thing that can fail.
fn brew() -> Command {
    let mut cmd = Command::new("brew");
    cmd.env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
        .env("HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK", "1");
    cmd
}

pub struct HomebrewScenario {
    root: tempfile::TempDir,
    layout: Layout,
}

impl HomebrewScenario {
    fn new(layout: Layout) -> anyhow::Result<HomebrewScenario> {
        Ok(HomebrewScenario {
            root: tempfile::Builder::new().prefix("gel-e2e-brew-").tempdir()?,
            layout,
        })
    }

    /// A one-file tarball for the formula to download.
    ///
    /// Uncompressed on purpose: Homebrew unpacks a plain `.tar` as readily as a
    /// `.tar.gz`, and this crate has no gzip encoder among its dependencies, so
    /// compressing would mean either a new dependency or shelling out to
    /// `gzip` for nothing.
    fn tarball(&self, source: &Path) -> anyhow::Result<(PathBuf, String)> {
        let staged = scenario::stage_binary(source, &self.root.path().join("stage"))?;
        let path = self.root.path().join("gel.tar");
        let file = fs_err::File::create(&path)?;
        let mut builder = tar::Builder::new(file);
        builder
            .append_path_with_name(&staged, "gel")
            .context("adding the staged binary to the tarball")?;
        builder.finish().context("finishing the tarball")?;
        drop(builder);

        let digest = Sha256::digest(fs_err::read(&path)?);
        let sha256 = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok((path, sha256))
    }
}

fn formula(tarball: &Path, sha256: &str) -> String {
    format!(
        r#"class Gel < Formula
  desc "gel CLI install-manager e2e fixture"
  homepage "https://example.invalid/gel-cli-e2e"
  url "file://{url}"
  sha256 "{sha256}"
  version "{FORMULA_VERSION}"

  def install
    bin.install "gel"
  end
end
"#,
        url = tarball.display(),
    )
}

impl Scenario for HomebrewScenario {
    fn expected_slug(&self) -> &'static str {
        "homebrew"
    }

    fn install(&self, source: &Path) -> anyhow::Result<PathBuf> {
        let (tarball, sha256) = self.tarball(source)?;

        let tap_dir = self.layout.tap_dir();
        // A tap left behind by a run that died mid-scenario is ours by name, so
        // clear it rather than refusing: otherwise one interrupted run would
        // block this scenario until someone deleted the directory by hand.
        fs_err::remove_dir_all(self.layout.tap_user_dir()).ok();
        let formula_dir = tap_dir.join("Formula");
        fs_err::create_dir_all(&formula_dir)?;
        fs_err::write(formula_dir.join("gel.rb"), formula(&tarball, &sha256))?;

        scenario::checked(
            brew()
                .arg("install")
                .arg("--formula")
                .arg(format!("{TAP_USER}/{TAP_REPO}/gel")),
            "`brew install`",
        )?;

        let installed = self.layout.installed_bin();
        anyhow::ensure!(
            installed.exists(),
            "`brew install` succeeded but {} does not exist",
            installed.display(),
        );
        Ok(installed)
    }

    fn cleanup(&self) {
        // Unconditional, unlike the distro scenarios: `brew install` is the
        // first thing that can leave state behind, and `brew uninstall` on a
        // formula that was never installed is a cheap no-op message rather than
        // a change to the machine.
        if self.layout.installed_bin().exists() || self.layout.cellar().exists() {
            scenario::report_cleanup(
                brew().arg("uninstall").arg("--force").arg("gel"),
                "`brew uninstall gel`",
            );
        }
        if let Err(error) = fs_err::remove_dir_all(self.layout.tap_user_dir())
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("cleanup: could not remove the e2e tap: {error}");
        }
    }
}
