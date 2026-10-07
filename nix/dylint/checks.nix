# Keep the workspace targets, doctest controls and lint UI tests as separate checks.
{
  pkgs,
  common,
  toolchain,
  packages,
}:
let
  lib = pkgs.lib;
  lintCrane = toolchain.craneLib;
  workspaceCommon =
    common
    // toolchain.environment packages.driver
    // {
      src = lib.cleanSourceWith {
        src = common.src.origSrc;
        filter = path: type: common.src.filter path type || lib.hasSuffix ".sh" path;
      };
      CARGO_NET_OFFLINE = "true";
      nativeBuildInputs = common.nativeBuildInputs ++ [
        packages.tools
        pkgs.pkg-config
        pkgs.jq
      ];
      buildInputs = common.buildInputs ++ [ pkgs.openssl ];
      OPENSSL_NO_VENDOR = "1";
      preConfigure = toolchain.enter packages.tools;
    };
  workspaceArtifacts = lintCrane.buildDepsOnly (
    workspaceCommon
    // {
      # Match the compiler, vendoring and native environment used by cargo dylint.
      # Its target directory is nested beneath Cargo's normal target directory.
      src = lib.cleanSourceWith {
        src = common.src;
        filter = path: type: baseNameOf path != "lints";
      };
      CARGO_PROFILE = "dev";
      cargoExtraArgs = "--workspace --locked --offline --all-targets --all-features --target-dir target/dylint/target/${toolchain.name}";
      doCheck = false;
    }
  );
  workspaceCheck =
    name: features:
    lintCrane.mkCargoDerivation (
      workspaceCommon
      // {
        pname = name;
        cargoArtifacts = workspaceArtifacts;
        doInstallCargoArtifacts = false;
        buildPhaseCargoCommand = ''
          cargo dylint --all --workspace --fail-on-no-libraries --no-build --no-metadata \
            --lib-path ${packages.libraryPath} -- --locked --offline --all-targets ${features}
          export DYLINT_TOOLCHAIN='${toolchain.channel}'
          export DYLINT_DRIVER='${packages.driver}/lib/dylint/${toolchain.name}/dylint-driver'
          export DYLINT_LIBRARY='${packages.libraryPath}'
          export CARGO_TARGET_DIR="$PWD/target/dylint/target/${toolchain.name}"
          bash scripts/test-no-dyn-doctests.sh ${features}
        '';
      }
    );
in
{
  no-dyn = workspaceCheck "evm-abstract-no-dyn" "";
  imbl-no-dyn = workspaceCheck "evm-abstract-imbl-no-dyn" "--features imbl";
  all-features-no-dyn = workspaceCheck "evm-abstract-all-features-no-dyn" "--all-features";
  no-dyn-ui = lintCrane.mkCargoDerivation (
    packages.lintCommon
    // {
      cargoArtifacts = null;
      doInstallCargoArtifacts = false;
      doCheck = true;
      buildPhaseCargoCommand = "cargo build --locked --offline";
      checkPhaseCargoCommand = "cargo test --locked --offline";
    }
  );
  no-dyn-fmt = lintCrane.cargoFmt (packages.lintCommon // { cargoExtraArgs = "--all"; });
}
