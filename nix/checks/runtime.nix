{
  pkgs,
  sourceRoot,
  build,
  ...
}:
let
  package = build.release.package;
  imblPackage = build.imblRelease.package;
in
{
  state-backend-parity =
    pkgs.runCommand "evm-abstract-state-backend-parity" { nativeBuildInputs = [ pkgs.jq ]; }
      ''
        set -euo pipefail
        mkdir std imbl
        for world in ${sourceRoot}/examples/worlds/*.json; do
          name="$(basename "$world" .json)"
          std_status=0
          imbl_status=0
          ${package}/bin/evm-abstract analyze --world "$world" \
            --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
            --format json > "std/$name.json" || std_status=$?
          ${imblPackage}/bin/evm-abstract analyze --world "$world" \
            --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
            --format json > "imbl/$name.json" || imbl_status=$?
          test "$std_status" -eq "$imbl_status"
          case "$std_status" in 0|2) ;; *) exit 1 ;; esac
          cmp "std/$name.json" "imbl/$name.json"
          if test "$std_status" -eq 0; then
            ${package}/bin/evm-abstract analyze --world "$world" \
              --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
              --format json --ssa > "std/$name-ssa.json"
            ${imblPackage}/bin/evm-abstract analyze --world "$world" \
              --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x \
              --format json --ssa > "imbl/$name-ssa.json"
            cmp "std/$name-ssa.json" "imbl/$name-ssa.json"
          else
            jq -e '.status == "Incomplete"' "std/$name.json" > /dev/null
          fi
        done
        mkdir $out
        cp -r std imbl $out/
      '';
  smt-providers =
    pkgs.runCommand "evm-abstract-installed-smt-providers" { nativeBuildInputs = [ pkgs.jq ]; }
      ''
        set -euo pipefail
        mkdir "$out"
        for provider in z3 bitwuzla cvc5; do
          PATH= ${package}/bin/evm-abstract cfg \
            --hex 3480600114600957005b80600214601257005b00 \
            --context-depth 0 --smt.provider "$provider" --smt.rlimit 200000 \
            --format json > "$out/$provider.json"
          jq -e --arg provider "$provider" '
            .status == "Converged" and .frontiers == [] and
            .config.relations.provider == $provider and .config.relations.rlimit == 200000 and
            (.program.blocks as $blocks |
             [.states[].key.basic_block_index | $blocks[.].start_pc] as $reachable |
             ($reachable | index(9)) != null and ($reachable | index(8)) != null and
             ($reachable | index(17)) != null and ($reachable | index(18)) == null)
          ' "$out/$provider.json" > /dev/null
        done
        ${package}/bin/evm-abstract cfg --hex 00 --format json > "$out/default.json"
        jq -e '.config.relations.provider == "z3" and .config.relations.rlimit == 10000000' "$out/default.json" > /dev/null
      '';
}
