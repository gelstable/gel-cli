{
  description = "The EdgeDB CLI";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/25.05";
    tooling-nixpkgs.url =
      "github:NixOS/nixpkgs/643809054d65fdd466a63e3155b8c498cb483c04";
    flake-parts.url = "github:hercules-ci/flake-parts";

    # provides rust toolchain
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };

    edgedb = {
      url = "github:edgedb/packages-nix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-parts.follows = "flake-parts";
    };
  };

  outputs =
    inputs@{
      flake-parts,
      fenix,
      edgedb,
      tooling-nixpkgs,
      ...
    }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      perSystem =
        {
          config,
          system,
          pkgs,
          ...
        }:
        let
          fenix_pkgs = fenix.packages.${system};
          tooling_pkgs = tooling-nixpkgs.legacyPackages.${system};
          release_tools = [
            (assert tooling_pkgs.knope.version == "0.23.0"; tooling_pkgs.knope)
            (assert tooling_pkgs.cargo-dist.version == "0.32.0"; tooling_pkgs.cargo-dist)
          ];
          # This pinned cargo-dist package exposes `dist`; the wrapper supplies Cargo's `cargo-dist` plugin name.
          cargo_dist_wrapper = pkgs.writeShellScriptBin "cargo-dist" ''
            exec ${tooling_pkgs.cargo-dist}/bin/dist "$@"
          '';
          local_tools = release_tools ++ [
            pkgs.actionlint
            cargo_dist_wrapper
            pkgs.gh
            pkgs.jq
            pkgs.python3
            pkgs.ripgrep
            pkgs.shellcheck
            pkgs.zstd
          ] ++ pkgs.lib.optional (builtins.hasAttr "powershell" pkgs) pkgs.powershell;
          rust_toolchain = fenix_pkgs.toolchainOf {
            channel = "1.88";
            sha256 = "sha256-Qxt8XAuaUR2OMdKbN4u8dBJOhSHxS+uS06Wl9+flVEk=";
          };

          common = [
            # needed for running tests
            edgedb.packages.${system}.gel-server-nightly
          ]
          ++ pkgs.lib.optional pkgs.stdenv.isDarwin [
            pkgs.libiconv
            pkgs.darwin.apple_sdk.frameworks.CoreServices
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
          ];

        in
        {
          devShells.default = pkgs.mkShell {
            buildInputs = common ++ local_tools ++ [
              (rust_toolchain.withComponents [
                "rustc"
                "cargo"
                "rust-std"
                "clippy"
                "rustfmt"
                "rust-src"
                "rust-analyzer"
              ])
            ];
            shellHook = ''
              export PATH="$(git rev-parse --show-toplevel)/target/debug:$PATH"
            '';
          };
        };
    };
}
