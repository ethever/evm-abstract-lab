# Cargo dependencies remain pinned in Cargo.lock and are vendored by Crane.
{
  pkgs,
  craneLib,
  dependencies,
  sourceRoot,
}:
let
  lib = pkgs.lib;
  src = lib.cleanSourceWith {
    src = sourceRoot;
    filter =
      path: type:
      let
        relative = lib.removePrefix "${toString sourceRoot}/" (toString path);
        application = !(relative == "lints" || lib.hasPrefix "lints/" relative);
        fixture = lib.hasPrefix "examples/" relative || lib.hasPrefix "crates/" relative;
        native = lib.hasPrefix "crates/" relative;
      in
      application
      && (
        craneLib.filterCargoSources path type
        || (native && (lib.hasSuffix ".cpp" path || lib.hasSuffix ".h" path))
        || (fixture && (lib.hasSuffix ".hex" path || lib.hasSuffix ".json" path))
      );
  };
  common = dependencies.environment // {
    inherit src;
    inherit (dependencies) nativeBuildInputs buildInputs;
    strictDeps = true;
    pname = "evm-abstract";
    version =
      (builtins.fromTOML (builtins.readFile (sourceRoot + "/Cargo.toml"))).workspace.package.version;
    cargoExtraArgs = "--workspace --locked";
    meta = {
      description = "Analyze multi-account EVM worlds, cross-contract graphs and SSA";
      license = lib.licenses.mit;
      mainProgram = "evm-abstract";
    };
  };
  cargoArtifacts = craneLib.buildDepsOnly common;
  package = craneLib.buildPackage (common // { inherit cargoArtifacts; });
  imblCommon = common // {
    cargoExtraArgs = "--workspace --locked --features imbl";
  };
  imblArtifacts = craneLib.buildDepsOnly imblCommon;
  imblPackage = craneLib.buildPackage (imblCommon // { cargoArtifacts = imblArtifacts; });
in
{
  inherit
    common
    cargoArtifacts
    package
    imblCommon
    imblArtifacts
    imblPackage
    ;
}
