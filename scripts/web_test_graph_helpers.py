"""Graph helpers browser acceptance helpers."""

from pathlib import Path
from playwright.sync_api import expect
import re
from web_test_harness import rendered_frame, resize_view, settle_gesture
from web_test_pixels import assert_same_scene, assert_translation, node_snapshot, screenshot_metrics


def frozen_camera(page, canvas, output: Path, prefix: str, inspect_scene: bool):
    page.mouse.move(2, 2)
    rendered_frame(page)
    baseline = node_snapshot(page, canvas, output, f"{prefix}-before", measure_nodes=inspect_scene)
    if inspect_scene:
        assert len(baseline["nodes"]) == 5, baseline["nodes"]
    enlarged = {}
    for width, height in [(1920, 1100), (1440, 1200)]:
        resize_view(page, width, height)
        current = node_snapshot(page, canvas, output, f"{prefix}-{width}x{height}",
                                (4, 123, width - 4, height - 30), inspect_scene)
        # Same canvas origin: even the immediate enlarged frame must preserve
        # the camera. Merely returning to the old size could hide auto-recentering.
        assert_translation(baseline, current, 0, 0, tolerance=1.0)
        if inspect_scene:
            assert_same_scene(baseline, current)
        enlarged[f"{width}x{height}"] = current["bounds"]
    resize_view(page, 500, 380)
    canvas.screenshot(path=str(output / f"{prefix}-clipped.png"))
    resize_view(page, 1440, 1000)
    for key in ["1", "3", "0", "2"]:
        page.keyboard.press(key)
        rendered_frame(page)
    returned = node_snapshot(page, canvas, output, f"{prefix}-returned", measure_nodes=inspect_scene)
    assert_translation(baseline, returned, 0, 0, tolerance=1.0)
    if inspect_scene:
        assert_same_scene(baseline, returned)
    return {"before": baseline["bounds"], "enlarged": enlarged, "returned": returned["bounds"]}

def workspace_panes(page, canvas, output: Path, name: str):
    page.mouse.move(2, 2)
    rendered_frame(page)
    viewport = page.viewport_size
    width, height = viewport["width"], viewport["height"]
    png = canvas.screenshot(path=str(output / f"{name}.png"))
    metrics = screenshot_metrics(page, png, (4, 160, width - 4, height - 30))
    assert len(metrics["dividers"]) == 2, f"expected two visible workspace dividers: {metrics['dividers']}"
    left, right = metrics["dividers"]
    return {"left": left, "right": right, "disassembly_width": left - 6.5,
            "ssa_width": width - right - 6.5, "graph_width": right - left - 5}, png

def pinch_wheel(page, cdp, pointer, delta_y: float) -> None:
    page.mouse.move(*pointer)
    rendered_frame(page)
    # Browser touchpad pinch arrives as Ctrl+wheel without a physical Ctrl
    # keydown. eframe distinguishes this from keyboard Ctrl+wheel.
    cdp.send("Input.dispatchMouseEvent", {
        "type": "mouseWheel", "x": pointer[0], "y": pointer[1],
        "deltaX": 0, "deltaY": delta_y, "modifiers": 2,
    })
    settle_gesture(page)

def graph_mode(page, canvas, mode: str):
    # Product toolbar labels, exercised through real pointer events. Its first
    # content/navigation row stays unchanged; scopes occupy the second row.
    x = {"Blocks": 29, "States": 84, "Local": 133}[mode]
    canvas.click(position={"x": x, "y": 109})
    expect(page.locator("#analysis-status")).to_contain_text(f"CFG view: {mode}")
    settle_gesture(page)

def graph_scope(page, mode: str, nodes: int, shown: int, total: int):
    status = page.locator("#analysis-status")
    for phrase in [f"CFG view: {mode}", f"{nodes} shown nodes", f"{shown}/{total} states"]:
        expect(status).to_contain_text(phrase)
    match = re.search(r"CFG view: (\w+); (\d+) shown nodes; (\d+)/(\d+) states; (\d+) shown edges; (\d+) native edges; (\d+) hidden states; (\d+) boundary edges; ([^;]+)", status.text_content())
    assert match, status.text_content()
    fields = ["nodes", "shown_states", "total_states", "edges", "native_edges", "hidden_states", "boundary_edges"]
    return {"mode": match[1], **dict(zip(fields, map(int, match.group(2,3,4,5,6,7,8)), strict=True)), "scope": match[9]}

def symbol(base: str, index: int):
    return base + str(index).translate(str.maketrans("0123456789", "₀₁₂₃₄₅₆₇₈₉"))

def pick_graph_instance(page, canvas, state: int, query: str | None = None):
    # Real native TextEdit and virtual list coordinates from the egui geometry
    # probe. Search accepts an original state ID, not a display-node ordinal.
    canvas.click(position={"x": 200, "y": 108})
    page.keyboard.press("Control+A")
    page.keyboard.insert_text(query if query is not None else symbol("σ", state))
    settle_gesture(page)
    canvas.click(position={"x": 110, "y": 131})
    expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ', state)}; source {symbol('P', 0)}")
    settle_gesture(page)
