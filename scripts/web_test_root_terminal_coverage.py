"""Real form coverage controls and native terminal tuples, independent of CFG evidence.

This case runs only with matched server/Wasm assets. Its small two-block input
uses one transfer deliberately; every other execution/precision default stays
unchanged. Negative transport controls are recorded separately from real jobs.
"""

from copy import deepcopy
import json
import re
import time

from playwright.sync_api import expect

from web_test_harness import (complete_submission, observe_webgpu, open_input,
                              rendered_frame, resize_view, settle_gesture, webgpu_evidence)
from web_test_inspector_helpers import (inspector_painted_controls, inspector_selected_tab, inspector_title_action,
                                        select_inspector_tab)
from web_test_layout_helpers import LAYOUT_DEFAULT_LIMITS, layout_modal
from web_test_pixels import assert_translation, node_snapshot, text_pixels, workspace_content_bottom


RUNTIME = "6003565b602a5f5260205ff3"
OWNER = "0x" + "11" * 20
WORD_MAX = (1 << 256) - 1
COVERED = "Root terminal results conservatively covered"
NOT_ATTACHED = "Root terminal certificate not attached"
STORE_FIELDS = {
    "storage_default", "transient_default", "storage_root", "transient_root",
    "balance_default", "nonce_default", "code_default", "existence_default",
    "created_default", "pending_destruction_default", "touched_default",
    "accounts", "logs", "logs_unknown", "storage_facts", "transient_facts",
}


def _decimal_identity(value):
    assert type(value) is str and re.fullmatch(r"0|[1-9][0-9]*", value), value
    assert int(value) <= (1 << 64) - 1, value
    return value


def _node(report, ref, kind=None):
    assert type(ref) is dict and set(ref) == {"node"}, ref
    index = ref["node"]
    assert type(index) is int and 0 <= index < len(report["symbolic"]["nodes"]), ref
    node = report["symbolic"]["nodes"][index]
    if kind is not None:
        assert type(node) is dict and set(node) == {kind}, node
        return node[kind]
    return node


def _pool(report, name, index):
    assert type(index) is int and 0 <= index < len(report[name]), (name, index)
    return report[name][index]


def _top(value):
    assert value["constants"] is None and not value["nonzero"], value
    assert int(value["known_zero"], 16) == int(value["known_one"], 16) == 0, value
    assert {key: int(word, 16) for key, word in value["unsigned"].items()} == {
        "lower": 0, "upper": WORD_MAX}, value
    assert {key: int(word, 16) for key, word in value["signed"].items()} == {
        "lower": 1 << 255, "upper": (1 << 255) - 1}, value
    assert value["congruence"] == "Any", value


def _zero(value):
    assert value["constants"] == ["0x0"], value
    assert int(value["unsigned"]["lower"], 16) == int(value["unsigned"]["upper"], 16) == 0


def _empty_data(report, data, expected):
    # Empty constructors can retain an extra length certificate. Admit only a
    # typed zero proof; every other scalar, source and byte-array field stays exact.
    for value in [data["length"], expected["length"], data["default"], expected["default"]]:
        _zero(value)
        if value["expression"] is not None:
            assert _node(report, value["expression"], "Constant") == "0x0", value
    assert data == {**expected, "length": {**expected["length"],
                                          "expression": data["length"]["expression"]}}, (data, expected)


def _empty_data_controls(report, data, expected):
    """Exercise proof-layout equivalence and semantic refusals on real tuples."""
    zero = _node(report, data["symbolic"], "View")["length"]
    for actual_proof in [None, zero]:
        for expected_proof in [None, zero]:
            actual, template = deepcopy(data), deepcopy(expected)
            actual["length"] = {**actual["length"], "expression": actual_proof}
            template["length"] = {**template["length"], "expression": expected_proof}
            _empty_data(report, actual, template)
    nodes = report["symbolic"]["nodes"]
    fresh = next({"node": index} for index, node in enumerate(nodes)
                 if type(node) is dict and set(node) == {"Fresh"})
    one = {"node": len(nodes)}
    control = {**report, "symbolic": {**report["symbolic"], "nodes": [*nodes, {"Constant": "0x1"}]}}
    invalid_fields = [
        ("length", "constants", ["0x1"]), ("length", "known_one", "0x1"),
        ("length", "origins", ["Memory"]), ("length", "identity", {"scope": "0"}),
        ("length", "expression", one), ("length", "expression", fresh),
        ("length", "expression", {"node": True}),
        ("default", "constants", ["0x1"]), ("default", "origins", ["Memory"]),
        ("default", "expression", one), (None, "symbolic", fresh),
        (None, "cells", [{"offset": "0x0"}]), (None, "memory", True),
        (None, "unexpected", True),
    ]
    for container, field, value in invalid_fields:
        invalid = deepcopy(data)
        if container is None:
            invalid[field] = value
        else:
            invalid[container] = {**invalid[container], field: value}
        try:
            _empty_data(control, invalid, expected)
        except AssertionError:
            continue
        raise AssertionError(("accepted invalid empty data", container, field, value))
    invalid_expected = deepcopy(expected)
    invalid_expected["length"] = {**invalid_expected["length"], "expression": one}
    try:
        _empty_data(control, data, invalid_expected)
    except AssertionError:
        return {"positive": 4, "negative": len(invalid_fields) + 1}
    raise AssertionError("accepted a contradictory expected length proof")


def _stores(report):
    # These are required on ordinary reports too. Equal base refs remain
    # owner-scoped templates; this check never equates two account values.
    for store in report["stores"]:
        assert set(store) == STORE_FIELDS, set(store)
        for field in ["storage_root", "transient_root"]:
            node = _node(report, store[field])
            assert node == "SlotsZero" or (type(node) is dict and len(node) == 1
                                           and next(iter(node)) in {"SlotsInput", "Store", "SlotsIte"}), node
        assert store["existence_default"] in {"Unknown", "Present", "Absent"}
        code = store["code_default"]
        assert code is None or code in ("Empty", "Unknown") or (
            type(code) is dict and len(code) == 1 and next(iter(code)) in {"Runtime", "Delegation"}), code
        for field in ["created_default", "pending_destruction_default", "touched_default"]:
            assert store[field] is None or type(store[field]) is bool, (field, store[field])


def _binding(report, request, *, is_static):
    coverage = report["root_terminal_coverage"]
    assert set(coverage) == {"scope", "input", "outcomes", "trigger"}
    assert coverage["scope"] == "AllInputStates" and coverage["trigger"] == "Transfers", coverage
    binding = coverage["input"]
    assert set(binding) == {
        "snapshot", "revision", "storage_source", "input_scope", "fork", "entry_address",
        "environment", "config", "domain_spec", "numeric_model", "frame", "program", "calldata",
        "call_value", "saved_store", "initial_store"}, set(binding)
    assert set(binding["snapshot"]) == {"Offline"} and report["metadata"]["snapshot"] is None
    snapshot = binding["snapshot"]["Offline"]
    assert set(snapshot) == {"label", "namespace"} and snapshot["label"] == "web-bytecode"
    _decimal_identity(snapshot["namespace"])
    _decimal_identity(binding["revision"])
    scope = _decimal_identity(binding["input_scope"])
    assert binding["fork"] == report["fork"] == request["fork"] == "Osaka"
    assert binding["entry_address"] == report["metadata"]["entry_address"] == OWNER
    assert binding["environment"] == report["metadata"]["environment"]
    config = binding["config"]
    limits = request["limits"]
    assert report["metadata"]["postprocessing_max_work"] == 2_000_000_000
    assert 0 <= report["metadata"]["postprocessing_work"] <= 2_000_000_000
    relation_fields = {"enabled": "relations_enabled", "max_constraints": "max_constraints",
                       "max_nodes": "max_expression_nodes", "max_depth": "max_expression_depth"}
    assert binding["numeric_model"] == limits["numeric_model"] == "Product"
    execution_fields = {"context_depth", "max_states", "max_transfers", "max_work",
                        "max_call_depth", "max_memory_bytes", "use_summaries", "root_terminal_coverage"}
    assert config == {key: limits[key] for key in execution_fields}
    assert config["root_terminal_coverage"] is True
    assert binding["domain_spec"] == {
        "schema_version": 3, "word_bits": 256, "capacity": 512,
        "reduction_rounds": 16, "fact_limit": 4096, "cost_version": 8,
        "widening_after_updates": 2, "provenance_policy": "scoped-expressions-and-value-identities-v3",
        "relations": {key: limits[source] for key, source in relation_fields.items()}}
    original = next(state for state in report["states"] if state["state"] == 0)["entry"]
    root, = original["frames"]
    program, = [item for item in report["programs"] if item["id"] == binding["program"]]
    assert program["bytecode"].removeprefix("0x") == RUNTIME and program["kind"] == "Runtime"
    assert binding["program"] == report["metadata"]["root_program"] == root["program"]
    frame = binding["frame"]
    assert frame == {
        "code_address": program["code_address"], "code_hash": program["code_hash"],
        "mode": "Runtime", "address": OWNER,
        "address_value": {"Symbolic": {"kind": "To", "index": None}},
        "caller": {"Symbolic": {"kind": "Caller", "index": None}},
        "is_static": is_static, "basic_block_index": 0, "stack_height": 0, "jump_history": []}
    assert frame["address_value"] == root["address_value"] == binding["environment"]["to"]
    assert frame["caller"] == root["caller"] == binding["environment"]["caller"]
    assert root["stack"] == [] and root["context"] == [] and root["basic_block"] == 0
    assert binding["calldata"] == root["calldata"]
    assert binding["call_value"] == root["call_value"]
    identity = binding["call_value"]["identity"]
    assert identity == {"scope": scope, "symbol": {"kind": "CallValue", "index": None}}
    assert _node(report, binding["call_value"]["expression"], "Input") == identity
    assert binding["saved_store"] == root["rollback_store"]
    assert binding["initial_store"] == original["store"]
    saved = _pool(report, "stores", binding["saved_store"])
    initial = _pool(report, "stores", binding["initial_store"])
    calldata = _pool(report, "byte_arrays", binding["calldata"])
    _top(calldata["length"])
    view = _node(report, calldata["symbolic"], "View")
    assert _node(report, view["offset"], "Constant") == "0x0"
    assert calldata["length"]["expression"] == view["length"]
    assert _node(report, view["length"], "Input") == {
        "scope": scope, "symbol": {"kind": "CalldataLength", "index": None}}
    assert _node(report, view["state"], "BytesInput") == {
        "source": {"token": view["length"], "namespace": "0x0"},
        "length": view["length"], "bindings": []}
    original_calldata = _pool(report, "byte_arrays", binding["environment"]["calldata"])
    assert calldata == {**original_calldata,
                        "length": {**original_calldata["length"], "expression": view["length"]},
                        "symbolic": calldata["symbolic"]}
    assert _node(report, initial["storage_root"], "SlotsInput") == {
        "source": {"token": binding["storage_source"], "namespace": "0x0"}, "bindings": []}
    _node(report, binding["storage_source"])
    _stores(report)
    assert saved["code_default"] is None and initial["code_default"] is None
    return binding


def _rollback_store(report, binding, store):
    """The code-only fixture's saved root plus real sticky owner-3 cleanup.

    Each finalizer allocates its own selector. Check the complete Store under
    that guard, retaining original source/owner identities and all DTO fields.
    Anonymous top balance/nonce values have no scalar ITE in Core; their full
    metadata must still equal the original defaults.
    """
    saved = _pool(report, "stores", binding["saved_store"])
    root, = saved["accounts"]
    assert root["address"] == OWNER and root["code_kind"] == "Runtime"
    assert root["program"] == binding["program"]
    assert root["code_hash"] == binding["frame"]["code_hash"]
    assert _node(report, root["storage_root"], "SlotsInput") == {
        "source": {"token": binding["storage_source"], "namespace": hex(int(OWNER, 16) << 8)},
        "bindings": []}
    _zero(saved["transient_default"])
    assert _node(report, saved["transient_root"]) == "SlotsZero"
    assert _node(report, saved["storage_root"], "SlotsInput") == {
        "source": {"token": binding["storage_source"], "namespace": "0x0"}, "bindings": []}
    ripemd = "0x" + "00" * 19 + "03"
    cleanup, = [account for account in store["accounts"] if account["address"] == ripemd]
    merged = _node(report, cleanup["storage_root"], "SlotsIte")
    assert set(merged) == {"condition", "then_state", "else_state"}
    selector = _node(report, merged["condition"], "Fresh")
    assert type(selector) is int and 0 <= selector < 1 << 64
    assert _node(report, merged["then_state"], "SlotsInput") == {
        "source": {"token": binding["storage_source"], "namespace": "0x300"}, "bindings": []}
    assert _node(report, merged["else_state"]) == "SlotsZero"
    expected = deepcopy(saved)
    for field in ["created_default", "pending_destruction_default", "touched_default"]:
        expected[field] = False
    expected["transient_facts"] = []
    known = {**root, "transient_default": saved["transient_default"],
             "transient_root": saved["transient_root"], "transient": [],
             "created": False, "pending_destruction": False, "touched": False}
    unknown = {
        "address": ripemd, "existence": "Unknown", "balance": saved["balance_default"],
        "nonce": saved["nonce_default"], "code_kind": "Unknown", "code_hash": None,
        "program": None, "delegation_target": None,
        "storage_default": saved["storage_default"], "storage_root": cleanup["storage_root"],
        "storage": [], "transient_default": saved["transient_default"],
        "transient_root": saved["transient_root"], "transient": [],
        "created": False, "pending_destruction": False, "touched": False}
    expected["accounts"] = [unknown, known]
    assert store == expected, (store, expected)
    return selector


def _tuples(report, binding):
    outcomes = report["root_terminal_coverage"]["outcomes"]
    assert [outcome["kind"] for outcome in outcomes] == ["Return", "Revert", "Failure"]
    saved = _pool(report, "stores", binding["saved_store"])
    calldata = _pool(report, "byte_arrays", binding["calldata"])
    # Keep one set of independent source/length identities for the whole tuple.
    # A cleanup guard cannot also determine terminal bytes or original storage.
    independent = [_node(report, binding["storage_source"], "Fresh")]
    original = _pool(report, "byte_arrays", binding["environment"]["calldata"])
    original_view = _node(report, original["symbolic"], "View")
    original_source = _node(report, original_view["state"], "BytesInput")
    assert _node(report, original_view["offset"], "Constant") == "0x0"
    assert original_source == {
        "source": {"token": original_source["source"]["token"], "namespace": "0x0"},
        "length": original_view["length"], "bindings": []}
    assert original["length"]["expression"] == original_view["length"]
    independent.extend(_node(report, ref, "Fresh") for ref in
                       [original_view["length"], original_source["source"]["token"]])
    for outcome in outcomes:
        assert set(outcome) == {"kind", "data", "store"}, outcome
        data = _pool(report, "byte_arrays", outcome["data"])
        store = _pool(report, "stores", outcome["store"])
        assert data["memory"] is False
        view = _node(report, data["symbolic"], "View")
        _node(report, view["state"])
        assert set(view) == {"state", "offset", "length"}
        assert _node(report, view["offset"], "Constant") == "0x0"
        _zero(store["transient_default"])
        assert _node(report, store["transient_root"]) == "SlotsZero"
        assert all(store[field] is False for field in
                   ["created_default", "pending_destruction_default", "touched_default"])
        if outcome["kind"] == "Failure":
            _zero(data["length"])
            assert _node(report, view["length"], "Constant") == "0x0"
            assert _node(report, view["state"], "BytesExact") == "0x"
            expected = {**calldata, "length": saved["transient_default"],
                        "default": saved["transient_default"], "symbolic": data["symbolic"]}
            _empty_data(report, data, expected)
        else:
            _top(data["length"])
            assert data["cells"] == []
            assert _node(report, view["state"], "BytesInput")["bindings"] == []
            source = _node(report, view["state"], "BytesInput")
            assert source == {"source": {"token": source["source"]["token"], "namespace": "0x0"},
                              "length": view["length"], "bindings": []}
            independent.extend(_node(report, ref, "Fresh") for ref in
                               [view["length"], source["source"]["token"]])
            expected = {**calldata, "length": {**calldata["length"], "expression": view["length"]},
                        "symbolic": data["symbolic"]}
            assert data == expected, (outcome["kind"], data, expected)
    returned, reverted, failed = [_pool(report, "stores", item["store"]) for item in outcomes]
    # Static uses the same deliberately coarse Return upper bound. Its actual
    # input.static remains true; no identity/feasibility claim is made here.
    assert returned["accounts"] == [] and returned["logs_unknown"] is True
    assert returned["code_default"] is None and returned["existence_default"] == "Unknown"
    for field in ["storage_default", "balance_default", "nonce_default"]:
        _top(returned[field])
    assert returned["storage_root"] != report["stores"][binding["initial_store"]]["storage_root"]
    base = _node(report, returned["storage_root"], "SlotsInput")
    assert base == {"source": {"token": base["source"]["token"], "namespace": "0x0"}, "bindings": []}
    independent.append(_node(report, base["source"]["token"], "Fresh"))
    assert all(type(label) is int and 0 <= label < 1 << 64 for label in independent)
    assert len(set(independent)) == len(independent), independent
    expected_return = {**saved, "accounts": [], "storage_root": returned["storage_root"],
                       "logs": [], "logs_unknown": True, "storage_facts": [], "transient_facts": [],
                       "created_default": False, "pending_destruction_default": False,
                       "touched_default": False}
    assert returned == expected_return, (returned, expected_return)
    prepared_guards = [_rollback_store(report, binding, store) for store in [reverted, failed]]
    assert all(guard not in independent for guard in prepared_guards)
    _observed_failures(report, [*independent, *prepared_guards])
    assert not failed["logs"] and failed["logs_unknown"] is False


def _observed_failures(report, independent=()):
    # Ordinary detail has no certificate. Read the actual saved root and source
    # from the entry frame, retaining their identities for the rollback oracle.
    entry, = [state["entry"] for state in report["states"] if state["state"] == 0]
    root, = entry["frames"]
    saved = _pool(report, "stores", root["rollback_store"])
    storage = _node(report, saved["storage_root"], "SlotsInput")
    binding = {"saved_store": root["rollback_store"], "program": root["program"],
               "frame": {"code_hash": root["code_hash"]},
               "storage_source": storage["source"]["token"]}
    calldata = _pool(report, "byte_arrays", root["calldata"])
    original = _pool(report, "byte_arrays", report["metadata"]["environment"]["calldata"])
    original_view = _node(report, original["symbolic"], "View")
    original_source = _node(report, original_view["state"], "BytesInput")
    assert _node(report, original_view["offset"], "Constant") == "0x0"
    assert original_source == {
        "source": {"token": original_source["source"]["token"], "namespace": "0x0"},
        "length": original_view["length"], "bindings": []}
    assert original["length"]["expression"] == original_view["length"]
    original_refs = [binding["storage_source"], original_view["length"],
                     original_source["source"]["token"]]
    independent = [*independent, *(_node(report, ref, "Fresh") for ref in original_refs)]
    assert all(type(label) is int and 0 <= label < 1 << 64 for label in independent)
    pre_entry, finalized = 0, 0
    for outcome in report["outcomes"]:
        assert set(outcome) == {"state", "kind", "data", "store"}, outcome
        assert outcome["kind"] == "Failure" and outcome["state"] == 0, outcome
        data = _pool(report, "byte_arrays", outcome["data"])
        assert data["memory"] is False
        _zero(data["length"])
        view = _node(report, data["symbolic"], "View")
        assert set(view) == {"state", "offset", "length"}
        assert _node(report, view["offset"], "Constant") == "0x0"
        assert _node(report, view["length"], "Constant") == "0x0"
        assert _node(report, view["state"], "BytesExact") == "0x"
        expected = {**calldata, "length": saved["transient_default"],
                    "default": saved["transient_default"], "symbolic": data["symbolic"]}
        _empty_data(report, data, expected)
        store = _pool(report, "stores", outcome["store"])
        # Engine's pre-entry Failure clones the whole saved Store. Every other
        # observed Failure must match the complete independently finalized root.
        if store == saved:
            pre_entry += 1
        else:
            assert _rollback_store(report, binding, store) not in independent
            finalized += 1
    assert pre_entry > 0 and finalized > 0, (pre_entry, finalized)
    _empty_data_controls(report, data, expected)


def _detail(report):
    assert report["status"] == "Incomplete" and report["transfers"] == 1
    assert report["ssa"]["complete"] is False
    assert report["frontiers"] and all(item["kind"] == "Transfers" for item in report["frontiers"])
    entry, = [block for block in report["cfg"] if block["id"] == 0]
    assert entry["executed_pcs"] == [0, 2]
    assert all(block["executed_pcs"] == [] for block in report["cfg"] if block["id"] != 0)
    block, = [block for block in report["ssa"]["blocks"] if block["state"] == 0]
    assert [(item["pc"], item["opcode"], item["progress"]) for item in block["instructions"]] == [
        (0, 0x60, "Completed"), (2, 0x56, "Dispatched")]
    _observed_failures(report)
    return {**{key: report[key] for key in
               ["status", "transfers", "cfg", "edges", "ssa", "frontiers", "diagnostics", "outcomes"]},
            "pending_machines": len(report["pending_machines"]),
            "native_states": [state["state"] for state in report["states"]]}


def _selection(status):
    found = re.search(r"Selected [^;]+; source [^;]+;", status)
    assert found, status
    return found[0]


def _panel_snapshot(page, canvas, output, name):
    page.mouse.move(2, 2)
    settle_gesture(page)
    # CollapsingHeader's real animation must finish before row targeting.
    page.wait_for_timeout(200)
    toolbar = inspector_selected_tab(page, canvas, "Outcomes")
    observations = [rect for rect in inspector_painted_controls(page, canvas)["selected"]
                    if 20 <= rect[0] <= 50 and 70 <= rect[2] - rect[0] <= 85
                    and 0 <= rect[1] - toolbar[3] <= 10 and 14 <= rect[3] - rect[1] <= 20]
    assert len(observations) == 1, f"expected one selected Block entry control, got {observations}"
    png = canvas.screenshot(path=str(output / f"{name}.png"))
    # Outcomes content follows the native observation bar and its separator gap.
    region = (8, observations[0][3] + 6, 400, workspace_content_bottom(page, png))
    pixels = text_pixels(page, png, region)
    assert pixels["white_pixels"] > 100 and len(pixels["rows"]) >= 8, pixels
    return pixels


def _row_click(page, canvas, pixels, index, *, x=80):
    assert 0 <= index < len(pixels["rows"]), (index, pixels)
    top, bottom = pixels["rows"][index]
    canvas.click(position={"x": x, "y": (top + bottom) / 2})
    settle_gesture(page)


def _inspect(page, canvas, report, output, posts):
    resize_view(page, 1920, 1200)
    select_inspector_tab(page, canvas, "Outcomes")
    inspector_title_action(page, canvas, "Maximize", inspector_selected_tab(page, canvas, "Outcomes"))
    status = page.locator("#analysis-status")
    selected = _selection(status.text_content())
    count = len(posts)
    expect(status).to_contain_text(f"{len(report['outcomes'])} observed outcomes")
    for label in ["Return alternative", "Revert alternative", "Failure alternative"]:
        expect(status).to_contain_text(label)
    # At this width the two fixed explanatory labels each occupy one line.
    # Existing painted-text measurement targets real CollapsingHeaders: title,
    # coverage, explanation, binding, then four rows per default-open tuple.
    initial = _panel_snapshot(page, canvas, output, "coverage-all-alternatives")
    for row in [12, 8, 4]:
        pixels = _panel_snapshot(page, canvas, output, f"coverage-before-collapse-{row}")
        _row_click(page, canvas, pixels, row)
    collapsed = _panel_snapshot(page, canvas, output, "coverage-alternatives-collapsed")
    assert len(collapsed["rows"]) < len(initial["rows"])
    result = {}
    for index, outcome in enumerate(report["root_terminal_coverage"]["outcomes"]):
        name = outcome["kind"].lower()
        row = 4 + index
        before = _panel_snapshot(page, canvas, output, f"coverage-{name}-closed")
        _row_click(page, canvas, before, row)
        opened = _panel_snapshot(page, canvas, output, f"coverage-{name}-alternative")
        assert len(opened["rows"]) >= len(before["rows"]) + 3
        _row_click(page, canvas, opened, row + 2)  # Actual Terminal bytes header.
        data = _panel_snapshot(page, canvas, output, f"coverage-{name}-bytes")
        assert data["hash"] != opened["hash"] and len(data["rows"]) > len(opened["rows"])
        _row_click(page, canvas, data, row + 2)
        opened = _panel_snapshot(page, canvas, output, f"coverage-{name}-bytes-closed")
        _row_click(page, canvas, opened, row + 3)  # Actual Finalized store header.
        store = _panel_snapshot(page, canvas, output, f"coverage-{name}-store")
        assert len(store["rows"]) > len(opened["rows"])
        _row_click(page, canvas, store, row + 5)  # Unlisted accounts and storage bases.
        defaults = _panel_snapshot(page, canvas, output, f"coverage-{name}-store-defaults")
        assert len(defaults["rows"]) >= len(store["rows"]) + 9
        _row_click(page, canvas, defaults, row + 5)
        store = _panel_snapshot(page, canvas, output, f"coverage-{name}-defaults-closed")
        _row_click(page, canvas, store, row + 3)
        opened = _panel_snapshot(page, canvas, output, f"coverage-{name}-store-closed")
        _row_click(page, canvas, opened, row)
        assert _selection(status.text_content()) == selected and len(posts) == count
        result[name] = {"tuple": outcome, "bytes": data, "store": defaults}
    closed = _panel_snapshot(page, canvas, output, "coverage-before-binding")
    _row_click(page, canvas, closed, 3)
    binding = _panel_snapshot(page, canvas, output, "coverage-original-input-binding")
    assert len(binding["rows"]) >= len(closed["rows"]) + 21
    assert _selection(status.text_content()) == selected and len(posts) == count
    result["binding"] = binding
    result["selected_native_state_unchanged"] = selected
    result["analysis_posts_during_inspection"] = len(posts) - count
    return result


def _invalid_requests(page, url, request, output):
    controls = [
        ("numeric-string", "max_transfers", "1"),
        ("numeric-overflow", "max_transfers", 1 << 64),
        ("boolean-string", "root_terminal_coverage", "true"),
        ("boolean-number", "root_terminal_coverage", 1),
        ("unknown-limit", "root_terminal_coverag", True),
        ("obsolete-selector", "domain_profile", "Product"),
        ("obsolete-model", "numeric_model", "ConstantsOnly"),
        ("missing-model", "numeric_model", None),
    ]
    results = {}
    for name, field, value in controls:
        invalid = deepcopy(request)
        if name == "missing-model":
            del invalid["limits"][field]
        else:
            invalid["limits"][field] = value
        response = page.request.post(url + "/api/tasks", data=json.dumps(invalid),
                                     headers={"Content-Type": "application/json"})
        assert response.status == 400, (name, response.text())
        error = response.json()["result"]["Err"]
        assert error["code"] == "InvalidRequest" and error["details"] is not None, (name, error)
        results[name] = {"request": invalid, "status": response.status, "error": error}
    (output / "coverage-rejected-requests.json").write_text(json.dumps(results, indent=2) + "\n")
    return results


def _static_control(page, url, request, output):
    """One real typed API companion; the main opt-in path uses the form above."""
    request = deepcopy(request)
    request["environment"]["is_static"] = True
    admitted = page.request.post(url + "/api/tasks", data=json.dumps(request),
                                 headers={"Content-Type": "application/json"})
    assert admitted.status == 202, admitted.text()
    job = admitted.json()["result"]["Ok"]
    task = url + "/api/tasks/" + str(job["id"])
    deadline = time.monotonic() + 60
    while job["state"] != "Completed":
        assert job["state"] in ("Queued", "Running"), job
        assert time.monotonic() < deadline, job
        page.wait_for_timeout(30)
        polled = page.request.get(task)
        assert polled.ok, polled.text()
        job = polled.json()["result"]["Ok"]
    response = page.request.get(task + "/result")
    assert response.ok, response.text()
    report = response.json()["result"]["Ok"]
    _detail(report)
    binding = _binding(report, request, is_static=True)
    _tuples(report, binding)
    (output / "coverage-static-request.json").write_text(json.dumps(request, indent=2) + "\n")
    (output / "coverage-static-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    return {"job_id": job["id"], "is_static": binding["frame"]["is_static"],
            "return_store": report["root_terminal_coverage"]["outcomes"][0]["store"]}


def _schema_control(page, canvas, output, expected_request):
    """Reject an old wire version in the real WASM client, retaining its report."""
    intercepted = []
    status = page.locator("#analysis-status")
    before_status = status.text_content()
    evidence = {"before": {"status": before_status}}
    evidence_path = output / "coverage-schema-retained-result.json"
    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n")
    before_card = node_snapshot(page, canvas, output, "coverage-schema-before")
    before_ink = text_pixels(page, (output / "coverage-schema-before.png").read_bytes(), before_card["bounds"])
    selected = _selection(before_status)
    result_ids = re.findall(r"(?:^|; )Job ID: ([^;]+)(?=;|$)", before_status)
    assert before_status.startswith("Ready: ") and len(result_ids) == 1, before_status
    result_id = _decimal_identity(result_ids[0])
    evidence["before"].update(selection=selected, result_job_id=result_id, card=before_card, ink=before_ink)
    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n")
    posts = []
    record_post = lambda request: posts.append(request.post_data) if (
        request.method == "POST" and request.url.endswith("/api/tasks")) else None

    def old_schema(route):
        response = route.fetch()
        native_text = response.text()
        envelope = json.loads(native_text)
        assert envelope["result"]["Ok"]["schema_version"] == 19
        envelope["result"]["Ok"]["schema_version"] = 16
        replacement = json.dumps(envelope)
        headers = dict(response.headers)
        headers.pop("content-encoding", None)
        headers.pop("content-length", None)
        # This body is a deliberately invalid client control, never evidence
        # of a live wire schema-16 report or of an attached certificate.
        (output / "coverage-schema-control-native.json").write_text(native_text + "\n")
        (output / "coverage-schema-control-mutated.json").write_text(replacement + "\n")
        route.fulfill(status=response.status, headers=headers, body=replacement)
        intercepted.append(True)

    page.route("**/api/tasks/*/result", old_schema)
    page.on("request", record_post)
    try:
        open_input(page)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                  and response.request.method == "POST") as submitted:
            page.keyboard.press("Control+Enter")
        assert submitted.value.status == 202
        assert json.loads(submitted.value.request.post_data) == expected_request
        current_id = _decimal_identity(str(submitted.value.json()["result"]["Ok"]["id"]))
        try:
            expect(status).to_contain_text("Unsupported response schema 16; expected 17", timeout=60000)
        finally:
            canvas.screenshot(path=str(output / "coverage-schema-rejected.png"))
        after_status = status.text_content()
        evidence["after"] = {"status": after_status, "admitted_job_id": current_id, "task_posts": posts}
        evidence_path.write_text(json.dumps(evidence, indent=2) + "\n")
        after_card = node_snapshot(page, canvas, output, "coverage-schema-rejected")
        after_ink = text_pixels(page, (output / "coverage-schema-rejected.png").read_bytes(), after_card["bounds"])
        evidence["after"].update(card=after_card, ink=after_ink)
        evidence_path.write_text(json.dumps(evidence, indent=2) + "\n")
        assert intercepted == [True], intercepted
        assert len(posts) == 1 and json.loads(posts[0]) == expected_request, posts
        assert after_status.startswith("Error: Unsupported response schema 16; expected 17; "), after_status
        current_ids = re.findall(r"(?:^|; )Current Job ID: ([^;]+)(?=;|$)", after_status)
        retained_ids = re.findall(r"(?:^|; )Result Job ID: ([^;]+)(?=;|$)", after_status)
        assert len(current_ids) == len(retained_ids) == 1, after_status
        assert _decimal_identity(current_ids[0]) == current_id != result_id
        assert _decimal_identity(retained_ids[0]) == result_id
        # Error text omits Ready's Selected fields. The retained result ID and
        # identical selected-card geometry/ink bind that earlier selection here.
        assert_translation(before_card, after_card, 0, 0, tolerance=1.0)
        assert after_card["selected_kind"] == before_card["selected_kind"]
        assert after_ink == before_ink, (before_ink, after_ink)
        return {"received": 16, "expected": 17, "retained_selection": selected,
                "retained_result_job_id": result_id, "current_job_id": current_id,
                "evidence": evidence, "status": after_status, "control_only": True}
    finally:
        page.remove_listener("request", record_post)
        page.unroute("**/api/tasks/*/result", old_schema)


def root_terminal_coverage_interactions(context):
    page = context.browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors, posts = [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("request", lambda request: posts.append(request.post_data)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    output = context.output
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(context.url, wait_until="networkidle")
    first = complete_submission(page, initial.value)
    assert first.status == 200
    defaults = json.loads(first.request.post_data)
    assert defaults["limits"] == LAYOUT_DEFAULT_LIMITS
    assert defaults["limits"]["root_terminal_coverage"] is True
    ordinary = first.json()["result"]["Ok"]
    assert ordinary["schema_version"] == 19 and ordinary["root_terminal_coverage"] is None
    assert ordinary["status"] == "Converged" and ordinary["ssa"]["complete"] is True
    assert ordinary["frontiers"] == []
    _stores(ordinary)
    canvas, status = page.locator("#evm-canvas"), page.locator("#analysis-status")
    assert COVERED not in status.text_content() and NOT_ATTACHED not in status.text_content()
    (output / "coverage-default-enabled-request.json").write_text(first.request.post_data + "\n")
    (output / "coverage-default-enabled-analysis.json").write_text(json.dumps(ordinary, indent=2) + "\n")
    canvas.screenshot(path=str(output / "coverage-default-enabled-complete.png"))

    execution_open, coverage_enabled = False, True
    header_offset = None

    def submit(name, *, enabled, transfers=1, invalid_numeric=False):
        nonlocal execution_open, coverage_enabled, header_offset
        geometry = {"captures": {}, "targets": []}

        def save_geometry():
            (output / f"{name}-geometry.json").write_text(json.dumps(geometry, indent=2) + "\n")

        def capture(stage):
            page.mouse.move(2, 2)
            settle_gesture(page)
            page.wait_for_timeout(200)  # Finish the native collapse/recentering animation.
            png = canvas.screenshot(path=str(output / f"{stage}.png"))
            bounds = layout_modal(page, canvas)
            geometry["captures"][stage] = {"modal": bounds}
            save_geometry()
            assert page.viewport_size == {"width": 1440, "height": 1000}
            assert (0 < bounds[0] < bounds[2] < 1440 and 0 < bounds[1] < bounds[3] < 1000
                    and 750 <= bounds[2] - bounds[0] <= 780), (stage, bounds)
            # A narrow label column excludes numeric editor borders/error text.
            # The footer is outside this strip; the last six collapsed rows are
            # the six native headers visible in the saved 1440x1000 capture.
            region = (bounds[0] + 29, bounds[1] + (header_offset or 10),
                      bounds[0] + 95, bounds[3] - 40)
            pixels = text_pixels(page, png, region)
            errors = text_pixels(page, png, (bounds[0] + 10, bounds[1] + 10,
                                             bounds[2] - 10, bounds[3] - 40))["error_pixels"]
            captured = {"stage": stage, "modal": bounds, "region": region,
                        **pixels, "form_error_pixels": errors}
            geometry["captures"][stage] = captured
            save_geometry()
            return captured

        def click(captured, row, x, label):
            top, bottom = captured["rows"][row]
            bounds = captured["modal"]
            target = {"stage": captured["stage"], "label": label, "row": [top, bottom],
                      "x": x, "y": (top + bottom) / 2}
            geometry["targets"].append(target)
            save_geometry()
            assert bounds[0] < x < bounds[2] and bounds[1] < top < bottom < bounds[3], target
            _row_click(page, canvas, captured, row, x=x)

        open_input(page)
        page.keyboard.press("Control+A")
        page.keyboard.insert_text(RUNTIME)
        rendered_frame(page)
        form = capture(f"{name}-before-execution")
        if not execution_open:
            headers = form["rows"][-6:]
            assert len(headers) == 6 and all(20 <= second[0] - first[0] <= 26
                                           for first, second in zip(headers, headers[1:])), form
            header_offset = headers[0][0] - form["modal"][1] - 2
            header_positions = [row[0] - form["modal"][1] for row in headers[:4]]
            click(form, len(form["rows"]) - 3, form["modal"][0] + 70, "Execution budget")
            form = capture(f"{name}-execution-expanded")
            # limits.rs: three environment headers, Execution budget, five
            # numeric rows, summaries, coverage, explanation, Precision, RPC.
            # A missing control or a hit on Analyze must fail before editing.
            assert len(form["rows"]) == 14, form
            assert all(abs(row[0] - form["modal"][1] - expected) <= 2
                       for row, expected in zip(form["rows"][:4], header_positions)), form
            assert form["rows"][-2][0] - form["rows"][3][1] > 150, form
            execution_open = True

        def transfer(text):
            assert len(form["rows"]) == 14, form
            assert 20 <= form["rows"][5][0] - form["rows"][4][0] <= 26, form
            # The second measured grid label is Transfers; x is inside the
            # native 180-point value editor, beyond the label column.
            click(form, 5, form["modal"][0] + 180, "Transfers")
            page.keyboard.press("Control+A")
            page.keyboard.insert_text(text)
            rendered_frame(page)

        if invalid_numeric:
            transfer("18446744073709551616")
            before_posts = len(posts)
            page.keyboard.press("Control+Enter")
            rendered_frame(page)
            page.wait_for_timeout(200)
            assert len(posts) == before_posts, "invalid numeric form admitted a task"
            invalid = capture("coverage-invalid-numeric-form")
            assert len(invalid["rows"]) == 14 and invalid["form_error_pixels"] > 20, invalid
            # The unsupported-u64 error adds a real grid row below Transfers.
            assert (invalid["rows"][6][0] - invalid["rows"][5][1]
                    > form["rows"][6][0] - form["rows"][5][1] + 10), invalid
            form = invalid
        transfer(str(transfers))
        form = capture(f"{name}-numeric-recovered")
        assert len(form["rows"]) == 14 and form["form_error_pixels"] == 0, form
        assert all(20 <= second[0] - first[0] <= 26
                   for first, second in zip(form["rows"][4:8], form["rows"][5:9])), form
        if enabled != coverage_enabled:
            click(form, 10, form["modal"][0] + 70, "Prepare conservative root terminal coverage")
            coverage_enabled = enabled
        final_form = capture(f"{name}-form")
        assert len(final_form["rows"]) == 14 and final_form["form_error_pixels"] == 0, final_form
        assert all(abs(sum(after) / 2 - sum(before) / 2) <= 2
                   for before, after in zip(form["rows"], final_form["rows"])), final_form
        with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                  and response.request.method == "POST") as posted:
            page.keyboard.press("Control+Enter")
        # Preserve the actual form POST even if completion or exact decoding fails.
        (output / f"{name}-request.json").write_text(posted.value.request.post_data + "\n")
        reply = complete_submission(page, posted.value)
        assert reply.admission_status == 202 and reply.status == 200, reply.json()
        request = json.loads(reply.request.post_data)
        expected = {**defaults, "input": {"Bytecode": {"bytecode": RUNTIME, "address": None}},
                    "limits": {**LAYOUT_DEFAULT_LIMITS, "max_transfers": transfers,
                               "root_terminal_coverage": enabled}}
        assert request == expected, (name, request, expected)
        report = reply.json()["result"]["Ok"]
        assert report["schema_version"] == 19 and report["metadata"]["limits"] == expected["limits"]
        assert report["fork"] == request["fork"] == "Osaka"
        (output / f"{name}-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
        return report, request

    disabled, _ = submit("coverage-disabled-cap", enabled=False, invalid_numeric=True)
    assert disabled["root_terminal_coverage"] is None
    prior_detail = _detail(disabled)
    _stores(disabled)

    covered, request = submit("coverage-enabled-cap", enabled=True)
    assert _detail(covered) == prior_detail, "attachment changed detailed receipts/CFG/SSA/frontiers/outcomes"
    binding = _binding(covered, request, is_static=False)
    _tuples(covered, binding)
    expect(status).to_contain_text("Detail incomplete / partial SSA")
    expect(status).to_contain_text(COVERED)
    inspect = _inspect(page, canvas, covered, output, posts)
    # Return to the original viewport before editing another actual form.
    inspector_title_action(page, canvas, "Restore", inspector_selected_tab(page, canvas, "Outcomes"))
    resize_view(page, 1440, 1000)

    missing_certificates = []

    def without_certificate(route):
        job_match = re.search(r"/api/tasks/(0|[1-9][0-9]*)/result\Z", route.request.url)
        assert job_match, route.request.url
        job_id = _decimal_identity(job_match[1])
        response = route.fetch()
        native_text = response.text()
        envelope = json.loads(native_text)
        native = envelope["result"]["Ok"]
        assert native["metadata"]["limits"]["root_terminal_coverage"] is True
        assert native["root_terminal_coverage"] is not None and _detail(native) == prior_detail
        (output / "coverage-missing-certificate-client-control-native.json").write_text(native_text + "\n")
        # Explicit client presentation control: preserve real Incomplete detail
        # and remove only its certificate. This is not a native producer claim.
        ui_envelope = deepcopy(envelope)
        ui_envelope["result"]["Ok"]["root_terminal_coverage"] = None
        replacement = json.dumps(ui_envelope)
        ui = json.loads(replacement)["result"]["Ok"]
        assert ui == {**native, "root_terminal_coverage": None}, "client control changed other report fields"
        (output / "coverage-missing-certificate-client-control-ui.json").write_text(replacement + "\n")
        headers = dict(response.headers)
        headers.pop("content-encoding", None)
        headers.pop("content-length", None)
        route.fulfill(status=response.status, headers=headers, body=replacement)
        missing_certificates.append({"request_url": route.request.url, "job_id": job_id,
                                     "native_text": native_text, "ui_text": replacement})

    page.route("**/api/tasks/*/result", without_certificate)
    try:
        missing, _ = submit("coverage-missing-certificate-control", enabled=True)
        # complete_submission reads the native API directly, bypassing this UI route.
        assert missing["root_terminal_coverage"] is not None and _detail(missing) == prior_detail
        assert len(missing_certificates) == 1, missing_certificates
        intercepted, = missing_certificates
        assert missing == json.loads(intercepted["native_text"])["result"]["Ok"], "same-job native report changed"
        assert json.loads(intercepted["ui_text"])["result"]["Ok"] == {**missing, "root_terminal_coverage": None}
        expect(status).to_contain_text(NOT_ATTACHED)
        expect(status).to_contain_text("Incomplete / partial SSA")
        assert COVERED not in status.text_content()
        missing_status = status.text_content()
        job_ids = re.findall(r"(?:^|; )Job ID: ([^;]+)(?=;|$)", missing_status)
        assert len(job_ids) == 1, missing_status
        assert _decimal_identity(job_ids[0]) == intercepted["job_id"], intercepted["request_url"]
        canvas.screenshot(path=str(output / "coverage-missing-certificate-control.png"))
    finally:
        page.unroute("**/api/tasks/*/result", without_certificate)

    complete, complete_request = submit("coverage-enabled-complete", enabled=True,
                                         transfers=LAYOUT_DEFAULT_LIMITS["max_transfers"])
    assert complete_request["limits"]["root_terminal_coverage"] is True
    assert complete["status"] == "Converged" and complete["ssa"]["complete"] is True
    assert complete["frontiers"] == [] and complete["root_terminal_coverage"] is None
    assert COVERED not in status.text_content() and NOT_ATTACHED not in status.text_content()
    negatives = _invalid_requests(page, context.url, request, output)
    static = _static_control(page, context.url, request, output)
    schema = _schema_control(page, canvas, output, complete_request)
    result = {"default_enabled": defaults["limits"]["root_terminal_coverage"],
              "attached": {"scope": covered["root_terminal_coverage"]["scope"],
                           "trigger": covered["root_terminal_coverage"]["trigger"],
                           "binding": binding, "inspection": inspect},
              "ordinary_outcome_count": len(covered["outcomes"]),
              "detail_preserved": prior_detail, "enabled_converged_certificate": None,
              "missing_certificate_control": {"status": missing_status, "control_only": True},
              "rejected_requests": negatives, "static_api_companion": static,
              "schema_control": schema, "renderer": webgpu_evidence(page)}
    assert len(posts) == 6, posts
    assert not errors, errors
    (output / "coverage-report.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result


def run(context):
    return root_terminal_coverage_interactions(context)
