# The one dependency inventory used by builds, development and editor children.
{ pkgs, nativeSmt }:
let
  fontConfig = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
in
{
  inherit fontConfig;
  nativeBuildInputs = [
    pkgs.cmake
    pkgs.pkg-config
    pkgs.rustPlatform.bindgenHook
  ];
  buildInputs = [
    nativeSmt.z3
    nativeSmt.bitwuzla
    nativeSmt.cvc5
  ];
  environment = {
    CVC5_LIB_DIR = "${nativeSmt.cvc5}/lib";
    CVC5_INCLUDE_DIR = "${nativeSmt.cvc5}/include";
    SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
    RUST_TEST_THREADS = "8";
  };
  # OpenSSL is needed by the Dylint UI test's git2 dependency. Keep it explicit
  # instead of inheriting it accidentally from whichever checks happen to run.
  developmentLibraries = [ pkgs.openssl ];
  developmentTools = with pkgs; [
    cargo-nextest
    graphviz
    jq
    nixfmt
    taplo
    lychee
    git
    bash
    coreutils
    diffutils
    findutils
    gnugrep
    gnused
    ripgrep
    nixVersions.stable
    # Only protocol/regression tests and the optional comparison report use Python.
    python3
  ];
}
