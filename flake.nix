{
  inputs.nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      supportedSystems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      pkgsFor = system: nixpkgs.legacyPackages.${system};
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShellNoCC {
            packages = with pkgs; [
              cargo
              clippy
              rust-analyzer
              rustc
              rustfmt
              taplo
            ];
          };
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "ren-check";
            version = "0.0.0";
            src = pkgs.lib.cleanSource ./.;
            cargoDepsName = "ren-dependencies";
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = with pkgs; [
              clippy
              rustfmt
              taplo
            ];
            strictDeps = true;
            buildPhase = ''
              runHook preBuild
              cargo fmt --all -- --check
              cargo clippy --offline --workspace --all-targets -- --deny warnings
              cargo test --offline --workspace --all-targets
              taplo lint --colors never
              taplo format --check --colors never
              runHook postBuild
            '';
            doCheck = false;
            installPhase = ''
              touch $out
            '';
          };
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt-tree);
    };
}
