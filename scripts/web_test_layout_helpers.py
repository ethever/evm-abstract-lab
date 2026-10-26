"""Layout helpers browser acceptance helpers."""

from pathlib import Path
import json
import re
from web_test_graph_helpers import graph_scope
from web_test_harness import complete_submission, observe_webgpu, open_input, posted_bytecode, rendered_frame, settle_gesture
from web_test_inspector_helpers import hide_inspector
from web_test_pixels import screenshot_metrics, text_pixels
from web_test_hybrid import assert_hybrid, oracle_graph


LAYOUT_FIXTURES = Path(__file__).resolve().parent.parent / "crates/evm-abstract-web/tests/fixtures/layout"

LAYOUT_DEFAULT_LIMITS = {
    "max_states":100_000, "max_transfers":10_000_000, "context_depth":128,
    "max_constants":512, "max_work":2_000_000_000, "max_call_depth":1025,
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

def layout_entry_form(page, canvas, entry, source_height_offset=0):
    """Edit actual egui controls; the typed POST below verifies every choice."""
    root_offset = source_height_offset + (10 if "address" in entry else 0)
    if "address" in entry:
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+18, "y": modal[1]+173+source_height_offset})
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+250, "y": modal[1]+198+source_height_offset})
        page.keyboard.insert_text(entry["address"])
        rendered_frame(page)
    modal = layout_modal(page, canvas)
    canvas.click(position={"x": modal[0]+100, "y": modal[1]+209+root_offset})
    settle_gesture(page)
    extra_rows = 0
    if "caller" in entry:
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+175, "y": modal[1]+237+root_offset})
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+250, "y": modal[1]+258+root_offset})
        page.keyboard.insert_text(entry["caller"])
        rendered_frame(page)
        extra_rows += 24
    if "call_value" in entry:
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+175, "y": modal[1]+293+root_offset+extra_rows})
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+250, "y": modal[1]+314+root_offset+extra_rows})
        page.keyboard.insert_text(entry["call_value"])
        rendered_frame(page)
        extra_rows += 24
    modal = layout_modal(page, canvas)
    canvas.click(position={"x": modal[0]+175, "y": modal[1]+337+root_offset+extra_rows})
    settle_gesture(page)
    modal = layout_modal(page, canvas)
    canvas.click(position={"x": modal[0]+250, "y": modal[1]+358+root_offset+extra_rows})
    page.keyboard.insert_text(entry["calldata"])
    rendered_frame(page)
    if "chain_id" in entry:
        # Original 20-point native rows, measured before and after modal scroll.
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+100, "y": modal[1]+439+root_offset})
        settle_gesture(page)
        page.mouse.move(800, 660)
        page.mouse.wheel(0, 240)
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+36, "y": modal[1]+466})
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+250, "y": modal[1]+489})
        page.keyboard.insert_text(entry["chain_id"])
        rendered_frame(page)

def _layout_execution_limits(page, canvas, output, name, limits):
    """Select the legacy runtime policy before opening any environment header."""
    assert limits == {**LAYOUT_DEFAULT_LIMITS, "max_work": limits["max_work"],
                      "root_terminal_coverage": limits["root_terminal_coverage"]}
    geometry = {"captures": {}, "targets": []}
    header_offset = 10

    def save():
        (output / f"{name}-execution-policy-geometry.json").write_text(json.dumps(geometry, indent=2) + "\n")

    def capture(stage):
        page.mouse.move(2, 2)
        settle_gesture(page)
        page.wait_for_timeout(200)
        png = canvas.screenshot(path=str(output / f"{name}-execution-policy-{stage}.png"))
        bounds = layout_modal(page, canvas)
        assert page.viewport_size == {"width": 1440, "height": 1000}
        assert (0 < bounds[0] < bounds[2] < 1440 and 0 < bounds[1] < bounds[3] < 1000
                and 750 <= bounds[2] - bounds[0] <= 780), (stage, bounds)
        region = (bounds[0] + 29, bounds[1] + header_offset, bounds[0] + 95, bounds[3] - 40)
        captured = {"stage": stage, "modal": bounds, "region": region,
                    **text_pixels(page, png, region)}
        geometry["captures"][stage] = captured
        save()
        return captured

    def click(captured, row, x, label):
        top, bottom = captured["rows"][row]
        bounds = captured["modal"]
        target = {"stage": captured["stage"], "label": label, "row": [top, bottom],
                  "x": x, "y": (top + bottom) / 2}
        geometry["targets"].append(target)
        save()
        assert bounds[0] < x < bounds[2] and bounds[1] < top < bottom < bounds[3], target
        canvas.click(position={"x": x, "y": target["y"]})
        settle_gesture(page)

    before = capture("before")
    headers = before["rows"][-6:]
    assert len(headers) == 6 and all(20 <= second[0] - first[0] <= 26
                                   for first, second in zip(headers, headers[1:])), before
    header_offset = headers[0][0] - before["modal"][1] - 2
    header_positions = [row[0] - before["modal"][1] for row in headers]
    click(before, len(before["rows"]) - 3, before["modal"][0] + 70, "Execution budget")
    opened = capture("open")
    # Same painted-row contract as the root-coverage case: three environment
    # headers, Execution, five numeric labels, two checkboxes, note, Precision, RPC.
    assert len(opened["rows"]) == 14, opened
    assert all(abs(row[0] - opened["modal"][1] - expected) <= 2
               for row, expected in zip(opened["rows"][:4], header_positions[:4])), opened
    assert all(20 <= second[0] - first[0] <= 26
               for first, second in zip(opened["rows"][4:8], opened["rows"][5:9])), opened
    assert opened["rows"][-2][0] - opened["rows"][3][1] > 150, opened
    click(opened, 6, opened["modal"][0] + 180, "Logical work")
    page.keyboard.press("Control+A")
    page.keyboard.insert_text(str(limits["max_work"]))
    rendered_frame(page)
    if limits["root_terminal_coverage"] != LAYOUT_DEFAULT_LIMITS["root_terminal_coverage"]:
        click(opened, 10, opened["modal"][0] + 70, "Prepare conservative root terminal coverage")
    applied = capture("applied")
    assert len(applied["rows"]) == 14, applied
    assert all(abs(sum(after) / 2 - sum(before) / 2) <= 2
               for before, after in zip(opened["rows"], applied["rows"])), applied
    click(applied, 3, applied["modal"][0] + 70, "Execution budget")
    closed = capture("closed")
    # Restore the original collapsed layout before the existing entry editor.
    assert len(closed["rows"]) == 6, closed
    assert all(abs(row[0] - closed["modal"][1] - expected) <= 2
               for row, expected in zip(closed["rows"], header_positions)), closed


def layout_submission(page, canvas, code: str, output: Path, name: str, timeout_seconds=60, entry=None,
                      source_height_offset=0, execution_limits=None):
    # Exercise the real multiline editor as a paste. Typing every byte with a
    # delay would add minutes unrelated to analysis or layout for real runtimes.
    open_input(page)
    page.keyboard.press("Control+A")
    page.keyboard.insert_text(code)
    rendered_frame(page)
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
    result = complete_submission(page, admitted, timeout_seconds, namespace="Native", close_directory=True)
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
    match = re.search(r"; (\d+) groups; CFG detail: (identities|SSA preview)", status)
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
