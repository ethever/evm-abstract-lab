"""Unknown world browser acceptance cases."""

from playwright.sync_api import expect
import json
import re
from web_test_graph_helpers import assert_availability, graph_mode, graph_overview, symbol, workspace_route
from web_test_harness import select_report_context, set_control, settle_gesture, ui_control, ui_controls, webgpu_evidence
from web_test_inspector_helpers import hide_inspector, inspector_frame_tab, inspector_title_action, select_inspector_tab
from web_test_layout_helpers import layout_page, layout_submission
from web_test_pixels import node_snapshot, text_pixels, workspace_content_bottom
from web_test_wire_bytes import assert_return_word42
from web_test_hybrid import oracle_graph, source_instructions


def uncaptured_effect_evidence(page, canvas, full_report, entry, output):
    """Uncaptured code and retained Native SSA are independent capabilities."""
    report = oracle_graph(full_report, "Native")
    nodes = {node["id"]: node for node in report["cfg"]}
    receipts = {block["state"]: block for block in report["ssa"]["blocks"]}
    target_address = f"0x{0x0201:040x}"
    candidates = []
    for snapshot in report["states"]:
        frame = snapshot["entry"]["frames"][-1]
        node = nodes[snapshot["state"]]
        receipt = receipts.get(node["id"])
        if (node["program"] is None and node["process"] is None
                and frame["kind"] == "AbstractEnvironment" and frame["is_static"]
                and frame["code_address"] == target_address
                and frame["storage_address"] == target_address
                and frame["caller"] == {"Concrete": entry["address"]}
                and receipt is not None and receipt["coverage"] in {"Current", "Stale"}):
            candidates.append((node, frame, receipt))
    assert len(candidates) == 1, candidates
    node, frame, receipt = candidates[0]
    assert node["code_address"] == frame["code_address"] and node["storage_address"] == frame["storage_address"]
    assert source_instructions(report, node) == [] and node["executed_pcs"] == receipt["instructions"] == []
    select_report_context(page, full_report, "Native", state=node["id"])
    hide_inspector(page, canvas)
    workspace_route(page, "Code")
    code_reason = assert_availability(page, "Source bytes were not captured for this context")
    canvas.screenshot(path=str(output / "unknown-world-uncaptured-code.png"))

    workspace_route(page, "SSA")
    set_control(page, canvas, "Selected state", True, role="tab")
    header = (f"{symbol('σ', node['id'])}  {receipt['coverage']} · "
              f"{'complete' if receipt['incoming_complete'] else 'open'} incoming")
    ui_control(page, header, role="ssa-row", selected=True)
    incoming = ", ".join(f"{symbol('e', item['edge'])}:{symbol('μ', item['effect'])}"
                         for item in receipt["effect_inputs"])
    open_edges = (" · open [" + ", ".join(symbol("e", edge) for edge in receipt["open_incoming"]) + "]"
                  if receipt["open_incoming"] else "")
    effect = f"{symbol('μ', receipt['effect'])} = effect φ({incoming}){open_edges}"
    ui_control(page, effect, role="ssa-row")
    edges = [edge for edge in report["edges"] if edge["from"] == node["id"]]
    assert {edge["kind"] for edge in edges} == {"SummaryReturn", "SummaryRevert", "SummaryFailure"}, edges
    transitions = {transition["edge"]: transition for transition in report["ssa"]["transitions"]}
    for edge in edges:
        transition = transitions[edge["id"]]
        label = (f"{symbol('e', edge['id'])} → {symbol('σ', edge['to'])} · {edge['kind']} · "
                 f"{symbol('μ', transition['effect_input'])}→{symbol('μ', transition['effect_result'])}")
        if transition["result"] is not None:
            label += f" · %{transition['result']}"
        ui_control(page, label, role="ssa-row")
    rows = [control for control in ui_controls(page) if control["role"] == "ssa-row" and control["enabled"]]
    assert not any(re.match(r"^[0-9a-fA-F]{4,}  ", row["label"]) for row in rows), rows
    assert not any(control["role"] == "availability" and control["enabled"] for control in ui_controls(page))
    region = (min(row["rect"][0] for row in rows), min(row["rect"][1] for row in rows),
              max(row["rect"][2] for row in rows), max(row["rect"][3] for row in rows))
    png = canvas.screenshot(path=str(output / "unknown-world-retained-effect-ssa.png"))
    pixels = text_pixels(page, png, region)
    assert pixels["white_pixels"] + pixels["muted_pixels"] > 50, pixels
    return {"state": node["id"], "frame": frame, "code_unavailable": code_reason,
            "retained_ssa": receipt, "edges": edges, "painted_rows": rows, "painted": pixels}


def unknown_world_interactions(browser, url, output):
    """Run an actual closed unknown relation through the form, SSA and inspector."""
    page, canvas, errors = layout_page(browser, url)
    posts = []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    code = "602a5f555f5f5f5f6102015afa505f545f5260205ff3"
    entry = {"address": "0x0000000000000000000000000000000000000101",
             "caller": "0x0000000000000000000000000000000000000900",
             "call_value": "0x0", "calldata": "0x"}
    report = layout_submission(page, canvas, code, output, "unknown-world", entry=entry,
                               source_height_offset=-62)
    assert report["schema_version"] == 21 and report["status"] == "Converged"
    assert report["frontiers"] == [] and report["ssa"]["complete"]
    summaries = report["abstract_summaries"]
    assert summaries and all(summary["closed"] and summary["procedures"] for summary in summaries)
    assert any(edge["kind"] == "SummaryReturn" for edge in report["edges"])
    unknown = [state for state in report["states"]
               if state["entry"]["frames"][-1]["kind"] == "AbstractEnvironment"]
    assert unknown
    for state in unknown:
        frame = state["entry"]["frames"][-1]
        block = next(block for block in report["cfg"] if block["id"] == state["state"])
        assert frame["program"] is None
        assert source_instructions(report, block) == [] and block["executed_pcs"] == []
    returns = [outcome for outcome in report["outcomes"] if outcome["kind"] == "Return"]
    assert returns
    for outcome in returns:
        assert_return_word42(report, outcome)
    graph_overview(page, canvas, all_programs=True)
    graph_mode(page, canvas, "States")
    page.keyboard.press("f")
    settle_gesture(page)
    scene = node_snapshot(page, canvas, output, "unknown-world-summary-graph", measure_nodes=True)
    source = summaries[0]["source_state"]
    status = page.locator("#analysis-status")
    for record in scene["node_records"]:
        left, top, right, bottom = record["bounds"]
        canvas.click(position={"x": (left + right) / 2, "y": (top + bottom) / 2})
        page.mouse.move(2, 2)
        settle_gesture(page)
        if f"Selected {symbol('σ', source)};" in status.text_content():
            break
    expect(status).to_contain_text(f"Selected {symbol('σ', source)};")
    select_inspector_tab(page, canvas, "Frame")
    expect(status).to_contain_text("Inspector: Frame")
    expect(status).to_contain_text("Abstract interactions")
    expect(status).to_contain_text("closed")
    tab = inspector_frame_tab(page, canvas)
    inspector_title_action(page, canvas, "Maximize", tab)
    tab = inspector_frame_tab(page, canvas)
    page.mouse.move(2, 2)
    settle_gesture(page)
    png = canvas.screenshot(path=str(output / "unknown-world-summary-inspector.png"))
    pixels = text_pixels(page, png, (8, tab[3] + 3, 1410, workspace_content_bottom(page, png)))
    assert len(pixels["rows"]) >= 10 and pixels["white_pixels"] > 100, pixels
    inspection = status.text_content()
    inspector_title_action(page, canvas, "Restore", tab)
    full_report = json.loads((output / "unknown-world-analysis.json").read_text())
    availability = uncaptured_effect_evidence(page, canvas, full_report, entry, output)
    assert len(posts) == 1, "summary selection or inspection reran analysis"
    assert not errors, errors
    result = {"status": report["status"], "ssa_complete": report["ssa"]["complete"],
              "summaries": summaries, "unknown_states": [state["state"] for state in unknown],
              "source_state": source, "inspector": inspection, "availability": availability,
              "painted": pixels, "analysis_submissions": len(posts),
              "renderer": webgpu_evidence(page)}
    (output / "unknown-world-report.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result


def run(context):
    return unknown_world_interactions(context.browser, context.url, context.output)
