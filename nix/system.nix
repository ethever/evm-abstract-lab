# Assemble independent concerns; modules receive an explicit common toolchain.
{ inputs, system }:
let
  sourceRoot = ../.;
  pkgs = import inputs.nixpkgs {
    inherit system;
    overlays = [ inputs.rust-overlay.overlays.default ];
  };
  toolchain = pkgs.rust-bin.fromRustupToolchainFile (sourceRoot + "/rust-toolchain.toml");
  craneLib = (inputs.crane.mkLib pkgs).overrideToolchain toolchain;
  nativeSmt = import ./smt.nix { inherit pkgs; };
  dependencies = import ./dependencies.nix { inherit pkgs nativeSmt; };
  build = import ./build.nix {
    inherit
      pkgs
      craneLib
      dependencies
      sourceRoot
      ;
  };
  dylint = import ./dylint.nix {
    inherit pkgs craneLib sourceRoot;
    crane = inputs.crane;
    common = build.common;
  };
  checks = import ./checks.nix {
    inherit
      pkgs
      craneLib
      sourceRoot
      build
      dependencies
      ;
  };
in
{
  packages = dylint.packages // { default = build.package; };
  apps =
    dylint.apps
    // (import ./commands.nix { inherit pkgs; })
    // {
      default = {
        type = "app";
        program = "${build.package}/bin/evm-abstract";
        meta = build.package.meta;
      };
    };
  checks = checks // dylint.checks // nativeSmt.checks;
  devShells.default = import ./shell.nix { inherit craneLib dependencies dylint; };
  formatter = pkgs.nixfmt;
}
