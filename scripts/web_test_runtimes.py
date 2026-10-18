"""Real runtime input, partial execution evidence and browser projection oracles."""

import hashlib
import json
import re
from playwright.sync_api import expect
from web_test_graph_helpers import graph_mode, pick_graph_instance, symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_layout_helpers import LAYOUT_DEFAULT_LIMITS, layout_counts, layout_fixture, layout_page, layout_submission
from web_test_pixels import layout_ink, node_snapshot


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
    assert list(programs) == list(range(len(programs)))
    root = programs[report["metadata"]["root_program"]]
    assert root["kind"] == "Runtime" and root["code_hash"] == metadata["code_hash"]
    assert root["bytecode"].removeprefix("0x") == code
    assert report["byte_len"] == metadata["byte_length"] == len(bytes.fromhex(code))
    assert report["disassembly"] == root["blocks"]
    ledgers = {}
    for program in programs.values():
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
        ledgers[program["id"]] = {instruction["pc"]: instruction for instruction in actual}
    return ledgers


def layout_runtime_request(request, report, code, metadata):
    entry = metadata["browser_entry"]
    assert report["schema_version"] == 15 and report["root_terminal_coverage"] is None
    assert set(request) == {"input", "fork", "environment", "limits"}
    assert request["input"] == {"Bytecode": {"bytecode": code, "address": entry.get("address")}}
    assert request["fork"] == report["fork"] == entry.get("fork", "Osaka")
    assert LAYOUT_DEFAULT_LIMITS["root_terminal_coverage"] is False
    assert request["limits"] == report["metadata"]["limits"] == LAYOUT_DEFAULT_LIMITS
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


def layout_runtime_references(report):
    """Validate actual report pools; schema 14 is retained for offline old-report controls only."""
    assert report["schema_version"] in {14, 15}
    graph = report["symbolic"]["nodes"]
    word_kinds = {"Constant", "Input", "Fresh", "Operation", "Ite", "ByteRead", "WordRead", "SlotRead", "Keccak"}
    byte_kinds = {"BytesZero", "BytesInput", "BytesExact", "WriteByte", "WriteWord", "Copy", "Slice", "BytesIte"}
    slot_kinds = {"SlotsZero", "SlotsInput", "Store", "SlotsIte"}
    path_kinds = {"PathTrue", "PathFalse", "PathAtoms", "PathAnd", "PathIte"}
    sorts = {"word": word_kinds, "bytes": byte_kinds, "slots": slot_kinds, "path": path_kinds, "view": {"View"}}
    programs = {program["id"]: program for program in report["programs"]}

    def word(value):
        assert isinstance(value, str) and re.fullmatch(r"0x[0-9a-f]{1,64}", value)
        return int(value, 16)

    def reference(value, sort, before=None):
        assert isinstance(value, dict) and set(value) == {"node"}
        index = value["node"]
        assert type(index) is int and 0 <= index < (len(graph) if before is None else before)
        node = graph[index]
        assert isinstance(node, str) or (isinstance(node, dict) and len(node) == 1)
        assert (node if isinstance(node, str) else next(iter(node))) in sorts[sort]
        return index

    def identity(value):
        assert set(value) == {"scope", "symbol"}
        scope = value["scope"]
        if report["schema_version"] == 15:
            assert isinstance(scope, str) and re.fullmatch(r"0|[1-9][0-9]*", scope) and int(scope) < 2**64
        else:
            assert type(scope) is int and 0 <= scope < 2**64
        assert set(value["symbol"]) == {"kind", "index"}
        assert value["symbol"]["kind"] in {"To", "Caller", "Origin", "Coinbase", "CallValue", "GasPrice", "Timestamp",
                                            "Number", "Prevrandao", "GasLimit", "ChainId", "BaseFee", "BlobBaseFee",
                                            "CalldataLength", "CalldataWord", "BlockHash", "BlobHash"}
        assert value["symbol"]["index"] is None or word(value["symbol"]["index"]) >= 0

    def facts(value):
        assert set(value) == {"summary", "constants", "known_zero", "known_one", "nonzero", "unsigned", "signed",
                              "congruence", "origins", "code_address_role", "identity", "expression", "symbolic_limit"}
        assert isinstance(value["summary"], str)
        assert all(type(value[name]) is bool for name in ["nonzero", "code_address_role", "symbolic_limit"])
        assert word(value["known_zero"]) & word(value["known_one"]) == 0
        for name in ["unsigned", "signed"]:
            assert set(value[name]) == {"lower", "upper"}
            bias = 1 << 255 if name == "signed" else 0
            assert word(value[name]["lower"]) ^ bias <= word(value[name]["upper"]) ^ bias
        constants = value["constants"]
        assert constants is None or isinstance(constants, list)
        if constants is not None:
            assert len(set(constants)) == len(constants)
            assert all(_runtime_numeric_contains(value, word(item)) for item in constants)
        congruence = value["congruence"]
        if congruence != "Any":
            assert isinstance(congruence, dict) and len(congruence) == 1
            if "Exact" in congruence:
                word(congruence["Exact"])
            else:
                assert set(congruence) == {"Modulo"}
                modulo = congruence["Modulo"]
                assert set(modulo) == {"modulus", "residue"}
                assert 0 <= word(modulo["residue"]) < word(modulo["modulus"])
        assert value["origins"] is None or (isinstance(value["origins"], list) and set(value["origins"]) <= {
            "Constant", "Calldata", "Memory", "Storage", "TransientStorage", "Environment", "Address", "CodeAddress",
            "CallValue", "Returndata", "Arithmetic", "Balance", "Nonce"})
        if value["identity"] is not None:
            identity(value["identity"])
        if value["expression"] is not None:
            reference(value["expression"], "word")

    def pool(index, values):
        assert type(index) is int and 0 <= index < len(values)
        return values[index]

    def fields(value, schema, before):
        assert set(value) == set(schema)
        for key, sort in schema.items():
            reference(value[key], sort, before)

    # Check every DAG child and its sort, including roots not reached by one pending frame.
    schemas = {
        "Ite": {"condition": "word", "then_value": "word", "else_value": "word"},
        "ByteRead": {"state": "bytes", "index": "word"},
        "WordRead": {"view": "view", "offset": "word"},
        "SlotRead": {"state": "slots", "key": "word"},
        "WriteByte": {"previous": "bytes", "offset": "word", "value": "word"},
        "WriteWord": {"previous": "bytes", "offset": "word", "value": "word"},
        "Copy": {"previous": "bytes", "destination": "word", "source": "view", "length": "word"},
        "BytesIte": {"condition": "word", "then_state": "bytes", "else_state": "bytes"},
        "Store": {"previous": "slots", "key": "word", "value": "word"},
        "SlotsIte": {"condition": "word", "then_state": "slots", "else_state": "slots"},
        "View": {"state": "bytes", "offset": "word", "length": "word"},
        "PathIte": {"selector": "word", "then_path": "path", "else_path": "path"},
    }
    for index, node in enumerate(graph):
        if isinstance(node, str):
            assert node in {"BytesZero", "SlotsZero", "PathTrue", "PathFalse"}
            continue
        assert isinstance(node, dict) and len(node) == 1
        kind, value = next(iter(node.items()))
        if kind in schemas:
            fields(value, schemas[kind], index)
        elif kind in {"Keccak", "Slice"}:
            reference(value, "view", index)
        elif kind == "Constant":
            word(value)
        elif kind == "Fresh":
            assert type(value) is int and 0 <= value < 2**64
        elif kind == "Input":
            identity(value)
        elif kind == "BytesExact":
            assert isinstance(value, str) and re.fullmatch(r"0x(?:[0-9a-f]{2})*", value)
        elif kind == "Operation":
            assert set(value) == {"opcode", "arguments"} and type(value["opcode"]) is int and 0 <= value["opcode"] <= 255
            for argument in value["arguments"]:
                reference(argument, "word", index)
        elif kind in {"BytesInput", "SlotsInput"}:
            assert set(value) == ({"source", "bindings", "length"} if kind == "BytesInput" else {"source", "bindings"})
            assert set(value["source"]) == {"token", "namespace"}
            reference(value["source"]["token"], "word", index); word(value["source"]["namespace"])
            if kind == "BytesInput":
                reference(value["length"], "word", index)
            for binding in value["bindings"]:
                assert set(binding) == {"index", "value", "numeric"}
                word(binding["index"]); reference(binding["value"], "word", index); facts(binding["numeric"])
        elif kind == "PathAnd":
            for child in value:
                reference(child, "path", index)
        else:
            assert kind == "PathAtoms"
            for atom in value:
                assert len(atom) == 1
                tag, payload = next(iter(atom.items()))
                if tag == "Equal":
                    fields(payload, {"left": "word", "right": "word"}, index)
                else:
                    assert tag in {"Truth", "Range", "SignedRange", "Modulo", "Bits", "Members"}
                    reference(payload["expression"], "word", index)
                    if tag == "Truth":
                        assert set(payload) == {"expression", "nonzero"} and type(payload["nonzero"]) is bool
                    elif tag == "Members":
                        assert set(payload) == {"expression", "values"}
                        for item in payload["values"]: word(item)
                    else:
                        keys = {"Range": {"lower", "upper"}, "SignedRange": {"lower", "upper"},
                                "Modulo": {"modulus", "residue"}, "Bits": {"zero", "one"}}[tag]
                        assert set(payload) == {"expression"} | keys
                        for key in keys: word(payload[key])

    def walk(value):
        if isinstance(value, dict):
            if "known_zero" in value or "known_one" in value:
                facts(value)
            if set(value) == {"expression", "value"}:
                reference(value["expression"], "word"); facts(value["value"])
            for child in value.values(): walk(child)
        elif isinstance(value, list):
            for child in value: walk(child)
    walk(report)

    def account(value):
        assert set(value) == {"address", "existence", "balance", "nonce", "code_kind", "code_hash", "program",
                              "delegation_target", "storage_default", "storage_root", "transient_root", "storage",
                              "transient_default", "transient", "created", "pending_destruction", "touched"}
        assert value["existence"] in {"Unknown", "Present", "Absent"}
        for name in ["created", "pending_destruction", "touched"]:
            assert value[name] is None or type(value[name]) is bool
        assert value["program"] is None or value["program"] in programs
        for name in ["balance", "nonce", "storage_default", "transient_default"]: facts(value[name])
        for name in ["storage_root", "transient_root"]: reference(value[name], "slots")
        for name in ["storage", "transient"]:
            slots = [word(slot["slot"]) for slot in value[name]]
            assert len(set(slots)) == len(slots)
            for slot in value[name]:
                assert set(slot) == {"slot", "value"}
                facts(slot["value"])

    def bindings(items):
        for item in items:
            assert set(item) == {"expression", "value"}
            reference(item["expression"], "word"); facts(item["value"])

    for value in report["accounts"]: account(value)
    for store in report["stores"]:
        expected = {"storage_default", "transient_default", "balance_default", "accounts", "logs", "logs_unknown",
                    "storage_facts", "transient_facts", "touched_default"}
        if report["schema_version"] == 15:
            expected |= {"storage_root", "transient_root", "nonce_default", "code_default", "existence_default",
                         "created_default", "pending_destruction_default"}
            for name in ["storage_root", "transient_root"]: reference(store[name], "slots")
            facts(store["nonce_default"])
            assert store["existence_default"] in {"Unknown", "Present", "Absent"}
            for name in ["created_default", "pending_destruction_default"]:
                assert store[name] is None or type(store[name]) is bool
            default = store["code_default"]
            if default is not None and default not in ("Empty", "Unknown"):
                assert isinstance(default, dict) and len(default) == 1
                if "Runtime" in default:
                    assert re.fullmatch(r"0x(?:[0-9a-fA-F]{2})*", default["Runtime"])
                else:
                    assert set(default) == {"Delegation"} and re.fullmatch(r"0x[0-9a-fA-F]{40}", default["Delegation"])
        assert set(store) == expected
        assert store["touched_default"] is None or type(store["touched_default"]) is bool
        assert type(store["logs_unknown"]) is bool
        for name in ["storage_default", "transient_default", "balance_default"]: facts(store[name])
        bindings(store["storage_facts"]); bindings(store["transient_facts"])
        for value in store["accounts"]: account(value)
        for log in store["logs"]:
            pool(log["data"], report["byte_arrays"])
            for topic in log["topics"]: facts(topic)
    for buffer in report["byte_arrays"]:
        assert set(buffer) == {"length", "default", "cells", "values", "memory", "symbolic", "symbolic_facts"}
        reference(buffer["symbolic"], "view")
        assert type(buffer["memory"]) is bool
        for value in [buffer["length"], buffer["default"], *buffer["values"]]: facts(value)
        bindings(buffer["symbolic_facts"])
        offsets = [cell["offset"] for cell in buffer["cells"]]
        assert all(type(offset) is int and offset >= 0 for offset in offsets)
        assert offsets == sorted(set(offsets))
        for cell in buffer["cells"]:
            assert set(cell) == {"offset", "value"}
            pool(cell["value"], buffer["values"])

    def machine_snapshot(machine):
        assert set(machine) == {"frames", "store", "path"} and machine["frames"]
        pool(machine["store"], report["stores"]); reference(machine["path"], "path")
        for index, frame in enumerate(machine["frames"]):
            assert set(frame) == {"index", "program", "code_address", "code_hash", "storage_address", "address_value",
                                  "caller", "address_operand", "caller_operand", "call_value", "is_static", "kind", "basic_block", "context", "stack",
                                  "memory", "calldata", "returndata", "rollback_store", "continuation"}
            assert type(frame["index"]) is int and frame["index"] == index and type(frame["is_static"]) is bool
            assert frame["program"] is None or type(frame["program"]) is int
            facts(frame["call_value"])
            for name in ["address_operand", "caller_operand"]:
                if frame[name] is not None: facts(frame[name])
            for value in frame["stack"]: facts(value)
            if frame["program"] is not None:
                program = programs[frame["program"]]
                assert frame["code_hash"] == program["code_hash"] and frame["kind"] == program["kind"]
                assert frame["code_address"].lower() == program["code_address"].lower()
                pool(frame["basic_block"], program["blocks"])
            for name in ["memory", "calldata", "returndata"]: pool(frame[name], report["byte_arrays"])
            assert report["byte_arrays"][frame["memory"]]["memory"] is True
            pool(frame["rollback_store"], report["stores"])
            assert (frame["continuation"] is None) == (index == 0)
            if frame["continuation"] is not None:
                assert set(frame["continuation"]) == {"return_block", "output_offset", "output_size", "creation"}
                facts(frame["continuation"]["output_offset"]); facts(frame["continuation"]["output_size"])
        return machine["frames"][-1]
    for state in report["states"]:
        machine_snapshot(state["entry"])
        if state["exit"] is not None: machine_snapshot(state["exit"])
    for machine in report["pending_machines"]: machine_snapshot(machine)
    for outcome in report["outcomes"]:
        pool(outcome["store"], report["stores"]); pool(outcome["data"], report["byte_arrays"])
    pool(report["metadata"]["environment"]["calldata"], report["byte_arrays"])
    for name in ["call_value", "gas_price", "timestamp", "number", "prevrandao", "gas_limit", "chain_id", "base_fee", "blob_base_fee", "blob_count"]:
        facts(report["metadata"]["environment"][name])
    return {"byte_arrays": len(report["byte_arrays"]), "stores": len(report["stores"]), "symbolic_nodes": len(graph)}

def layout_runtime_ssa(report, ledgers):
    nodes = {node["id"]: node for node in report["cfg"]}
    states = {state["state"]: state for state in report["states"]}
    ssa = report["ssa"]
    blocks = {block["state"]: block for block in ssa["blocks"]}
    edges = {edge["id"]: edge for edge in report["edges"]}
    transitions = {item["edge"]: item for item in ssa["transitions"]}
    deferred = {item["edge"]: item["reason"] for item in ssa["deferred_edges"]}
    assert nodes and len(nodes) == len(report["cfg"]) == len(states) == len(report["states"]) == len(blocks) == len(ssa["blocks"])
    assert set(nodes) == set(states) == set(blocks)
    assert len(edges) == len(report["edges"]) and all(edge["from"] in nodes and edge["to"] in nodes for edge in edges.values())
    assert len(transitions) == len(ssa["transitions"]) and len(deferred) == len(ssa["deferred_edges"])
    assert not set(transitions) & set(deferred) and set(transitions) | set(deferred) == set(edges)
    assert ssa["value_count"] == len(ssa["values"])
    assert [value["id"] for value in ssa["values"]] == list(range(ssa["value_count"]))

    def values(items):
        assert all(type(item) is int and 0 <= item < ssa["value_count"] for item in items)

    def effect(item):
        assert type(item) is int and 0 <= item < ssa["effect_count"]

    for state_id, node in nodes.items():
        state = states[state_id]; block = blocks[state_id]; active = state["entry"]["frames"][-1]
        assert (active["program"], active["basic_block"], len(state["entry"]["frames"]), active["context"],
                active["code_address"], active["storage_address"]) == (
                node["program"], node["basic_block"], node["frame_depth"], node["context"], node["code_address"], node["storage_address"])
        assert [value["summary"] for value in active["stack"]] == node["entry_stack"]
        if node["program"] is not None:
            source = report["programs"][node["program"]]["blocks"][node["basic_block"]]
            assert node["start_pc"] == source["start_pc"] and node["instructions"] == source["instructions"]
        else:
            assert node["start_pc"] is None and not node["instructions"] and not node["executed_pcs"]
        assert block["coverage"] in {"Current", "Stale", "Unexecuted"}
        assert block["incoming_complete"] is (report["status"] == "Converged")
        if block["coverage"] == "Current":
            assert [instruction["pc"] for instruction in block["instructions"]] == node["executed_pcs"]
        else:
            assert not block["instructions"]
        entry_frames = state["entry"]["frames"]
        assert {(phi["frame"], phi["slot"]) for phi in block["phis"]} == {
            (frame_index, slot) for frame_index, frame in enumerate(entry_frames) for slot in range(len(frame["stack"]))}
        effect(block["effect"]); effect(block["exit_effect"])
        for frame in block["exit_frames"]: values(frame)
        if block["coverage"] == "Current" and state["exit"] is not None:
            assert [len(frame) for frame in block["exit_frames"]] == [len(frame["stack"]) for frame in state["exit"]["frames"]]
        incoming_edges = {edge_id for edge_id, edge in edges.items() if edge["to"] == state_id}
        for phi in block["phis"]:
            values([phi["result"]])
            assert len(phi["inputs"]) == len(incoming_edges & set(transitions))
            assert {item["edge"] for item in phi["inputs"]} == incoming_edges & set(transitions)
            for incoming in phi["inputs"]:
                transition = transitions[incoming["edge"]]; edge = edges[incoming["edge"]]
                assert edge["to"] == state_id and edge["from"] == incoming["predecessor"]
                assert incoming["value"] == transition["stacks"][phi["frame"]][phi["slot"]]
                values([incoming["value"]])
        assert set(block["open_incoming"]) == incoming_edges & set(deferred)
        assert {item["edge"] for item in block["effect_inputs"]} == incoming_edges & set(transitions)
        for item in block["effect_inputs"]:
            assert item["effect"] == transitions[item["edge"]]["effect_result"]
        for instruction in block["instructions"]:
            source = ledgers[node["program"]][instruction["pc"]]
            assert all(instruction[key] == source[key] for key in ["opcode", "name", "immediate"])
            values(instruction["operands"]); values(instruction["results"]); effect(instruction["effect_input"])
            assert instruction["progress"] in {"Started", "OperandsConsumed", "Completed", "Dispatched", "Faulted"}
            if instruction["effect_result"] is not None: effect(instruction["effect_result"])
            if instruction["progress"] in {"Started", "OperandsConsumed"}: assert not instruction["results"]
            operation = instruction["operation"]
            assert isinstance(operation, dict) and len(operation) == 1
            if "Opcode" in operation:
                assert operation["Opcode"]["opcode"] == instruction["opcode"]
                assert operation["Opcode"]["name"] == instruction["name"]
                operands = operation["Opcode"]["operands"]
            else:
                assert set(operation) in ({"Constant"}, {"Copy"})
                operands = [operation["Copy"]] if "Copy" in operation else []
            for operand in operands:
                assert set(operand) in ({"Value"}, {"Constant"})
                if "Value" in operand: values([operand["Value"]])
    for edge_id, transition in transitions.items():
        edge = edges[edge_id]; source = blocks[edge["from"]]
        assert transition["kind"] == edge["kind"] and source["coverage"] == "Current"
        assert not source["instructions"] or source["instructions"][-1]["progress"] not in {"Started", "OperandsConsumed"}
        values(transition["operands"])
        for frame in transition["stacks"]: values(frame)
        assert [len(frame) for frame in transition["stacks"]] == [len(frame["stack"]) for frame in states[edge["to"]]["entry"]["frames"]]
        effect(transition["effect_input"]); effect(transition["effect_result"])
        assert transition["effect_input"] == source["exit_effect"]
        if transition["result"] is not None: values([transition["result"]])
    for edge_id, reason in deferred.items():
        source = blocks[edges[edge_id]["from"]]
        if source["coverage"] == "Current":
            assert reason in {"NotInExecutionEvidence", "SourcePendingInstruction"}
            if reason == "SourcePendingInstruction":
                assert source["instructions"][-1]["progress"] in {"Started", "OperandsConsumed"}
        else:
            assert reason == {"Stale": "SourceStale", "Unexecuted": "SourceUnexecuted"}[source["coverage"]]
    for value in ssa["values"]:
        values(value["dependencies"])
        if value["equivalent"] is not None: values([value["equivalent"]])
        tag, origin = next(iter(value["origin"].items()))
        if tag == "Phi":
            assert any(phi["result"] == value["id"] and phi["frame"] == origin["frame"] and phi["slot"] == origin["slot"]
                       for phi in blocks[origin["state"]]["phis"])
        elif tag == "Instruction":
            instruction = blocks[origin["state"]]["instructions"][origin["instruction_index"]]
            assert instruction["pc"] == origin["pc"] and value["id"] in instruction["results"]
        else:
            assert tag == "Transition" and edges[origin["edge"]]["from"] == origin["from"] and edges[origin["edge"]]["to"] == origin["to"]
            assert transitions[origin["edge"]]["result"] == value["id"]
        for fact in value["facts"]:
            assert set(fact) == {"status", "scope", "source", "constant", "value"}
            assert fact["status"] in {"Current", "Uncollected", "Stale", "Unexecuted"}
            if fact["value"] is not None:
                assert "known_zero" in fact["value"] and "known_one" in fact["value"]
            if fact["constant"] is not None:
                assert re.fullmatch(r"0x[0-9a-f]{1,64}", fact["constant"])
                if fact["value"] is not None: assert _runtime_numeric_contains(fact["value"], int(fact["constant"], 16))
            scope = fact["scope"]
            if scope != "Definition":
                assert isinstance(scope, dict) and len(scope) == 1
                if "State" in scope: assert scope["State"] in nodes
                else: assert set(scope) == {"Edge"} and scope["Edge"] in transitions
    for outcome in report["outcomes"]: assert outcome["state"] in nodes
    return {"states": len(nodes), "edges": len(edges), "transitions": len(transitions),
            "deferred_edges": ssa["deferred_edges"], "ssa_coverage": {kind: sum(block["coverage"] == kind for block in blocks.values())
                for kind in ["Current", "Stale", "Unexecuted"]}}


def layout_pending_machines(report, entry):
    policy = entry["product_oracle"]
    pending = report["pending_machines"]
    frontiers = report["frontiers"]
    assert report["status"] == entry["expected_status"] == "Incomplete" and frontiers and not report["ssa"]["complete"]
    nodes = {node["id"]: node for node in report["cfg"]}
    states = {state["state"]: state for state in report["states"]}
    blocks = {block["state"]: block for block in report["ssa"]["blocks"]}
    transitions = {item["edge"]: item for item in report["ssa"]["transitions"]}
    deferred = {item["edge"]: item["reason"] for item in report["ssa"]["deferred_edges"]}
    payloads = [frontier["payload"] for frontier in frontiers if frontier["payload"] is not None]
    assert all(type(payload) is int for payload in payloads)
    assert len(payloads) == len(pending) and set(payloads) == set(range(len(pending)))
    locations = []; instructions = []
    for frontier in frontiers:
        assert frontier["kind"] == "Relations" and frontier["reason"] == {"Relations": "ExpressionLimit"}
        assert frontier["detail"] == "Relations(ExpressionLimit)" and frontier["range"] is None
        source = nodes[frontier["from"]]; block = blocks[source["id"]]
        assert block["coverage"] == "Current" and not block["incoming_complete"]
        instruction = block["instructions"][-1]
        if frontier["payload"] is not None:
            assert policy["boundary"] == "incoming" and frontier["pc"] is None
            machine = pending[frontier["payload"]]; active = machine["frames"][-1]
            targets = [node for node in nodes.values() if (
                node["program"], node["basic_block"], node["frame_depth"], node["context"], node["code_address"], node["storage_address"]
            ) == (active["program"], active["basic_block"], len(machine["frames"]), active["context"], active["code_address"], active["storage_address"])]
            assert len(targets) == 1
            target = targets[0]
            edges = [edge for edge in report["edges"] if edge["from"] == source["id"] and edge["to"] == target["id"]]
            assert len(edges) == 1
            edge = edges[0]
            assert source["program"] == target["program"] == report["metadata"]["root_program"]
            assert source["frame_depth"] == target["frame_depth"] == 1 and active["kind"] == "Runtime"
            assert instruction["progress"] == "Dispatched" and instruction["pc"] == source["executed_pcs"][-1]
            source_frames = states[source["id"]]["exit"]["frames"]
            target_frames = states[target["id"]]["entry"]["frames"]
            assert [len(frame["stack"]) for frame in machine["frames"]] == [len(frame["stack"]) for frame in source_frames] == [len(frame["stack"]) for frame in target_frames]
            for frame, before, after in zip(machine["frames"], source_frames, target_frames, strict=True):
                for key in ["program", "code_address", "code_hash", "storage_address", "kind", "is_static", "address_value", "caller"]:
                    assert frame[key] == before[key] == after[key]
            location = {"source_pc": instruction["pc"], "source_name": instruction["name"],
                        "edge_kind": edge["kind"], "target_start_pc": target["start_pc"]}
            if edge["id"] in deferred:
                assert deferred[edge["id"]] == "NotInExecutionEvidence"
                location["deferred_reason"] = deferred[edge["id"]]
                # A branch's incoming state retains the whole source frame;
                # only its program counter changes, and its path may strengthen.
                assert active == {**source_frames[-1], "basic_block": active["basic_block"]}
            else:
                assert transitions[edge["id"]]["kind"] == edge["kind"]
            locations.append(location)
        else:
            assert policy["boundary"] == "root_instruction" and frontier["pc"] is not None
            assert source["program"] == report["metadata"]["root_program"] and source["frame_depth"] == 1
            machine = states[source["id"]]["exit"]; active = machine["frames"][-1]
            assert (active["program"], active["basic_block"], len(machine["frames"]), active["code_address"], active["storage_address"]) == (
                source["program"], source["basic_block"], source["frame_depth"], source["code_address"], source["storage_address"])
            assert instruction["pc"] == frontier["pc"] == source["executed_pcs"][-1]
            assert instruction["name"] == "MSTORE" and instruction["opcode"] == 0x52
            assert instruction["progress"] == "OperandsConsumed" and not instruction["fault"]
            assert not instruction["results"] and len(instruction["operands"]) == 2
            assert instruction["effect_result"] == block["exit_effect"] and instruction["effect_result"] is not None
            assert len(states[source["id"]]["exit"]["frames"][-1]["stack"]) == len(block["exit_frames"][-1])
            # Gas-failure alternatives can precede this refusal in the same
            # block. They do not complete the pending MSTORE's normal path.
            for outcome in report["outcomes"]:
                if outcome["state"] == source["id"]:
                    data = report["byte_arrays"][outcome["data"]]
                    assert outcome["kind"] == "Failure" and data["length"]["constants"] == ["0x0"] and not data["cells"]
            outgoing = [edge for edge in report["edges"] if edge["from"] == source["id"]]
            assert outgoing and all(edge["id"] not in transitions and deferred.get(edge["id"]) == "NotInExecutionEvidence" for edge in outgoing)
            instructions.append({"source_start_pc": source["start_pc"], "source_frame_depth": source["frame_depth"],
                                 "pc": instruction["pc"], "opcode": instruction["opcode"], "name": instruction["name"],
                                 "progress": instruction["progress"], "coverage": block["coverage"],
                                 "deferred_edge_kinds": sorted(edge["kind"] for edge in outgoing)})
    if policy["boundary"] == "incoming":
        assert pending and locations, "Router must retain a determinate nonempty typed incoming witness"
        assert sorted(locations, key=lambda item: item["source_pc"]) == policy["pending_sources"]
        assert not instructions
    else:
        assert not pending and not locations
        assert instructions == policy["instruction_frontiers"]
    return {"pending_machines": len(pending), "pending_sources": locations, "instruction_frontiers": instructions,
            "typed_reasons": [frontier["reason"] for frontier in frontiers]}


def layout_runtime_coverage(report, metadata):
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
               "timings": report["metadata"]["timings"], "export_usage": report["metadata"]["export_usage"]}
    assert type(current["work"]) is int and type(current["transfers"]) is int
    assert all(type(value) is int for value in [*current["timings"].values(), *current["export_usage"].values()])
    historical = metadata.get("historical_observations")
    failures = []
    if historical:
        old = historical["original_expectations"]
        for field, actual in [("minimum_native_states", len(nodes)), ("minimum_source_blocks", len(source_blocks))]:
            if actual < old[field]: failures.append(field)
        for field, actual in [("expected_frontiers", len(report["frontiers"])), ("expected_pending_machines", len(report["pending_machines"])),
                              ("expected_closed_procedure_summaries", current["closed_summaries"])]:
            if field in old and actual != old[field]: failures.append(field)
        if "expected_frontiers_by_kind" in old:
            expected = old["expected_frontiers_by_kind"]
            if {kind: sum(item["kind"] == kind for item in report["frontiers"]) for kind in {item["kind"] for item in report["frontiers"]}} != {kind: item["count"] for kind, item in expected.items()}:
                failures.append("expected_frontiers_by_kind")
        if "expected_instruction_frontiers" in old:
            actual = [{"source_start_pc": nodes[item["from"]]["start_pc"], "source_frame_depth": nodes[item["from"]]["frame_depth"],
                       "pc": item["pc"]} for item in report["frontiers"] if item["pc"] is not None]
            expected = [{key: item[key] for key in ["source_start_pc", "source_frame_depth", "pc"]} for item in old["expected_instruction_frontiers"]]
            if actual != expected: failures.append("expected_instruction_frontiers")
        if "expected_pending_frontiers" in old:
            actual = []
            for item in report["frontiers"]:
                if item["payload"] is None: continue
                machine = report["pending_machines"][item["payload"]]; active = machine["frames"][-1]
                source = nodes[item["from"]]
                target = report["programs"][active["program"]]["blocks"][active["basic_block"]]
                actual.append({"source_start_pc": source["start_pc"], "source_frame_depth": source["frame_depth"],
                               "target_start_pc": target["start_pc"], "target_frame_depth": len(machine["frames"])})
            if sorted(actual, key=lambda item: item["source_start_pc"]) != old["expected_pending_frontiers"]:
                failures.append("expected_pending_frontiers")
    return {"current": current, "historical": historical, "original_expectation_failures": failures,
            "b8e_count_deltas": {key: current[key] - historical["b8e_observed"][key]
                for key in ["cfg_states", "native_edges", "known_source_blocks", "pending_machines", "work", "transfers"]} if historical else {},
            "b8e_timing_deltas_ns": {key: value - historical["b8e_observed"]["timings"][key]
                for key, value in current["timings"].items()} if historical else {},
            "interpretation": "Captured graph coverage and SSA entry reachability are separate; a missing SSA entry path does not prove nonexecution. Historical precision regressions are retained, without a restoration claim."}


def hierarchy_real_runtime_interactions(browser, url, args, fixture_names=("ethereum-weth9", "ethereum-uniswap-v2-router02", "ethereum-multicall3")):
    results = {}
    for name in fixture_names:
        page, canvas, errors = layout_page(browser, url)
        code, metadata = layout_fixture(name)
        assert metadata["chain_id"] == 1 and metadata["requireCanonical"] is True
        entry = metadata["browser_entry"]
        print(f"real-runtime analysis start: {name}", flush=True)
        report = layout_submission(page, canvas, code, args.output, name, timeout_seconds=1800, entry=entry)
        coverage = layout_runtime_coverage(report, metadata)
        (args.output / f"{name}-coverage-comparison.json").write_text(json.dumps(coverage, indent=2) + "\n")
        print(f"real-runtime analysis done: {name} status={report['status']} states={len(report['cfg'])} "
              f"frontiers={[(frontier['kind'], frontier['detail']) for frontier in report['frontiers']]} "
              f"historical_failures={coverage['original_expectation_failures']} "
              f"analyzing_ns={report['metadata']['timings']['analyzing_ns']}", flush=True)
        request = json.loads((args.output / f"{name}-request.json").read_text())
        request_evidence = layout_runtime_request(request, report, code, metadata)
        ledgers = layout_runtime_programs(report, code, metadata)
        references = layout_runtime_references(report)
        ssa = layout_runtime_ssa(report, ledgers)
        assert report["status"] == entry["expected_status"]
        pending = None
        if "product_oracle" in entry:
            pending = layout_pending_machines(report, entry)
        else:
            assert len(report["cfg"]) >= entry["minimum_native_states"]
            assert len(report["frontiers"]) == entry["expected_frontiers"]
        if entry.get("product_oracle", {}).get("summary_policy") == "closed_static_unknown_environment":
            summaries = report["abstract_summaries"]
            assert summaries
            nodes = {node["id"]: node for node in report["cfg"]}
            states = {state["state"]: state for state in report["states"]}
            for summary in summaries:
                assert summary["closed"] and summary["kind"] == "UnknownEnvironment" and summary["procedures"]
                source = nodes[summary["source_state"]]
                active = states[source["id"]]["entry"]["frames"][-1]
                assert source["program"] is None and active["kind"] == "AbstractEnvironment" and active["is_static"]
                for procedure in summary["procedures"]:
                    assert procedure["kind"] == "AbstractEnvironment" and procedure["is_static"]
                    assert not procedure["covered_blocks"] and set(procedure["outcomes"]) == {"Return", "Revert", "Failure"}
        ending_status = re.search(r"CFG endings: (.+?); Selected CFG ending:", page.locator("#analysis-status").text_content())[1]

        def scene_counts(mode="Blocks"):
            counts = layout_counts(page, report, mode=mode)
            assert counts["scope"] == "All programs" and counts["hidden_states"] == counts["boundary_edges"] == 0
            status = page.locator("#analysis-status").text_content()
            assert re.search(r"CFG endings: (.+?); Selected CFG ending:", status)[1] == ending_status
            assert report["status"] in status
            return counts

        scene = scene_counts()
        if "minimum_source_blocks" in entry: assert scene["nodes"] >= entry["minimum_source_blocks"]
        if report["status"] == "Incomplete":
            assert report["frontiers"] and not report["ssa"]["complete"]
            expect(page.locator("#analysis-status")).to_contain_text("Incomplete")
        # Observe every camera/projection action, not just the native projection.
        posts = []
        page.on("request", lambda request: posts.append(request.url)
                if request.method == "POST" and request.url.endswith("/api/tasks") else None)
        canvas.screenshot(path=str(args.output / f"hierarchy-{name}-overview.png"))
        canvas.click(position={"x": 225, "y": 83}); settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
        focused = node_snapshot(page, canvas, args.output, f"hierarchy-{name}-focused")
        assert scene_counts() == {**scene, "detail": "SSA preview"}
        graph_mode(page, canvas, "States")
        page.keyboard.press("f"); settle_gesture(page)
        native_scene = scene_counts("States")
        assert native_scene["nodes"] == len(report["cfg"]) and native_scene["edges"] == len(report["edges"])
        assert native_scene["hidden_states"] == native_scene["boundary_edges"] == 0
        canvas.screenshot(path=str(args.output / f"hierarchy-{name}-states-overview.png"))
        chosen = next(block for block in report["ssa"]["blocks"] if block["instructions"] and block["coverage"] == "Current")
        pick_graph_instance(page, canvas, chosen["state"])
        canvas.click(position={"x": 225, "y": 83}); settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
        native_focus = node_snapshot(page, canvas, args.output, f"hierarchy-{name}-states-focused")
        selected = re.search(r"Selected (σ[^;]+);", page.locator("#analysis-status").text_content())[1]
        assert selected == symbol("σ", chosen["state"]) and chosen["instructions"]
        assert chosen["incoming_complete"] is (report["status"] == "Converged")
        ink = layout_ink(page, canvas.screenshot(), native_focus["bounds"])
        assert ink["preview_pixels"] > 0, "real-runtime native focus did not paint SSA instructions"
        native_expected = {**native_scene, "detail": "SSA preview"}
        assert scene_counts("States") == native_expected
        for action in ["fit", "zoom", "focus"]:
            if action == "fit": page.keyboard.press("f")
            elif action == "zoom":
                page.mouse.move(700, 500); page.mouse.wheel(0, -80)
            else: canvas.click(position={"x": 225, "y": 83})
            settle_gesture(page)
            assert scene_counts("States") == native_expected
            expect(page.locator("#analysis-status")).to_contain_text(f"Selected {selected};")
            expect(page.locator("#analysis-status")).to_contain_text(report["status"])
        graph_mode(page, canvas, "Blocks")
        canvas.click(position={"x": 225, "y": 83}); settle_gesture(page)
        assert scene_counts() == {**scene, "detail": "SSA preview"}
        expect(page.locator("#analysis-status")).to_contain_text(f"Selected {selected};")
        source_focus = node_snapshot(page, canvas, args.output, f"hierarchy-{name}-source-ssa-focused")
        source_ink = layout_ink(page, canvas.screenshot(), source_focus["bounds"])
        assert source_ink["preview_pixels"] > 0, "source projection lost the selected native SSA preview"
        assert not posts, "runtime focus/fit/zoom/projection triggered another analysis"
        assert not errors, errors
        native_scene.update(focused=native_focus, selected_native_state=selected, ssa_instructions=len(chosen["instructions"]),
                            ssa_ink=ink, source_focus=source_focus, source_ink=source_ink, unexpected_task_posts=posts)
        results[name] = {"provenance": metadata, "request": request_evidence, "coverage_comparison": coverage,
                         "status": report["status"], "scene": scene, "native_scene": native_scene, "focused": focused,
                         "frontiers": len(report["frontiers"]), "pending": pending, "references": references, "ssa": ssa, "ending_status": ending_status,
                         "renderer": webgpu_evidence(page)}
        page.close()
    return results


def run(context, fixture):
    return hierarchy_real_runtime_interactions(context.browser, context.url, context.args, fixture_names=(fixture,))
