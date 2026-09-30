import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

from gel_release import assets, linux_packages

AMD64 = assets.BY_TRIPLE["x86_64-unknown-linux-musl"]
ARM64 = assets.BY_TRIPLE["aarch64-unknown-linux-musl"]


def test_native_config_layout_and_relationships():
    config = yaml.safe_load(Path("packaging/native/gel-cli.nfpm.yaml").read_text())
    assert config["name"] == "gel-cli"
    assert config["epoch"] == "1"
    assert config["release"] == "1"
    assert config["version_schema"] == "none"
    assert config["replaces"] == config["conflicts"] == ["edgedb-cli"]
    destinations = {row["dst"] for row in config["contents"]}
    assert "/usr/bin/gel" in destinations
    assert "/usr/bin/edgedb" not in destinations
    assert "/usr/share/doc/gel-cli/LICENSE-MIT" in destinations
    assert "/usr/share/bash-completion/completions/gel" in destinations
    assert "/usr/share/zsh/site-functions/_gel" in destinations
    assert "/usr/share/fish/vendor_completions.d/gel.fish" in destinations


def test_native_versions_and_names():
    assert linux_packages.package_manager_version("7.10.3") == "1:7.10.3-1"
    assert linux_packages.package_manager_version("7.11.0-rc.1") == "1:7.11.0~rc.1-1"
    assert assets.deb_name("7.11.0-rc.1", AMD64) == "gel-cli-7.11.0-rc.1-1-amd64.deb"
    assert assets.rpm_name("7.11.0-rc.1", ARM64) == "gel-cli-7.11.0-rc.1-1-aarch64.rpm"
    with pytest.raises(ValueError):
        linux_packages.package_manager_version("7.11.0-dev.1")


def test_epoch_upgrades_legacy_packages():
    if not shutil.which("dpkg"):
        pytest.skip("dpkg version comparison requires Linux")
    subprocess.run(
        ["dpkg", "--compare-versions", "1:7.10.3-1", "gt", "7.10.2+feff35c-202511231812~bookworm"],
        check=True,
    )
    subprocess.run(
        ["dpkg", "--compare-versions", "1:7.11.0~rc.1-1", "lt", "1:7.11.0-1"],
        check=True,
    )


def test_build_uses_prebuilt_target_without_rewriting_cargo(monkeypatch, tmp_path):
    original = Path("Cargo.toml").read_bytes()
    seen = []

    def run(argv, *, env, check):
        assert check
        assert env["NATIVE_VERSION"] == "7.11.0~rc.1"
        assert env["NATIVE_ARCH"] == "arm64"
        assert env["NATIVE_BINARY"].endswith(f"target/{ARM64.triple}/release/gel")
        assert env["COMPLETIONS_DIR"].endswith("custom-completions")
        seen.append(argv)
        Path(argv[argv.index("--target") + 1]).write_bytes(b"package")

    monkeypatch.setattr(subprocess, "run", run)
    produced = linux_packages.build(ARM64, "7.11.0-rc.1", Path("custom-completions"), tmp_path)
    assert {path.suffix for path in produced} == {".deb", ".rpm"}
    assert all(argv[:2] == ["nfpm", "package"] for argv in seen)
    assert Path("Cargo.toml").read_bytes() == original


def test_non_linux_target_is_rejected(tmp_path):
    with pytest.raises(ValueError):
        linux_packages.build(assets.BY_TRIPLE["aarch64-apple-darwin"], "7.11.0", tmp_path, tmp_path)
