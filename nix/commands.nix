# Optional experiments use the same development closure as Cargo and the editor.
{ pkgs }:
{
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
