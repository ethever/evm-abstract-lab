{
  pkgs,
  crane,
  craneLib,
  common,
  sourceRoot ? ../.,
}:
let
  lintRoot = sourceRoot + "/lints/no_dyn";
  toolchain = import ./dylint/toolchain.nix { inherit pkgs crane lintRoot; };
  packages = import ./dylint/packages.nix {
    inherit
      pkgs
      craneLib
      toolchain
      lintRoot
      ;
  };
  commands = import ./dylint/commands.nix { inherit pkgs packages toolchain; };
in
{
  checks = import ./dylint/checks.nix {
    inherit
      pkgs
      common
      toolchain
      packages
      ;
  };
  packages = {
    dylint-tools = packages.tools;
    dylint-driver = packages.driver;
    no-dyn-library = packages.library;
  };
  devPackages = builtins.attrValues commands ++ [ packages.tools ];
  apps = builtins.mapAttrs (name: _: {
    type = "app";
    program = pkgs.lib.getExe (
      pkgs.writeShellApplication {
        inherit name;
        runtimeInputs = [ pkgs.nix ];
        text = ''exec nix develop --no-update-lock-file --command ${name} "$@"'';
      }
    );
  }) (builtins.removeAttrs commands [ "cargo-dylint" ]);
}
