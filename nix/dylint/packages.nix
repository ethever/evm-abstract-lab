# Build the Dylint tools, driver and repository lint once, without first-use downloads.
{
  pkgs,
  craneLib,
  toolchain,
  lintRoot,
}:
let
  lintCrane = toolchain.craneLib;
  upstream = pkgs.fetchFromGitHub {
    owner = "trailofbits";
    repo = "dylint";
    rev = "8aeea4b0c27722d46fcc8288330401418328e8fa";
    hash = "sha256-KgEn3AZnITS6Uhc6CElCMqOucu+/Cc4w9Jm5oU+v5Iw=";
  };
  native = {
    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [ pkgs.openssl ];
    OPENSSL_NO_VENDOR = "1";
  };
  tools = craneLib.buildPackage (
    native
    // {
      pname = "dylint-tools";
      version = "6.1.0";
      src = upstream;
      cargoExtraArgs = "--locked -p cargo-dylint -p dylint-link";
      doCheck = false;
    }
  );
  driver = lintCrane.buildPackage (
    native
    // {
      pname = "dylint-driver";
      version = "6.1.0";
      src = upstream;
      sourceRoot = "source/driver";
      cargoArtifacts = null;
      cargoVendorDir = lintCrane.vendorCargoDeps { cargoLock = upstream + "/driver/Cargo.lock"; };
      cargoExtraArgs = "--locked --target-dir target";
      doCheck = false;
      doNotRemoveReferencesToRustToolchain = true;
      # This spelling disables an upstream Clippy download; Nix still pins rustc.
      RUSTUP_TOOLCHAIN = "nightly-${toolchain.target}";
      RUSTFLAGS = "-C link-arg=-Wl,-rpath,${toolchain.compiler}/lib";
      postPatch = ''
        cat > src/main.rs <<'EOF'
        #![feature(rustc_private)]
        fn main() -> anyhow::Result<()> {
            dylint_driver::dylint_driver(&std::env::args_os().collect::<Vec<_>>())
        }
        EOF
      '';
      installPhaseCommand = ''
        mkdir -p "$out/lib/dylint/${toolchain.name}" "$out/toolchains"
        cp target/release/dylint_driver "$out/lib/dylint/${toolchain.name}/dylint-driver"
        ln -s ${toolchain.compiler} "$out/toolchains/${toolchain.name}"
      '';
    }
  );
  lintCommon =
    native
    // toolchain.environment driver
    // {
      pname = "no-dyn";
      version = "0.1.0";
      src = pkgs.lib.cleanSourceWith {
        src = lintRoot;
        filter = path: type: lintCrane.filterCargoSources path type || pkgs.lib.hasSuffix ".stderr" path;
      };
      strictDeps = true;
      CARGO_NET_OFFLINE = "true";
      nativeBuildInputs = native.nativeBuildInputs ++ [ tools ];
      preConfigure = toolchain.enter tools;
    };
  libraryFile = "libno_dyn@${toolchain.name}${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}";
  library = lintCrane.buildPackage (
    lintCommon
    // {
      cargoExtraArgs = "--locked --offline --lib";
      doCheck = false;
      doNotRemoveReferencesToRustToolchain = true;
      installPhaseCommand = ''
        mkdir -p "$out/lib"
        cp target/release/libno_dyn${pkgs.stdenv.hostPlatform.extensions.sharedLibrary} "$out/lib/${libraryFile}"
      '';
    }
  );
in
{
  inherit
    tools
    driver
    lintCommon
    library
    ;
  libraryPath = "${library}/lib/${libraryFile}";
}
