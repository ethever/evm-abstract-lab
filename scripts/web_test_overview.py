"""Overview browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import json
from web_test_graph_helpers import graph_mode, graph_scope, pick_graph_instance
from web_test_harness import complete_submission, observe_webgpu, rendered_frame, settle_gesture, submit_bytecode, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_pixels import node_snapshot, screenshot_metrics


def block_overview_interactions(browser, url: str, output: Path):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    initial_reply = complete_submission(page, initial.value)
    report = initial_reply.json()["result"]["Ok"]
    assert len(report["cfg"]) == len(report["ssa"]["blocks"]) == 5
    identities = {state["id"]: (state["program"], state["basic_block"], state["start_pc"]) for state in report["cfg"]}
    assert len(set(identities.values())) == 4
    aggregate_edges = {(identities[edge["from"]], identities[edge["to"]], edge["kind"]) for edge in report["edges"]}
    assert len(aggregate_edges) == 4
    submitted = []
    page.on("request", lambda request: submitted.append(request.post_data_json)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    canvas = page.locator("#evm-canvas")
    hide_inspector(page, canvas)
    page.keyboard.press("2")
    settle_gesture(page)
    blocks = graph_scope(page, "Blocks", 4, 5, 5)
    assert blocks["edges"] == 4 and blocks["native_edges"] == len(report["edges"])
    block_pixels = node_snapshot(page, canvas, output, "overview-blocks", measure_nodes=True)
    assert len(block_pixels["nodes"]) == 4, block_pixels
    graph_mode(page, canvas, "States")
    states = graph_scope(page, "States", 5, 5, 5)
    assert states["edges"] == states["native_edges"] == len(report["edges"])
    state_pixels = node_snapshot(page, canvas, output, "overview-states", measure_nodes=True)
    assert len(state_pixels["nodes"]) == 5, state_pixels
    graph_mode(page, canvas, "Local")
    local = graph_scope(page, "Local", 5, 5, 5)
    assert local["scope"] == "Cross-program neighborhood"
    canvas.screenshot(path=str(output / "overview-local-branch.png"))
    graph_mode(page, canvas, "Blocks")
    assert not submitted, "changing graph scope unexpectedly reran analysis"

    loop_reply = submit_bytecode(page, canvas, "5b600056", output / "overview-loop-input.png")
    assert loop_reply.status == 200 and "Ok" in loop_reply.json()["result"], loop_reply.json()
    loop = loop_reply.json()["result"]["Ok"]
    assert loop["metadata"]["limits"]["context_depth"] == 128
    assert loop["status"] == "Converged" and loop["ssa"]["complete"]
    assert len(loop["cfg"]) == len(loop["ssa"]["blocks"]) == 129
    assert {state["id"] for state in loop["cfg"]} == set(range(129))
    assert len(loop["edges"]) == 129
    settle_gesture(page)
    grouped = graph_scope(page, "Blocks", 1, 129, 129)
    assert grouped["edges"] == 1 and grouped["native_edges"] == 129
    one_card = node_snapshot(page, canvas, output, "overview-loop-129-instances", measure_nodes=True)
    assert len(one_card["nodes"]) == 1, one_card
    left, top, right, bottom = one_card["bounds"]
    canvas.click(position={"x": (left + right) / 2, "y": (top + bottom) / 2})
    settle_gesture(page)
    canvas.screenshot(path=str(output / "overview-loop-instances.png"))
    search_forms = [(127, "127"), (128, "σ₁₂₈"), (127, "sigma_127"), (128, "S128")]
    for index, (state, query) in enumerate(search_forms):
        if index:
            canvas.click(position={"x": 300, "y": 109})  # Instances toolbar
            settle_gesture(page)
        pick_graph_instance(page, canvas, state, query)
    canvas.screenshot(path=str(output / "overview-loop-symbol-search.png"))
    graph_scope(page, "Blocks", 1, 129, 129)
    # Expose both the native inspector and the full SSA table for the selected
    # instance. A virtualized table must finish its long focus animation.
    hide_inspector(page, canvas)  # toggle the initially hidden inspector on
    page.keyboard.press("3")
    page.wait_for_timeout(800)
    rendered_frame(page)
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Selected σ₁₂₈; source P₀")
    expect(status).to_contain_text("Inspector: Frame")
    expect(status).to_contain_text("Frame f₀")
    png = canvas.screenshot(path=str(output / "overview-loop-native-ssa.png"))
    ssa_pixels = screenshot_metrics(page, png, (4, 75, 1436, 760))
    assert ssa_pixels["selected_pixels"] > 100, "the selected σ₁₂₈ SSA block was not scrolled into view"
    selected = next(state for state in loop["states"] if state["state"] == 128)
    assert len(selected["entry"]["frames"][0]["context"]) == 128
    page.keyboard.press("2")
    settle_gesture(page)
    canvas.click(position={"x": 300, "y": 109})  # Instances toolbar
    settle_gesture(page)
    pick_graph_instance(page, canvas, 128)
    graph_mode(page, canvas, "Local")
    neighborhood = graph_scope(page, "Local", 3, 3, 129)
    assert neighborhood["hidden_states"] == 126 and neighborhood["boundary_edges"] == 1
    assert neighborhood["edges"] == 3
    expect(status).to_contain_text("Converged")
    canvas.screenshot(path=str(output / "overview-loop-local-boundary.png"))
    assert len(submitted) == 1, "instance selection or local display changed the backend analysis"
    assert not errors, errors
    result = {"blocks": blocks, "states": states, "local": local, "loop": grouped,
              "loop_local": neighborhood, "selected_native_state": 128,
              "block_pixels": block_pixels, "state_pixels": state_pixels,
              "loop_pixels": one_card, "selected_ssa_pixels": ssa_pixels["selected_pixels"],
              "renderer": webgpu_evidence(page)}
    (output / "overview-loop-analysis.json").write_text(json.dumps(loop, indent=2) + "\n")
    page.close()
    return result


def run(context):
    return block_overview_interactions(context.browser, context.url, context.output)
