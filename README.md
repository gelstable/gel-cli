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
