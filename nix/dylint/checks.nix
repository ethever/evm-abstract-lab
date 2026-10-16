# The nightly compiler has its own artifact DAG. Package lint/check and doctest
# boundaries mirror stable checks without mixing incompatible compiler caches.
{
  pkgs,
  common,
  toolchain,
  packages,
  build,
}:
let
  lib = pkgs.lib;
  aggregateCheck = import ../check-aggregate.nix { inherit pkgs; };
  lintCrane = toolchain.craneLib;
  targetDir = "target/dylint/target/${toolchain.name}";
  doctestScript = builtins.path {
    path = common.src.origSrc + "/scripts/test-no-dyn-doctests.sh";
    name = "test-no-dyn-doctests.sh";
  };
  environment = toolchain.environment packages.driver;
  workspaceCommon =
    common
    // environment
    // {
      CARGO_NET_OFFLINE = "true";
      CARGO_PROFILE = "dev";
      nativeBuildInputs = common.nativeBuildInputs ++ [
        packages.tools
        pkgs.pkg-config
        pkgs.jq
      ];
      buildInputs = common.buildInputs ++ [ pkgs.openssl ];
      OPENSSL_NO_VENDOR = "1";
      preConfigure = toolchain.enter packages.tools;
    };
  commonFor =
    name: mode: configuration:
    let
      base = build.commonFor name mode configuration;
    in
    base
    // environment
    // {
      # Preserve the target-specific environment as well as source selection:
      # applying the broad native common would put SMT libraries into Wasm drvs.
      CARGO_NET_OFFLINE = "true";
      CARGO_PROFILE = "dev";
      nativeBuildInputs = base.nativeBuildInputs ++ [
        packages.tools
        pkgs.pkg-config
        pkgs.jq
      ];
      buildInputs = base.buildInputs ++ [ pkgs.openssl ];
      OPENSSL_NO_VENDOR = "1";
      preConfigure = toolchain.enter packages.tools;
      cargoExtraArgs = base.cargoExtraArgs + " --offline";
    };
  dylint = args: targets: ''
    cargo dylint --all --fail-on-no-libraries --no-build --no-metadata \
      --lib-path ${packages.libraryPath} -- ${args.cargoExtraArgs} ${targets}
  '';
  doctestControls =
    pkgs.runCommand "no-dyn-doctest-controls"
      (
        environment
        // {
          nativeBuildInputs = [
            packages.tools
            pkgs.jq
            pkgs.stdenv.cc
          ];
        }
      )
      ''
        ${toolchain.enter packages.tools}
        mkdir -p scripts
        cp ${doctestScript} scripts/test-no-dyn-doctests.sh
        export DYLINT_TOOLCHAIN='${toolchain.channel}'
        export DYLINT_DRIVER='${packages.driver}/lib/dylint/${toolchain.name}/dylint-driver'
        export DYLINT_LIBRARY='${packages.libraryPath}'
        DYLINT_DOCTEST_CONTROLS_ONLY=1 bash scripts/test-no-dyn-doctests.sh
        mkdir -p "$out"
      '';
  forVariant =
    prefix: configuration:
    let
      mode = {
        kind = "test";
        target = "native";
      };
      artifacts = build.mkArtifacts {
        inherit configuration mode commonFor;
        craneLib = lintCrane;
        common = workspaceCommon;
        profile = "dev";
        # The shared registry command owns its package/feature selection, while
        # test mode adds --all-targets. Each CLI flag must occur only once.
        baseExtraArgs = "--offline --target-dir ${targetDir}";
        libraryCommand =
          args:
          dylint args "--lib"
          + ''
            cargoWithProfile build ${args.cargoExtraArgs} --lib --target-dir ${targetDir}
          '';
      };
      forPackage =
        name:
        let
          args = commonFor name mode configuration // {
            cargoArtifacts = artifacts.forPackage name;
            doInstallCargoArtifacts = false;
          };
          lint = lintCrane.mkCargoDerivation (
            args
            // {
              pname = "${name}-no-dyn";
              doCheck = false;
              buildPhaseCargoCommand = dylint args "--all-targets";
            }
          );
          docs =
            if build.sources.hasDoctests name then
              lintCrane.mkCargoDerivation (
                args
                // {
                  pname = "${name}-no-dyn-doctests";
                  inherit doctestControls;
                  doCheck = false;
                  buildPhaseCargoCommand = ''
                    mkdir -p scripts
                    cp ${doctestScript} scripts/test-no-dyn-doctests.sh
                    export DYLINT_TOOLCHAIN='${toolchain.channel}'
                    export DYLINT_DRIVER='${packages.driver}/lib/dylint/${toolchain.name}/dylint-driver'
                    export DYLINT_LIBRARY='${packages.libraryPath}'
                    export CARGO_TARGET_DIR="$PWD/${targetDir}"
                    DYLINT_SKIP_DOCTEST_CONTROLS=1 bash scripts/test-no-dyn-doctests.sh ${
                      build.configurations.packageArgs name mode configuration
                    }
                  '';
                }
              )
            else
              pkgs.runCommand "${name}-no-dyn-no-doctests" { } ''mkdir -p "$out"'';
        in
        {
          "${prefix}no-dyn-${name}" = lint;
          "${prefix}no-dyn-doctests-${name}" = docs;
        };
      checks = lib.foldl' (result: name: result // forPackage name) { } build.sources.names;
    in
    checks
    // {
      "${prefix}no-dyn" = aggregateCheck {
        name = "${prefix}no-dyn";
        inherit checks;
      };
    };
  webMode = {
    kind = "build";
    target = "wasm";
    featureRoots = [ "evm-abstract-web" ];
  };
  webPackageArgs = "--locked --offline ${
    build.configurations.packageArgs "evm-abstract-web" webMode build.configurations.default
  } --lib";
  webCommon = commonFor "evm-abstract-web" webMode build.configurations.default // {
    pname = "evm-abstract-web-no-dyn";
    CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
    CARGO_PROFILE = "dev";
    nativeBuildInputs = [ packages.tools ];
    buildInputs = [ ];
    cargoExtraArgs = "${webPackageArgs} --target-dir ${targetDir}";
    doCheck = false;
  };
  webArtifacts = lintCrane.buildDepsOnly (
    (builtins.removeAttrs webCommon [ "src" ])
    // {
      dummySrc = build.configurations.configuredDummy webMode build.configurations.default;
    }
  );
in
forVariant "" build.configurations.default
// forVariant "imbl-" build.configurations.imbl
// forVariant "all-features-" build.configurations.allFeatures
// {
  dylint-driver-concurrency =
    (import ../tests/cargo-concurrency.nix {
      inherit pkgs;
      craneLib = lintCrane;
    })
      "dylint-driver-concurrency"
      { inherit (packages.driver) prePatch postPatch; };
  no-dyn-doctest-controls = doctestControls;
  web-no-dyn = lintCrane.mkCargoDerivation (
    webCommon
    // {
      cargoArtifacts = webArtifacts;
      doInstallCargoArtifacts = false;
      buildPhaseCargoCommand = ''
        # Dylint owns --target-dir; only plain Cargo dependency preparation
        # receives that flag explicitly through webCommon.
        cargo dylint --all --fail-on-no-libraries --no-build --no-metadata \
          --lib-path ${packages.libraryPath} -- ${webPackageArgs} --target wasm32-unknown-unknown
      '';
    }
  );
  no-dyn-ui = lintCrane.mkCargoDerivation (
    packages.lintCommon
    // {
      cargoArtifacts = packages.testArtifacts;
      CARGO_PROFILE = "test";
      doInstallCargoArtifacts = false;
      doCheck = true;
      buildPhaseCargoCommand = "";
      checkPhaseCargoCommand = "cargoWithProfile test --locked --offline -- --include-ignored";
    }
  );
  no-dyn-fmt = lintCrane.cargoFmt (
    packages.lintCommon
    // {
      src = packages.formattingSource;
      cargoExtraArgs = "--all";
    }
  );
}
