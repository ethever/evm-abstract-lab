"""Graph helpers browser acceptance helpers."""

from pathlib import Path
from playwright.sync_api import expect
import re
from web_test_harness import click_control, edit_control, rendered_frame, resize_view, set_control, settle_gesture, ui_control, ui_controls
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
    assert mode in {"Blocks", "States", "Local"}, mode
    click_control(page, canvas, mode, role="tab")
    ui_control(page, mode, role="tab", selected=True)
    expect(page.locator("#analysis-status")).to_contain_text(f"CFG view: {mode}")
    settle_gesture(page)


def workspace_route(page, route: str):
    """Use the declared product shortcuts and verify the actual workspace route."""
    key = {"Compare": "0", "Code": "1", "CFG": "2", "SSA": "3"}[route]
    page.keyboard.press(key)
    settle_gesture(page)
    expect(page.locator("#analysis-status")).to_contain_text(
        re.compile(rf"(?:^|; )Route: {re.escape(route)}(?=;|$)"))


def graph_overview(page, canvas, *, all_programs=True, fit=False):
    """Explicit Native all-context display for the original full-scene oracles.

    The caller selects an actual qualified Native context first. This changes
    presentation only; it neither chooses a namespace nor invents a state.
    """
    workspace_route(page, "CFG")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("View: Detailed paths")
    graph_mode(page, canvas, "Blocks")
    overview = set_control(page, canvas, "Overview", True, role="checkbox")
    if all_programs:
        graph_program_scope(page, canvas)
    if fit:
        click_control(page, canvas, "Fit graph", role="action")
    return overview


def graph_program_scope(page, canvas, program=None):
    """Select All programs or an actual retained ProgramInfo source alias."""
    if program is None:
        caption, scope = "All programs", "All programs"
    else:
        address = program["code_address"]
        caption = (address[:8] + "…" + address[-6:]
                   if address is not None and address.isascii() and len(address) > 18
                   else address or f"{program['kind']} code")
        scope = symbol("P", program["id"])
    click_control(page, canvas, "cfg_program_scope", role="combobox")
    chosen = click_control(page, canvas, caption, role="tab")
    expect(page.locator("#analysis-status")).to_contain_text(
        re.compile(rf"; \d+ boundary edges; {re.escape(scope)}(?=;|$)"))
    return chosen


def graph_content(page, canvas, content: str):
    """Select graph instructions independently of the workspace's SSA route."""
    assert content in {"Disasm", "SSA"}, content
    group = ui_control(page, "graph_content", role="group")
    click_control(page, canvas, content, role="tab", within=group)
    ui_control(page, content, role="tab", selected=True, within=group)
    expect(page.locator("#analysis-status")).to_contain_text(f"CFG nodes: {content}")


def compare_tab(page, canvas, label: str):
    """Pick a real tile tab in the responsive Compare source tab strip."""
    assert label in {"Disassembly", "Control flow", "SSA"}, label
    expect(page.locator("#analysis-status")).to_contain_text("Route: Compare")
    reference = ui_control(page, "Disassembly", role="tab")
    controls = ui_controls(page)
    graph_groups = [control["rect"] for control in controls
                    if control["role"] == "group" and control["label"] == "graph_content"]
    candidates = [control for control in controls
                  if control["label"] == label and control["role"] == "tab" and control["enabled"]
                  and reference["rect"][1] - 0.5 <= control["rect"][1]
                  and control["rect"][3] <= reference["rect"][3] + 0.5
                  and not any(left <= control["rect"][0] < control["rect"][2] <= right
                              and top <= control["rect"][1] < control["rect"][3] <= bottom
                              for left, top, right, bottom in graph_groups)]
    assert len(candidates) == 1, (label, reference, candidates)
    chosen, = candidates
    click_control(page, canvas, label, role="tab", id=chosen["id"])
    return ui_control(page, label, role="tab", selected=True, id=chosen["id"])


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

def pick_graph_instance(page, canvas, state: int, query: str | None = None, *, program: int):
    # The caller opens the real State instances chooser for a selected card.
    # Preserve its accepted sigma aliases and original namespace-local state ID.
    assert type(state) is int and state >= 0, state
    expect(page.locator("#analysis-status")).to_contain_text("View: Detailed paths")
    edit_control(page, canvas, "cfg_instance_search", query if query is not None else symbol("σ", state))
    rows = [control for control in ui_controls(page)
            if control["role"] == "instance" and control["enabled"]
            and control["label"].startswith(symbol("σ", state) + " · ")]
    assert len(rows) == 1, (state, rows)
    row, = rows
    click_control(page, canvas, row["label"], role="instance", id=row["id"])
    expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ', state)}; source {symbol('P', program)}")
    settle_gesture(page)
