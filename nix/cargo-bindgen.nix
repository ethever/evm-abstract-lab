# rust-bindgen-hook copies NIX_CFLAGS_COMPILE into its parser arguments. Nix's
# per-output GCC random seed has no binding-generation meaning, but bindgen's
# build script fingerprints that whole environment value and rebuilds for every
# artifact consumer. Remove only this output's complete seed token from bindgen;
# the real C/C++ compilation flags and all other parser arguments stay intact.
{ pkgs }:
''
  if [[ -v BINDGEN_EXTRA_CLANG_ARGS ]]; then
    task_nix_bindgen_args="$(${pkgs.python3}/bin/python - <<'PY'
  import os
  import shlex
  seed = os.path.basename(os.environ['out'])[:10]
  arguments = shlex.split(os.environ['BINDGEN_EXTRA_CLANG_ARGS'])
  print(shlex.join(argument for argument in arguments if argument != f'-frandom-seed={seed}'))
  PY
    )"
    export BINDGEN_EXTRA_CLANG_ARGS="$task_nix_bindgen_args"
    unset task_nix_bindgen_args
  fi
''
