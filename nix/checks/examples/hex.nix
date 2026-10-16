# One SSA envelope carries each successful program's CFG and SSA assertions.
# The memory frontier case keeps both CLI exit branches because cfg and
# allow-partial-ssa deliberately expose different incomplete report shapes.
{
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
  memory = mkCheck "hex-memory-symbolic-offset" [ ] { } ''
    fixture=${inputs.file "examples/memory-symbolic-offset.hex"}
    if evm-abstract cfg --file "$fixture" --format json > memory-policy.json; then
      exit 1
    else
      test "$?" -eq 2
    fi
    jq -e '.status == "Incomplete" and .program.fork == "osaka" and .states[0].exit_stack[0].Constants == ["0x7"] and .frontiers == [{"reason":"Memory","from":0,"target":{"basic_block_index":0,"stack_height":1,"context":[]},"limit":"Memory"}]' memory-policy.json > /dev/null
    if evm-abstract ssa --file "$fixture" --format json --allow-partial-ssa > memory-policy-ssa.json; then
      exit 1
    else
      test "$?" -eq 2
    fi
    jq -e '
      .analysis.status == "Incomplete" and .machine_analysis.status == "Incomplete" and
      .partial_ssa.status == "Incomplete" and .partial_ssa.complete == false and (has("ssa") | not) and
      (.machine_analysis.frontiers | length) == 1 and
      .partial_ssa.frontiers == .machine_analysis.frontiers and
      (.machine_analysis.frontiers[0] as $frontier |
        $frontier.reason == "Memory" and $frontier.pc == 6 and
        $frontier.range.request.kind == "MemoryPolicy" and $frontier.range.nonzero == false and
        ($frontier.range.request.offset | has("Constants") | not) and
        $frontier.range.request.offset.provenance.origins.Sources == ["Calldata"] and
        $frontier.range.request.size.Constants == ["0x20"] and
        $frontier.range.request.limit.Constants == ["0x4000000"] and
        ($frontier.range.payload.call_stack.root.state.memory as $before |
          $before.length.Constants == ["0x0"] and $before.root.nodes[$before.root.root] == "BytesZero")) and
      (.partial_ssa.blocks[0] as $block |
        ($block.instructions[] | select(.pc == 7 and .opcode == 81)) as $read |
        $read.progress == "Completed" and $block.exit_frames == [[$read.results[0]]] and
        any($block.instructions[]; .pc == 6 and .opcode == 82 and .progress == "Completed" and .operands[0] == $read.operands[0]) and
        any(.partial_ssa.facts[]; .value == $read.results[0] and .scope == {"State":{"state":$block.state}} and .validity == "Current" and .constant == "0x7" and .numeric.Constants == ["0x7"] and .provenance.origins.Sources == ["Memory"]))
    ' memory-policy-ssa.json > /dev/null
    cp memory-policy.json memory-policy-ssa.json "$out/"
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
