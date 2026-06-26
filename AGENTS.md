# Project commands

- Always run repository commands through direnv: `direnv exec . bash -lc '<command>'`
- After cloning, allow the environment once with `direnv allow`.
- Use the repo shell for Rust work: `direnv exec . bash -lc 'cargo test'`
- Use the same wrapper for formatting and linting: `direnv exec . bash -lc 'cargo fmt'` and `direnv exec . bash -lc 'cargo clippy --all-features --workspace --all-targets'`
- If you need an interactive shell, prefer `direnv exec . bash` or `nix develop`.
