"""Overview browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import json
from web_test_graph_helpers import graph_mode, graph_scope, pick_graph_instance, symbol
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
    full_report = initial_reply.json()["result"]["Ok"]
    (output / "overview-initial-full-analysis.json").write_text(json.dumps(full_report, indent=2) + "\n")
    assert full_report["hybrid"]["coverage"]["baseline_current"]
    report = full_report["hybrid"]["detail"]
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

    # Count down from 129, keeping one additional zero below the counter on
    # every trip. Distinct stack heights retain 129 instances at PC 2 even
    # when repeated jump sources are canonicalized to a single context entry.
    loop_reply = submit_bytecode(page, canvas, "60815b600190035f908060025700", output / "overview-loop-input.png")
    assert loop_reply.status == 200 and "Ok" in loop_reply.json()["result"], loop_reply.json()
    loop = loop_reply.json()["result"]["Ok"]
    (output / "overview-loop-analysis.json").write_text(json.dumps(loop, indent=2) + "\n")
    loop = loop["hybrid"]["detail"]
    assert loop["metadata"]["limits"]["context_depth"] == 128
    assert loop["status"] == "Converged" and loop["ssa"]["complete"]
    assert len(loop["cfg"]) == len(loop["ssa"]["blocks"]) == 131
    assert {state["id"] for state in loop["cfg"]} == set(range(131))
    instances = [state for state in loop["cfg"] if state["start_pc"] == 2]
    assert {state["id"] for state in instances} == set(range(1, 130))
    assert {len(state["entry_stack"]) for state in instances} == set(range(1, 130))
    expected_edges = {(0, 1, "Fallthrough"), (129, 130, "BranchFalse")}
    expected_edges.update((state, state + 1, "BranchTrue") for state in range(1, 129))
    assert len(loop["edges"]) == 130
    assert {(edge["from"], edge["to"], edge["kind"]) for edge in loop["edges"]} == expected_edges
    settle_gesture(page)
    grouped = graph_scope(page, "Blocks", 3, 131, 131)
    assert grouped["edges"] == 3 and grouped["native_edges"] == 130
    graph_mode(page, canvas, "States")
    native = graph_scope(page, "States", 131, 131, 131)
    assert native["edges"] == native["native_edges"] == 130
    canvas.screenshot(path=str(output / "overview-loop-native-states.png"))
    graph_mode(page, canvas, "Blocks")
    cards = node_snapshot(page, canvas, output, "overview-loop-129-instances", measure_nodes=True)
    assert len(cards["nodes"]) == 3, cards
    status = page.locator("#analysis-status")
    for record in cards["node_records"]:
        left, top, right, bottom = record["bounds"]
        canvas.click(position={"x": (left + right) / 2, "y": (top + bottom) / 2})
        settle_gesture(page)
        if f"Selected {symbol('σ', 1)};" in status.text_content():
            break
    expect(status).to_contain_text(f"Selected {symbol('σ', 1)}; source P₀")
    canvas.screenshot(path=str(output / "overview-loop-instances.png"))
    search_forms = [(128, "128"), (129, "σ₁₂₉"), (128, "σ128"), (129, "σ_129"),
                    (128, "sigma_128"), (129, "\\sigma_{129}"), (128, "S128"), (129, "s129"),
                    (129, "  σ₁₂₉  ")]
    for index, (state, query) in enumerate(search_forms):
        if index:
            canvas.click(position={"x": 300, "y": 133})  # Instances toolbar
            settle_gesture(page)
        pick_graph_instance(page, canvas, state, query)
    canvas.screenshot(path=str(output / "overview-loop-symbol-search.png"))
    graph_scope(page, "Blocks", 3, 131, 131)
    # Expose both the native inspector and the full SSA table for the selected
    # instance. A virtualized table must finish its long focus animation.
    hide_inspector(page, canvas)  # toggle the initially hidden inspector on
    page.keyboard.press("3")
    page.wait_for_timeout(800)
    rendered_frame(page)
    expect(status).to_contain_text("Selected σ₁₂₉; source P₀")
    expect(status).to_contain_text("Inspector: Frame")
    expect(status).to_contain_text("Frame f₀")
    png = canvas.screenshot(path=str(output / "overview-loop-native-ssa.png"))
    ssa_pixels = screenshot_metrics(page, png, (4, 75, 1436, 760))
    assert ssa_pixels["selected_pixels"] > 100, "the selected σ₁₂₉ SSA block was not scrolled into view"
    selected = next(state for state in loop["states"] if state["state"] == 129)
    frame = selected["entry"]["frames"][0]
    assert frame["context"] == next(state for state in instances if state["id"] == 129)["context"] == [2]
    assert len(frame["stack"]) == 129 and frame["stack"][-1]["constants"] == ["0x1"]
    assert all(value["constants"] == ["0x0"] for value in frame["stack"][:-1])
    page.keyboard.press("2")
    settle_gesture(page)
    canvas.click(position={"x": 300, "y": 133})  # Instances toolbar
    settle_gesture(page)
    pick_graph_instance(page, canvas, 129)
    graph_mode(page, canvas, "Local")
    neighborhood = graph_scope(page, "Local", 4, 4, 131)
    assert neighborhood["hidden_states"] == 127 and neighborhood["boundary_edges"] == 1
    assert neighborhood["edges"] == 3
    expect(status).to_contain_text("Converged")
    canvas.screenshot(path=str(output / "overview-loop-local-boundary.png"))
    assert len(submitted) == 1, "instance selection or local display changed the backend analysis"
    assert not errors, errors
    result = {"blocks": blocks, "states": states, "local": local, "loop": grouped,
              "loop_states": native, "loop_local": neighborhood, "selected_native_state": 129,
              "block_pixels": block_pixels, "state_pixels": state_pixels,
              "loop_pixels": cards, "selected_ssa_pixels": ssa_pixels["selected_pixels"],
              "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context):
    return block_overview_interactions(context.browser, context.url, context.output)
