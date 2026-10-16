# Proof-only cache boundary: Rust, RPC adapters and Markdown cannot invalidate
# this leaf. The aggregate and affected-check selector consume the same leaf.
{
  pkgs,
  sourceRoot,
  ...
}:
let
  inputs = import ../check-inputs.nix {
    inherit (pkgs) lib;
    inherit sourceRoot;
  };
  source = inputs.files [ "proofs" ];
in
assert pkgs.lib.assertMsg (pkgs.lean4.version == "4.30.0") "proofs require Lean 4.30.0";
{
  lean-proofs =
    pkgs.runCommand "evm-abstract-lean-proofs"
      {
        nativeBuildInputs = [
          pkgs.lean4
          pkgs.bash
          pkgs.python3
        ];
      }
      ''
        export LEAN_BIN=${pkgs.lean4}/bin/lean
        bash ${source}/proofs/check.sh "$out"
      '';
}
