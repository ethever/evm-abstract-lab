# Cargo, rather than a second registry-feature resolver, owns transitive feature
# unification. A selected package with only a disabled binary contributes its
# dependency requests to Cargo's resolver without producing compilation units.
{
  pkgs,
  sources,
  craneLib ? null,
  cargoVendorDir ? null,
}:
{ mode, configuration }:
let
  lib = pkgs.lib;
  name = "nix-feature-unifier";
  directory = "__nix-feature-unifier";
  roots =
    mode.featureRoots or (if mode.target == "wasm" then [ "evm-abstract-web" ] else sources.names);
  selected = sources.closure roots (mode // { inherit configuration; });
  sorted = values: lib.sort builtins.lessThan (lib.unique values);
  groups = lib.foldl' (
    result: package:
    lib.foldl'
      (
        current: dependency:
        if
          dependency.name != null
          || !(lib.elem dependency.alias configuration.packages.${package}.dependencies)
        then
          current
        else
          let
            spec = dependency.spec;
            packageName = spec.package or dependency.alias;
            source = builtins.removeAttrs spec [
              "package"
              "features"
              "default-features"
              "optional"
            ];
            kind = if dependency.kind == "build" then "build-dependencies" else "dependencies";
            predicate = dependency.predicate or null;
            key = builtins.toJSON {
              inherit
                packageName
                source
                kind
                predicate
                ;
            };
            previous =
              current.${key} or {
                features = [ ];
                defaults = false;
              };
          in
          current
          // {
            ${key} = {
              inherit
                packageName
                source
                kind
                predicate
                ;
              features = sorted (
                previous.features
                ++ (spec.features or [ ])
                ++ (configuration.packages.${package}.requests.${dependency.alias} or [ ])
              );
              defaults = previous.defaults || (spec.default-features or true);
            };
          }
      )
      result
      (
        sources.dependencies package (
          mode
          // {
            kind = if lib.elem package roots then mode.kind else "build";
          }
        )
      )
  ) { } selected;
  tables = lib.foldl' (
    result: key:
    let
      group = groups.${key};
      aliasKey = builtins.toJSON (
        builtins.removeAttrs group [
          "features"
          "defaults"
          "predicate"
        ]
      );
      alias = "${group.packageName}_${builtins.substring 0 12 (builtins.hashString "sha256" aliasKey)}";
      dependency = group.source // {
        package = group.packageName;
        features = group.features;
        default-features = group.defaults;
      };
      path =
        (lib.optionals (group.predicate != null) [
          "target"
          group.predicate
        ])
        ++ [
          group.kind
          alias
        ];
    in
    lib.recursiveUpdate result (lib.setAttrByPath path dependency)
  ) { } (builtins.attrNames groups);
  manifest = tables // {
    package = {
      inherit name;
      version = "0.0.0";
      edition = "2024";
      build = "build.rs";
      publish = false;
      autobins = false;
      autotests = false;
      autoexamples = false;
      autobenches = false;
    };
    features.nix-unifier-disabled = [ ];
    bin = [
      {
        inherit name;
        path = "main.rs";
        required-features = [ "nix-unifier-disabled" ];
        test = false;
        bench = false;
        doc = false;
      }
    ];
  };
  writer = pkgs.formats.toml { };
  manifestFile = writer.generate "feature-unifier-Cargo.toml" manifest;
  rootManifest = writer.generate "Cargo.toml" (
    sources.rootManifest
    // {
      workspace = sources.rootManifest.workspace // {
        members = sources.rootManifest.workspace.members ++ [ directory ];
      };
    }
  );
  # This overlay depends only on Cargo metadata and generated target stubs. A
  # Rust body/fixture edit never reruns lock resolution or changes this output.
  overlay =
    pkgs.runCommand "cargo-feature-unifier"
      {
        nativeBuildInputs = [
          craneLib.cargo
          craneLib.rustc
          pkgs.python3
        ];
      }
      ''
        mkdir work
        cp -R ${sources.dummy}/. work/
        chmod -R u+w work
        cp ${rootManifest} work/Cargo.toml
        mkdir -p work/${directory}
        cp ${manifestFile} work/${directory}/Cargo.toml
        printf 'fn main() {}\n' > work/${directory}/main.rs
        cp work/${directory}/main.rs work/${directory}/build.rs
        export CARGO_HOME="$TMPDIR/cargo-home"
        (cd work; cargo --config ${cargoVendorDir}/config.toml metadata --offline --format-version 1 > /dev/null)
        python - ${sources.dummy}/Cargo.lock work/Cargo.lock <<'PY'
        import sys
        import tomllib
        def registry_packages(path):
            with open(path, 'rb') as source:
                return [package for package in tomllib.load(source)['package'] if 'source' in package]
        assert registry_packages(sys.argv[1]) == registry_packages(sys.argv[2]), 'feature context changed locked registry versions'
        PY
        mkdir -p "$out"
        cp work/Cargo.toml work/Cargo.lock "$out/"
        cp -R work/${directory} "$out/"
      '';
in
{
  inherit
    name
    directory
    manifest
    overlay
    roots
    ;
}
