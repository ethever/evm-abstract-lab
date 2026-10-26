# Use the build's source replacement without writing user Cargo configuration.
{
  pkgs,
  cargo,
  cargoVendorDir,
}:
pkgs.writeShellScriptBin "cargo" ''
  exec ${cargo}/bin/cargo --config ${cargoVendorDir}/config.toml "$@"
''
