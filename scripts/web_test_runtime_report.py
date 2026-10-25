"""Pure current-report checks, usable without Playwright, a GPU or a server.

Browser cases call these on their newly received report. The offline entrypoint
accepts those same artifacts; it does not convert historical output into current
execution evidence. Negative mutations are always restored before returning.
"""
import hashlib
import json
from collections import Counter
from web_test_hybrid import oracle_graph, source_block as resolve_source_block


def _runtime_numeric_contains(value, actual):
    # Keep this local: the runtime cases' exact manifest closure intentionally
    # excludes the separate RPC wire-byte fixture module.
    word = lambda text: int(text, 16)
    constants = value["constants"]
    zero, one = word(value["known_zero"]), word(value["known_one"])
    unsigned, signed = value["unsigned"], value["signed"]
    sign = 1 << 255
    congruence = value["congruence"]
    if congruence == "Any":
        congruent = True
    elif "Exact" in congruence:
        congruent = actual == word(congruence["Exact"])
    else:
        modulo = congruence["Modulo"]
        congruent = actual % word(modulo["modulus"]) == word(modulo["residue"])
    return (0 <= actual < 2**256 and (constants is None or actual in map(word, constants))
            and actual & zero == 0 and actual & one == one
            and word(unsigned["lower"]) <= actual <= word(unsigned["upper"])
            and word(signed["lower"]) ^ sign <= actual ^ sign <= word(signed["upper"]) ^ sign
            and (not value["nonzero"] or actual != 0) and congruent)


def layout_runtime_programs(report, code, metadata):
    """Decode the complete captured source independently of CFG reachability."""
    names = {}
    for start, row in [
        (0x00, "STOP ADD MUL SUB DIV SDIV MOD SMOD ADDMOD MULMOD EXP SIGNEXTEND"),
        (0x10, "LT GT SLT SGT EQ ISZERO AND OR XOR NOT BYTE SHL SHR SAR CLZ"),
        (0x20, "KECCAK256"),
        (0x30, "ADDRESS BALANCE ORIGIN CALLER CALLVALUE CALLDATALOAD CALLDATASIZE CALLDATACOPY CODESIZE CODECOPY GASPRICE EXTCODESIZE EXTCODECOPY RETURNDATASIZE RETURNDATACOPY EXTCODEHASH"),
        (0x40, "BLOCKHASH COINBASE TIMESTAMP NUMBER DIFFICULTY GASLIMIT CHAINID SELFBALANCE BASEFEE BLOBHASH BLOBBASEFEE"),
        (0x50, "POP MLOAD MSTORE MSTORE8 SLOAD SSTORE JUMP JUMPI PC MSIZE GAS JUMPDEST TLOAD TSTORE MCOPY PUSH0"),
        (0xa0, "LOG0 LOG1 LOG2 LOG3 LOG4"),
        (0xf0, "CREATE CALL CALLCODE RETURN DELEGATECALL CREATE2"),
        (0xfa, "STATICCALL"), (0xfd, "REVERT INVALID SELFDESTRUCT"),
    ]:
        names.update((start + i, name) for i, name in enumerate(row.split()))
    for start, prefix, count in [(0x60, "PUSH", 32), (0x80, "DUP", 16), (0x90, "SWAP", 16)]:
        names.update((start + i, f"{prefix}{i+1}") for i in range(count))
    assert report["fork"] == metadata["browser_entry"].get("fork", "Osaka")
    assert report["scope"] == "SingleProgram" and report["metadata"]["snapshot"] is None
    programs = {program["id"]: program for program in report["programs"]}
    assert len(programs) == len(report["programs"]) and programs
    assert all(type(program_id) is int and program_id >= 0 for program_id in programs)
    root = programs[report["metadata"]["root_program"]]
    assert root["kind"] == "Runtime" and root["code_hash"] == metadata["code_hash"]
    assert root["bytecode"].removeprefix("0x") == code
    assert report["byte_len"] == metadata["byte_length"] == len(bytes.fromhex(code))
    assert report["disassembly"] == root["blocks"]
    for program in programs.values():
        assert program["fork"] == report["fork"]
        raw = bytes.fromhex(program["bytecode"].removeprefix("0x"))
        expected = []; starts = []; pc = 0; new_block = True
        while pc < len(raw):
            opcode = raw[pc]
            width = opcode - 0x5f if 0x60 <= opcode <= 0x7f else 0
            immediate = (hex(int.from_bytes(raw[pc+1:pc+1+width].ljust(width, b"\0"), "big"))
                         if width else "0x0" if opcode == 0x5f else None)
            valid = opcode in names and opcode != 0xfe
            if new_block or opcode == 0x5b:
                starts.append(pc)
            expected.append((pc, opcode, names.get(opcode, "Unknown"), immediate, width+1, valid))
            new_block = not valid or opcode in {0x00, 0x56, 0x57, 0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xfa, 0xfd, 0xff}
            pc += width + 1
        blocks = program["blocks"]
        assert [block["id"] for block in blocks] == list(range(len(blocks)))
        assert [block["start_pc"] for block in blocks] == starts
        assert all(block["instructions"] and block["start_pc"] == block["instructions"][0]["pc"] for block in blocks)
        actual = [instruction for block in blocks for instruction in block["instructions"]]
        assert [tuple(instruction[key] for key in ("pc", "opcode", "name", "immediate", "size", "valid"))
                for instruction in actual] == expected


def layout_runtime_request(request, report, code, metadata, expected_limits):
    entry = metadata["browser_entry"]
    assert report["schema_version"] == 21 and report["root_terminal_coverage"] is None
    assert set(request) == {"input", "fork", "environment", "limits"}
    assert request["input"] == {"Bytecode": {"bytecode": code, "address": entry.get("address")}}
    assert request["fork"] == report["fork"] == entry.get("fork", "Osaka")
    assert expected_limits["max_work"] == 1_000_000_000_000
    assert request["limits"] == report["metadata"]["limits"] == expected_limits
    assert request["limits"]["root_terminal_coverage"] is report["metadata"]["limits"]["root_terminal_coverage"] is False
    expected = {"caller": "Unknown", "origin": None, "call_value": "Unknown",
                "calldata": {"Exact": entry["calldata"]}, "is_static": False,
                "gas_upper_bound": None, "gas_price": "Unknown", "coinbase": None,
                "timestamp": None, "number": None, "prevrandao": None, "gas_limit": None,
                "chain_id": entry.get("chain_id"), "base_fee": None, "blob_base_fee": None,
                "block_hashes": [], "blob_hashes": [], "blob_count": "Unknown"}
    for field in ["caller", "call_value"]:
        if field in entry:
            expected[field] = {"Concrete": entry[field]}
    assert request["environment"] == expected
    environment = report["metadata"]["environment"]
    assert environment["is_static"] is False and environment["gas_upper_bound"] is None
    if "address" in entry:
        assert environment["to"]["Concrete"].lower() == entry["address"].lower()
    else:
        assert environment["to"] == {"Symbolic": {"kind": "To", "index": None}}
    if "caller" not in entry:
        assert environment["caller"] == {"Symbolic": {"kind": "Caller", "index": None}}
    if "call_value" not in entry:
        # The exact POST is authoritative. The initial projection can retain
        # top numeric facts without an immutable equality identity.
        value = environment["call_value"]
        assert value["constants"] is None
        assert all(_runtime_numeric_contains(value, item) for item in [0, 2**255-1, 2**255, 2**256-1])
        if value["identity"] is not None: assert value["identity"]["symbol"]["kind"] == "CallValue"
    if "chain_id" in entry:
        assert environment["chain_id"]["constants"] == [hex(int(entry["chain_id"], 16))]
    calldata = report["byte_arrays"][environment["calldata"]]
    raw = bytes.fromhex(entry["calldata"].removeprefix("0x"))
    assert calldata["length"]["constants"] == [hex(len(raw))]
    cells = {cell["offset"]: calldata["values"][cell["value"]] for cell in calldata["cells"]}
    for offset, byte in enumerate(raw):
        value = cells.get(offset, calldata["default"])
        assert value["constants"] == [hex(byte)] and _runtime_numeric_contains(value, byte)
    assert metadata["rpc_block_selector"] == {"blockHash": metadata["block_hash"], "requireCanonical": True}
    if "abi_validation" in entry:
        # The saved concrete witness stays independent of abstract execution.
        witness = entry["abi_validation"]
        assert witness["method"] == "eth_call" and witness["success"] is True
        assert witness["returned_bytes"] == 1344 and witness["requireCanonical"] is True
        assert witness["block_hash"] == metadata["block_hash"]
    return {"request_sha256": hashlib.sha256(json.dumps(request, sort_keys=True).encode()).hexdigest(),
            "bytecode_sha256": hashlib.sha256(bytes.fromhex(code)).hexdigest(),
            "calldata_sha256": hashlib.sha256(raw).hexdigest(), "root_terminal_coverage": False}


def _runtime_boundary_profile(report, entry, nodes, expected_limits):
    """Recognize exact current boundary sets; saved legacy oracles stay strict."""
    policy = entry["product_oracle"]
    frontiers = report["frontiers"]
    starts = {nodes[frontier["from"]]["start_pc"] for frontier in frontiers
              if frontier["from"] in nodes}
    profile = None
    if report["schema_version"] in {17,18,19,20,21} and policy["boundary"] == "root_instruction" and 3634 in starts:
        profile = "copy"
        expected = [(3634, 3645, "Relations"), (3634, 3638, "Relations")]
        assert len({frontier["from"] for frontier in frontiers}) == 1
    elif report["schema_version"] in {17,18,19,20,21} and policy["boundary"] == "incoming" and any(
            frontier["kind"] == "Work" for frontier in frontiers):
        profile = "router"
        expected = [(877, 884, "Relations"), (16597, 16802, "Work"),
                    (None, None, "Work"), (None, None, "Work")]
    if profile is None:
        return None
    assert report["metadata"]["limits"] == expected_limits
    assert report["scope"] == "SingleProgram" and report["root_terminal_coverage"] is None
    actual = []
    for frontier in frontiers:
        source = nodes.get(frontier["from"])
        assert frontier["range"] is None and frontier["payload"] is None
        assert frontier["reason"] == ({"Relations": "ExpressionLimit"}
                                      if frontier["kind"] == "Relations" else "Work")
        assert frontier["detail"] == ("Relations(ExpressionLimit)" if frontier["kind"] == "Relations" else "Work")
        actual.append((source["start_pc"] if source else None, frontier["pc"], frontier["kind"]))
    assert Counter(actual) == Counter(expected), actual
    return profile


def _runtime_current_prefix(report, source, state, block):
    """Cross-check current SSA IDs, phases and numeric/source facts against native frames."""
    sid = source["id"]
    decoded = resolve_source_block(report, source)["instructions"]
    instructions = block["instructions"]
    assert [item["pc"] for item in instructions] == source["executed_pcs"]
    assert source["executed_pcs"] == [item["pc"] for item in decoded[:len(instructions)]]
    assert instructions and instructions[-1]["progress"] == "OperandsConsumed"
    assert all(item["progress"] == "Completed" for item in instructions[:-1])
    assert source["frame_depth"] == len(state["entry"]["frames"]) == len(state["exit"]["frames"]) == 1
    before, after = state["entry"]["frames"][0], state["exit"]["frames"][0]
    stable = set(before) - {"stack", "memory"}
    assert all(before[key] == after[key] for key in stable)
    assert before["context"] == source["context"]
    assert before["code_address"].lower() == source["code_address"].lower()
    assert before["storage_address"].lower() == source["storage_address"].lower()
    assert before["storage_address"].lower() == before["code_address"].lower()
    assert before["address_value"] == report["metadata"]["environment"]["to"]
    assert before["caller"] == report["metadata"]["environment"]["caller"]
    assert not before["is_static"] and before["kind"] == "Runtime"
    assert state["entry"]["store"] == state["exit"]["store"]
    # This receipt contains only local stack/memory operations, never a hidden
    # call, storage mutation, log, successful termination or dispatched effect.
    assert not {item["opcode"] for item in instructions}.intersection(
        {0x00, 0x55, 0x5d, *range(0xa0, 0xa5), 0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xfa, 0xfd, 0xff})
    values = {value["id"]: value for value in report["ssa"]["values"]}
    assert len(values) == len(report["ssa"]["values"])
    # SSA observations deliberately retain numeric/provenance layers rather
    # than the native value's optional expression or equality identity.
    fields = {"summary", "constants", "known_zero", "known_one", "nonzero", "unsigned",
              "signed", "congruence", "origins", "code_address_role"}

    def observed(vid, native, origin, observation_source):
        value = values[vid]
        assert value["origin"] == origin
        facts = [fact for fact in value["facts"] if fact["status"] == "Current"
                 and fact["scope"] == {"State": sid} and fact["source"] == observation_source]
        assert len(facts) == 1
        if native is not None:
            assert all(facts[0]["value"][key] == native[key] for key in fields)
        return facts[0]["value"]

    def rendered(vid):
        origin = values[vid]["origin"].get("Instruction")
        if origin is not None and origin["state"] == sid:
            definition = instructions[origin["instruction_index"]]
            if definition["immediate"] is not None:
                assert vid in definition["results"]
                return {"Constant": definition["immediate"]}
        return {"Value": vid}

    phis = {(phi["frame"], phi["slot"]): phi for phi in block["phis"]}
    assert len(phis) == len(block["phis"]) == len(before["stack"])
    stack = []
    for slot, native in enumerate(before["stack"]):
        phi = phis[0, slot]
        observed(phi["result"], native, {"Phi": {"state": sid, "frame": 0, "slot": slot}}, "Entry")
        stack.append(phi["result"])
    effect = block["effect"]
    effects = {effect}
    for order, (item, native) in enumerate(zip(instructions, decoded, strict=False)):
        assert all(item[key] == native[key] for key in ["pc", "opcode", "name", "immediate"])
        assert not item["fault"] and item["effect_input"] == effect
        assert type(item["effect_result"]) is int and 0 <= item["effect_result"] < report["ssa"]["effect_count"]
        assert item["effect_result"] not in effects
        effect = item["effect_result"]
        effects.add(effect)
        op = item["opcode"]
        if 0x80 <= op <= 0x8f:
            assert item["operands"] == [stack[-(op - 0x7f)]] and not item["results"]
            stack.append(item["operands"][0])
        elif 0x90 <= op <= 0x9f:
            other = -(op - 0x8f + 1)
            assert item["operands"] == [stack[-1], stack[other]] and not item["results"]
            stack[-1], stack[other] = stack[other], stack[-1]
        else:
            count = native["stack_inputs"]
            operands = list(reversed(stack[-count:])) if count else []
            assert item["operands"] == operands
            if count:
                del stack[-count:]
            results = native["stack_outputs"] if item["progress"] == "Completed" else 0
            assert len(item["results"]) == results
            for vid in item["results"]:
                fact = observed(vid, None, {"Instruction": {"state": sid, "instruction_index": order,
                                                          "pc": item["pc"]}}, "Execution")
                if item["immediate"] is not None:
                    assert _runtime_numeric_contains(fact, int(item["immediate"], 16))
                    assert fact["origins"] == ["Constant"]
            stack.extend(item["results"])
        if item["immediate"] is None:
            assert item["operation"] == {"Opcode": {"opcode": op, "name": item["name"],
                                                    "operands": [rendered(vid) for vid in item["operands"]]}}
        else:
            assert item["operation"] == {"Constant": item["immediate"]}
    assert block["exit_frames"] == [stack] and len(stack) == len(after["stack"])
    assert effect == block["exit_effect"]
    for vid, native in zip(stack, after["stack"], strict=True):
        origin = values[vid]["origin"]
        observed(vid, native, origin, "Entry" if "Phi" in origin else "Execution")
    return before, after


def layout_pending_machines(report, entry, expected_limits):
    # Schema 14 is used only by unchanged saved-report offline controls;
    # layout_runtime_request requires schema 21 for every browser submission.
    assert report["schema_version"] in {14,17,18,19,20,21}
    policy = entry["product_oracle"]
    assert report["status"] == entry["expected_status"] == "Incomplete" and report["frontiers"]
    pending = report["pending_machines"]
    pending_frontiers = [frontier for frontier in report["frontiers"] if frontier["payload"] is not None]
    assert all(type(frontier["payload"]) is int for frontier in pending_frontiers)
    assert len(pending_frontiers) == len(pending)
    assert {frontier["payload"] for frontier in pending_frontiers} == set(range(len(pending)))
    nodes = {node["id"]:node for node in report["cfg"]}
    states = {state["state"]:state for state in report["states"]}
    ssa_blocks = {block["state"]:block for block in report["ssa"]["blocks"]}
    programs = {program["id"]:program for program in report["programs"]}
    edges = {edge["id"]:edge for edge in report["edges"]}
    transitions = {item["edge"]:item for item in report["ssa"]["transitions"]}
    deferred = {item["edge"]:item["reason"] for item in report["ssa"]["deferred_edges"]}
    assert len(nodes) == len(report["cfg"]) == len(states) == len(report["states"]) == len(ssa_blocks) == len(report["ssa"]["blocks"])
    assert set(nodes) == set(states) == set(ssa_blocks)
    assert len(edges) == len(report["edges"]) and all(edge["from"] in nodes and edge["to"] in nodes for edge in edges.values())
    assert len(transitions) == len(report["ssa"]["transitions"]) and len(deferred) == len(report["ssa"]["deferred_edges"])
    assert not set(transitions) & set(deferred) and set(transitions) | set(deferred) == set(edges)
    assert all(ssa_blocks[edges[index]["from"]]["coverage"] == "Current" for index,reason in deferred.items() if reason == "NotInExecutionEvidence")
    profile = _runtime_boundary_profile(report, entry, nodes, expected_limits)
    graph = report["symbolic"]["nodes"]
    word_kinds = {"Constant","Input","Fresh","Operation","Ite","ByteRead","WordRead","SlotRead","Keccak"}
    byte_kinds = {"BytesZero","BytesInput","BytesExact","WriteByte","WriteWord","Copy","Slice","BytesIte"}
    slot_kinds = {"SlotsZero","SlotsInput","Store","SlotsIte"}
    path_kinds = {"PathTrue","PathFalse","PathAtoms","PathAnd","PathIte"}
    def reference(value, kinds):
        assert set(value) == {"node"}
        index = value["node"]
        assert type(index) is int and 0 <= index < len(graph)
        node = graph[index]
        assert (node if isinstance(node,str) else next(iter(node))) in kinds
    def facts(value):
        assert set(value) == {"summary","constants","known_zero","known_one","nonzero","unsigned","signed",
                              "congruence","origins","code_address_role","identity","expression","symbolic_limit"}
        words = [value["known_zero"],value["known_one"],*value["unsigned"].values(),*value["signed"].values()]
        words.extend(value["constants"] or [])
        assert all(isinstance(word,str) and word.startswith("0x") and 0 <= int(word,16) < 2**256 for word in words)
        if value["expression"] is not None:reference(value["expression"],word_kinds)
    def pool(index, values):
        assert type(index) is int and 0 <= index < len(values)
        return values[index]
    buffers = set(); stores = set()
    def machine_snapshot(machine):
        assert set(machine) == {"frames","store","path"} and machine["frames"]
        reference(machine["path"],path_kinds)
        pool(machine["store"],report["stores"]);stores.add(machine["store"])
        for index,frame in enumerate(machine["frames"]):
            assert set(frame) == {"process","process_operands","process_creation","index","program","code_address","code_hash","storage_address","address_value",
                                  "caller","address_operand","caller_operand","call_value","is_static","kind","basic_block","context","stack",
                                  "memory","calldata","returndata","rollback_store","continuation"}
            assert frame["process"] is None and frame["process_operands"] is None and frame["process_creation"] is None
            assert frame["index"] == index and frame["program"] in programs
            program = programs[frame["program"]]
            assert frame["code_hash"] == program["code_hash"] and frame["kind"] == program["kind"]
            assert frame["code_address"].lower() == program["code_address"].lower()
            pool(frame["basic_block"],program["blocks"])
            facts(frame["call_value"])
            for name in ["address_operand","caller_operand"]:
                if frame[name] is not None: facts(frame[name])
            for value in frame["stack"]:facts(value)
            for name in ["memory","calldata","returndata"]:
                pool(frame[name],report["byte_arrays"]);buffers.add(frame[name])
            pool(frame["rollback_store"],report["stores"]);stores.add(frame["rollback_store"])
            if frame["continuation"] is not None:
                facts(frame["continuation"]["output_offset"]);facts(frame["continuation"]["output_size"])
        active = machine["frames"][-1]
        assert any(node["program"] == active["program"] and node["basic_block"] == active["basic_block"]
                   and node["frame_depth"] == len(machine["frames"]) for node in nodes.values())
        return active
    pending_locations = []; instruction_frontiers = []; retained_frontiers = []; work_frontiers = []
    for frontier in report["frontiers"]:
        if profile == "router" and frontier["from"] is None:
            # Queue obligations carry no instruction or owning frame. Never
            # reinterpret their absent location as the current memory opcode.
            assert frontier == {"from": None, "pc": None, "kind": "Work", "reason": "Work",
                                "detail": "Work", "range": None, "payload": None}
            work_frontiers.append({"source": None, "pc": None, "progress": None, "kind": "Work"})
            continue
        assert frontier["from"] in nodes and frontier["range"] is None
        if profile == "router" and frontier["kind"] == "Work":
            assert frontier["reason"] == frontier["detail"] == "Work"
        else:
            assert frontier["kind"] == "Relations" and frontier["reason"] == {"Relations":"ExpressionLimit"}
            assert frontier["detail"] == f"{frontier['kind']}({frontier['reason'][frontier['kind']]})"
        source = nodes[frontier["from"]]
        state = states[source["id"]]; block = ssa_blocks[source["id"]]
        assert source["program"] == report["metadata"]["root_program"] and source["frame_depth"] == 1
        assert block["coverage"] == "Current" and not block["incoming_complete"]
        for name in ["entry","exit"]:
            active = machine_snapshot(state[name])
            assert (active["program"],active["basic_block"],len(state[name]["frames"])) == (
                source["program"],source["basic_block"],source["frame_depth"])
        instruction = block["instructions"][-1]
        source_block = resolve_source_block(report, source)
        assert source["start_pc"] == source_block["start_pc"] and "instructions" not in source
        decoded = next(item for item in source_block["instructions"] if item["pc"] == instruction["pc"])
        assert all(instruction[key] == decoded[key] for key in ["opcode","name","immediate"])
        assert instruction["pc"] == source["executed_pcs"][-1]
        if profile is not None:
            before, after = _runtime_current_prefix(report, source, state, block)
            if profile == "copy":
                assert (source["start_pc"], instruction["pc"], instruction["opcode"], instruction["name"]) == (
                    3634, 3638, 0x37, "CALLDATACOPY")
                assert len(instruction["operands"]) == 3 and before["stack"] == after["stack"]
                memory_before = pool(before["memory"], report["byte_arrays"])
                memory_after = pool(after["memory"], report["byte_arrays"])
                assert all(memory_before[key] == memory_after[key] for key in set(memory_before) - {"length", "symbolic"})
                assert all(memory_before["length"][key] == memory_after["length"][key]
                           for key in set(memory_before["length"]) - {"expression"})
                views = [graph[memory["symbolic"]["node"]]["View"] for memory in [memory_before, memory_after]]
                assert views[0]["state"] == views[1]["state"] and views[0]["offset"] == views[1]["offset"]
                assert all(view["length"] == memory["length"]["expression"]
                           for view, memory in zip(views, [memory_before, memory_after], strict=True))
                if frontier["pc"] != instruction["pc"]:
                    # Frontiers accumulate across visits; this latest receipt
                    # stops earlier. The retained MSTORE has no current phase,
                    # operands/results or effect to borrow from CALLDATACOPY.
                    assert frontier["pc"] == 3645
                    retained = next(item for item in source_block["instructions"] if item["pc"] == frontier["pc"])
                    assert (retained["opcode"], retained["name"], retained["stack_inputs"], retained["stack_outputs"]) == (
                        0x52, "MSTORE", 2, 0)
                    assert all(item["pc"] != frontier["pc"] for item in block["instructions"])
                    retained_frontiers.append({"source_start_pc": source["start_pc"], "pc": frontier["pc"],
                                               "opcode": retained["opcode"], "name": retained["name"],
                                               "current_progress": None, "current_stop_pc": instruction["pc"]})
                    continue
            elif instruction["name"] == "MSTORE":
                assert (source["start_pc"], instruction["pc"]) == (877, 884)
                assert before["stack"] == after["stack"]
                assert pool(before["memory"], report["byte_arrays"]) == pool(after["memory"], report["byte_arrays"])
        if frontier["payload"] is not None:
            assert policy["boundary"] == "incoming" and frontier["pc"] is None
            machine = pool(frontier["payload"],pending)
            active = machine_snapshot(machine)
            targets = [node for node in nodes.values() if (node["program"],node["basic_block"],node["frame_depth"],
                       node["context"],node["code_address"],node["storage_address"]) == (active["program"],active["basic_block"],
                       len(machine["frames"]),active["context"],active["code_address"],active["storage_address"])]
            assert len(targets) == 1
            target = targets[0]; machine_snapshot(states[target["id"]]["entry"])
            incoming = [edge for edge in edges.values() if edge["from"] == source["id"] and edge["to"] == target["id"]]
            assert len(incoming) == 1 and instruction["progress"] == "Dispatched"
            edge = incoming[0]
            assert target["program"] == source["program"] and target["frame_depth"] == 1
            source_frames = state["exit"]["frames"]; target_frames = states[target["id"]]["entry"]["frames"]
            assert [len(frame["stack"]) for frame in machine["frames"]] == [len(frame["stack"]) for frame in source_frames] == [len(frame["stack"]) for frame in target_frames]
            for frame,before,after in zip(machine["frames"],source_frames,target_frames,strict=True):
                assert all(frame[key] == before[key] == after[key] for key in ["program","code_address","code_hash",
                           "storage_address","kind","is_static","address_value","caller"])
            location = {"source_pc":instruction["pc"],"source_name":instruction["name"],
                        "edge_kind":edge["kind"],"target_start_pc":target["start_pc"]}
            if edge["id"] in deferred:
                assert deferred[edge["id"]] == "NotInExecutionEvidence"
                location["deferred_reason"] = deferred[edge["id"]]
                assert active == {**source_frames[-1],"basic_block":active["basic_block"]}
            else:
                assert transitions[edge["id"]]["kind"] == edge["kind"]
            pending_locations.append(location)
            continue
        assert (policy["boundary"] == "root_instruction" or profile == "router") and frontier["pc"] is not None
        assert instruction["pc"] == frontier["pc"] == source["executed_pcs"][-1]
        expected = ((0x37, "CALLDATACOPY", 3) if profile == "copy" else
                    (0x51, "MLOAD", 1) if profile == "router" and frontier["kind"] == "Work" else
                    (0x52, "MSTORE", 2))
        assert (instruction["opcode"], instruction["name"], len(instruction["operands"])) == expected
        assert instruction["progress"] == "OperandsConsumed" and not instruction["fault"] and not instruction["results"]
        # The exit snapshot is after operand consumption, not a resumable
        # pre-instruction checkpoint or a claim that its ordinary effect ran.
        assert instruction["effect_result"] is not None and instruction["effect_result"] == block["exit_effect"]
        assert [len(frame) for frame in block["exit_frames"]] == [len(frame["stack"]) for frame in state["exit"]["frames"]]
        # Earlier gas-failure alternatives do not complete the refused opcode.
        for outcome in report["outcomes"]:
            if outcome["state"] == source["id"]:
                data = pool(outcome["data"],report["byte_arrays"])
                assert outcome["kind"] == "Failure" and data["length"]["constants"] == ["0x0"] and not data["cells"]
        outgoing = [edge for edge in report["edges"] if edge["from"] == frontier["from"]]
        if profile is None or instruction["name"] == "MSTORE":
            assert outgoing
        else:
            assert not outgoing
        edge_ids = {edge["id"] for edge in outgoing}
        assert len(edge_ids) == len(outgoing)
        assert not edge_ids.intersection(transition["edge"] for transition in report["ssa"]["transitions"])
        assert all(deferred.get(edge) == "NotInExecutionEvidence" for edge in edge_ids)
        location = {"source_start_pc":source["start_pc"],"source_frame_depth":source["frame_depth"],
                                      "pc":instruction["pc"],"opcode":instruction["opcode"],"name":instruction["name"],
                                      "progress":instruction["progress"],"coverage":block["coverage"],
                                      "deferred_edge_kinds":sorted(edge["kind"] for edge in outgoing)}
        if frontier["kind"] == "Work":
            work_frontiers.append(location)
        else:
            instruction_frontiers.append(location)
    if profile == "copy":
        assert not pending and not pending_locations and not work_frontiers
        assert len(instruction_frontiers) == len(retained_frontiers) == 1
    elif profile == "router":
        assert not pending and not pending_locations and not retained_frontiers
        assert len(instruction_frontiers) == 1 and len(work_frontiers) == 3
    elif policy["boundary"] == "incoming":
        assert pending and pending_locations, "Router must retain nonempty typed incoming payloads"
        assert sorted(pending_locations,key=lambda location:location["source_pc"]) == policy["pending_sources"] and not instruction_frontiers
    else:
        assert not pending and not pending_locations and instruction_frontiers == policy["instruction_frontiers"]
    for index in stores:
        store = report["stores"][index]
        fields = {"storage_default","transient_default","balance_default","accounts","logs",
                  "logs_unknown","storage_facts","transient_facts","touched_default"}
        if report["schema_version"] in {17,18,19,20,21}:
            fields |= {"storage_root","transient_root","nonce_default","code_default","existence_default",
                       "created_default","pending_destruction_default"}
            for name in ["storage_root","transient_root"]:reference(store[name],slot_kinds)
            facts(store["nonce_default"])
            assert store["existence_default"] in {"Unknown","Present","Absent"}
            assert all(store[name] is None or type(store[name]) is bool for name in ["created_default","pending_destruction_default"])
            code = store["code_default"]
            assert code is None or code in ("Empty","Unknown") or (isinstance(code,dict) and len(code) == 1 and (
                ("Runtime" in code and re.fullmatch(r"0x(?:[0-9a-fA-F]{2})*",code["Runtime"])) or
                ("Delegation" in code and re.fullmatch(r"0x[0-9a-fA-F]{40}",code["Delegation"]))))
        assert set(store) == fields
        assert store["touched_default"] is None or isinstance(store["touched_default"],bool)
        for name in ["storage_default","transient_default","balance_default"]:facts(store[name])
        for account in store["accounts"]:
            assert set(account) == {"address","existence","balance","nonce","code_kind","code_hash","program",
                                    "delegation_target","storage_default","storage_root","transient_root","storage",
                                    "transient_default","transient","created","pending_destruction","touched"}
            assert account["touched"] is None or isinstance(account["touched"],bool)
            assert account["program"] is None or account["program"] in programs
            for name in ["balance","nonce","storage_default","transient_default"]:facts(account[name])
            for name in ["storage_root","transient_root"]:reference(account[name],slot_kinds)
            for slot in account["storage"]+account["transient"]:
                assert slot["slot"].startswith("0x") and 0 <= int(slot["slot"],16) < 2**256
                facts(slot["value"])
        for binding in store["storage_facts"]+store["transient_facts"]:
            reference(binding["expression"],word_kinds);facts(binding["value"])
        for log in store["logs"]:
            pool(log["data"],report["byte_arrays"]);buffers.add(log["data"])
            for value in log["topics"]:facts(value)
    for index in buffers:
        buffer = report["byte_arrays"][index]
        reference(buffer["symbolic"],{"View"})
        view = graph[buffer["symbolic"]["node"]]["View"]
        reference(view["state"],byte_kinds)
        reference(view["offset"],word_kinds);reference(view["length"],word_kinds)
        for value in [buffer["length"],buffer["default"],*buffer["values"]]:facts(value)
        for cell in buffer["cells"]:pool(cell["value"],buffer["values"])
        for binding in buffer["symbolic_facts"]:
            reference(binding["expression"],word_kinds);facts(binding["value"])
    assert not report["ssa"]["complete"]
    assert {block["state"] for block in report["ssa"]["blocks"]} == set(nodes)
    assert all(not block["incoming_complete"] for block in report["ssa"]["blocks"])
    return {"pending_machines":len(pending),"pending_sources":pending_locations,"instruction_frontiers":instruction_frontiers,
            "retained_instruction_frontiers":retained_frontiers,"work_frontiers":work_frontiers,
            "byte_arrays":sorted(buffers),"stores":sorted(stores),
            "typed_reasons":[frontier["reason"] for frontier in report["frontiers"]]}


def _runtime_boundary_negative_controls(report, entry, expected_limits):
    """Reject plausible PC, phase, frame and fact misbindings on the actual report."""
    nodes = {node["id"]: node for node in report["cfg"]}
    if _runtime_boundary_profile(report, entry, nodes, expected_limits) is None:
        return []
    states = {state["state"]: state for state in report["states"]}
    blocks = {block["state"]: block for block in report["ssa"]["blocks"]}
    values = {value["id"]: value for value in report["ssa"]["values"]}
    rejected = []

    def reject(label, target, key, replacement):
        original = target[key]
        target[key] = replacement
        try:
            try:
                layout_pending_machines(report, entry, expected_limits)
            except AssertionError:
                rejected.append(label)
            else:
                raise AssertionError(f"runtime boundary accepted corrupted {label}")
        finally:
            target[key] = original

    for frontier in report["frontiers"]:
        if frontier["from"] is None:
            reject("unlocated-work-fabricated-pc", frontier, "pc", 16802)
            reject("unlocated-work-wrong-reason", frontier, "reason", "SummaryWork")
            continue
        source = nodes[frontier["from"]]
        block = blocks[source["id"]]
        if frontier["pc"] != source["executed_pcs"][-1]:
            reject("retained-mstore-wrong-pc", frontier, "pc", 3644)
            continue
        sid = source["id"]
        step = block["instructions"][-1]
        native = states[sid]["entry"]["frames"][0]
        phi = block["phis"][0]
        fact = next(fact for fact in values[phi["result"]]["facts"]
                    if fact["scope"] == {"State": sid} and fact["source"] == "Entry")
        label = f"{step['name']}@{step['pc']}"
        reject(label + "-frontier-pc", frontier, "pc", step["pc"] + 1)
        reject(label + "-source-state", frontier, "from", 0)
        reject(label + "-native-prefix", source["executed_pcs"], -1, step["pc"] + 1)
        reject(label + "-opcode", step, "opcode", 0x00)
        reject(label + "-completed-phase", step, "progress", "Completed")
        reject(label + "-fault", step, "fault", True)
        reject(label + "-normal-result", step, "results", [report["ssa"]["value_count"]])
        reject(label + "-operand-binding", step, "operands", [phi["result"]] * len(step["operands"]))
        reject(label + "-effect", step, "effect_result", block["effect"])
        reject(label + "-frame-owner", native, "code_address", "0x2222222222222222222222222222222222222222")
        reject(label + "-source-owner", source, "storage_address", "0x2222222222222222222222222222222222222222")
        reject(label + "-frame-context", native, "context", [])
        reject(label + "-provenance", fact["value"], "origins", ["Storage"])
        reject(label + "-fact-scope", fact, "scope", {"State": 0})
    # Restoring every mutation must also restore the complete positive oracle.
    layout_pending_machines(report, entry, expected_limits)
    return rejected


def layout_runtime_coverage(report, metadata, timings):
    """Keep exploration and cost deltas visible even when the product oracle fails."""
    nodes = {node["id"]: node for node in report["cfg"]}
    blocks = {block["state"]: block for block in report["ssa"]["blocks"]}
    source_blocks = {(node["program"], node["basic_block"]) for node in nodes.values() if node["program"] is not None}
    executed = {program["id"]: sorted({pc for node in nodes.values() if node["program"] == program["id"] for pc in node["executed_pcs"]})
                for program in report["programs"]}
    reached = {node["id"] for node in nodes.values() if node["program"] == report["metadata"]["root_program"]
               and node["frame_depth"] == 1 and node["start_pc"] == 0 and not node["context"]}
    edges = {edge["id"]: edge for edge in report["edges"]}
    changed = True
    while changed:
        following = {edges[item["edge"]]["to"] for item in report["ssa"]["transitions"] if edges[item["edge"]]["from"] in reached}
        changed = not following <= reached
        reached |= following
    current = {"status": report["status"], "cfg_states": len(nodes), "native_edges": len(edges),
               "known_source_blocks": len(source_blocks), "pending_machines": len(report["pending_machines"]),
               "current_ssa_entry_reachable_states": len(reached), "ssa_deferred_edges": report["ssa"]["deferred_edges"],
               "calls": [edge for edge in report["edges"] if edge["kind"] == "Call"],
               "root_return_states": sorted({outcome["state"] for outcome in report["outcomes"] if outcome["kind"] == "Return"}),
               "closed_summaries": sum(summary["closed"] for summary in report["abstract_summaries"]),
               "frontiers": report["frontiers"], "maximum_frame_depth": max(node["frame_depth"] for node in nodes.values()),
               "source_blocks": sorted(source_blocks), "latest_transfer_executed_pcs": executed,
               "current_ssa_states": sorted(state for state, block in blocks.items() if block["coverage"] == "Current"),
               "current_ssa_entry_reachable_ids": sorted(reached),
               "work": report["metadata"]["work"], "transfers": report["transfers"],
               "timings": timings, "export_usage": report["metadata"]["export_usage"]}
    assert type(current["work"]) is int and type(current["transfers"]) is int
    assert all(type(value) is int for value in [*current["timings"].values(), *current["export_usage"].values()])
    historical = metadata.get("historical_observations")
    return {"current": current, "historical": historical,
            "b8e_count_deltas": {key: current[key] - historical["b8e_observed"][key]
                for key in ["cfg_states", "native_edges", "known_source_blocks", "pending_machines", "work", "transfers"]} if historical else {},
            "b8e_timing_deltas_ns": {key: value - historical["b8e_observed"]["timings"][key]
                for key, value in current["timings"].items()} if historical else {},
            "interpretation": "Captured graph coverage and SSA entry reachability are separate; a missing SSA entry path does not prove nonexecution. Historical precision regressions are retained, without a restoration claim."}



def main():
    import argparse
    from pathlib import Path
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("report", "request", "metadata", "bytecode", "limits"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    envelope, request, metadata, limits = (json.loads(getattr(args, name).read_text())
        for name in ("report", "request", "metadata", "limits"))
    code = args.bytecode.read_text().strip().removeprefix("0x")
    entry = metadata["browser_entry"]
    report = oracle_graph(envelope, "Native")
    original = oracle_graph(envelope, "Baseline")
    layout_runtime_request(request, original, code, metadata, limits)
    layout_runtime_programs(report, code, metadata)
    if "product_oracle" in entry:
        layout_pending_machines(report, entry, limits)
        _runtime_boundary_negative_controls(report, entry, limits)
    print("current runtime report and negative controls passed")


if __name__ == "__main__":
    main()
