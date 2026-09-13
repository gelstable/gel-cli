//! One module per install method.
//!
//! Implementations only — the `#[test]` wrappers live at the root of the target
//! (see `main.rs`) so that CI can select them with `--exact e2e_<slug>`.

pub mod apt;
pub mod direct;
pub mod dnf;
pub mod homebrew;
pub mod nix;
pub mod pacman;

/// Not a scenario: what apt, dnf and pacman share.
mod unix_package;
