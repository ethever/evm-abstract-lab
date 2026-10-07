{ pkgs }:
let
  # Keep the independently embedded SAT implementations private. Matching
  # source versions alone would not prevent ELF symbol interposition.
  privateElf =
    package:
    pkgs.runCommand "${package.pname}-${package.version}-embedded"
      {
        inherit (package) pname version meta;
        nativeBuildInputs = [
          pkgs.python3
          pkgs.patchelf
        ];
      }
      ''
        set -euo pipefail
        mkdir -p "$out"
        cp -a ${package}/. "$out/"
        chmod -R u+w "$out"
        python3 - "$out" ${package} <<'PY'
        import subprocess
        import sys
        from pathlib import Path
        output, original = Path(sys.argv[1]), sys.argv[2]
        for path in output.rglob("*"):
            if not path.is_file() or path.is_symlink():
                continue
            if path.suffix in (".pc", ".cmake"):
                text = path.read_text()
                path.write_text(text.replace(original, str(output)))
            with path.open("rb") as source:
                magic = source.read(4)
            if magic == b"\x7fELF":
                result = subprocess.run(["patchelf", "--print-rpath", str(path)],
                                        text=True, capture_output=True, check=True)
                old = result.stdout.strip().replace(original, str(output))
                rpath = str(output / "lib") + (":" + old if old else "")
                subprocess.run(["patchelf", "--set-rpath", rpath, str(path)], check=True)
        PY
        # The pinned cvc5 DSO labels externally visible definitions with the
        # reserved LOCAL version index. Normalize only these definitions, preserve
        # imported versions and executable bytes, and fail on an unexpected ABI.
        python3 ${./smt/elf.py} "$out" ${package} ${package.pname} > "$out/native-isolation.json"
      '';
  # Mach-O has no ELF version table. Hide the static archive at compilation
  # instead; no binary-format edits are applied to Darwin artifacts.
  privateCadical = (pkgs.cadical.override { version = "2.1.3"; }).overrideAttrs (old: {
    env = (old.env or { }) // {
      NIX_CXXFLAGS_COMPILE =
        (old.env.NIX_CXXFLAGS_COMPILE or "") + " -fvisibility=hidden -fvisibility-inlines-hidden";
    };
    nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.python3 ];
    # The solvers themselves emit inline Tracer methods from these headers.
    # Namespace visibility keeps those private too, without hiding Bitwuzla's
    # unannotated public C++ API with a blanket compiler flag.
    postInstall = (old.postInstall or "") + ''
      python3 - "$dev/include" <<'PY'
      import re
      import sys
      from pathlib import Path
      changed = 0
      for path in Path(sys.argv[1]).rglob("*.hpp"):
          source = path.read_text()
          revised, count = re.subn(
              r"namespace\s+CaDiCaL\s*\{",
              'namespace __attribute__((visibility("hidden"))) CaDiCaL {',
              source,
          )
          if count:
              path.write_text(revised)
              changed += count
      if not changed:
          raise RuntimeError("expected CaDiCaL declarations in installed headers")
      PY
    '';
  });
  cvc5 =
    if pkgs.stdenv.hostPlatform.isLinux then
      privateElf pkgs.cvc5
    else
      pkgs.cvc5.override { cadical' = privateCadical; };
  bitwuzla =
    if pkgs.stdenv.hostPlatform.isLinux then
      privateElf pkgs.bitwuzla
    else
      pkgs.bitwuzla.override { cadical' = privateCadical; };
  isolation =
    pkgs.runCommand "native-smt-isolation"
      {
        nativeBuildInputs = [
          pkgs.python3
          pkgs.binutils
        ];
      }
      (
        if pkgs.stdenv.hostPlatform.isLinux then
          ''
            set -euo pipefail
            python3 ${./smt/bindings.py} ${cvc5}/lib/libcvc5.so ${bitwuzla}/lib/libbitwuzla.so cvc5 bitwuzla
              python3 ${./smt/bindings.py} ${cvc5}/lib/libcvc5.so ${bitwuzla}/lib/libbitwuzla.so bitwuzla cvc5
              touch "$out"
          ''
        else
          ''
            set -euo pipefail
            for library in ${cvc5}/lib/*.dylib ${bitwuzla}/lib/*.dylib; do
              nm -gU "$library" > symbols
              if grep -q CaDiCaL symbols; then
                  echo "native library exposes its private CaDiCaL implementation: $library" >&2
                  exit 1
                fi
              done
              touch "$out"
          ''
      );
in
assert pkgs.cvc5.version == "1.4.0";
assert pkgs.bitwuzla.version == "0.9.1";
assert pkgs.stdenv.hostPlatform.isLinux || pkgs.stdenv.hostPlatform.isDarwin;
{
  inherit cvc5 bitwuzla;
  checks.native-smt-isolation = isolation;
}
