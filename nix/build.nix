# Vendor once; keep dependency artifacts separate by profile and features.
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
  cargoVendorDir = craneLib.vendorCargoDeps { inherit src; };
  common = dependencies.environment // {
    inherit src cargoVendorDir;
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
  variant =
    profile: features:
    let
      variantCommon = common // {
        CARGO_PROFILE = profile;
        cargoExtraArgs = "--workspace --locked ${features}";
      };
      cargoArtifacts = craneLib.buildDepsOnly (
        variantCommon
        // {
          # The test cache compiles dummy tests (--no-run) and dev-dependencies.
          # The release cache only builds production dependencies.
          doCheck = profile == "test";
        }
      );
    in
    {
      common = variantCommon;
      inherit cargoArtifacts;
      package = craneLib.buildPackage (
        variantCommon
        // {
          inherit cargoArtifacts;
          doCheck = false;
        }
      );
    };
in
{
  inherit common;
  release = variant "release" "";
  imblRelease = variant "release" "--features imbl";
  test = variant "test" "";
  imblTest = variant "test" "--features imbl";
  allFeaturesTest = variant "test" "--all-features";
}
