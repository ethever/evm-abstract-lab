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
    src = lib.cleanSourceWith {
      src = sourceRoot;
      filter =
        path: type:
        build.common.src.filter path type
        || lib.hasSuffix "/crates/evm-abstract-web/index.html" (toString path);
    };
    pname = "evm-abstract-web";
    inherit (build.common) version;
    strictDeps = true;
    CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
    cargoExtraArgs = "--locked -p evm-abstract-web";
    doCheck = false;
  };
  cargoArtifacts = craneLib.buildDepsOnly common;
  assets = craneLib.buildPackage (
    common
    // {
      inherit cargoArtifacts;
      nativeBuildInputs = [ wasmBindgen ];
      installPhaseCommand = ''
        mkdir -p "$out"
        wasm-bindgen --target web --out-name evm_abstract_web --out-dir "$out" \
          target/wasm32-unknown-unknown/release/evm_abstract_web.wasm
        cp crates/evm-abstract-web/index.html "$out/index.html"
      '';
      meta = {
        description = "egui browser assets for EVM disassembly, CFG and SSA";
        license = lib.licenses.mit;
      };
    }
  );
  launcher = pkgs.writeShellApplication {
    name = "evm-abstract-web";
    text = ''
      exec ${build.package}/bin/evm-abstract-server --assets ${assets} "$@"
    '';
  };
  browserPython = pkgs.python3.withPackages (python: [ python.playwright ]);
in
{
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
      common
      // {
        inherit cargoArtifacts;
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
            --server ${build.package}/bin/evm-abstract-server \
            --assets ${assets} --browser ${lib.getExe pkgs.chromium} --output "$out"
        '';
  };
  devPackages = [ wasmBindgen ];
}
