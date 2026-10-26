"""Layout helpers browser acceptance helpers."""

from pathlib import Path
import json
import re
from web_test_graph_helpers import graph_scope
from web_test_harness import click_control, complete_submission, edit_control, observe_webgpu, open_input, posted_bytecode, rendered_frame, set_control, settle_gesture, ui_control
from web_test_inspector_helpers import hide_inspector
from web_test_pixels import screenshot_metrics
from web_test_hybrid import assert_hybrid, oracle_graph


LAYOUT_FIXTURES = Path(__file__).resolve().parent.parent / "crates/evm-abstract-web/tests/fixtures/layout"

LAYOUT_DEFAULT_LIMITS = {
    "max_states":100_000, "max_transfers":10_000_000, "context_depth":128,
    "max_constants":512, "max_work":2_000_000_000, "max_call_depth":1025,
    "max_detail_regions":1,
    "max_memory_bytes":64*1024*1024, "use_summaries":True, "root_terminal_coverage":True,
    "numeric_model":"Product",
    "reduction_rounds":16, "max_facts":4096, "relations_enabled":True,
    "max_constraints":2048, "max_expression_nodes":16_384, "max_expression_depth":256,
    "rpc_max_accounts":4096,
    "rpc_max_requests":1_000_000, "rpc_max_response_bytes":64*1024*1024,
    "rpc_timeout_ms":120_000,
}

def layout_fixture(name: str):
    code = (LAYOUT_FIXTURES / f"{name}.hex").read_text().strip()
    metadata = json.loads((LAYOUT_FIXTURES / f"{name}.json").read_text())
    assert len(bytes.fromhex(code)) == metadata["byte_length"]
    return code, metadata

def layout_modal(page, canvas):
    viewport = page.viewport_size
    boxes = screenshot_metrics(page, canvas.screenshot(),
                               (0, 0, viewport["width"], viewport["height"]), True)["nodes"]
    return max(boxes, key=lambda rect: (rect[2]-rect[0])*(rect[3]-rect[1]))


def layout_detail_regions(page, canvas, output, name, requested):
    """Edit the actual Precision field; its caller checks the complete applied policy."""
    assert type(requested) is int and 0 <= requested <= (1 << 64) - 1
    section = set_control(page, canvas, "Precision", True, role="section")
    field = edit_control(page, canvas, "Detailed code sources", requested)
    canvas.screenshot(path=str(output / f"{name}-precision-edited.png"))
    set_control(page, canvas, "Precision", False, role="section")
    (output / f"{name}-precision-geometry.json").write_text(
        json.dumps(dict(section=section, field=field, requested=requested), indent=2) + "\n")


def layout_entry_form(page, canvas, entry, source_height_offset=0):
    """Choose each declared input through its qualified, real native form row."""
    if "address" in entry:
        set_control(page, canvas, "Root address", True, role="checkbox")
        edit_control(page, canvas, "bytecode_address", entry["address"])
    set_control(page, canvas, "Transaction environment", True, role="section")
    for field, row, choice in [("caller", "environment_caller", "Address"),
                               ("call_value", "environment_value", "Exact"),
                               ("calldata", "environment_calldata", "Exact")]:
        if field not in entry:
            continue
        group = ui_control(page, row, role="group")
        click_control(page, canvas, choice, role="tab", within=group)
        edit_control(page, canvas, row, entry[field])
    set_control(page, canvas, "Transaction environment", False, role="section")
    if "chain_id" in entry:
        set_control(page, canvas, "Block environment overrides", True, role="section")
        set_control(page, canvas, "Execution chain ID", True, role="checkbox")
        edit_control(page, canvas, "environment_chain_id", entry["chain_id"])
        set_control(page, canvas, "Block environment overrides", False, role="section")


def _layout_execution_limits(page, canvas, output, name, limits):
    """Retain the original explicit runtime policy without changing any defaults."""
    assert limits == {**LAYOUT_DEFAULT_LIMITS, "max_work": limits["max_work"],
                      "root_terminal_coverage": limits["root_terminal_coverage"]}
    section = set_control(page, canvas, "Execution budget", True, role="section")
    field = edit_control(page, canvas, "Logical work", limits["max_work"])
    coverage = set_control(page, canvas, "Prepare conservative root terminal coverage",
                           limits["root_terminal_coverage"], role="checkbox")
    canvas.screenshot(path=str(output / f"{name}-execution-policy-applied.png"))
    set_control(page, canvas, "Execution budget", False, role="section")
    (output / f"{name}-execution-policy-geometry.json").write_text(
        json.dumps(dict(section=section, field=field, coverage=coverage), indent=2) + "\n")


def layout_submission(page, canvas, code: str, output: Path, name: str, timeout_seconds=60, entry=None,
                      source_height_offset=0, execution_limits=None):
    # Exercise the real multiline editor as a paste. Typing every byte with a
    # delay would add minutes unrelated to analysis or layout for real runtimes.
    open_input(page, mode="Bytecode")
    edit_control(page, canvas, "Runtime bytecode", code)
    expected_limits = LAYOUT_DEFAULT_LIMITS if execution_limits is None else execution_limits
    if execution_limits is not None:
        _layout_execution_limits(page, canvas, output, name, execution_limits)
    if entry is not None:
        layout_entry_form(page, canvas, entry, source_height_offset)
    canvas.screenshot(path=str(output / f"{name}-input.png"))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as response:
        page.keyboard.press("Control+Enter")
    admitted = response.value
    assert posted_bytecode(admitted) == code
    request = admitted.request.post_data_json
    (output / f"{name}-request.json").write_text(admitted.request.post_data + "\n")
    assert request["limits"] == expected_limits, request["limits"]
    if entry is not None:
        assert request["environment"]["calldata"] == {"Exact": entry["calldata"]}, request
        if "caller" in entry:
            assert request["environment"]["caller"] == {"Concrete": entry["caller"]}, request
        if "call_value" in entry:
            assert request["environment"]["call_value"] == {"Concrete": entry["call_value"]}, request
        if "address" in entry:
            assert request["input"]["Bytecode"]["address"] == entry["address"], request
        if "chain_id" in entry:
            assert request["environment"]["chain_id"] == entry["chain_id"], request
    result = complete_submission(page, admitted, timeout_seconds, namespace="Native", close_directory=True,
                                 expected_detail_regions=expected_limits["max_detail_regions"])
    assert result.status == 200 and "Ok" in result.json()["result"], result.json()
    full_report = result.report
    (output / f"{name}-task-reply.json").write_text(json.dumps(result.json(), indent=2) + "\n")
    assert_hybrid(full_report)
    # These layout/SSA scenarios assert Native execution, including source and
    # input pools. The final publication can preserve the initial Global view.
    report = oracle_graph(full_report, "Native")
    assert report is not None
    root = next(program for program in report["programs"] if program["id"] == report["metadata"]["root_program"])
    assert root["bytecode"].removeprefix("0x") == code
    assert report["metadata"]["limits"] == expected_limits
    if entry is not None:
        environment = report["metadata"]["environment"]
        calldata = report["byte_arrays"][environment["calldata"]]
        expected = bytes.fromhex(entry["calldata"].removeprefix("0x"))
        assert [int(value,16) for value in calldata["length"]["constants"]] == [len(expected)]
        cells = {cell["offset"]:calldata["values"][cell["value"]] for cell in calldata["cells"]}
        for offset, byte in enumerate(expected):
            value = cells.get(offset, calldata["default"])
            assert [int(word,16) for word in value["constants"]] == [byte], (offset,value,byte)
        if "address" in entry:
            assert environment["to"]["Concrete"].lower() == entry["address"].lower()
        if "caller" in entry:
            assert environment["caller"]["Concrete"].lower() == entry["caller"].lower()
        if "call_value" in entry:
            assert [int(value,16) for value in environment["call_value"]["constants"]] == [int(entry["call_value"],16)]
        if "chain_id" in entry:
            assert [int(value,16) for value in environment["chain_id"]["constants"]] == [int(entry["chain_id"],16)]
    (output / f"{name}-analysis.json").write_text(json.dumps(full_report, indent=2) + "\n")
    return report

def layout_counts(page, report, expected_groups: int | None = None, mode="Blocks"):
    identities = {
        node["id"]: ((node["program"], node["basic_block"], node["start_pc"])
                     if mode == "Blocks" and node["program"] is not None and node["start_pc"] is not None
                     else ("state", node["id"]))
        for node in report["cfg"]
    }
    edge_count = len({(identities[edge["from"]], identities[edge["to"]], edge["kind"])
                      for edge in report["edges"]}) if mode == "Blocks" else len(report["edges"])
    scene = graph_scope(page, mode, len(set(identities.values())), len(report["cfg"]), len(report["cfg"]))
    assert scene["edges"] == edge_count and scene["native_edges"] == len(report["edges"]), scene
    status = page.locator("#analysis-status").text_content()
    assert "Layout unavailable:" not in status, status
    match = re.search(r"; (\d+) groups; CFG detail: (identities|SSA preview|overview shapes)", status)
    assert match, status
    scene.update(groups=int(match[1]), detail=match[2])
    if expected_groups is not None:
        assert scene["groups"] == expected_groups, scene
    return scene

def layout_page(browser, url: str):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    complete_submission(page, initial.value, namespace="Native", close_directory=True)
    canvas = page.locator("#evm-canvas")
    hide_inspector(page, canvas)
    page.keyboard.press("2")
    settle_gesture(page)
    return page, canvas, errors
