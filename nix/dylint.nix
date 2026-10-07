{
  pkgs,
  crane,
  craneLib,
  common,
}:
let
  lib = pkgs.lib;
  lintRoot = ../lints/no_dyn;
  lintToolchain = pkgs.rust-bin.fromRustupToolchainFile (lintRoot + "/rust-toolchain.toml");
  lintCrane = (crane.mkLib pkgs).overrideToolchain lintToolchain;
  channel =
    (builtins.fromTOML (builtins.readFile (lintRoot + "/rust-toolchain.toml"))).toolchain.channel;
  target = pkgs.stdenv.hostPlatform.rust.rustcTarget;
  toolchainName = "${channel}-${target}";
  upstream = pkgs.fetchFromGitHub {
    owner = "trailofbits";
    repo = "dylint";
    rev = "8aeea4b0c27722d46fcc8288330401418328e8fa";
    hash = "sha256-KgEn3AZnITS6Uhc6CElCMqOucu+/Cc4w9Jm5oU+v5Iw=";
  };
  tools = craneLib.buildPackage {
    pname = "dylint-tools";
    version = "6.1.0";
    src = upstream;
    cargoExtraArgs = "--locked -p cargo-dylint -p dylint-link";
    doCheck = false;
    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [ pkgs.openssl ];
    OPENSSL_NO_VENDOR = "1";
  };
  # Dylint normally compiles its driver on first use. Build and retain it here so
  # the checks run without Cargo installation, rustup downloads or network access.
  driver = lintCrane.buildPackage {
    pname = "dylint-driver";
    version = "6.1.0";
    src = upstream;
    sourceRoot = "source/driver";
    cargoArtifacts = null;
    cargoVendorDir = lintCrane.vendorCargoDeps { cargoLock = upstream + "/driver/Cargo.lock"; };
    cargoExtraArgs = "--locked --target-dir target";
    doCheck = false;
    doNotRemoveReferencesToRustToolchain = true;
    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [ pkgs.openssl ];
    OPENSSL_NO_VENDOR = "1";
    # no_dyn does not use Clippy's additional symbols. The upstream driver uses
    # this channel spelling to skip fetching Clippy, while Nix pins the compiler.
    RUSTUP_TOOLCHAIN = "nightly-${target}";
    RUSTFLAGS = "-C link-arg=-Wl,-rpath,${lintToolchain}/lib";
    postPatch = ''
      cat > src/main.rs <<'EOF'
      #![feature(rustc_private)]
      fn main() -> anyhow::Result<()> {
          dylint_driver::dylint_driver(&std::env::args_os().collect::<Vec<_>>())
      }
      EOF
    '';
    installPhaseCommand = ''
      mkdir -p "$out/lib/dylint/${toolchainName}" "$out/toolchains"
      cp target/release/dylint_driver "$out/lib/dylint/${toolchainName}/dylint-driver"
      ln -s ${lintToolchain} "$out/toolchains/${toolchainName}"
    '';
  };
  # Dylint asks rustup for a toolchain's name and path even when Cargo/Rust are
  # supplied by Nix. Limit this adapter to those queries; Nix owns the sysroot.
  nixToolchain = pkgs.symlinkJoin {
    name = "dylint-nix-toolchain";
    paths = [
      (pkgs.writeShellScriptBin "rustup" ''
        case "$*" in
          'show active-toolchain') printf '%s\n' '${toolchainName} (Nix)' ;;
          'which rustc') printf '%s\n' '${lintToolchain}/bin/rustc' ;;
          'which cargo'|'+stable which cargo') printf '%s\n' '${lintToolchain}/bin/cargo' ;;
          *) echo "Unsupported Nix rustup query: $*" >&2; exit 1 ;;
        esac
      '')
      (pkgs.writeShellScriptBin "cargo" ''
        case "''${1-}" in
          '+${channel}'|'+${toolchainName}') shift ;;
          +*) echo "Nix Dylint requires ${channel}: $1" >&2; exit 1 ;;
        esac
        export RUSTUP_TOOLCHAIN='${toolchainName}'
        exec ${lintToolchain}/bin/cargo "$@"
      '')
      (pkgs.writeShellScriptBin "rustc" ''
        case "''${1-}" in
          '+${channel}'|'+${toolchainName}') shift ;;
          +*) echo "Nix Dylint requires ${channel}: $1" >&2; exit 1 ;;
        esac
        export RUSTUP_TOOLCHAIN='${toolchainName}'
        exec ${lintToolchain}/bin/rustc "$@"
      '')
    ];
  };
  environment = {
    DYLINT_DRIVER_PATH = "${driver}/lib/dylint";
    RUSTUP_HOME = "${driver}";
    RUSTUP_TOOLCHAIN = toolchainName;
    RUSTDOC = "${lintToolchain}/bin/rustdoc";
    LD_LIBRARY_PATH = "${lintToolchain}/lib";
    DYLD_LIBRARY_PATH = "${lintToolchain}/lib";
    CARGO_NET_OFFLINE = "true";
  };
  enterToolchain = ''
    export PATH="${
      lib.makeBinPath [
        nixToolchain
        tools
        lintToolchain
      ]
    }:$PATH"
  '';
  lintSrc = lib.cleanSourceWith {
    src = lintRoot;
    filter = path: type: lintCrane.filterCargoSources path type || lib.hasSuffix ".stderr" path;
  };
  lintCommon = environment // {
    pname = "no-dyn";
    version = "0.1.0";
    src = lintSrc;
    strictDeps = true;
    nativeBuildInputs = [
      tools
      pkgs.pkg-config
    ];
    buildInputs = [ pkgs.openssl ];
    OPENSSL_NO_VENDOR = "1";
    preConfigure = enterToolchain;
  };
  lintUi = lintCrane.mkCargoDerivation (
    lintCommon
    // {
      cargoArtifacts = null;
      doInstallCargoArtifacts = false;
      doCheck = true;
      buildPhaseCargoCommand = "cargo build --locked --offline";
      checkPhaseCargoCommand = "cargo test --locked --offline";
    }
  );
  workspaceVendor = lintCrane.vendorMultipleCargoDeps {
    cargoLockList = [
      ../Cargo.lock
      (lintRoot + "/Cargo.lock")
    ];
  };
  workspaceCommon =
    common
    // environment
    // {
      src = lib.cleanSourceWith {
        src = common.src.origSrc;
        filter = path: type: common.src.filter path type || lib.hasSuffix ".sh" path;
      };
      cargoVendorDir = workspaceVendor;
      nativeBuildInputs = common.nativeBuildInputs ++ [
        tools
        pkgs.pkg-config
        pkgs.python3
      ];
      buildInputs = common.buildInputs ++ [ pkgs.openssl ];
      OPENSSL_NO_VENDOR = "1";
      preConfigure = enterToolchain;
    };
  workspaceArtifacts = lintCrane.buildDepsOnly (
    common
    // {
      # Cache application dependencies independently of the lint's UI fixtures.
      src = lib.cleanSourceWith {
        src = common.src;
        filter = path: type: baseNameOf path != "lints";
      };
      CARGO_PROFILE = "dev";
      cargoExtraArgs = "--workspace --locked --all-targets --all-features --target-dir target/dylint/target/${toolchainName}";
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
          cargo dylint --all --workspace --fail-on-no-libraries -- --locked --offline --all-targets ${features}
          export DYLINT_TOOLCHAIN='${channel}'
          export DYLINT_DRIVER='${driver}/lib/dylint/${toolchainName}/dylint-driver'
          export DYLINT_LIBRARY="$PWD/target/dylint/libraries/${toolchainName}/release/libno_dyn@${toolchainName}${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}"
          export CARGO_TARGET_DIR="$PWD/target/dylint/target/${toolchainName}"
          bash scripts/test-no-dyn-doctests.sh ${features}
        '';
      }
    );
  shellEnvironment = ''
    ${enterToolchain}
    export DYLINT_DRIVER_PATH='${environment.DYLINT_DRIVER_PATH}'
    export RUSTUP_HOME='${environment.RUSTUP_HOME}'
    export RUSTUP_TOOLCHAIN='${environment.RUSTUP_TOOLCHAIN}'
    export RUSTDOC='${environment.RUSTDOC}'
    export LD_LIBRARY_PATH='${environment.LD_LIBRARY_PATH}'
    export DYLD_LIBRARY_PATH='${environment.DYLD_LIBRARY_PATH}'
  '';
  cargoDylint = pkgs.writeShellScriptBin "cargo-dylint" ''
    set -euo pipefail
    ${shellEnvironment}
    exec ${tools}/bin/cargo-dylint "$@"
  '';
  uiCommand = pkgs.writeShellScriptBin "no-dyn-ui" ''
    set -euo pipefail
    ${shellEnvironment}
    cd lints/no_dyn
    exec cargo test --locked "$@"
  '';
  checkCommand = pkgs.writeShellScriptBin "no-dyn" ''
    set -euo pipefail
    ${shellEnvironment}
    export DYLINT_TOOLCHAIN='${channel}'
    exec bash scripts/check-no-dyn.sh "$@"
  '';
in
{
  checks = {
    no-dyn = workspaceCheck "evm-abstract-no-dyn" "";
    imbl-no-dyn = workspaceCheck "evm-abstract-imbl-no-dyn" "--features imbl";
    all-features-no-dyn = workspaceCheck "evm-abstract-all-features-no-dyn" "--all-features";
    no-dyn-ui = lintUi;
    no-dyn-fmt = lintCrane.cargoFmt (lintCommon // { cargoExtraArgs = "--all"; });
  };
  packages = {
    dylint-tools = tools;
    dylint-driver = driver;
  };
  devPackages = [
    cargoDylint
    tools
    uiCommand
    checkCommand
  ];
}
