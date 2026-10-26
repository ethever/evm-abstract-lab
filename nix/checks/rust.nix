# Each package owns tests, doctests, lint and API documentation. Matrix aliases
# remain public checks and share derivations when resolved features are equal.
{
  pkgs,
  craneLib,
  build,
  sourceRoot,
  ...
}:
let
  lib = pkgs.lib;
  aggregateCheck = import ../check-aggregate.nix { inherit pkgs; };
  forVariant =
    prefix: variant:
    let
      # Clippy's workspace wrapper has its own fingerprints. Share linted local
      # metadata along the DAG instead of relinting core for each consumer.
      clippyArtifacts = build.mkArtifacts {
        inherit (variant) mode configuration;
        profile = "test";
        libraryCommand = args: "cargoWithProfile clippy ${args.cargoExtraArgs} --lib -- -D warnings";
      };
      docMode = variant.mode // {
        kind = "build";
        sourceKind = "build";
      };
      docArtifacts = build.mkArtifacts {
        mode = docMode;
        configuration = variant.configuration;
        profile = build.configurations.libraryProfileFor "test";
        baseCommand = args: "cargoWithProfile check ${args.cargoExtraArgs}";
        libraryCommand = args: "cargoWithProfile check ${args.cargoExtraArgs} --lib";
      };
      forPackage =
        name:
        let
          args = build.commonFor name variant.mode variant.configuration // {
            CARGO_PROFILE = "test";
            cargoArtifacts = variant.artifacts.forPackage name;
          };
          compiledTests = craneLib.mkCargoDerivation (
            args
            // {
              pname = "${name}-compiled-tests";
              installCargoArtifactsMode = "use-symlink";
              doCheck = false;
              buildPhaseCargoCommand = "cargoWithProfile test ${args.cargoExtraArgs} --all-targets --no-run";
            }
          );
          testArgs = args // {
            cargoArtifacts = compiledTests;
            doInstallCargoArtifacts = false;
          };
        in
        {
          "${prefix}tests-${name}" = craneLib.cargoTest (
            testArgs
            // {
              cargoTestExtraArgs = "--all-targets -- --include-ignored";
            }
          );
          # --all-targets excludes doctests; keep ignored examples explicit.
          "${prefix}doctests-${name}" =
            if build.sources.hasDoctests name then
              craneLib.cargoDocTest (
                testArgs
                // {
                  cargoTestExtraArgs = "-- --include-ignored";
                }
              )
            else
              pkgs.runCommand "${name}-no-doctests" { } ''mkdir -p "$out"'';
          "${prefix}clippy-${name}" = craneLib.cargoClippy (
            args
            // {
              cargoClippyExtraArgs = "--all-targets -- -D warnings";
              cargoArtifacts = clippyArtifacts.forPackage name;
              doInstallCargoArtifacts = false;
            }
          );
          "${prefix}docs-${name}" =
            if build.sources.hasDocs name then
              craneLib.cargoDoc (
                (build.commonFor name docMode variant.configuration)
                // {
                  CARGO_PROFILE = "test";
                  cargoArtifacts = docArtifacts.forPackage name;
                  RUSTDOCFLAGS = "-D warnings";
                }
              )
            else
              pkgs.runCommand "${name}-no-doc-targets" { } ''mkdir -p "$out"'';
        };
      checks = lib.foldl' (result: name: result // forPackage name) { } build.sources.names;
      aggregate =
        kind:
        aggregateCheck {
          name = "${prefix}${kind}";
          checks = lib.genAttrs (map (name: "${prefix}${kind}-${name}") build.sources.names) (
            name: checks.${name}
          );
        };
    in
    checks
    // lib.genAttrs (map (kind: "${prefix}${kind}") [
      "tests"
      "doctests"
      "clippy"
      "docs"
    ]) (name: aggregate (lib.removePrefix prefix name));
  forRelease =
    prefix: variant:
    let
      checks = lib.mapAttrs' (
        name: package: lib.nameValuePair "${prefix}release-build-${name}" package
      ) variant.packages;
    in
    checks
    // {
      "${prefix}release-build" = aggregateCheck {
        name = "${prefix}release-build";
        inherit checks;
      };
    };
in
forVariant "" build.test
// forVariant "imbl-" build.imblTest
// forVariant "all-features-" build.allFeaturesTest
// forRelease "" build.release
// forRelease "imbl-" build.imblRelease
// (import ../tests/cargo-feature-contexts.nix {
  inherit
    pkgs
    craneLib
    build
    sourceRoot
    ;
})
// {
  cargo-bindgen-environment = import ../tests/cargo-bindgen.nix { inherit pkgs craneLib build; };
  cargo-vendor-environment = import ../tests/cargo-vendor.nix { inherit pkgs craneLib build; };
  cargo-concurrency =
    (import ../tests/cargo-concurrency.nix { inherit pkgs craneLib; }) "cargo-concurrency"
      { inherit (build.common) prePatch; };
  cargo-feature-resolution =
    assert import ../tests/cargo-configurations.nix {
      inherit pkgs;
      inherit (build) sources;
    };
    assert import ../tests/wasm-feature-scope.nix { inherit pkgs; };
    pkgs.runCommand "cargo-feature-resolution" { } ''mkdir -p "$out"'';
  fmt = craneLib.cargoFmt (
    build.common
    // {
      src = build.sources.formatting;
      cargoExtraArgs = "--all";
    }
  );
}
