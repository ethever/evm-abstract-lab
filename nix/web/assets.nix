# Only the Web build dependency closure enters Wasm compilation. HTML changes
# repackage assets without invalidating Rust or the wasm-bindgen transformation.
{
  pkgs,
  craneLib,
  sourceRoot,
  build,
  ...
}:
let
  lib = pkgs.lib;
  inputs = import ../check-inputs.nix { inherit lib sourceRoot; };
  lockedPackages = (builtins.fromTOML (builtins.readFile (sourceRoot + "/Cargo.lock"))).package;
  wasmBindgenVersion =
    (lib.findFirst (package: package.name == "wasm-bindgen") null lockedPackages).version;
  wasmBindgen =
    assert lib.assertMsg (wasmBindgenVersion == pkgs.wasm-bindgen-cli.version)
      "Cargo.lock wasm-bindgen ${wasmBindgenVersion} must match Nix wasm-bindgen-cli ${pkgs.wasm-bindgen-cli.version}";
    pkgs.wasm-bindgen-cli;
  mode = {
    kind = "build";
    target = "wasm";
    # Predecessor libraries use the features resolved by the original Web root.
    featureRoots = [ "evm-abstract-web" ];
  };
  configuration = build.configurations.default;
  # The same dependency DAG shares notation/protocol/layout compilation. Target
  # and compiler/profile boundaries remain explicit throughout that graph.
  wasmCommonFor =
    name: selectedMode: selectedConfiguration:
    build.commonFor name selectedMode selectedConfiguration
    // {
      CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
    };
  common = wasmCommonFor "evm-abstract-web" mode configuration // {
    CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
    CARGO_PROFILE = "release";
    doCheck = false;
  };
  dependencyArtifacts =
    profile:
    (build.mkArtifacts {
      roots = [ "evm-abstract-web" ];
      inherit mode configuration profile;
      commonFor = wasmCommonFor;
    }).forPackage
      "evm-abstract-web";
  cargoArtifacts = dependencyArtifacts "release";
  wasm = craneLib.buildPackage (
    common
    // {
      inherit cargoArtifacts;
      installPhaseCommand = ''
        mkdir -p "$out"
        cp target/wasm32-unknown-unknown/release/evm_abstract_web.wasm "$out/"
      '';
      meta = {
        description = "Optimized egui Wasm module for EVM disassembly, CFG and SSA";
        license = lib.licenses.mit;
      };
    }
  );
  bindings = pkgs.runCommand "evm-abstract-web-bindings" { nativeBuildInputs = [ wasmBindgen ]; } ''
    mkdir "$out"
    wasm-bindgen --target web --out-name evm_abstract_web --out-dir "$out" ${wasm}/evm_abstract_web.wasm
  '';
  index = inputs.file "crates/evm-abstract-web/index.html";
  assets = pkgs.runCommand "evm-abstract-web-assets" { } ''
    mkdir "$out"
    cp ${bindings}/* "$out/"
    cp ${index} "$out/index.html"
  '';
  fastCommon = common // {
    CARGO_PROFILE = "dev";
  };
  fastArtifacts = dependencyArtifacts "dev";
  launcher = pkgs.writeShellApplication {
    name = "evm-abstract-web";
    text = ''
      exec ${build.release.packages.evm-abstract-server}/bin/evm-abstract-server --assets ${assets} "$@"
    '';
  };
in
{
  inherit
    common
    cargoArtifacts
    wasm
    assets
    fastCommon
    fastArtifacts
    ;
  packages = {
    web-assets = assets;
    web = launcher;
  };
  apps.web = {
    type = "app";
    program = lib.getExe launcher;
    meta.description = "Serve the egui application and typed analysis API";
  };
  checks = {
    web-build = assets;
    web-clippy = craneLib.cargoClippy (
      fastCommon
      // {
        cargoArtifacts = fastArtifacts;
        cargoClippyExtraArgs = "--lib -- -D warnings";
      }
    );
  };
  devPackages = [ wasmBindgen ];
}
