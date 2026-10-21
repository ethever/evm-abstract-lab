# Independent installed-runtime contracts. Each fixture is a separate
# check, allowing Nix to cache and schedule them without a second worker pool.
{
  pkgs,
  sourceRoot,
  build,
  ...
}:
let
  lib = pkgs.lib;
  aggregateCheck = import ../check-aggregate.nix { inherit pkgs; };
  fixtures = import ./fixtures.nix { inherit pkgs sourceRoot build; };
  normalization = fixtures.inputs.file "scripts/normalize_identity_scopes.py";
  normalizationTests = fixtures.inputs.files [
    "scripts/normalize_identity_scopes.py"
    "scripts/test_normalize_identity_scopes.py"
  ];
  parity = lib.listToAttrs (
    map (
      world:
      let
        name = lib.removeSuffix ".json" world;
        std = fixtures.worldReport { world = "examples/worlds/${world}"; };
        imbl = fixtures.worldReport {
          world = "examples/worlds/${world}";
          variant = "imbl";
        };
      in
      lib.nameValuePair "state-backend-parity-${name}" (
        pkgs.runCommand "evm-abstract-state-backend-parity-${name}"
          { nativeBuildInputs = [ pkgs.python3 ]; }
          ''
            mkdir -p "$out/std" "$out/imbl" "$out/normalized/std" "$out/normalized/imbl"
            cmp ${std}/exit-code ${imbl}/exit-code
            for kind in std imbl; do
              case "$kind" in std) report=${std} ;; imbl) report=${imbl} ;; esac
              cp "$report/analysis.json" "$out/$kind/${name}.json"
              python ${normalization} \
                < "$report/analysis.json" > "$out/normalized/$kind/${name}.json"
              if [ "$(cat "$report/exit-code")" = 0 ]; then
                cp "$report/report.json" "$out/$kind/${name}-ssa.json"
                python ${normalization} \
                  < "$report/report.json" > "$out/normalized/$kind/${name}-ssa.json"
              fi
            done
            diff -r "$out/normalized/std" "$out/normalized/imbl"
          ''
      )
    ) fixtures.worlds
  );
  package = build.release.packages.evm-abstract-cli;
in
parity
// rec {
  state-backend-normalization =
    pkgs.runCommand "evm-abstract-state-backend-normalization" { nativeBuildInputs = [ pkgs.python3 ]; }
      ''
        python ${normalizationTests}/scripts/test_normalize_identity_scopes.py
        touch "$out"
      '';
  # Compatibility aggregators only reference their leaves; they execute no
  # test a second time, and the automatic planner deduplicates identical drvs.
  state-backend-parity = aggregateCheck {
    name = "evm-abstract-state-backend-parity";
    checks = parity;
  };
  symbolic-defaults =
    pkgs.runCommand "evm-abstract-installed-symbolic-defaults" { nativeBuildInputs = [ pkgs.jq ]; }
      ''
        mkdir "$out"
        ${package}/bin/evm-abstract cfg --hex 00 --format json > "$out/report.json"
        jq -e '.domain_spec.relations == {
          enabled: true, max_constraints: 2048, max_nodes: 16384, max_depth: 256
        }' "$out/report.json" > /dev/null
      '';
  symbolic-path-facts =
    pkgs.runCommand "evm-abstract-installed-symbolic-path-facts" { nativeBuildInputs = [ pkgs.jq ]; }
      ''
        mkdir "$out"
        PATH= ${package}/bin/evm-abstract cfg \
          --hex 3480600114600957005b80600214601257005b00 \
          --context-depth 0 --format json > "$out/report.json"
        jq -e '
          .status == "Converged" and .frontiers == [] and
          (.program.blocks as $blocks |
           [.states[].key.basic_block_index | $blocks[.].start_pc] as $reachable |
           ($reachable | index(9)) != null and ($reachable | index(8)) != null and
           ($reachable | index(17)) != null and ($reachable | index(18)) == null)
        ' "$out/report.json" > /dev/null
      '';
  symbolic-analysis = aggregateCheck {
    name = "evm-abstract-installed-symbolic-analysis";
    checks = {
      inherit symbolic-defaults symbolic-path-facts;
    };
  };
}
