//! End-to-end registry-sources surface check.
//!
//! Spawns the built `gel` binary with per-scenario config roots and asserts
//! that composition warnings reach stderr and versions reach stdout. Distinct
//! from Layer A (in-crate, asserts on pub(crate) internals): this layer proves
//! the user-visible surface, reachable only via the binary because gel-cli is
//! a binary-only crate.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Mirror the host's `portable::platform::get_server()` so the test knows which
/// index file the spawned CLI will look for. Returns the six pinned server
/// platforms on supported hosts, or a sentinel on unsupported hosts (test skips).
fn host_server_platform() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        if cfg!(target_os = "macos") {
            "x86_64-apple-darwin"
        } else if cfg!(target_os = "linux") {
            if cfg!(target_env = "musl") {
                "x86_64-unknown-linux-musl"
            } else {
                "x86_64-unknown-linux-gnu"
            }
        } else {
            "x86_64-unknown-linux-gnu" // windows: server runs in WSL
        }
    } else if cfg!(target_arch = "aarch64") {
        if cfg!(target_os = "macos") {
            "aarch64-apple-darwin"
        } else if cfg!(target_os = "linux") {
            if cfg!(target_env = "musl") {
                "aarch64-unknown-linux-musl"
            } else {
                "aarch64-unknown-linux-gnu"
            }
        } else {
            "aarch64-unknown-linux-gnu"
        }
    } else {
        "unsupported-host"
    }
}

/// Layer B isolates the CLI config via HOME/XDG_CONFIG_HOME, but on Windows the
/// CLI resolves config via LOCALAPPDATA and forwards `server list-versions` into
/// WSL — a different isolation model we don't implement here. Skip rather than
/// risk reading the user's real config (false positive) or failing.
fn windows_not_supported() -> Option<&'static str> {
    if cfg!(windows) {
        Some("Windows (WSL config isolation not implemented for this harness)")
    } else {
        None
    }
}

fn mirror_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/registry/mirror")
}

/// Copy every file in the committed mirror into `dest`.
fn copy_mirror(dest: &std::path::Path) {
    for entry in fs::read_dir(mirror_dir()).expect("read mirror dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_file() {
            fs::copy(&path, dest.join(entry.file_name())).expect("copy fixture");
        }
    }
}

/// Write `cli.toml` with the given registry sources into the config directory
/// the spawned CLI resolves on this host.
///
/// `dirs::config_dir()` honors `XDG_CONFIG_HOME` on Linux but ignores it on
/// macOS (where it returns `$HOME/Library/Application Support`). Because
/// `run_list_versions` points both `HOME` and `XDG_CONFIG_HOME` at `root`, the
/// CLI resolves its config dir under the tempdir on every host; we write to
/// whichever subpath that host's `config_dir()` picks.
fn write_cli_toml(root: &std::path::Path, sources: &[String]) {
    let config_dir = if cfg!(target_os = "macos") {
        root.join("Library/Application Support/edgedb")
    } else {
        root.join("edgedb")
    };
    fs::create_dir_all(&config_dir).expect("create config dir");
    let mut body = String::from("[registry]\nsources = [\n");
    for src in sources {
        body.push_str(&format!("  {src:?},\n"));
    }
    body.push_str("]\n");
    fs::write(config_dir.join("cli.toml"), body).expect("write cli.toml");
}

/// Spawn `gel --no-cli-update-check server list-versions` with the scenario's
/// config root and return (exit_ok, stdout, stderr).
///
/// Points both `HOME` and `XDG_CONFIG_HOME` at `root` so the CLI resolves its
/// config directory under the tempdir on every host. Removes `RUST_LOG` (the
/// test harness exports an empty `RUST_LOG`, which `env_logger` treats as
/// "off" instead of applying the built-in `warn` default) and the `*_PKG_ROOT`
/// overrides (an empty `GEL_PKG_ROOT`/`EDGEDB_PKG_ROOT` would bypass
/// `[registry].sources` entirely and fall back to the legacy package root).
fn run_list_versions(root: &std::path::Path) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_gel"))
        .arg("--no-cli-update-check")
        .arg("server")
        .arg("list-versions")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root)
        .env_remove("RUST_LOG")
        .env_remove("GEL_PKG_ROOT")
        .env_remove("EDGEDB_PKG_ROOT")
        .output()
        .expect("spawn gel");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn faithful_mirror_lists_versions() {
    let platform = host_server_platform();
    if platform == "unsupported-host" {
        eprintln!("skipping: unsupported host arch");
        return;
    }
    if let Some(reason) = windows_not_supported() {
        eprintln!("skipping: {reason}");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_mirror(tmp.path());
    write_cli_toml(
        tmp.path(),
        &[tmp.path().join("registry.json").display().to_string()],
    );

    let (ok, stdout, _stderr) = run_list_versions(tmp.path());
    assert!(ok, "list-versions should succeed on the faithful mirror");
    assert!(
        !stdout.trim().is_empty(),
        "versions should appear on stdout"
    );
}

#[test]
fn conflicting_mirror_warns_on_stderr() {
    let platform = host_server_platform();
    if platform == "unsupported-host" {
        eprintln!("skipping: unsupported host arch");
        return;
    }
    if let Some(reason) = windows_not_supported() {
        eprintln!("skipping: {reason}");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_mirror(tmp.path());

    // Build a second, mutated source: copy the mirror again and, in the host
    // platform's stable index, bump the size of EVERY installref of a
    // gel-server package (validates as Server; present in every stable index),
    // so whichever installref validate_server selects carries the changed
    // size. Same identity, differing size → conflict.
    let alt = tempfile::tempdir().expect("tempdir");
    copy_mirror(alt.path());
    let index_path = alt.path().join(format!("stable-{platform}.json"));
    let mut doc: serde_json::Value =
        serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
    let packages = doc["packages"].as_array_mut().expect("packages array");
    let target = packages
        .iter_mut()
        .find(|p| p["basename"] == "gel-server")
        .expect("host stable index contains gel-server packages");
    for installref in target["installrefs"]
        .as_array_mut()
        .expect("installrefs array")
    {
        let size = installref["verification"]["size"]
            .as_u64()
            .expect("size present");
        installref["verification"]["size"] = serde_json::Value::from(size + 1);
    }
    fs::write(&index_path, serde_json::to_vec(&doc).unwrap()).unwrap();

    write_cli_toml(
        tmp.path(),
        &[
            tmp.path().join("registry.json").display().to_string(),
            alt.path().join("registry.json").display().to_string(),
        ],
    );

    let (ok, _stdout, stderr) = run_list_versions(tmp.path());
    assert!(ok, "list-versions should still succeed with a conflict");
    assert!(
        stderr.contains("contested"),
        "stderr should report the contested artifact: {stderr}"
    );
}

#[test]
fn unavailable_source_warns_on_stderr() {
    let platform = host_server_platform();
    if platform == "unsupported-host" {
        eprintln!("skipping: unsupported host arch");
        return;
    }
    if let Some(reason) = windows_not_supported() {
        eprintln!("skipping: {reason}");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_mirror(tmp.path());

    // A second source whose manifest points at a missing index.
    let bad = tempfile::tempdir().expect("tempdir");
    fs::write(
        bad.path().join("registry.json"),
        format!(
            r#"{{"schema_version":1,"indexes":[{{"channel":"stable","platform":"{platform}","ref":"does-not-exist.json"}}]}}"#,
        ),
    )
    .unwrap();

    write_cli_toml(
        tmp.path(),
        &[
            bad.path().join("registry.json").display().to_string(),
            tmp.path().join("registry.json").display().to_string(),
        ],
    );

    let (ok, stdout, stderr) = run_list_versions(tmp.path());
    assert!(ok, "fallback source should keep list-versions succeeding");
    assert!(!stdout.trim().is_empty(), "versions should still appear");
    assert!(
        stderr.contains("is unavailable"),
        "stderr should report the unavailable source: {stderr}"
    );
}

#[test]
fn rejected_source_warns_on_stderr() {
    let platform = host_server_platform();
    if platform == "unsupported-host" {
        eprintln!("skipping: unsupported host arch");
        return;
    }
    if let Some(reason) = windows_not_supported() {
        eprintln!("skipping: {reason}");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_mirror(tmp.path());

    // A second source whose index is invalid JSON → atomic rejection.
    let bad = tempfile::tempdir().expect("tempdir");
    copy_mirror(bad.path());
    fs::write(
        bad.path().join(format!("stable-{platform}.json")),
        b"{not json",
    )
    .unwrap();

    write_cli_toml(
        tmp.path(),
        &[
            bad.path().join("registry.json").display().to_string(),
            tmp.path().join("registry.json").display().to_string(),
        ],
    );

    let (ok, stdout, stderr) = run_list_versions(tmp.path());
    assert!(ok, "fallback source should keep list-versions succeeding");
    assert!(!stdout.trim().is_empty(), "versions should still appear");
    assert!(
        stderr.contains("was rejected"),
        "stderr should report the rejected source: {stderr}"
    );
}
