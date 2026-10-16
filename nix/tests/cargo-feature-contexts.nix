# Cargo's unit graph resolves real registry features without compiling crates.
# Both sides use manifest/lock/virtual-target sources: Rust body edits therefore
# preserve every check below. Each operation/matrix is independently scheduled;
# matrices with equal local resolution share a check and validate their raw
# Cargo flags as aliases before comparing the generated split invocations.
{
  pkgs,
  craneLib,
  build,
  sourceRoot,
  ...
}:
let
  lib = pkgs.lib;
  configurations = build.configurations;
  sources = build.sources;
  inputs = import ../check-inputs.nix { inherit lib sourceRoot; };
  harness = inputs.file "scripts/test_cargo_feature_context.py";
  relative =
    path:
    builtins.unsafeDiscardStringContext (lib.removePrefix "${toString sourceRoot}/" (toString path));
  normalize =
    path:
    lib.concatStringsSep "/" (
      lib.foldl' (
        parts: part:
        if part == "" || part == "." then
          parts
        else if part == ".." then
          lib.init parts
        else
          parts ++ [ part ]
      ) [ ] (lib.splitString "/" path)
    );
  # Only names enter this JSON. Virtual files retain implicit integration-test
  # and example discovery, while their original Rust bodies are never inputs.
  targetStubPaths = lib.unique (
    lib.concatMap (
      name:
      let
        package = sources.packages.${name};
        manifest = package.manifest;
        explicit = lib.concatMap (kind: map (target: target.path or null) (manifest.${kind} or [ ])) [
          "bin"
          "test"
          "bench"
          "example"
        ];
        declared = lib.filter builtins.isString (
          explicit
          ++ [
            (manifest.lib.path or null)
            (manifest.package.build or null)
          ]
        );
      in
      map relative (
        lib.filter (path: lib.hasSuffix ".rs" (toString path)) (
          lib.filesystem.listFilesRecursive (sourceRoot + "/${package.path}")
        )
      )
      ++ map (path: normalize "${package.path}/${path}") declared
    ) sources.names
  );
  originalManifests = lib.listToAttrs (
    [ (lib.nameValuePair "Cargo.toml" sources.rootManifest) ]
    ++ map (
      name:
      let
        package = sources.packages.${name};
      in
      lib.nameValuePair "${package.path}/Cargo.toml" package.manifest
    ) sources.names
  );
  variants = [
    {
      label = "default";
      configuration = configurations.default;
      originalArguments = "--workspace";
    }
    {
      label = "imbl";
      configuration = configurations.imbl;
      originalArguments = "--workspace --features imbl";
    }
    {
      label = "all-features";
      configuration = configurations.allFeatures;
      originalArguments = "--workspace --all-features";
    }
  ];
  contracts = [
    {
      name = "tests";
      kind = "test";
      operation = [
        "test"
        "--all-targets"
        "--no-run"
      ];
    }
    {
      name = "check";
      kind = "test";
      operation = [
        "check"
        "--all-targets"
        "--profile"
        "test"
      ];
    }
    {
      name = "doctests";
      kind = "test";
      operation = [
        "test"
        "--doc"
      ];
    }
    {
      name = "docs";
      kind = "build";
      operation = [
        "doc"
        "--no-deps"
        "--profile"
        "test"
      ];
    }
    {
      name = "release";
      kind = "build";
      operation = [
        "build"
        "--release"
      ];
    }
  ];
  common = {
    version = 1;
    cargo = "${craneLib.cargo}/bin/cargo";
    rustc = "${craneLib.rustc}/bin/rustc";
    rustdoc = "${craneLib.rustc}/bin/rustdoc";
    vendor_config = "${build.common.cargoVendorDir}/config.toml";
    package_paths = lib.mapAttrs (_: package: package.path) sources.packages;
    helper_package = "nix-feature-unifier";
    target_stub_paths = targetStubPaths;
    original_manifests = originalManifests;
  };
  makeCheck =
    specification:
    let
      file = pkgs.writeText "cargo-feature-context.json" (builtins.toJSON (common // specification));
    in
    pkgs.runCommand specification.name { nativeBuildInputs = [ pkgs.python3 ]; } ''
      mkdir -p "$out"
      python -B ${harness} --spec ${file} --output "$out/report.json"
    '';
  native =
    contract: variant:
    let
      mode = {
        target = "native";
        inherit (contract) kind;
      };
      signature = (configurations.forMode mode variant.configuration).signature;
      aliases = lib.filter (
        candidate: (configurations.forMode mode candidate.configuration).signature == signature
      ) variants;
      canonical = builtins.head aliases;
      packages =
        if contract.name == "docs" then
          lib.filter sources.hasDocs sources.names
        else if contract.name == "doctests" then
          lib.filter sources.hasDoctests sources.names
        else
          sources.names;
    in
    makeCheck {
      name = "cargo-context-native-${canonical.label}-${contract.name}";
      inherit (contract) operation;
      baselines = map (alias: {
        label = alias.label;
        source = toString sources.dummy;
        arguments = alias.originalArguments;
        original_manifests = true;
      }) aliases;
      candidates = map (package: {
        inherit package;
        source = toString (configurations.configuredDummyFor [ package ] mode canonical.configuration);
        arguments = configurations.packageArgs package mode canonical.configuration;
      }) packages;
    };
  nativeChecks = lib.listToAttrs (
    lib.concatMap (
      contract:
      map (
        variant:
        lib.nameValuePair "cargo-context-native-${variant.label}-${contract.name}" (native contract variant)
      ) variants
    ) contracts
  );
  wasm =
    command: profile:
    let
      mode = {
        target = "wasm";
        kind = "build";
        featureRoots = [ "evm-abstract-web" ];
      };
      configuration = configurations.default;
      resolved = configurations.forMode mode configuration;
      packages = lib.filter sources.hasLibrary (
        sources.closure [ "evm-abstract-web" ] (mode // { configuration = resolved; })
      );
    in
    makeCheck {
      name = "cargo-context-wasm-${command}-${profile}";
      operation = [
        command
        "--lib"
        "--profile"
        profile
      ];
      target = "wasm32-unknown-unknown";
      baselines = [
        {
          label = "original-web-root";
          source = toString sources.dummy;
          arguments = "-p evm-abstract-web";
          original_manifests = true;
        }
      ];
      candidates = map (package: {
        inherit package;
        library_subtree = package != "evm-abstract-web";
        source = toString (configurations.configuredDummyFor [ package ] mode configuration);
        arguments = configurations.packageArgs package mode configuration;
      }) packages;
    };
  wasmChecks = lib.listToAttrs (
    lib.concatMap
      (
        command:
        map (profile: lib.nameValuePair "cargo-context-wasm-${command}-${profile}" (wasm command profile)) [
          "dev"
          "release"
        ]
      )
      [
        "check"
        "build"
      ]
  );
in
nativeChecks // wasmChecks
