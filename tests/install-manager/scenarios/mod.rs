//! One module per install method.
//!
//! Implementations only — the `#[test]` wrappers live at the root of the target
//! (see `main.rs`) so that CI can select them with `--exact e2e_<slug>`.

pub mod direct;
