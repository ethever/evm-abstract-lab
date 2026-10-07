{ pkgs }:
let
  privateCadical = import ./smt/cadical.nix { inherit pkgs; };
  cvc5 = (pkgs.cvc5.override { cadical' = privateCadical; }).overrideAttrs (old: {
    # cvc5 otherwise auto-selects gold on x86_64 Linux. The pinned gold emits
    # version index 0 for public definitions, which Rust's bundled LLD rejects.
    # Use the platform linker (BFD on Linux) to emit compatible ELF metadata.
    cmakeFlags = (old.cmakeFlags or [ ]) ++ [ "-DUSE_DEFAULT_LINKER=ON" ];
    # Upstream otherwise runs CTest on every host CPU, ignoring Nix's --cores.
    # Its check target explicitly supports overriding that count through ARGS.
    preCheck = (old.preCheck or "") + ''
      checkFlagsArray+=("ARGS=-j$NIX_BUILD_CORES")
    '';
  });
  bitwuzla = pkgs.bitwuzla.override { cadical' = privateCadical; };
in
assert pkgs.cvc5.version == "1.4.0";
assert pkgs.bitwuzla.version == "0.9.1";
assert pkgs.stdenv.hostPlatform.isLinux || pkgs.stdenv.hostPlatform.isDarwin;
{
  inherit (pkgs) z3;
  inherit cvc5 bitwuzla;
  checks.native-smt-isolation = import ./smt/isolation.nix { inherit pkgs cvc5 bitwuzla; };
}
