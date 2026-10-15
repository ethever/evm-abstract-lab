# Exercise Crane's real phase hooks without compiling Rust. The probe first
# observes Crane's injected values, then checks that our shared hook removes
# them, including when a caller appends another postPatch implementation.
{ pkgs, craneLib }:
name: hooks:
craneLib.mkCargoDerivation {
  pname = name;
  version = "0";
  src = pkgs.runCommand "cargo-concurrency-fixture" { } ''
    mkdir -p "$out/src"
    printf '[package]\nname="cargo-concurrency-fixture"\nversion="0.0.0"\nedition="2024"\n' > "$out/Cargo.toml"
    printf 'fn main() {}\n' > "$out/src/main.rs"
  '';
  cargoArtifacts = null;
  cargoVendorDir = null;
  doCheck = false;
  doInstallCargoArtifacts = false;
  prePatch = ''
    observeInjectedCargoConcurrency() {
      test -n "''${CARGO_BUILD_JOBS+x}"
      test -n "''${RUST_TEST_THREADS+x}"
      export CARGO_CONCURRENCY_INJECTION_OBSERVED=1
    }
    postPatchHooks+=(observeInjectedCargoConcurrency)
  ''
  + hooks.prePatch;
  postPatch = hooks.postPatch or "";
  buildPhaseCargoCommand = ''
    test "''${CARGO_CONCURRENCY_INJECTION_OBSERVED-}" = 1
    test -z "''${CARGO_BUILD_JOBS+x}"
    test -z "''${RUST_TEST_THREADS+x}"
  '';
  installPhaseCommand = ''
    mkdir -p "$out"
    printf 'Crane injection observed; Cargo/libtest limits cleared.\n' > "$out/result"
  '';
}
