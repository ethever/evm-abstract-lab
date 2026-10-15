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
    build.commonFor name mode configuration
    // workspaceCommon
    // {
      # Preserve the precise configured source/selection after applying nightly
      # environment defaults, whose broad workspace source is for vendoring only.
      inherit (build.commonFor name mode configuration) src;
      cargoExtraArgs = (build.commonFor name mode configuration).cargoExtraArgs + " --offline";
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
        # commonFor owns --offline and the shared test cache adds --all-targets.
        # Cargo rejects either flag when it occurs twice in one invocation.
        baseExtraArgs = "--target-dir ${targetDir}";
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
                    DYLINT_SKIP_DOCTEST_CONTROLS=1 bash scripts/test-no-dyn-doctests.sh -p ${lib.escapeShellArg name} ${build.configurations.featureArgs name configuration}
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
  webCommon =
    commonFor "evm-abstract-web" {
      kind = "build";
      target = "wasm";
    } build.configurations.default
    // {
      pname = "evm-abstract-web-no-dyn";
      CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
      CARGO_PROFILE = "dev";
      nativeBuildInputs = [ packages.tools ];
      buildInputs = [ ];
      cargoExtraArgs = "--locked --offline -p evm-abstract-web --lib --target-dir ${targetDir}";
      doCheck = false;
    };
  webArtifacts = lintCrane.buildDepsOnly (
    (builtins.removeAttrs webCommon [ "src" ])
    // {
      dummySrc = build.configurations.configuredDummy {
        kind = "build";
        target = "wasm";
      } build.configurations.default;
    }
  );
in
forVariant "" build.configurations.default
// forVariant "imbl-" build.configurations.imbl
// forVariant "all-features-" build.configurations.allFeatures
// {
  no-dyn-doctest-controls = doctestControls;
  web-no-dyn = lintCrane.mkCargoDerivation (
    webCommon
    // {
      cargoArtifacts = webArtifacts;
      doInstallCargoArtifacts = false;
      buildPhaseCargoCommand = ''
        cargo dylint --all --fail-on-no-libraries --no-build --no-metadata \
          --lib-path ${packages.libraryPath} -- --locked --offline \
          -p evm-abstract-web --lib --target wasm32-unknown-unknown
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
  no-dyn-fmt = lintCrane.cargoFmt (packages.lintCommon // { cargoExtraArgs = "--all"; });
}
