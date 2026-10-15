# Use the actual bindgen/stdenv hooks twice with different output hashes. No
# Rust or native compilation runs: only the environment fingerprint is checked.
{
  pkgs,
  craneLib,
  build,
}:
let
  common = build.commonFor "embedded-smt" {
    target = "native";
    kind = "build";
  } build.configurations.default;
  fixture = pkgs.runCommand "cargo-bindgen-fixture" { } ''
    mkdir -p "$out/src"
    printf '[package]\nname="cargo-bindgen-fixture"\nversion="0.0.0"\nedition="2024"\n' > "$out/Cargo.toml"
    printf 'fn main() {}\n' > "$out/src/main.rs"
  '';
  probe =
    name:
    craneLib.mkCargoDerivation (
      common
      // {
        pname = name;
        src = fixture;
        cargoArtifacts = null;
        cargoVendorDir = null;
        doCheck = false;
        doInstallCargoArtifacts = false;
        prePatch = ''
          task_bindgen_probe_output="''${out##*/}"
          export TASK_BINDGEN_PROBE_SEED="''${task_bindgen_probe_output:0:10}"
          case " $BINDGEN_EXTRA_CLANG_ARGS " in
            *" -frandom-seed=$TASK_BINDGEN_PROBE_SEED "*) ;;
            *) echo 'bindgen probe did not observe the original Nix seed' >&2; exit 1 ;;
          esac
          # Exercise a caller-owned seed and a quoted parser argument as well.
          export BINDGEN_EXTRA_CLANG_ARGS="$BINDGEN_EXTRA_CLANG_ARGS -frandom-seed=caller-owned -DPROBE_HEADER=\"value with spaces\""
          export TASK_BINDGEN_PROBE_RAW_ARGS="$BINDGEN_EXTRA_CLANG_ARGS"
          export TASK_BINDGEN_PROBE_CFLAGS="$NIX_CFLAGS_COMPILE"
          export TASK_BINDGEN_PROBE_CXXFLAGS="''${NIX_CXXFLAGS_COMPILE-}"
          export TASK_BINDGEN_PROBE_USER_CFLAGS="''${CFLAGS-}"
          export TASK_BINDGEN_PROBE_USER_CXXFLAGS="''${CXXFLAGS-}"
        ''
        + common.prePatch;
        buildPhaseCargoCommand = ''
          export TASK_BINDGEN_PROBE_CANONICAL_ARGS="$BINDGEN_EXTRA_CLANG_ARGS"
          case " $TASK_BINDGEN_PROBE_CANONICAL_ARGS " in
            *" -frandom-seed=$TASK_BINDGEN_PROBE_SEED "*) exit 1 ;;
          esac
          export BINDGEN_EXTRA_CLANG_ARGS="$TASK_BINDGEN_PROBE_RAW_ARGS -DSEED_TEXT=\"keep -frandom-seed=$TASK_BINDGEN_PROBE_SEED content\""
          export TASK_BINDGEN_PROBE_QUOTED_RAW_ARGS="$BINDGEN_EXTRA_CLANG_ARGS"
        ''
        + (import ../cargo-bindgen.nix { inherit pkgs; })
        + ''
          test "$TASK_BINDGEN_PROBE_CFLAGS" = "$NIX_CFLAGS_COMPILE"
          test "$TASK_BINDGEN_PROBE_CXXFLAGS" = "''${NIX_CXXFLAGS_COMPILE-}"
          test "$TASK_BINDGEN_PROBE_USER_CFLAGS" = "''${CFLAGS-}"
          test "$TASK_BINDGEN_PROBE_USER_CXXFLAGS" = "''${CXXFLAGS-}"
        '';
        installPhaseCommand = ''
          mkdir -p "$out"
          printf '%s\n' "$TASK_BINDGEN_PROBE_CANONICAL_ARGS" > "$out/bindgen-args"
          printf '%s\n' "$TASK_BINDGEN_PROBE_RAW_ARGS" > "$out/raw-bindgen-args"
          printf '%s\n' "$BINDGEN_EXTRA_CLANG_ARGS" > "$out/quoted-bindgen-args"
          printf '%s\n' "$TASK_BINDGEN_PROBE_QUOTED_RAW_ARGS" > "$out/raw-quoted-bindgen-args"
          printf '%s\n' "$NIX_CFLAGS_COMPILE" > "$out/native-cflags"
          printf '%s\n' "$TASK_BINDGEN_PROBE_SEED" > "$out/seed"
        '';
      }
    );
  first = probe "cargo-bindgen-environment-first";
  second = probe "cargo-bindgen-environment-second";
in
pkgs.runCommand "cargo-bindgen-environment"
  {
    nativeBuildInputs = [
      pkgs.diffutils
      pkgs.python3
    ];
  }
  ''
    cmp ${first}/bindgen-args ${second}/bindgen-args
    if cmp -s ${first}/native-cflags ${second}/native-cflags; then
      echo 'native flags unexpectedly lost their distinct output seeds' >&2
      exit 1
    fi
    if cmp -s ${first}/seed ${second}/seed; then exit 1; fi
    python - ${first} ${second} <<'PY'
    from pathlib import Path
    import shlex
    import sys
    for argument in sys.argv[1:]:
        root = Path(argument)
        seed = (root / 'seed').read_text().strip()
        before = shlex.split((root / 'raw-bindgen-args').read_text())
        after = shlex.split((root / 'bindgen-args').read_text())
        expected = [value for value in before if value != f'-frandom-seed={seed}']
        assert after == expected
        assert '-frandom-seed=caller-owned' in after
        assert '-DPROBE_HEADER=value with spaces' in after
        quoted_before = shlex.split((root / 'raw-quoted-bindgen-args').read_text())
        quoted_after = shlex.split((root / 'quoted-bindgen-args').read_text())
        assert quoted_after == [value for value in quoted_before if value != f'-frandom-seed={seed}']
        assert f'-DSEED_TEXT=keep -frandom-seed={seed} content' in quoted_after
    PY
    mkdir -p "$out"
    cp ${first}/bindgen-args "$out/"
    printf 'Stable bindgen arguments; distinct native compiler seeds preserved.\n' > "$out/result"
  ''
