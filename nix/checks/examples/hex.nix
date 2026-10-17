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
    python3 - <<'PY'
    import json
    from pathlib import Path

    def only(items):
        (item,) = items
        return item


    def memory_word_seven(word):
        # The numeric abstraction can be coarse; the typed root proves this word.
        assert word["provenance"]["origins"] == {"Sources": ["Memory"]}
        bounds = word["interval"]
        assert int(bounds["unsigned_lo"], 16) <= 7 <= int(bounds["unsigned_hi"], 16)
        biased = 7 ^ (1 << 255)
        assert int(bounds["signed_lo"], 16) <= biased <= int(bounds["signed_hi"], 16)
        congruence = word["congruence"]
        if congruence != "Top":
            if set(congruence) == {"Exact"}:
                assert int(congruence["Exact"], 16) == 7
            else:
                assert set(congruence) == {"Modulo"}
                modulo = congruence["Modulo"]
                modulus = int(modulo["modulus"], 16)
                assert modulus > 0 and 7 % modulus == int(modulo["residue"], 16)
        assert int(word["known_bits"]["zero"], 16) & 7 == 0
        assert int(word["known_bits"]["one"], 16) & 7 == int(word["known_bits"]["one"], 16)
        if "Constants" in word:
            assert "0x7" in word["Constants"]
        graph = word["expression"]
        assert graph["identity_scope"]
        nodes = graph["nodes"]
        read = nodes[graph["root"]]["WordRead"]
        assert nodes[read["offset"]] == {"Constant": "0x0"}
        view = nodes[read["view"]]["View"]
        assert nodes[view["length"]] == {"Constant": "0x20"}
        write = nodes[view["state"]]["WriteWord"]
        assert nodes[write["previous"]] == "BytesZero"
        assert nodes[write["value"]] == {"Constant": "0x7"}
        # Equality is between ExprIds in this graph, never two unrelated offsets.
        assert view["offset"] == write["offset"]
        return graph, view["state"]


    def effect_chain(ir, block):
        assert block["effect"] == {"result": 0, "inputs": []}
        for index, instruction in enumerate(block["instructions"]):
            assert instruction["fault"] is False
            assert instruction["effect_input"] == index
            assert instruction["effect_result"] == index + 1
        assert block["exit_effect"] == len(block["instructions"])
        assert ir["effect_count"] == block["exit_effect"] + 1


    def assert_logical_memory(payload):
        analysis = payload["analysis"] if "analysis" in payload else payload
        assert analysis["status"] == "Converged" and analysis["frontiers"] == []
        assert analysis["program"]["fork"] == "osaka"
        local = only(analysis["states"])
        assert local["executed_pcs"] == [0, 1, 2, 3, 5, 6, 7, 8]
        word = only(local["exit_stack"])
        graph, written = memory_word_seven(word)
        if "analysis" not in payload:
            return
        assert "partial_ssa" not in payload
        machine, ir = payload["machine_analysis"], payload["ssa"]
        assert machine["status"] == ir["status"] == "Converged"
        assert machine["frontiers"] == ir["frontiers"] == [] and ir["complete"] is True
        state, block = only(machine["states"]), only(ir["blocks"])
        assert payload["state_mapping"] == [{"local_state": local["id"], "machine_state": state["id"]}]
        assert state["exit_stack"] == [word]
        frame = state["exit"]["call_stack"]["root"]["state"]
        assert frame["stack"] == [word]
        memory = frame["memory"]["root"]
        assert memory["identity_scope"] == graph["identity_scope"] and memory["root"] == written
        assert memory["nodes"] == graph["nodes"][:len(memory["nodes"])]
        assert memory["nodes"][memory["root"]] == graph["nodes"][written]
        assert block["state"] == state["id"] and block["coverage"] == "Current"
        assert block["incoming_complete"] is True and block["open_incoming"] == []
        assert ir["transitions"] == ir["deferred_edges"] == []
        instructions = block["instructions"]
        assert [item["pc"] for item in instructions] == state["executed_pcs"] == local["executed_pcs"]
        assert all(item["progress"] == "Completed" for item in instructions[:-1])
        assert instructions[-1]["opcode"] == 0 and instructions[-1]["progress"] == "Dispatched"
        effect_chain(ir, block)
        write = only([item for item in instructions if item["pc"] == 6 and item["opcode"] == 82])
        read = only([item for item in instructions if item["pc"] == 7 and item["opcode"] == 81])
        offset = only(read["operands"])
        literal = only([item for item in instructions if item["pc"] == 3 and item["opcode"] == 96])
        assert literal["operation"] == {"Constant": {"value": "0x7"}}
        assert write["results"] == [] and write["operands"] == [offset, only(literal["results"])]
        assert write["operation"] == {"Opcode": {"opcode": 82, "operands": [{"Value": offset}, {"Constant": "0x7"}]}}
        assert read["operation"] == {"Opcode": {"opcode": 81, "operands": [{"Value": offset}]}}
        result = only(read["results"])
        assert block["exit_frames"] == [[result]]
        # This complete one-block capture exports the current read in Definition scope.
        fact = only([item for item in ir["facts"] if item["value"] == result
                     and item["scope"] == "Definition" and item["validity"] == "Current"
                     and item["source"] == "Instruction"])
        assert fact["provenance"] == word["provenance"]
        assert fact["numeric"] == {key: value for key, value in word.items()
                                   if key not in ("provenance", "expression")}
        assert fact["constant"] is None or fact["constant"] == "0x7"
        assert any(item["kind"] == "Failure" and item["state"] == state["id"] for item in machine["outcomes"])
        assert any(item["kind"] == "Return" and item["state"] == state["id"] for item in machine["outcomes"])


    def assert_retained_partial(payload):
        analysis, machine, ir = payload["analysis"], payload["machine_analysis"], payload["partial_ssa"]
        assert "ssa" not in payload and ir["complete"] is False
        assert analysis["status"] == machine["status"] == ir["status"] == "Incomplete"
        assert analysis["program"]["fork"] == "osaka"
        local, state, block = only(analysis["states"]), only(machine["states"]), only(ir["blocks"])
        assert local["id"] == state["id"] == block["state"] == 0
        assert payload["state_mapping"] == [{"local_state": 0, "machine_state": 0}]
        reason = {"Relations": "ExpressionLimit"}
        assert analysis["frontiers"] == [{"reason": reason, "from": 0,
            "target": {"basic_block_index": 0, "stack_height": 0, "context": []}, "limit": "Model"}]
        assert machine["frontiers"] == [{"from": 0, "target": state["key"], "pc": 4, "reason": reason, "range": None}]
        assert ir["frontiers"] == machine["frontiers"]
        assert analysis["edges"] == machine["edges"] == ir["transitions"] == ir["deferred_edges"] == []
        assert local["executed_pcs"] == state["executed_pcs"] == [0, 2, 4]
        assert local["exit_stack"] == state["exit_stack"] == []
        before = state["entry"]["call_stack"]["root"]["state"]["memory"]
        frame = state["exit"]["call_stack"]["root"]["state"]
        after = frame["memory"]
        assert frame["stack"] == [] and before["length"]["Constants"] == ["0x0"]
        # Expansion precedes refusal; the retained contents are still pristine.
        assert after["length"]["Constants"] == ["0x20"] and before["root"] == after["root"]
        for memory in (before, after):
            assert memory["memory"] is True and memory["default"]["Constants"] == ["0x0"]
            assert memory["root"]["nodes"][memory["root"]["root"]] == "BytesZero"
            assert memory["bytes"] == {} and memory["operands"] == memory["conditional"] == []
            assert memory["failure"] is None
        assert machine["outcomes"] and all(item["kind"] == "Failure" for item in machine["outcomes"])
        assert block["coverage"] == "Current" and block["incoming_complete"] is False
        assert block["open_incoming"] == [] and block["exit_frames"] == [[]]
        assert [(item["pc"], item["opcode"], item["progress"]) for item in block["instructions"]] == [
            (0, 96, "Completed"), (2, 96, "Completed"), (4, 82, "OperandsConsumed")]
        one, zero, write = block["instructions"]
        operands = [only(zero["results"]), only(one["results"])]
        assert len(set(operands)) == ir["value_count"] == 2
        assert write["operands"] == operands and write["results"] == []
        assert write["operation"] == {"Opcode": {"opcode": 82, "operands": [{"Constant": "0x0"}, {"Constant": "0x1"}]}}
        for instruction, literal in ((one, "0x1"), (zero, "0x0")):
            assert instruction["operation"] == {"Constant": {"value": literal}}
            fact = only([item for item in ir["facts"] if item["value"] == only(instruction["results"])
                         and item["scope"] == {"State": {"state": 0}} and item["validity"] == "Current"
                         and item["source"] == "Instruction"])
            assert fact["constant"] == literal and fact["numeric"]["Constants"] == [literal]
            assert fact["provenance"]["origins"] == {"Sources": ["Constant"]}
        effect_chain(ir, block)

    assert_logical_memory(json.loads(Path("memory-policy.json").read_text()))
    assert_logical_memory(json.loads(Path("memory-policy-ssa.json").read_text()))
    assert_retained_partial(json.loads(Path("expression-limit-ssa.json").read_text()))
    PY
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
