# Developer commands keep tool selection in Nix and operate on the caller's
# checkout. The affected-check runner evaluates immutable content snapshots.
{ pkgs, sourceRoot }:
let
  inputs = import ./check-inputs.nix {
    inherit (pkgs) lib;
    inherit sourceRoot;
  };
in
{
  check-changes = {
    type = "app";
    meta.description = "Build only checks whose actual Nix derivation changed";
    program = pkgs.lib.getExe (
      pkgs.writeShellApplication {
        name = "check-changes";
        runtimeInputs = [
          pkgs.python3
          pkgs.git
          pkgs.nix
        ];
        text = ''
          exec python ${inputs.file "scripts/check-changes.py"} "$@"
        '';
      }
    );
  };
  compare-state-backends = {
    type = "app";
    program = pkgs.lib.getExe (
      pkgs.writeShellApplication {
        name = "compare-state-backends";
        runtimeInputs = [ pkgs.nix ];
        text = ''
          exec nix develop --no-update-lock-file --command bash scripts/compare-state-backends.sh "$@"
        '';
      }
    );
  };
}
