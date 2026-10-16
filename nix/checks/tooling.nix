{
  pkgs,
  sourceRoot,
  dependencies,
  ...
}:
let
  sources = import ./tooling/sources.nix { inherit pkgs sourceRoot; };
  inputs = import ../check-inputs.nix {
    inherit (pkgs) lib;
    inherit sourceRoot;
  };
  planner = inputs.files [
    "scripts/check-changes.py"
    "scripts/test_check_changes.py"
    "scripts/check_cache_boundaries.json"
  ];
in
{
  nix-format =
    pkgs.runCommand "evm-abstract-nix-format"
      {
        nativeBuildInputs = [ pkgs.nixfmt ];
      }
      ''
        cd ${sources.nix}
        set -o pipefail
        nixfmt --check flake.nix
        find nix -name '*.nix' -type f -print0 | sort -z | xargs -0 nixfmt --check
        touch $out
      '';
  check-changes =
    pkgs.runCommand "evm-abstract-check-changes"
      {
        nativeBuildInputs = [
          pkgs.python3
          pkgs.git
        ];
      }
      ''
        export GIT_CONFIG_GLOBAL=/dev/null
        export GIT_CONFIG_NOSYSTEM=1
        python -B ${planner}/scripts/test_check_changes.py
        touch "$out"
      '';
  toml-lint =
    pkgs.runCommand "evm-abstract-toml-lint"
      {
        nativeBuildInputs = [ pkgs.taplo ];
      }
      ''
        cd ${sources.toml}
        taplo lint --no-schema
        touch $out
      '';
  toml-format =
    pkgs.runCommand "evm-abstract-toml-format"
      {
        nativeBuildInputs = [
          pkgs.taplo
          pkgs.python3
        ];
      }
      ''
        cd ${sources.tomlFormat}
        taplo fmt --check
        python scripts/test-toml-format.py
        touch $out
      '';
  doc-links =
    pkgs.runCommand "evm-abstract-doc-links"
      {
        nativeBuildInputs = [
          pkgs.lychee
          pkgs.bash
          pkgs.python3
        ];
        # Lychee initializes its TLS client even with --offline.
        inherit (dependencies.environment) SSL_CERT_FILE;
      }
      ''
        cd ${sources.docs}
        lychee --config ${sources.linksConfig} --offline --root-dir "$PWD" -- README.md '**/*.md'
        bash ${sources.linksTest} ${sources.linksConfig}
        python ${sources.linksProjectionTest} ${sources.docs} ${sources.linksConfig}
        touch $out
      '';
}
