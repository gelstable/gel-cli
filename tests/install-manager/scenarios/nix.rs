//! A Nix profile install, on Linux or macOS.
//!
//! Not gated to Linux: `nix profile install` works the same on macOS, and the
//! path it produces is the one detection cares about. As with Homebrew, what a
//! user runs is a symlink — `<profile>/bin/gel` points into
//! `/nix/store/<hash>-<name>/bin/gel` — so on macOS the raw `current_exe()`
//! path is `direct` and only the canonicalised store path says `nix`.
//!
//! The binary is copied into the store unchanged: it is not patchelf'd and its
//! interpreter is still the host's. That is fine and deliberate. Detection keys
//! on the store path, not on how the executable is linked, and the binary has
//! to keep running under the host loader for `assert_managed` to be able to ask
//! it anything.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::scenario::{self, Scenario};

/// Flakes and `nix profile` are still behind experimental-feature flags on a
/// stock nix.conf, and the flag is harmless on an installation that already
/// enables them.
const FEATURES: &str = "nix-command flakes";

/// A flake whose default package is the staged binary in a store path.
///
/// `runCommand` comes from nixpkgs, which is resolved through the flake
/// registry (`flake:nixpkgs`) rather than pinned to a revision: pinning would
/// make the scenario fetch a specific nixpkgs no machine running it has
/// cached, and nothing here depends on which nixpkgs it gets.
///
/// The systems are listed rather than taken from `flake-utils` so that the
/// flake has exactly one input.
const FLAKE: &str = r#"{
  description = "gel CLI install-manager e2e fixture";

  inputs.nixpkgs.url = "flake:nixpkgs";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
    in {
      packages = nixpkgs.lib.genAttrs systems (system: {
        default = nixpkgs.legacyPackages.${system}.runCommand "gel-cli-e2e" { } ''
          mkdir -p $out/bin
          cp ${./gel} $out/bin/gel
          chmod +x $out/bin/gel
        '';
      });
    };
}
"#;

pub fn run() {
    if !scenario::have("nix") {
        eprintln!("skipping: nix is not installed on this host");
        return;
    }
    let scenario = NixScenario::new().expect("prepare the nix scenario");
    scenario::assert_managed(&scenario);
}

pub struct NixScenario {
    root: tempfile::TempDir,
}

impl NixScenario {
    fn new() -> anyhow::Result<NixScenario> {
        Ok(NixScenario {
            root: tempfile::Builder::new().prefix("gel-e2e-nix-").tempdir()?,
        })
    }

    /// The flake source directory.
    ///
    /// Kept separate from the profile below it: `nix` copies the whole flake
    /// directory into the store, and a profile symlink inside it would be part
    /// of the source — changing the store path on every run and, worse, making
    /// the flake depend on its own output.
    fn flake_dir(&self) -> PathBuf {
        self.root.path().join("flake")
    }

    /// A profile of this scenario's own, rather than the user's default one.
    ///
    /// `nix profile install` with no `--profile` writes to `~/.nix-profile`,
    /// which is the developer's actual environment: a crashed run would leave a
    /// `gel` in it, and a concurrent `nix profile` operation elsewhere on the
    /// machine would contend with the test. A profile inside the tempdir has
    /// neither problem and produces the same `/nix/store/...` symlink target,
    /// which is all detection looks at.
    fn profile(&self) -> PathBuf {
        self.root.path().join("profile")
    }

    fn nix(&self) -> Command {
        let mut cmd = Command::new("nix");
        cmd.arg("--extra-experimental-features").arg(FEATURES);
        cmd
    }
}

impl Scenario for NixScenario {
    fn expected_slug(&self) -> &'static str {
        "nix"
    }

    fn install(&self, source: &Path) -> anyhow::Result<PathBuf> {
        let flake_dir = self.flake_dir();
        // `stage_binary` writes `<flake_dir>/gel`, which is what `${./gel}` in
        // the flake refers to.
        scenario::stage_binary(source, &flake_dir)?;
        fs_err::write(flake_dir.join("flake.nix"), FLAKE)?;

        scenario::checked(
            self.nix()
                .arg("profile")
                .arg("install")
                .arg("--profile")
                .arg(self.profile())
                .arg(format!("{}#default", flake_dir.display())),
            "`nix profile install`",
        )?;

        let installed = self.profile().join("bin").join("gel");
        anyhow::ensure!(
            installed.exists(),
            "`nix profile install` succeeded but {} does not exist",
            installed.display(),
        );
        Ok(installed)
    }

    fn cleanup(&self) {
        // The store path itself stays — it is content-addressed, shared, and
        // removed by `nix store gc` like any other build output. What must not
        // stay is the profile referencing it, and that lives in the tempdir
        // this scenario drops. Removing the entry first keeps `nix profile
        // list --profile` honest for anything that inspects the profile before
        // the directory goes away.
        if self.profile().exists() {
            scenario::report_cleanup(
                self.nix()
                    .arg("profile")
                    .arg("remove")
                    .arg("--profile")
                    .arg(self.profile())
                    .arg("--all"),
                "`nix profile remove`",
            );
        }
    }
}
