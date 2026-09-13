//! Install-manager end-to-end matrix.
//!
//! Every other layer of the install-manager work is a pure-function test: given
//! a path, which manager owns it? That proves the decision table but not the
//! thing users actually care about — that a Homebrew/Scoop/apt/... install is
//! never overwritten in place by `gel cli upgrade`, and that a plain curl-script
//! install still upgrades itself. Only a real install through a real package
//! manager can show that, so this target installs the binary for real and then
//! asserts on the bytes on disk.
//!
//! Because of that, these tests are gated twice and must stay that way:
//!
//! 1. The whole target is `#![cfg(feature = "install_manager_e2e")]`, so a plain
//!    `cargo test` does not even compile it.
//! 2. Every scenario is `#[ignore]`d, so `cargo test --all-features` compiles
//!    the target but still does not run anything. Installing system packages
//!    and rewriting the developer's `~/.local/bin` is not something a default
//!    test run may do.
//!
//! These tests are not hermetic and cannot be: an install is a thing that
//! happens to a machine. A scenario confines what it can — the installed binary
//! always lands in a tempdir it removes afterwards, and a `cli.toml` it has to
//! write outside that tempdir is backed up and restored. What it cannot confine
//! is the handful of files `cli install` writes relative to `$HOME` regardless
//! of where it installs: the shell completion scripts under
//! `~/.local/share/bash-completion`, `~/.config/fish/completions` and
//! `~/.zfunc`. Those are the same files a real install writes, with the same
//! contents.
//!
//! Run one deliberately:
//!
//! ```text
//! cargo test --features install_manager_e2e --test install-manager \
//!     -- --ignored --exact e2e_direct --nocapture
//! ```
//!
//! Scenario `#[test]` functions live here at the root of the target, not inside
//! `scenarios::*`, and are named `e2e_<slug>`. The CI workflow selects a single
//! scenario per container with `--exact e2e_<slug>`; a module prefix in the test
//! path would silently match nothing and the job would pass without testing
//! anything. Keep the wrappers thin — the work belongs in `scenarios`.
#![cfg(feature = "install_manager_e2e")]

mod scenario;
mod scenarios;

/// Unmanaged install: detects as `direct` *and* really replaces itself.
#[test]
#[ignore = "installs the CLI for real; run explicitly with --ignored"]
fn e2e_direct() {
    scenarios::direct::run();
}

/// Debian package: `dpkg -S /usr/bin/gel` answers, so the install is `apt`.
#[test]
#[ignore = "installs the CLI for real; run explicitly with --ignored"]
fn e2e_apt() {
    scenarios::apt::run();
}

/// RPM package: `rpm -qf /usr/bin/gel` answers, so the install is `dnf`.
#[test]
#[ignore = "installs the CLI for real; run explicitly with --ignored"]
fn e2e_dnf() {
    scenarios::dnf::run();
}

/// Arch package: `pacman -Qo /usr/bin/gel` answers, so the install is `pacman`.
#[test]
#[ignore = "installs the CLI for real; run explicitly with --ignored"]
fn e2e_pacman() {
    scenarios::pacman::run();
}

/// Homebrew, on macOS or Linux: a Cellar path linked into the prefix's `bin`.
#[test]
#[ignore = "installs the CLI for real; run explicitly with --ignored"]
fn e2e_homebrew() {
    scenarios::homebrew::run();
}

/// Nix, on Linux or macOS: a profile symlink into `/nix/store`.
#[test]
#[ignore = "installs the CLI for real; run explicitly with --ignored"]
fn e2e_nix() {
    scenarios::nix::run();
}
