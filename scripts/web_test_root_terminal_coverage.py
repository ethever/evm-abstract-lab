"""Real form opt-in and native terminal tuples, independent of CFG evidence.

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
from web_test_inspector_helpers import (inspector_selected_tab, inspector_title_action,
                                        select_inspector_tab)
from web_test_layout_helpers import LAYOUT_DEFAULT_LIMITS, layout_modal
from web_test_pixels import text_pixels, workspace_content_bottom


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
        "environment", "config", "domain_spec", "frame", "program", "calldata",
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
    relation_fields = {"enabled": "relations_enabled", "max_constraints": "max_constraints",
                       "max_nodes": "max_expression_nodes", "max_depth": "max_expression_depth"}
    assert config["relations"] == {key: limits[source] for key, source in relation_fields.items()}
    assert {key: value for key, value in config.items() if key != "relations"} == {
        key: value for key, value in limits.items()
        if not key.startswith("rpc_") and key not in relation_fields.values()}
    assert config["root_terminal_coverage"] is True
    assert binding["domain_spec"] == {
        "schema_version": 2, "word_bits": 256, "profile": "Product", "capacity": 512,
        "reduction_rounds": 16, "fact_limit": 4096, "cost_version": 3,
        "widening_after_updates": 2, "provenance_policy": "scoped-expressions-and-value-identities-v3",
        "relations": config["relations"]}
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
    _top(_pool(report, "byte_arrays", binding["calldata"])["length"])
    assert _node(report, initial["storage_root"], "SlotsInput")["source"]["token"] == binding["storage_source"]
    _node(report, binding["storage_source"])
    _stores(report)
    assert saved["code_default"] is None and initial["code_default"] is None
    return binding


def _tuples(report, binding):
    outcomes = report["root_terminal_coverage"]["outcomes"]
    assert [outcome["kind"] for outcome in outcomes] == ["Return", "Revert", "Failure"]
    for outcome in outcomes:
        assert set(outcome) == {"kind", "data", "store"}, outcome
        data = _pool(report, "byte_arrays", outcome["data"])
        store = _pool(report, "stores", outcome["store"])
        assert data["memory"] is False
        view = _node(report, data["symbolic"], "View")
        _node(report, view["state"])
        _zero(store["transient_default"])
        assert _node(report, store["transient_root"]) == "SlotsZero"
        assert all(store[field] is False for field in
                   ["created_default", "pending_destruction_default", "touched_default"])
        if outcome["kind"] == "Failure":
            _zero(data["length"])
            assert _node(report, view["length"], "Constant") == "0x0"
        else:
            _top(data["length"])
            assert data["cells"] == []
            assert _node(report, view["state"], "BytesInput")["bindings"] == []
    returned, reverted, failed = [_pool(report, "stores", item["store"]) for item in outcomes]
    # Static uses the same deliberately coarse Return upper bound. Its actual
    # input.static remains true; no identity/feasibility claim is made here.
    assert returned["accounts"] == [] and returned["logs_unknown"] is True
    assert returned["code_default"] is None and returned["existence_default"] == "Unknown"
    for field in ["storage_default", "balance_default", "nonce_default"]:
        _top(returned[field])
    assert returned["storage_root"] != report["stores"][binding["initial_store"]]["storage_root"]
    assert reverted == failed
    # Check one actual finalized rollback Store jointly. Optional owner-3
    # cleanup may widen the entry Failure, so do not demand checkpoint equality.
    observed, = report["outcomes"]
    assert observed["kind"] == "Failure"
    assert not failed["logs"] and failed["logs_unknown"] is False
    root, = [account for account in failed["accounts"] if account["address"] == OWNER]
    assert root["code_kind"] == "Runtime" and root["program"] == binding["program"]
    saved, = [account for account in report["stores"][binding["saved_store"]]["accounts"]
              if account["address"] == OWNER]
    for field in ["existence", "balance", "nonce", "code_kind", "code_hash", "program",
                  "delegation_target", "storage_default", "storage_root", "storage"]:
        assert root[field] == saved[field], (field, root, saved)


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
    assert len(report["outcomes"]) == 1 and report["outcomes"][0]["kind"] == "Failure"
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
    png = canvas.screenshot(path=str(output / f"{name}.png"))
    region = (8, toolbar[3] + 3, 400, workspace_content_bottom(page, png))
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
    ]
    results = {}
    for name, field, value in controls:
        invalid = deepcopy(request)
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
    selected = _selection(status.text_content())

    def old_schema(route):
        response = route.fetch()
        native_text = response.text()
        envelope = json.loads(native_text)
        assert envelope["result"]["Ok"]["schema_version"] == 15
        envelope["result"]["Ok"]["schema_version"] = 14
        replacement = json.dumps(envelope)
        headers = dict(response.headers)
        headers.pop("content-encoding", None)
        headers.pop("content-length", None)
        # This body is a deliberately invalid client control, never evidence
        # of a live native schema-14 report or of an attached certificate.
        (output / "coverage-schema-control-native.json").write_text(native_text + "\n")
        (output / "coverage-schema-control-mutated.json").write_text(replacement + "\n")
        route.fulfill(status=response.status, headers=headers, body=replacement)
        intercepted.append(True)

    page.route("**/api/tasks/*/result", old_schema)
    try:
        open_input(page)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                  and response.request.method == "POST") as submitted:
            page.keyboard.press("Control+Enter")
        assert submitted.value.status == 202
        assert json.loads(submitted.value.request.post_data) == expected_request
        expect(status).to_contain_text("Unsupported response schema", timeout=60000)
        assert intercepted == [True], intercepted
        assert _selection(status.text_content()) == selected
        canvas.screenshot(path=str(output / "coverage-schema-rejected.png"))
        return {"received": 14, "expected": 15, "retained_selection": selected,
                "status": status.text_content(), "control_only": True}
    finally:
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
    assert defaults["limits"]["root_terminal_coverage"] is False
    ordinary = first.json()["result"]["Ok"]
    assert ordinary["schema_version"] == 15 and ordinary["root_terminal_coverage"] is None
    _stores(ordinary)
    canvas, status = page.locator("#evm-canvas"), page.locator("#analysis-status")
    assert COVERED not in status.text_content() and NOT_ATTACHED not in status.text_content()
    (output / "coverage-default-off-request.json").write_text(first.request.post_data + "\n")
    (output / "coverage-default-off-analysis.json").write_text(json.dumps(ordinary, indent=2) + "\n")

    execution_open, coverage_enabled = False, False

    def submit(name, *, enabled, transfers=1, invalid_numeric=False):
        nonlocal execution_open, coverage_enabled
        open_input(page)
        page.keyboard.press("Control+A")
        page.keyboard.insert_text(RUNTIME)
        rendered_frame(page)
        modal = layout_modal(page, canvas)
        # Existing unknown-environment form: Transaction environment is at
        # +209, followed by three 23-point header rows to Execution budget.
        if not execution_open:
            canvas.click(position={"x": modal[0] + 70, "y": modal[1] + 278})
            settle_gesture(page)
            execution_open = True
        # Execution grid has five 23-point numeric rows; the new checkbox is
        # immediately after the existing summary checkbox (Web frozen contract).
        def transfer(text):
            bounds = layout_modal(page, canvas)
            canvas.click(position={"x": bounds[0] + 160, "y": bounds[1] + 324})
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
            canvas.screenshot(path=str(output / "coverage-invalid-numeric-form.png"))
        transfer(str(transfers))
        if enabled != coverage_enabled:
            bounds = layout_modal(page, canvas)
            canvas.click(position={"x": bounds[0] + 35, "y": bounds[1] + 439})
            settle_gesture(page)
            coverage_enabled = enabled
        canvas.screenshot(path=str(output / f"{name}-form.png"))
        with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                  and response.request.method == "POST") as posted:
            page.keyboard.press("Control+Enter")
        reply = complete_submission(page, posted.value)
        assert reply.admission_status == 202 and reply.status == 200, reply.json()
        request = json.loads(reply.request.post_data)
        expected = {**defaults, "input": {"Bytecode": {"bytecode": RUNTIME, "address": None}},
                    "limits": {**LAYOUT_DEFAULT_LIMITS, "max_transfers": transfers,
                               "root_terminal_coverage": enabled}}
        assert request == expected, (name, request, expected)
        report = reply.json()["result"]["Ok"]
        assert report["schema_version"] == 15 and report["metadata"]["limits"] == expected["limits"]
        assert report["fork"] == request["fork"] == "Osaka"
        (output / f"{name}-request.json").write_text(reply.request.post_data + "\n")
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
    complete, complete_request = submit("coverage-enabled-complete", enabled=True,
                                         transfers=LAYOUT_DEFAULT_LIMITS["max_transfers"])
    assert complete_request["limits"]["root_terminal_coverage"] is True
    assert complete["status"] == "Converged" and complete["ssa"]["complete"] is True
    assert complete["frontiers"] == [] and complete["root_terminal_coverage"] is None
    expect(status).to_contain_text(NOT_ATTACHED)
    assert COVERED not in status.text_content()
    assert all(label.strip() == NOT_ATTACHED
               for label in re.findall(r"Root terminal[^;·]*", status.text_content()))
    negatives = _invalid_requests(page, context.url, request, output)
    static = _static_control(page, context.url, request, output)
    schema = _schema_control(page, canvas, output, complete_request)
    result = {"default_off": defaults["limits"]["root_terminal_coverage"],
              "attached": {"scope": covered["root_terminal_coverage"]["scope"],
                           "trigger": covered["root_terminal_coverage"]["trigger"],
                           "binding": binding, "inspection": inspect},
              "ordinary_outcome_count": len(covered["outcomes"]),
              "detail_preserved": prior_detail, "opt_in_converged_certificate": None,
              "rejected_requests": negatives, "static_api_companion": static,
              "schema_control": schema, "renderer": webgpu_evidence(page)}
    assert len(posts) == 5, posts
    assert not errors, errors
    (output / "coverage-report.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result


def run(context):
    return root_terminal_coverage_interactions(context)
