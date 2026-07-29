{
  lib,
  rustPlatform,
}:
let
  root = toString ../.;
  workspaceManifest = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  workspaceMembers = workspaceManifest.workspace.members;
  sourceFilter =
    path: type:
    let
      relative = lib.removePrefix "${root}/" (toString path);
      isWorkspaceFile = builtins.elem relative [
        ".clippy.toml"
        ".rustfmt.toml"
        "Cargo.lock"
        "Cargo.toml"
      ];
      isCrateFile = member: relative == member || lib.hasPrefix "${member}/" relative;
      isWorkspaceCrateFile = builtins.any isCrateFile workspaceMembers;
      isBuildArtifact =
        relative == "target" || lib.hasPrefix "target/" relative || lib.hasInfix "/target/" relative;
    in
    lib.cleanSourceFilter path type
    && !isBuildArtifact
    && (type == "directory" || isWorkspaceFile || isWorkspaceCrateFile);
in
rustPlatform.buildRustPackage {
  pname = "ren";
  version = workspaceManifest.workspace.package.version;

  src = lib.cleanSourceWith {
    src = ../.;
    filter = sourceFilter;
    name = "ren-source";
  };

  cargoLock.lockFile = ../Cargo.lock;

  cargoBuildFlags = [
    "--package"
    "ren"
  ];
  cargoTestFlags = [ "--workspace" ];

  strictDeps = true;

  meta = {
    description = "Deterministic coding-agent workflows";
    homepage = "https://github.com/mokmok-dev/ren";
    mainProgram = "ren";
    platforms = lib.platforms.unix;
  };
}
