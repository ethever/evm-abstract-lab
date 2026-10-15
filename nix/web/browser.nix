# Each manifest case is an independent Nix node. Nix owns parallel scheduling;
# compatibility aliases reference those same nodes and never execute tests twice.
{
  pkgs,
  sourceRoot,
  build,
  dependencies,
  assets,
  ...
}:
let
  lib = pkgs.lib;
  inputs = import ../check-inputs.nix { inherit lib sourceRoot; };
  aggregate = import ../check-aggregate.nix { inherit pkgs; };
  manifest = builtins.fromJSON (builtins.readFile (sourceRoot + "/scripts/web_test_cases.json"));
  cases = manifest.cases;
  browserPython = pkgs.python3.withPackages (python: [ python.playwright ]);
  caseCheck =
    specification:
    let
      # Project exactly one spec: unrelated registry entries and helper modules
      # cannot alter this derivation. Registry completeness is checked separately.
      source = inputs.files specification.dependencies;
      projection = pkgs.writeText "browser-case.json" (
        builtins.toJSON {
          schema_version = manifest.schema_version;
          cases = [ specification ];
          legacy_flags = { };
        }
      );
    in
    pkgs.runCommand "evm-abstract-web-browser-${specification.id}"
      {
        nativeBuildInputs = [
          browserPython
          pkgs.xvfb-run
        ];
        FONTCONFIG_FILE = dependencies.fontConfig;
        inherit (dependencies.environment) SSL_CERT_FILE;
      }
      ''
        export XDG_CONFIG_HOME="$TMPDIR/browser-config"
        export XDG_CACHE_HOME="$TMPDIR/browser-cache"
        export PYTHONDONTWRITEBYTECODE=1
        mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME"
        xvfb-run -a python ${source}/scripts/test-web-browser.py \
          --manifest ${projection} --case ${lib.escapeShellArg specification.id} \
          --server ${build.release.packages.evm-abstract-server}/bin/evm-abstract-server \
          --assets ${assets} --browser ${lib.getExe pkgs.chromium} --output "$out"
      '';
  leaves = lib.listToAttrs (
    map (
      specification: lib.nameValuePair "web-browser-${specification.id}" (caseCheck specification)
    ) cases
  );
  registrySource = inputs.files (
    lib.unique (
      [
        "scripts/web_test_cases.json"
        "scripts/test_web_test_registry.py"
      ]
      # The global completeness check must see newly added, unregistered case
      # modules too. Individual case sources remain the exact manifest closure.
      ++ map (name: "scripts/${name}") (
        lib.filter (name: builtins.match "web_test_.*\\.py" name != null) (
          builtins.attrNames (builtins.readDir (sourceRoot + "/scripts"))
        )
      )
      ++ lib.concatMap (specification: specification.dependencies) cases
    )
  );
in
{
  web-browser-registry =
    pkgs.runCommand "evm-abstract-web-browser-registry"
      {
        nativeBuildInputs = [ pkgs.python3 ];
      }
      ''
        python -B ${registrySource}/scripts/test-web-browser.py --validate-manifest
        python -B ${registrySource}/scripts/test_web_test_registry.py
        touch "$out"
      '';
}
// lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux (
  leaves
  // {
    web-browser = aggregate {
      name = "evm-abstract-web-browser";
      checks = leaves;
    };
    web-compression = leaves.web-browser-compression;
  }
)
