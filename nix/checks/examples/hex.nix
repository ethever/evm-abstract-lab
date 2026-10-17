# One SSA envelope carries each successful program's CFG and SSA assertions.
# Symbolic memory completes; a separate expression refusal retains exit-2 and
# partial-SSA coverage in the same installed example check.
{
  pkgs,
  lib,
  inputs,
  hexFixtures,
  mkCheck,
  ...
}:
let
  normal = lib.listToAttrs (
    map (
      fixture:
      let
        name = lib.removeSuffix ".hex" fixture;
      in
      lib.nameValuePair "example-hex-${name}" (
        mkCheck "hex-${name}" [ ] { } ''
          evm-abstract ssa --file ${inputs.file "examples/${fixture}"} --format json > "$out/report.json"
          jq '.analysis' "$out/report.json" > "$out/analysis.json"
          jq -e '.status == "Converged" and .program.fork == "osaka"' "$out/analysis.json" > /dev/null
          jq -e '.ssa.value_count > 0' "$out/report.json" > /dev/null
        ''
      )
    ) (lib.filter (name: name != "memory-symbolic-offset.hex") hexFixtures)
  );
  memory = mkCheck "hex-memory-symbolic-offset" [ pkgs.python3 ] { } ''
    fixture=${inputs.file "examples/memory-symbolic-offset.hex"}
    # Preserve linked artifact names; these now contain complete logical memory.
    evm-abstract cfg --file "$fixture" --format json > memory-policy.json
    evm-abstract ssa --file "$fixture" --format json > memory-policy-ssa.json
    if evm-abstract ssa --hex 600160005200 --evm.to 0x0000000000000000000000000000000000000101 --evm.calldata 0x --max-symbolic-nodes 2 --format json --allow-partial-ssa > expression-limit-ssa.json; then
      exit 1
    else
      test "$?" -eq 2
    fi
    python3 ${inputs.file "scripts/verify_logical_memory.py"} memory-policy.json memory-policy-ssa.json expression-limit-ssa.json
    cp memory-policy.json memory-policy-ssa.json expression-limit-ssa.json "$out/"
  '';
  unavailableForks = lib.genAttrs [ "cancun" "prague" ] (
    fork:
    mkCheck "osaka-clz-${fork}" [ ] { } ''
      evm-abstract cfg --file ${inputs.file "examples/osaka-clz.hex"} --fork ${fork} --format json > "$out/report.json"
      jq -e '.edges == [] and any(.diagnostics[]; .kind.InvalidOpcode.Instruction == {"opcode":30,"mnemonic":"CLZ","reason":{"UnavailableInFork":$input_fork}} and .pc == 2)' --arg input_fork ${fork} "$out/report.json" > /dev/null
    ''
  );
in
{
  checks =
    normal
    // (lib.mapAttrs' (
      fork: value: lib.nameValuePair "example-osaka-clz-${fork}" value
    ) unavailableForks)
    // {
      example-hex-memory-symbolic-offset = memory;
      example-osaka-clz-edges = mkCheck "osaka-clz-edges" [ ] { } ''
        jq -e '.edges | length == 1' ${normal.example-hex-osaka-clz}/analysis.json > /dev/null
        ln -s ${normal.example-hex-osaka-clz}/analysis.json "$out/report.json"
      '';
      example-internal-call-context = mkCheck "internal-call-context" [ ] { } ''
        evm-abstract cfg --file ${inputs.file "examples/internal-calls.hex"} --context-depth 1 --format json > "$out/report.json"
        jq -e '[.states[] | select(.key.context != [])] | length > 0' "$out/report.json" > /dev/null
      '';
    };
  artifacts = [
    {
      name = "memory-policy.json";
      path = "${memory}/memory-policy.json";
    }
    {
      name = "memory-policy-ssa.json";
      path = "${memory}/memory-policy-ssa.json";
    }
  ];
}
