# The browser target has no native SMT or desktop-window dependencies.
{
  pkgs,
  craneLib,
  sourceRoot,
  build,
  dependencies,
}:
let
  lib = pkgs.lib;
  lockedPackages = (builtins.fromTOML (builtins.readFile (sourceRoot + "/Cargo.lock"))).package;
  wasmBindgenVersion =
    (lib.findFirst (package: package.name == "wasm-bindgen") null lockedPackages).version;
  wasmBindgen =
    assert lib.assertMsg (wasmBindgenVersion == pkgs.wasm-bindgen-cli.version)
      "Cargo.lock wasm-bindgen ${wasmBindgenVersion} must match Nix wasm-bindgen-cli ${pkgs.wasm-bindgen-cli.version}";
    pkgs.wasm-bindgen-cli;
  common = {
    inherit (build.common) src cargoVendorDir;
    pname = "evm-abstract-web";
    inherit (build.common) version;
    strictDeps = true;
    CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
    CARGO_PROFILE = "release";
    cargoExtraArgs = "--locked -p evm-abstract-web";
    doCheck = false;
  };
  cargoArtifacts = craneLib.buildDepsOnly common;
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
  # Editing the HTML shell only repackages assets; it does not recompile Rust.
  index = builtins.path {
    path = sourceRoot + "/crates/evm-abstract-web/index.html";
    name = "evm-abstract-index.html";
  };
  assets = pkgs.runCommand "evm-abstract-web-assets" { nativeBuildInputs = [ wasmBindgen ]; } ''
    mkdir -p "$out"
    wasm-bindgen --target web --out-name evm_abstract_web --out-dir "$out" \
      ${wasm}/evm_abstract_web.wasm
    cp ${index} "$out/index.html"
  '';
  fastCommon = common // {
    CARGO_PROFILE = "dev";
  };
  fastArtifacts = craneLib.buildDepsOnly fastCommon;
  launcher = pkgs.writeShellApplication {
    name = "evm-abstract-web";
    text = ''
      exec ${build.release.package}/bin/evm-abstract-server --assets ${assets} "$@"
    '';
  };
  browserPython = pkgs.python3.withPackages (python: [ python.playwright ]);
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
  }
  // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
    web-browser =
      pkgs.runCommand "evm-abstract-web-browser"
        {
          nativeBuildInputs = [ browserPython ];
          # egui bundles canvas fonts, but Chromium's hidden IME input still
          # needs a system font to insert text inside the isolated Nix builder.
          FONTCONFIG_FILE = dependencies.fontConfig;
        }
        ''
          # Chromium's crash reporter resolves its database through XDG even
          # when Playwright supplies a temporary browser profile.
          export XDG_CONFIG_HOME="$TMPDIR/browser-config"
          export XDG_CACHE_HOME="$TMPDIR/browser-cache"
          mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME"
          python ${sourceRoot}/scripts/test-web-browser.py \
            --server ${build.release.package}/bin/evm-abstract-server \
            --assets ${assets} --browser ${lib.getExe pkgs.chromium} --output "$out"
        '';
  };
  devPackages = [ wasmBindgen ];
}
