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
      if [[ -n ''${DYLINT_JOBS:-} ]]; then
        if [[ ! $DYLINT_JOBS =~ ^[1-9][0-9]*$ ]]; then
          echo 'DYLINT_JOBS must be a positive integer' >&2
          exit 1
        fi
        export CARGO_BUILD_JOBS=$DYLINT_JOBS
      fi
      cargo dylint --all --workspace --fail-on-no-libraries --no-build --no-metadata \
        --lib-path "$DYLINT_LIBRARY" -- --locked --all-targets --all-features "$@"
      cargo dylint --all --fail-on-no-libraries --no-build --no-metadata \
        --lib-path "$DYLINT_LIBRARY" -- --locked -p evm-abstract-web --lib \
        --target wasm32-unknown-unknown "$@"
      exec bash scripts/test-no-dyn-doctests.sh --all-features "$@"
    '';
  };
}
