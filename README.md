Gel Command-line Tools
======================

This repository contains the implementation of `gel` command-line tool.


Install
=======

Download a release from
[GitHub Releases](https://github.com/gelstable/gel-cli/releases/latest) and
verify it before use:

```bash
gh attestation verify gel-v7.11.0-x86_64-unknown-linux-musl.tar.gz --owner gelstable
```

Native package manager installation is added by the
[package manager configs plan](docs/superpowers/plans/2026-09-12-native-package-manager-configs.md).

The legacy `curl --proto '=https' --tlsv1.2 -sSfL https://geldata.com/sh | sh`
installer is no longer a recommended installation path for this fork.


Upgrading
=========

`gel cli upgrade` replaces the binary in place only when `gel` installed itself.
When the binary is owned by a package manager, it prints that manager's upgrade
command and exits without touching anything:

| Install source | What `gel cli upgrade` prints |
| --- | --- |
| Homebrew | `brew upgrade gel` |
| Scoop | `scoop update gel` |
| WinGet | `winget upgrade Gelstable.Gel` |
| Nix | `nix profile upgrade gel-cli` |
| apt, dnf, pacman | update through your system package manager |

`--force` does not override this. Overwriting a file a package manager owns
corrupts its record of the install, which is worse than refusing. The background
version check reports the same instruction when a newer release exists.


Development
===========

This repository is set up for Nix + `direnv` development to ensure a pinned Rust toolchain and development dependencies (such as `gel-server`) are active.

### Prerequisites

- [Nix](https://nixos.org/) with [Flakes enabled](https://nixos.wiki/wiki/Flakes)
- [direnv](https://direnv.net/) (with [nix-direnv](https://github.com/nix-community/nix-direnv) recommended for caching devshell builds)

After cloning the repository, allow `direnv` once to load the environment automatically:

```bash
direnv allow
```

Once allowed, your shell will automatically load the dev environment when entering this directory. You can run standard cargo commands directly:

```bash
cargo build
cargo run -- --admin -d tutorial
cargo test
```

Alternatively, if you don't have `direnv` hooked into your shell, you can enter the environment with `nix develop` or `direnv exec . bash`.

If you do not use Nix or direnv, standard `cargo` commands (`cargo build`, `cargo test`) will still work if you have a compatible Rust toolchain installed locally.


Registry configuration
======================

The registry configuration is read from `cli.toml` in the platform config
directory: `$XDG_CONFIG_HOME/edgedb/cli.toml` (usually
`~/.config/edgedb/cli.toml`) on Unix, or
`%LOCALAPPDATA%\EdgeDB\config\cli.toml` on Windows. Registry manifests can be
configured with a `[registry]` table and an ordered `sources` array:

```toml
[registry]
sources = [
  "https://mirror.example.com/gel/registry.json", # operator-supplied manifest
  "file:///srv/gel-registry/registry.json",
  "./registry.json",
]
```

The HTTP URL above is an operator-supplied manifest example. Sources are
checked in the order listed. A later source is used when an earlier source is
missing or unavailable; equivalent artifacts from mirrors keep the first
source, while conflicting mirror records are rejected. Relative paths are
resolved relative to `cli.toml`; absolute paths and `file://` URLs can be used
for local manifests.

Manifest documents use `schema_version = 1` and identify each package index with
`channel`, `platform`, and `ref`:

```json
{
  "schema_version": 1,
  "indexes": [
    {
      "channel": "stable",
      "platform": "x86_64-unknown-linux-gnu",
      "ref": "indexes/stable-x86_64-unknown-linux-gnu.json"
    }
  ]
}
```

Index references may be absolute HTTP(S) or `file://` URLs, root-relative or
document-relative URLs for HTTP manifests, or paths relative to local manifest
files.

Each configured source is loaded atomically for the requested channel and
platform. If any selected index from one source is unavailable or invalid, that
source is rejected without contributing partial package data. Other healthy
sources continue to work and the CLI reports the degraded source.

A source with no matching index, or a valid index with no packages, is healthy
and produces an empty result. The CLI reports a registry error only when no
configured source is healthy, or when healthy sources publish conflicting
metadata for the same artifact identity.

For compatibility, `GEL_PKG_ROOT` (preferred) and the legacy
`EDGEDB_PKG_ROOT` environment variable select the legacy package-root mode and
override `[registry].sources`. Migrate to `[registry].sources` for manifest
configuration; remove these environment variables when they are no longer
needed. With neither an environment override nor configured sources, the
built-in default is the legacy package root `https://packages.geldata.com`.
That URL is a package root, not a registry manifest.

Tests
=====

There are a few categories of tests in this repo:

- unit tests within `src/`
  - run with: `cargo test --bins`,
  - no additional requirements,

- `tests/func/`
  - invokes the cli binary,
  - run with: `cargo test --test=func`,
  - requires `gel-server` binary in PATH,
  - will use [test-utils](https://github.com/geldata/test-utils/) to start the server,

- `tests/shared-client-tests/`
  - generates tests from [shared-client-testcases](https://github.com/geldata/shared-client-testcases/),
  - invokes the cli binary,
  - run with: `cargo test --package=shared-client-tests`,
  - will write into `/home/gel`,

- `tests/portable_*.rs/`
  - tests installation of the portable Gel server,
  - will download large packages,
  - run with: `cargo test --features=portable_tests --test=portable_X`,
  - assumes you don't have any portables installed before running it,

- Github Actions & Nightly tests


Code Quality Assurance
======================

This project uses rustfmt and clippy to provide a unified code style.
When opening pull requests, it is advised to run the following commands
before doing so:

```bash
cargo clippy --all-features --workspace --all-targets
cargo fmt
```


License
=======


Licensed under either of

* Apache License, Version 2.0,
  (./LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license (./LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.


Releasing
=========

Every user-facing pull request adds a change file under `.changeset/`:

```bash
knope document-change
```

Its front matter names the package and the bump: `gel-cli: patch`, `gel-cli: minor`,
or `gel-cli: major`.

1. **Release PR.** Every push to `master` runs `Release PR`, which force-pushes
   the bot-owned `knope/release` branch with the version bump, `Cargo.lock`
   update, and `CHANGELOG.md` entry, then dispatches candidate staging. Never
   commit to `knope/release` by hand.
2. **Candidate staging.** `Release candidate` builds all six targets, packages
   the registry executables, archives, `.deb`, `.rpm`, `gel-registry.json`, and
   both digest manifests, uploads them to an unpublished draft release with build
   provenance, reads every byte back through the API to verify it, smoke-tests
   the archived binaries on native runners, and commits
   `packaging/release-candidate.json` to the release branch.
3. **Review.** Check the version bump, the changelog, the draft release's assets
   and provenance, and the recorded candidate identity. The required
   `Release candidate check / candidate` check re-verifies the draft, regenerates
   the packaging metadata independently, and proves the prospective merge tree is
   source-equivalent to the tree that was built.
4. **Merge.** Merging is the release approval. `Release publish` re-runs the
   equivalence and asset checks against the actual merge commit, tags it, and
   flips the reviewed draft to published. Nothing is rebuilt.

**Publication window.** Merge and publication are not atomic. Between the two,
package manifests on `master` briefly reference downloads that do not exist yet.
This is the accepted tradeoff for keeping final packaging metadata inside the
reviewed pull request with no follow-up commit after merge. A publication failure
surfaces as a failed `Release publish` check on `master`.

**Recovery.** `Release publish` is idempotent and retryable with the same merge
commit: re-run it. An already-created matching tag and an already-published
matching release are both treated as completed steps. A tag that resolves to a
different commit, a missing asset, or a replaced asset stops publication and
requires maintainer investigation — automation never moves a tag, rebuilds the
reviewed bytes, or publishes mismatched assets.

Required checks
---------------

Configure branch protection on `master` to require:

- `CI / quality`
- `CI / test (linux-x64)`, `CI / test (macos-arm64)`, `CI / test (windows-x64)`,
  `CI / test (windows-arm64)`
- `CI / release-config`
- `Release candidate check / candidate`

Configure each check only after it has reported once on a pull request, so the
stable check name is known.

Secrets and settings
--------------------

- `RELEASE_BOT_TOKEN` — a GitHub App installation token or fine-grained PAT
  scoped to `gelstable/gel-cli` with Contents: read and write and Pull requests:
  read and write. It deliberately has no `workflows` scope, so release
  automation cannot modify workflow files.
- Allow only the release bot to create `v*` tags.
- Enable GitHub Artifact Attestations; the staging workflow needs
  `id-token: write` and `attestations: write`.
