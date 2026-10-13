{
  pkgs,
  sourceRoot,
  build,
  dependencies,
  ...
}:
let
  package = build.release.package;
  inherit (dependencies) fontConfig;
  concreteInputs = "--evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x";
in
{
  examples =
    pkgs.runCommand "evm-abstract-installed-examples"
      {
        nativeBuildInputs = [
          package
          pkgs.graphviz
          pkgs.jq
          pkgs.python3
        ];
        FONTCONFIG_FILE = fontConfig;
      }
      ''
        set -euo pipefail
        export XDG_CACHE_HOME="$TMPDIR"
        for world in ${sourceRoot}/examples/worlds/*.json; do
          evm-abstract analyze --world "$world" ${concreteInputs} --format json |
            jq -e '.status == "Converged" and .world.fork == "osaka" and any(.edges[]; .kind == "Call")' > /dev/null
          evm-abstract analyze --world "$world" ${concreteInputs} --format json --ssa |
            jq -e '.analysis.status == "Converged" and (.ssa | type == "object")' > /dev/null
        done
        # Storage lessons use single-account worlds without requiring a Call edge.
        for world in ${sourceRoot}/examples/storage-*.json; do
          evm-abstract analyze --world "$world" ${concreteInputs} --format json --ssa |
            jq -e '.analysis.status == "Converged" and (.ssa | type == "object")' > /dev/null
        done
        evm-abstract analyze --world ${sourceRoot}/examples/storage-symbolic-key.json --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --format json |
          jq -e '.status == "Converged" and .states[0].exit.call_stack.root.state.stack[0].Constants == ["0x7"] and .states[0].exit.call_stack.root.state.stack[0].provenance.origins.Sources == ["Storage"]' > /dev/null
        evm-abstract analyze --world ${sourceRoot}/examples/storage-symbolic-value.json --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --format json |
          jq -e '.status == "Converged" and .states[0].exit.call_stack.root.state.stack[0].Constants == ["0x1"]' > /dev/null
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/missing-code.json ${concreteInputs} --format json --ssa > missing.json
        jq -e '.analysis.status == "Converged" and .analysis.frontiers == [] and (.analysis.unknown_summaries | length) > 0 and all(.analysis.unknown_summaries[]; .closed) and (.ssa | type == "object")' missing.json > /dev/null
        for world in ${sourceRoot}/examples/unknown-world/*.json; do
          name="$(basename "$world" .json)"
          evm-abstract analyze --world "$world" --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000000900 --evm.value 0 --evm.calldata 0x --format json --ssa > "$name-unknown.json"
          jq -e '.analysis.status == "Converged" and .analysis.frontiers == [] and (.analysis.unknown_summaries | length) > 0 and all(.analysis.unknown_summaries[]; .closed and (.procedures | length) > 0) and (.ssa | type == "object")' "$name-unknown.json" > /dev/null
          python3 - "$name" "$name-unknown.json" <<'PY'
        import json, sys
        name, path = sys.argv[1:]
        analysis = json.load(open(path))["analysis"]

        def number(value):
            return int(value, 0) if isinstance(value, str) else value

        def allowed_values(value, allowed):
            constants = value.get("Constants")
            if constants is not None:
                values = {number(item) for item in constants}
            else:
                interval = value["interval"]
                lo, hi = number(interval["unsigned_lo"]), number(interval["unsigned_hi"])
                assert 0 <= hi - lo <= 256, (name, "unbounded numerical result", value)
                bits = value["known_bits"]
                zero, one = number(bits["zero"]), number(bits["one"])
                values = {item for item in range(lo, hi + 1)
                          if item & zero == 0 and item & one == one}
                congruence = value["congruence"]
                if isinstance(congruence, dict) and "Exact" in congruence:
                    values &= {number(congruence["Exact"])}
                elif isinstance(congruence, dict) and "Modulo" in congruence:
                    relation = congruence["Modulo"]
                    values = {item for item in values
                              if item % number(relation["modulus"]) == number(relation["residue"])}
            assert values and values <= set(allowed), (name, values, allowed)
            return values

        def word_values(data, offset, allowed):
            for index in range(offset, offset + 31):
                allowed_values(data["bytes"][str(index)], [0])
            return allowed_values(data["bytes"][str(offset + 31)], allowed)

        def slot_value(store, owner, slot):
            plane = store["persistent"]
            for item in plane["slots"]:
                if item["address"] == owner and number(item["slot"]) == slot:
                    return item["value"]
            # An unchanged zero slot can be absent from the numerical cache.
            # Read its actual root; unsupported symbolic nodes fail the gate.
            root = plane["roots"][owner]
            nodes = root["nodes"]
            def read_node(index):
                node = nodes[index]
                if node == "SlotsZero":
                    return 0
                assert isinstance(node, dict) and "Store" in node, (name, "unsupported slot root", node)
                write = node["Store"]
                key = nodes[write["key"]]
                assert isinstance(key, dict) and "Constant" in key, (name, "symbolic slot key", key)
                if number(key["Constant"]) != slot:
                    return read_node(write["previous"])
                value = nodes[write["value"]]
                assert isinstance(value, dict) and "Constant" in value, (name, "symbolic stored value", value)
                return number(value["Constant"])
            return {"Constants": [hex(read_node(root["root"]))]}

        outcomes = analysis["outcomes"]
        owner_a = "0x0000000000000000000000000000000000000101"
        owner_c = "0x0000000000000000000000000000000000000301"
        if name.startswith("busy-x"):
            returns = [outcome for outcome in outcomes if outcome["kind"] == "Return"]
            assert returns, name
            possible = set()
            for outcome in returns:
                possible |= word_values(outcome["data"], 0, [0, 1, 2])
                word_values(outcome["data"], 32, [1])
                allowed_values(slot_value(outcome["store"], owner_a, 0), [0])
            assert possible == {0, 1, 2}, (name, possible)
        elif name == "outer-revert":
            assert outcomes and all(outcome["kind"] != "Return" for outcome in outcomes)
            reverted_values = set()
            for outcome in outcomes:
                allowed_values(slot_value(outcome["store"], owner_c, 0), [5])
                if outcome["kind"] == "Revert":
                    lengths = allowed_values(outcome["data"]["length"], [0, 64])
                    if lengths == {0}:
                        continue
                    assert lengths == {64}, (name, "ambiguous revert length", lengths)
                    word_values(outcome["data"], 0, [5])
                    reverted_values |= word_values(outcome["data"], 32, [5, 9])
            assert reverted_values == {5, 9}, (name, "missing old/new revert data", reverted_values)
        elif name == "static-reuse":
            returns = [outcome for outcome in outcomes if outcome["kind"] == "Return"]
            assert returns, name
            for outcome in returns:
                word_values(outcome["data"], 0, [42])
        elif name in {"external-guard", "external-change"}:
            returns = [outcome for outcome in outcomes if outcome["kind"] == "Return"]
            assert returns, name
            possible = set()
            for outcome in returns:
                word_values(outcome["data"], 0, [5])
                possible |= word_values(outcome["data"], 32, [5] if name == "external-guard" else [5, 9])
                allowed_values(slot_value(outcome["store"], owner_c, 1), [0] if name == "external-guard" else [1])
            assert possible == ({5} if name == "external-guard" else {5, 9}), (name, possible)
        else:
            raise AssertionError((name, "missing unknown-world numerical checks"))
        PY
        done
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/summary-reuse.json ${concreteInputs} --format json > summary-on.json
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/summary-reuse.json ${concreteInputs} --format json --no-summaries > summary-off.json
        jq -e '.status == "Converged" and .summary_stats.hits > 0 and .summary_stats.imported_states > 0 and any(.summaries[]; (.reused_at | length) > 0) and .world.identity.kind == "offline" and (.world.fingerprint | test("^0x[0-9a-f]{64}$"))' summary-on.json > /dev/null
        jq -e '.status == "Converged" and .summary_stats.hits == 0 and .summaries == []' summary-off.json > /dev/null
        # Producer namespaces are opaque across reports. Normalize the complete
        # reports bijectively before projecting, preserving all shared leaves.
        python3 ${sourceRoot}/scripts/normalize_identity_scopes.py < summary-on.json > normalized-on.json
        python3 ${sourceRoot}/scripts/normalize_identity_scopes.py < summary-off.json > normalized-off.json
        jq -S '[.outcomes[] | {kind,data,store}] | unique' normalized-on.json > on-relations.json
        jq -S '[.outcomes[] | {kind,data,store}] | unique' normalized-off.json > off-relations.json
        cmp on-relations.json off-relations.json
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/create-runtime.json ${concreteInputs} --format json > creation.json
        jq -e 'any(.states[]; .key.frames[-1].mode == "InitCode") and any(.states[]; .key.frames[-1].mode == "Runtime" and .key.frames[-1].code_hash == "0x30962a84ef989ca0f724a5b2ec94f9cbf6a731752ce2c0be5333bf96e460c9fd") and any(.outcomes[]; .store.nonces["0x0000000000000000000000000000000000000101"].Constants == ["0x1"] and any(.store.account_observations[]; .address == "0xea53a153a9a04fd632b2486d84732feb3b71afb7" and .existence == "present" and .code_size == 8 and .nonce.Constants == ["0x1"]))' creation.json > /dev/null
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/created-selfdestruct.json ${concreteInputs} --format json > destruction.json
        jq -e 'any(.states[]; .entry.store.pending_destruction["0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"] == true and .entry.store.codes["0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"].Runtime.byte_len == 8) and any(.outcomes[]; .kind == "Return" and .store.balances["0x0000000000000000000000000000000000000200"].Constants == ["0x7"] and any(.store.account_observations[]; .address == "0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb" and .existence == "absent" and .code_size == 0 and .nonce.Constants == ["0x0"]))' destruction.json > /dev/null
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/identity-precompile.json ${concreteInputs} --format json > native.json
        jq -e 'any(.states[]; .key.frames[-1].mode == {"Precompile":"0x0000000000000000000000000000000000000004"}) and any(.outcomes[]; .kind == "Return" and ((.data.bytes["31"].Constants // []) | index("0x2a")) != null)' native.json > /dev/null
        # Installed explain must cover actual call/creation code and preserve partial evidence.
        evm-abstract explain --world ${sourceRoot}/examples/worlds/call-return-branch.json ${concreteInputs} > explain-call.txt
        grep -q 'Execution code' explain-call.txt
        grep -q 'Captured instruction list' explain-call.txt
        grep -q 'Verified cross-contract SSA:' explain-call.txt
        grep -q 'CALL result %' explain-call.txt
        grep -Eq '%[0-9]+ = CONST 0x[0-9a-f]+ ; origin=(PUSH[0-9]+|MLOAD|EQ)' explain-call.txt
        if grep -q 'opcode=' explain-call.txt; then exit 1; fi
        evm-abstract explain --world ${sourceRoot}/examples/worlds/call-return-branch.json ${concreteInputs} --verbose > explain-verbose.txt
        grep -q 'captured frames:' explain-verbose.txt
        grep -q 'instruction effects:' explain-verbose.txt
        grep -q 'opcode=' explain-verbose.txt
        evm-abstract explain --world ${sourceRoot}/examples/worlds/create-runtime.json ${concreteInputs} > explain-create.txt
        grep -q 'mode=InitCode' explain-create.txt
        grep -q 'mode=Runtime' explain-create.txt
        grep -q 'CREATE address %' explain-create.txt
        evm-abstract explain --world ${sourceRoot}/examples/worlds/call-return-branch.json ${concreteInputs} --max-work 1 > explain-partial.txt && exit 1 || test "$?" = 2
        grep -q 'Input code observations (not execution evidence)' explain-partial.txt
        grep -q 'SSA unavailable' explain-partial.txt
        if grep -q 'Verified cross-contract SSA:' explain-partial.txt; then exit 1; fi
        # Packaged CLIs expose symbolic inputs and preserve the caller/origin alias.
        evm-abstract cfg --hex 33321400 --format json > symbolic-environment.json
        jq -e '
          .schema_version == 6 and .domain_spec.schema_version == 2 and
          .config.relations.enabled and (.config.relations | has("timeout_ms") | not) and
          .environment.to == {"Symbolic":"To"} and .environment.caller == {"Symbolic":"Caller"} and
          .environment.origin == null and .environment.value == "Top" and
          (.environment.calldata as $bytes | $bytes.length as $length |
            ($length | has("Constants") | not) and
            $length.known_bits == {"zero":"0x0","one":"0x0"} and
            $length.interval == {"unsigned_lo":"0x0","unsigned_hi":$max_word,"signed_lo":"0x0","signed_hi":$max_word} and
            $length.congruence == "Top" and $length.nonzero == false and
            $length.provenance == {"origins":"Top","code_address_role":false} and
            $bytes.bytes == {} and $bytes.memory == false and $bytes.failure == null and
            ($bytes.root.nodes[$bytes.root.root].BytesInput as $input |
              $input.bindings == [] and $input.source.namespace == "0x0" and
              $bytes.root.identity_scope == $length.expression.identity_scope and
              $bytes.root.nodes[$input.length] == $length.expression.nodes[$length.expression.root] and
              ($length.expression.nodes[$length.expression.root] | has("Fresh")))) and
          .states[0].exit_stack[0].Constants == ["0x1"]
        ' --arg max_word 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff symbolic-environment.json > /dev/null
        evm-abstract cfg --hex 3033363446484a00 ${concreteInputs} --evm.chain-id 56 --evm.basefee 7 --evm.blob-basefee 8 --format json > concrete-environment.json
        jq -e '[.states[0].exit_stack[].Constants[0]] == ["0x101","0x1000","0x0","0x0","0x38","0x7","0x8"]' concrete-environment.json > /dev/null
        evm-abstract explain --file ${sourceRoot}/examples/straight-line.hex > explain-program.txt
        grep -q 'stack SSA:' explain-program.txt
        evm-abstract explain --file ${sourceRoot}/examples/known-bits-branch.hex --context-depth 0 > explain-aligned.txt
        grep -q '^B₀ @ 0x0000:$' explain-aligned.txt
        grep -q '^       0000: PUSH0' explain-aligned.txt
        evm-abstract ssa --file ${sourceRoot}/examples/dynamic-jump.hex > ssa-aligned.txt
        grep -q '^σᵖ₀ | context=\[\]:$' ssa-aligned.txt
        grep -q '^B₀ @ 0x0000:$' ssa-aligned.txt
        grep -q '^       0000: %0 = CONST 0x0 ; origin=PUSH1$' ssa-aligned.txt
        grep -q '^       0002: %1 = CALLDATALOAD 0x0$' ssa-aligned.txt
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/call-return-branch.json ${concreteInputs} --ssa > ssa-world-aligned.txt
        grep -q '^       0000: %0 = CONST 0x20 ; origin=PUSH1 | opcode=0x60' ssa-world-aligned.txt
        if grep -Eq '^ +pc=0x[0-9a-f]+:' ssa-world-aligned.txt; then exit 1; fi
        # Alternation matches complete subscript digits even under the C locale.
        for report in explain-call.txt explain-verbose.txt explain-create.txt explain-partial.txt; do
          grep -Eq '^B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' "$report"
          if grep -Eq '^ +B(₀|₁|₂|₃|₄|₅|₆|₇|₈|₉)+ @ 0x[0-9a-f]+:$' "$report"; then exit 1; fi
        done
        evm-abstract explain --hex 5f351e00 --domain constants-only --max-constants 257 > explain-capacity.txt
        grep -q 'status=Converged' explain-capacity.txt
        evm-abstract cfg --hex 5f351e00 --domain constants-only --max-constants 257 --format json |
          jq -e '.config.max_constants == 257 and (.states[0].exit_stack[0].Constants | length == 257)' > /dev/null
        evm-abstract explain --hex 5f351e00 --domain constants-only --max-constants 1000 --max-work 20000000 > explain-capacity-work.txt && exit 1 || test "$?" = 2
        grep -q 'status=Incomplete' explain-capacity-work.txt
        grep -q 'Work' explain-capacity-work.txt
        grep -q 'SSA unavailable' explain-capacity-work.txt
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/summary-reuse.json ${concreteInputs} > summary.txt
        grep -Eq 'hits=[1-9][0-9]*' summary.txt
        grep -q 'reused_at=' summary.txt
        grep -q 'fingerprint=0x' summary.txt
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/summary-reuse.json ${concreteInputs} --format dot > summaries.dot
        dot -Tsvg summaries.dot -o summaries.svg
        test -s summaries.svg
        grep -q 'label="reused"' summaries.dot
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/create-runtime.json ${concreteInputs} --format dot > creation.dot
        dot -Tsvg creation.dot -o creation.svg
        test -s creation.svg
        evm-abstract analyze --world ${sourceRoot}/examples/worlds/proxy-storage.json ${concreteInputs} --format dot > proxies.dot
        dot -Tsvg proxies.dot -o proxies.svg
        test -s proxies.svg
        for fixture in ${sourceRoot}/examples/*.hex; do
          # An unbounded memory address also covers inputs beyond the declared
          # policy. Keep its pre-effect frontier and the successful same-address
          # read instead of claiming the entire input space converged.
          case "$fixture" in
            */memory-symbolic-offset.hex)
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
              continue
              ;;
          esac
          evm-abstract cfg --file "$fixture" --format json | jq -e '.status == "Converged" and .program.fork == "osaka"' > /dev/null
          evm-abstract ssa --file "$fixture" --format json | jq -e '.ssa.value_count > 0' > /dev/null
        done
        evm-abstract cfg --file ${sourceRoot}/examples/osaka-clz.hex --format json |
          jq -e '.edges | length == 1' > /dev/null
        for fork in cancun prague; do
          evm-abstract cfg --file ${sourceRoot}/examples/osaka-clz.hex --fork "$fork" --format json |
            jq -e '.edges == [] and any(.diagnostics[]; .kind.InvalidOpcode.Instruction == {"opcode":30,"mnemonic":"CLZ","reason":{"UnavailableInFork":$input_fork}} and .pc == 2)' --arg input_fork "$fork" > /dev/null
        done
        evm-abstract cfg --file ${sourceRoot}/examples/internal-calls.hex --context-depth 1 --format json |
          jq -e '[.states[] | select(.key.context != [])] | length > 0' > /dev/null
        evm-abstract cfg --file ${sourceRoot}/examples/diamond.hex --format dot > diamond.dot
        dot -Tsvg diamond.dot -o diamond.svg
        test -s diamond.svg
        mkdir $out
        cp diamond.dot diamond.svg proxies.dot proxies.svg summaries.dot summaries.svg creation.dot creation.svg summary-on.json summary-off.json creation.json destruction.json native.json memory-policy.json memory-policy-ssa.json summary.txt explain-call.txt explain-verbose.txt explain-create.txt explain-partial.txt explain-program.txt explain-aligned.txt ssa-aligned.txt ssa-world-aligned.txt explain-capacity.txt explain-capacity-work.txt $out/
      '';
}
