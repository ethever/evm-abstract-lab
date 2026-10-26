# Public build interface. Source ownership, feature equivalence and compiled
# artifact reuse each live in their own module; checks consume this shared graph.
{
  pkgs,
  craneLib,
  dependencies,
  sourceRoot,
}:
let
  lib = pkgs.lib;
  sources = import ./sources.nix { inherit pkgs craneLib sourceRoot; };
  configurations = import ./cargo-configurations.nix {
    inherit
      pkgs
      sources
      craneLib
      cargoVendorDir
      ;
  };
  cargoVendorDir = import ./cargo-vendor.nix {
    inherit craneLib;
    src = sources.workspace;
  };
  cargoCommand = import ./cargo-command.nix {
    inherit pkgs cargoVendorDir;
    inherit (craneLib) cargo;
  };
  common = dependencies.environment // {
    src = sources.workspace;
    inherit cargoVendorDir;
    inherit (dependencies) nativeBuildInputs buildInputs;
    strictDeps = true;
    # Cargo may refresh an existing output without changing its filename (for
    # example bindgen metadata between check/build). Writable inherited copies
    # preserve timestamps and fingerprints without mutating immutable caches.
    doNotLinkInheritedArtifacts = true;
    pname = "evm-abstract";
    version = sources.rootManifest.workspace.package.version;
    cargoExtraArgs = "--workspace --locked";
    # Crane otherwise exports per-command job/thread limits from NIX_BUILD_CORES.
    # Leave scheduling to Cargo, the test harness and the caller's environment.
    prePatch = import ./cargo-concurrency.nix;
    meta = {
      description = "Analyze multi-account EVM worlds, cross-contract graphs and SSA";
      license = lib.licenses.mit;
      mainProgram = "evm-abstract";
    };
  };
  commonFor =
    name: mode: configuration:
    (
      if mode.target == "wasm" then
        builtins.removeAttrs common (
          (builtins.attrNames dependencies.environment)
          ++ [
            "nativeBuildInputs"
            "buildInputs"
          ]
        )
        // {
          nativeBuildInputs = [ ];
          buildInputs = [ ];
        }
      else
        common
    )
    // {
      pname = name;
      prePatch =
        common.prePatch
        + lib.optionalString (mode.target != "wasm") (import ./cargo-bindgen.nix { inherit pkgs; });
      src = configurations.configuredSource [ name ] mode configuration;
      cargoExtraArgs = "--locked ${configurations.packageArgs name mode configuration}";
    };
  mkArtifacts =
    args:
    import ./cargo-artifacts.nix (
      {
        inherit
          pkgs
          craneLib
          sources
          common
          commonFor
          configurations
          ;
      }
      // args
    );
  variant =
    profile: configuration:
    let
      mode = {
        kind = if profile == "test" then "test" else "build";
        target = "native";
      };
      artifacts = mkArtifacts { inherit profile mode configuration; };
      packages = lib.genAttrs sources.names (
        name:
        craneLib.buildPackage (
          commonFor name mode configuration
          // {
            CARGO_PROFILE = profile;
            cargoArtifacts = artifacts.forPackage name;
            doCheck = false;
          }
        )
      );
    in
    {
      inherit
        configuration
        mode
        artifacts
        packages
        commonFor
        ;
      common = common // {
        CARGO_PROFILE = profile;
        cargoExtraArgs = "--locked ${configurations.workspaceArgs mode configuration}";
      };
      cargoArtifacts = artifacts.base;
      package = pkgs.symlinkJoin {
        name = "evm-abstract-${profile}-${common.version}";
        paths = builtins.attrValues packages;
        inherit (common) meta;
      };
    };
in
{
  inherit
    cargoCommand
    common
    commonFor
    sources
    configurations
    mkArtifacts
    ;
  release = variant "release" configurations.default;
  imblRelease = variant "release" configurations.imbl;
  test = variant "test" configurations.default;
  imblTest = variant "test" configurations.imbl;
  allFeaturesTest = variant "test" configurations.allFeatures;
}
