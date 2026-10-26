"""Unknown world browser acceptance cases."""

from playwright.sync_api import expect
import json
from web_test_graph_helpers import graph_mode, graph_overview, symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_inspector_helpers import inspector_frame_tab, inspector_title_action, select_inspector_tab
from web_test_layout_helpers import layout_page, layout_submission
from web_test_pixels import node_snapshot, text_pixels, workspace_content_bottom
from web_test_wire_bytes import assert_return_word42
from web_test_hybrid import source_instructions


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
    assert len(posts) == 1, "summary selection or inspection reran analysis"
    assert not errors, errors
    result = {"status": report["status"], "ssa_complete": report["ssa"]["complete"],
              "summaries": summaries, "unknown_states": [state["state"] for state in unknown],
              "source_state": source, "inspector": status.text_content(),
              "painted": pixels, "analysis_submissions": len(posts),
              "renderer": webgpu_evidence(page)}
    (output / "unknown-world-report.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result


def run(context):
    return unknown_world_interactions(context.browser, context.url, context.output)
