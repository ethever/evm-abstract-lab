# Each package owns tests, doctests, lint and API documentation. Matrix aliases
# remain public checks and share derivations when resolved features are equal.
{
  pkgs,
  craneLib,
  build,
  ...
}:
let
  lib = pkgs.lib;
  aggregateCheck = import ../check-aggregate.nix { inherit pkgs; };
  forVariant =
    prefix: variant:
    let
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
              doInstallCargoArtifacts = false;
            }
          );
          "${prefix}docs-${name}" =
            if build.sources.hasDocs name then
              craneLib.cargoDoc (
                args
                // {
                  inherit (build.commonFor name (variant.mode // { sourceKind = "build"; }) variant.configuration)
                    src
                    ;
                  cargoArtifacts = variant.artifacts.libraries.${name};
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
// {
  cargo-feature-resolution =
    assert import ../tests/cargo-configurations.nix {
      inherit pkgs;
      inherit (build) sources;
    };
    pkgs.runCommand "cargo-feature-resolution" { } ''mkdir -p "$out"'';
  fmt = craneLib.cargoFmt (
    build.common
    // {
      src = build.sources.formatting;
      cargoExtraArgs = "--all";
    }
  );
}
