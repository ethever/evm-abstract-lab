"""Known recursion browser acceptance cases."""

from playwright.sync_api import expect
import json
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector, inspector_frame_tab, inspector_title_action
from web_test_layout_helpers import layout_page, layout_submission
from web_test_pixels import node_snapshot, text_pixels, workspace_content_bottom
from web_test_wire_bytes import assert_return_word42


def known_recursion_interactions(browser, url, output):
    """Fold real self-CALL bytecode, then select its Runtime summary in the UI."""
    page, canvas, errors = layout_page(browser, url)
    posts = []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    code = "5f5f5f5f5f305af150602a5f5260205ff3"
    entry = {"address": "0x0000000000000000000000000000000000000101",
             "caller": "0x0000000000000000000000000000000000000900",
             "call_value": "0x0", "calldata": "0x"}
    report = layout_submission(page, canvas, code, output, "known-recursion", entry=entry,
                               source_height_offset=-62)
    assert report["schema_version"] == 15 and report["status"] == "Converged"
    assert report["frontiers"] == [] and report["ssa"]["complete"]
    summaries = report["abstract_summaries"]
    assert summaries
    for summary in summaries:
        assert summary["kind"] == "KnownRecursion" and summary["closed"]
        procedure = summary["procedures"][summary["root_procedure"]]
        assert procedure["kind"] == "Runtime" and procedure["code_hash"]
        assert procedure["covered_blocks"] and "Return" in procedure["outcomes"]
        state = next(state for state in report["states"]
                     if state["state"] == summary["source_state"])
        frame = state["entry"]["frames"][-1]
        assert frame["kind"] == "Runtime" and frame["program"] is not None
        block = next(block for block in report["cfg"] if block["id"] == state["state"])
        assert block["instructions"] and block["executed_pcs"] == []
    assert max(len(state["entry"]["frames"]) for state in report["states"]) <= 3
    assert any(edge["kind"] == "SummaryReturn" for edge in report["edges"])
    returns = [outcome for outcome in report["outcomes"] if outcome["kind"] == "Return"]
    assert returns
    for outcome in returns:
        assert_return_word42(report, outcome)
    graph_mode(page, canvas, "States")
    page.keyboard.press("f")
    settle_gesture(page)
    scene = node_snapshot(page, canvas, output, "known-recursion-summary-graph", measure_nodes=True)
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
    hide_inspector(page, canvas)
    expect(status).to_contain_text("Inspector: Frame")
    expect(status).to_contain_text("Recursive bytecode · closed")
    expect(status).not_to_contain_text("Abstract interactions")
    tab = inspector_frame_tab(page, canvas)
    inspector_title_action(page, canvas, "Maximize", tab)
    settle_gesture(page)
    png = canvas.screenshot(path=str(output / "known-recursion-summary-inspector.png"))
    pixels = text_pixels(page, png, (8, 80, 1410, workspace_content_bottom(page, png)))
    assert len(pixels["rows"]) >= 10 and pixels["white_pixels"] > 100, pixels
    assert len(posts) == 1 and not errors
    result = {"status": report["status"], "ssa_complete": report["ssa"]["complete"],
              "summaries": summaries, "source_state": source,
              "inspector": status.text_content(), "painted": pixels,
              "analysis_submissions": len(posts), "renderer": webgpu_evidence(page)}
    (output / "known-recursion-report.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result


def run(context):
    return known_recursion_interactions(context.browser, context.url, context.output)
