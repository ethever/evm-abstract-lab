# Cargo external subcommands and compiler adapters inherit CARGO_HOME, while
# parent command-line --config arguments do not reach every Cargo child.
{ pkgs, cargoVendorDir }:
''
  export CARGO_HOME="''${XDG_CACHE_HOME:-$HOME/.cache}/evm-abstract-lab/cargo/${baseNameOf cargoVendorDir}"
  ${pkgs.coreutils}/bin/mkdir -p "$CARGO_HOME"
  ${pkgs.coreutils}/bin/ln -s ${cargoVendorDir}/config.toml "$CARGO_HOME/config.toml" 2>/dev/null \
    || test "$(${pkgs.coreutils}/bin/readlink "$CARGO_HOME/config.toml")" = '${cargoVendorDir}/config.toml'
''
