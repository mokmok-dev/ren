{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
    rust-flake.url = "github:juspay/rust-flake";
    rust-flake.inputs.nixpkgs.follows = "nixpkgs";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    rust-flake.inputs.rust-overlay.follows = "rust-overlay";
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.rust-flake.flakeModules.default
        inputs.rust-flake.flakeModules.nixpkgs
      ];

      perSystem =
        {
          self',
          pkgs,
          config,
          lib,
          ...
        }:
        {
          devShells.default = pkgs.mkShellNoCC {
            inputsFrom = [ self'.devShells.rust ];
          };

          packages = {
            default = self'.packages.ren;
          };

          rust-project = {
            src = lib.cleanSourceWith {
              src = ./.;
              filter =
                path: type:
                config.rust-project.crane-lib.filterCargoSources path type
                || builtins.any (suffix: lib.hasSuffix suffix path) [
                  ".md"
                  ".rhai"
                  ".yaml"
                ];
            };
            toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          };

          treefmt = {
            projectRootFile = "flake.nix";
            programs = {
              actionlint.enable = true;
              nixfmt.enable = true;
              taplo.enable = true;
              yamlfmt.enable = true;
            };
          };
        };

      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
    };
}
