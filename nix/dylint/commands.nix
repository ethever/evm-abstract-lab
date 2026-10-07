# Developer commands use the same pinned compiler and prebuilt driver as checks.
{
  pkgs,
  packages,
  toolchain,
}:
let
  activation = toolchain.activate packages.tools packages.driver;
in
{
  cargo-dylint = pkgs.writeShellApplication {
    name = "cargo-dylint";
    text = ''
      ${activation}
      exec ${packages.tools}/bin/cargo-dylint "$@"
    '';
  };
  no-dyn-ui = pkgs.writeShellApplication {
    name = "no-dyn-ui";
    text = ''
      ${activation}
      cd lints/no_dyn
      exec cargo test --locked "$@"
    '';
  };
  no-dyn = pkgs.writeShellApplication {
    name = "no-dyn";
    runtimeInputs = [
      pkgs.bash
      pkgs.coreutils
      pkgs.jq
      pkgs.gnugrep
    ];
    text = ''
      ${activation}
      export DYLINT_TOOLCHAIN='${toolchain.channel}'
      export DYLINT_DRIVER='${packages.driver}/lib/dylint/${toolchain.name}/dylint-driver'
      export DYLINT_LIBRARY='${packages.libraryPath}'
      task_cpu_limit=$(( $(nproc) - 5 ))
      if (( task_cpu_limit < 1 )); then task_cpu_limit=1; fi
      task_default_jobs=$task_cpu_limit
      if (( task_default_jobs > 8 )); then task_default_jobs=8; fi
      task_jobs=''${DYLINT_JOBS:-$task_default_jobs}
      if [[ ! $task_jobs =~ ^[0-9]+$ ]] || (( ''${#task_jobs} > 9 )); then
        echo "DYLINT_JOBS must be between 1 and $task_cpu_limit" >&2
        exit 1
      fi
      task_jobs=$((10#$task_jobs))
      if (( task_jobs < 1 || task_jobs > task_cpu_limit )); then
        echo "DYLINT_JOBS must be between 1 and $task_cpu_limit" >&2
        exit 1
      fi
      export CARGO_BUILD_JOBS=$task_jobs
      cargo dylint --all --workspace --fail-on-no-libraries --no-build --no-metadata \
        --lib-path "$DYLINT_LIBRARY" -- --locked --all-targets --all-features --jobs "$task_jobs" "$@"
      exec bash scripts/test-no-dyn-doctests.sh --all-features --jobs "$task_jobs" "$@"
    '';
  };
}
