{
  pkgs,
  cvc5,
  bitwuzla,
}:
let
  extension = pkgs.stdenv.hostPlatform.extensions.sharedLibrary;
  cvc5Library = "${cvc5}/lib/libcvc5${extension}";
  bitwuzlaLibrary = "${bitwuzla}/lib/libbitwuzla${extension}";
  exportedSymbols = if pkgs.stdenv.hostPlatform.isLinux then "-D --defined-only" else "-gU";
in
pkgs.runCommandCC "native-smt-isolation" { nativeBuildInputs = [ pkgs.binutils ]; } ''
  set -euo pipefail
  $CC -Wall -Wextra -Werror ${./isolation.c} -o load \
    ${pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux "-ldl"}
  for library in ${cvc5Library} ${bitwuzlaLibrary}; do
    nm -C ${exportedSymbols} "$library" > symbols
    if grep -Eq ' [[:alpha:]] (CaDiCaL::|.* (for|to) CaDiCaL::)' symbols; then
      echo "native library exposes its private CaDiCaL implementation: $library" >&2
      exit 1
    fi
  done
  ./load ${cvc5Library} cvc5_new ${bitwuzlaLibrary} bitwuzla_new
  ./load ${bitwuzlaLibrary} bitwuzla_new ${cvc5Library} cvc5_new
  touch "$out"
''
