# Installed examples are independently cached acceptance cases. The aggregate
# only links their results; shared JSON analyses are produced by fixtures.nix.
{
  pkgs,
  sourceRoot,
  build,
  dependencies,
  ...
}:
let
  lib = pkgs.lib;
  aggregateCheck = import ../check-aggregate.nix { inherit pkgs; };
  fixtures = import ./fixtures.nix { inherit pkgs sourceRoot build; };
  inputs = import ../check-inputs.nix { inherit lib sourceRoot; };
  package = build.release.packages.evm-abstract-cli;
  mkCheck =
    name: tools: environment: script:
    pkgs.runCommand "evm-abstract-example-${name}"
      (
        {
          nativeBuildInputs = [
            package
            pkgs.jq
          ]
          ++ tools;
        }
        // environment
      )
      ''
        set -euo pipefail
        export XDG_CACHE_HOME="$TMPDIR"
        mkdir "$out"
        ${script}
      '';
  args = {
    inherit
      pkgs
      lib
      fixtures
      inputs
      package
      mkCheck
      ;
    inherit (fixtures) concrete;
    inherit (dependencies) fontConfig;
    hexFixtures = lib.filter (lib.hasSuffix ".hex") (
      builtins.attrNames (builtins.readDir (sourceRoot + "/examples"))
    );
  };
  groups = map (path: import path args) [
    ./examples/worlds.nix
    ./examples/storage.nix
    ./examples/unknown.nix
    ./examples/semantics.nix
    ./examples/explain.nix
    ./examples/environment.nix
    ./examples/programs.nix
    ./examples/hex.nix
    ./examples/graphs.nix
  ];
  checks = lib.foldl' (result: group: result // group.checks) { } groups;
  artifacts = lib.concatMap (group: group.artifacts or [ ]) groups;
in
checks
// {
  examples = aggregateCheck {
    name = "evm-abstract-installed-examples";
    inherit checks;
    links = artifacts;
  };
}
