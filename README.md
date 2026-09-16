Gel Command-line Tools
======================

This repository contains the implementation of `gel` command-line tool.


Install
=======

Install the latest stable build with:

```
curl --proto '=https' --tlsv1.2 -sSfL https://geldata.com/sh | sh
```

Nightly builds can be installed with:

```
$ curl --proto '=https' --tlsv1.2 -sSfL https://geldata.com/sh | sh -s -- --nightly
```


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


Release lines
=============

Gel CLI releases are prepared independently on long-lived `release/vN.x`
branches. Each line has its own version, changelog, generated release pull
request, and candidate record. A v7 release and a v8 release can therefore be
prepared at the same time without sharing generated state.

Starting a line
---------------

To start a major line, cut `release/vN.x` from `master`, set its intended plain
starting version in both `Cargo.toml` and `Cargo.lock`, and push the branch:

```
git switch -c release/v8.x master
# edit Cargo.toml and Cargo.lock to set the package to 8.0.0
git commit -am "chore: start release/v8.x at 8.0.0"
git push origin release/v8.x
```

Protect `master` and every active `release/vN.x` branch before accepting
release changes. The line rule must require the `Release candidate check /
stable merge gate` status check; the complete configuration is in
[`.github/branch-protection.md`](.github/branch-protection.md). The gate runs
again whenever a release PR is opened, synchronized, reopened, labeled, or
unlabeled, so a preview label cannot leave a stale green stable check.

Day-to-day line releases
------------------------

Every release change on a line needs a `.changeset/*.md` file. Fixes normally
land on `master` first and are cherry-picked onto the line when needed:

```
git switch release/v8.x
git cherry-pick <master-commit>
git push origin release/v8.x
```

The line-specific `Release PR` workflow validates the line's current base and
version, and prepares one pull request from `knope/release-vN.x`. A line with
no pending change files produces no release PR. After a release PR merges, the
next change on that line creates a new PR; other major lines keep their own
version and generated head.

The controller re-evaluates the live release PR on every label transition.
An unlabelled PR is a stable candidate. A maintainer can add exactly one of
`prerelease:alpha`, `prerelease:beta`, or `prerelease:rc` to request a testing
preview. The first published preview for a phase receives suffix `.1`; a
retry with the same phase and source snapshot does nothing, while a changed
phase or source receives the next published suffix. Failed or stale drafts do
not consume a suffix because only published preview tags advance the counter.
Use the controller's line/PR retry dispatch after a transient workflow
failure; it resolves the current labels and source again before staging.

Candidate staging creates the draft release and reads all staged assets back
through the GitHub API. If a draft is stale, leave published releases and tags
untouched, refresh the line PR from its current line tip, and retry the same
line. Preparation removes an old `packaging/release-candidate.json` from the
refreshed generated branch before replacing it. A draft is reusable only when
its tag and candidate identity still match; an identity mismatch is a stop for
maintainer investigation rather than an opportunity to replace bytes.

Stable merge and publication
----------------------------

Review the version, changelog, candidate record, draft assets, attestations,
and the `Release candidate check / stable merge gate` result. A phase-labeled
PR cannot merge because the stable gate requires no active prerelease label. An
unlabelled current candidate can merge only after the gate has verified the
prospective merge tree and the existing draft. Merging the release PR is the
approval to publish.

`Release publish` handles the line push at the actual merge SHA. It rechecks
the candidate record, draft bytes, source equivalence, and tag identity, then
publishes the existing draft and creates the stable tag. It never rebuilds,
uploads, replaces, or moves published assets or tags. A normal backport push
without a matching newly merged candidate is a no-op. If publication fails,
retry the same line push after fixing the cause; an existing matching tag and
published release are successful idempotent states.

GitHub's `latest` flag is selected from the greatest published stable SemVer
across all major lines, so a v7 patch does not displace a newer v8 stable
release. Preview releases remain testing releases and never become latest.

Version channels and package ordering
-------------------------------------

The release version selects the generated manifest channel explicitly:

| Version | Registry channel | GitHub release |
| --- | --- | --- |
| `7.1.0` | `stable` | published stable |
| `7.1.0-alpha.1`, `7.1.0-beta.1`, `7.1.0-rc.1` | `testing` | prerelease |

Only plain SemVer and the `alpha`, `beta`, and `rc` phase prereleases are
accepted. `-dev.1` and arbitrary suffixes are rejected during candidate
planning. Every index in `gel-registry.json` carries its `channel` field; the
registry uses that explicit field when selecting stable or testing entries.
Linux package metadata rewrites a supported prerelease as
`7.1.0~alpha.1` (and similarly for beta and rc), which makes it sort below the
matching `7.1.0` package in both Debian and RPM package managers.

Registry snapshot promotion
---------------------------

Publishing a GitHub release does not directly change the public registry. The
registry promotion job discovers only published, non-draft releases from
`gelstable/gel-cli`, accepts only release tags and assets on the registry
allowlist, and keeps the manifest's explicit `stable` or `testing` channel.
It ignores drafts, arbitrary releases, and unsupported version suffixes. The
job proposes those reviewed entries in a separate snapshot pull request in
the registry repository. Review and merge that separate snapshot pull request
to update the public registry; a pinned snapshot URL remains available for
clients that need a reviewable immutable view. Include releases from every
active major line when promoting the snapshot.

The client-side `[registry].sources` configuration below can point at a moving
registry root or a pinned snapshot. The release migration does not change the
legacy package-root path: with no configured source, the built-in default is
still `https://packages.geldata.com`, and `GEL_PKG_ROOT` plus the legacy
`EDGEDB_PKG_ROOT` overrides and the `nightly` channel continue to work.

Retiring the old release workflow
---------------------------------

Repositories migrating from the old master-based generated release workflow
must leave it in place while the line workflow is proven. First run the new
line workflow through an end-to-end candidate: build every target, run the
install matrix, read the draft assets back through the API, pass the stable
merge gate, and exercise merge/publish plus registry snapshot promotion for
one line. Confirm that two active lines keep independent PRs, candidates,
versions, and changelogs. Only after those checks pass end to end should the
old master-based generated release workflow and its trigger be retired. Keep
the line workflows and branch protection rules as the sole release path after
the cutover.

Use the same validator locally when diagnosing a preparation run:

```
gel-release prepare-line --base-ref release/v8.x
```

The command reports the line base SHA, pending files, generated head, and
prepared plain version as JSON. A moved line base or a prepared version whose
major differs from the line stops the workflow before it pushes. If a refresh
replaces a PR that already has a stable candidate record, preparation starts
again from the latest line and removes that old record before the branch is
updated.


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
    },
    {
      "channel": "testing",
      "platform": "x86_64-unknown-linux-gnu",
      "ref": "indexes/testing-x86_64-unknown-linux-gnu.json"
    }
  ]
}
```

The `testing` entry is selected explicitly by its `channel` field; a client
does not infer a channel from a package version or an index filename.

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
