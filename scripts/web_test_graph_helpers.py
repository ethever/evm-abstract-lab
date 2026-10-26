"""Graph helpers browser acceptance helpers."""

from pathlib import Path
from playwright.sync_api import expect
import re
from web_test_harness import click_control, edit_control, rendered_frame, resize_view, select_report_context, set_control, settle_gesture, ui_control, ui_controls
from web_test_hybrid import process_control_relation, source_program
from web_test_pixels import assert_same_scene, assert_translation, layout_ink, node_snapshot, screenshot_metrics


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
    rendered_frame(page)
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


def compare_tab_control(page, label: str, *, selected=None):
    """Observe the Compare tile preference without selecting it again."""
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
    if selected is not None:
        assert chosen["selected"] is selected, chosen
    return chosen


def compare_tab(page, canvas, label: str):
    """Pick a real tile tab in the responsive Compare source tab strip."""
    chosen = compare_tab_control(page, label)
    click_control(page, canvas, label, role="tab", id=chosen["id"])
    return ui_control(page, label, role="tab", selected=True, id=chosen["id"])


def assert_availability(page, message: str):
    """Require the actual painted scope reason, rather than an empty SSA route."""
    reason = ui_control(page, message, role="availability")
    controls = ui_controls(page)
    assert not any(control["enabled"] and control["role"] in {"graph-canvas", "ssa-row"}
                   for control in controls), (message, controls)
    return reason


def default_entry_cfg(page, canvas, report, output: Path, name: str):
    """Observe the honest initial MAY entry before any namespace/panel action."""
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Route: CFG")
    expect(status).to_contain_text("View: Global model")
    minimum = report["minimum"]
    assert minimum is not None and len(minimum["summaries"]) == 1, "choose an explicit entry relation"
    root = minimum["summaries"][0]["relation"]["root_procedure"]
    expect(status).to_contain_text(re.compile(
        rf"Scoped control: (?:Entry \{{ summary: 0 \}}|Context \{{ summary: 0, procedure: {root}, anchor: [^;]+\}});"))
    expect(status).to_contain_text("; Source level;")
    ui_control(page, "Model overview", role="checkbox", selected=False)
    viewport = page.viewport_size
    if viewport["width"] < 680 or viewport["height"] < 560:
        ui_control(page, "Sources", role="toggle", selected=False)
        ui_control(page, "Inspect", role="toggle", selected=False)
    plot = ui_control(page, "CFG canvas", role="graph-canvas")
    cards = [control for control in ui_controls(page)
             if control["role"] == "graph-node" and control["enabled"]
             and re.match(r"^Block 0x0000(?:–|$)", control["label"])]
    assert len(cards) == 1 and cards[0]["selected"] is True, cards
    card, = cards
    left, top, right, bottom = card["rect"]
    pl, pt, pr, pb = plot["rect"]
    # Observed card hits are clipped to the painter. An entry touching a clip
    # edge cannot establish a complete readable initial body.
    assert pl < left < right < pr and pt < top < bottom < pb, (card, plot)
    assert right-left >= 240 and bottom-top >= 40, (card, plot)
    text = status.text_content()
    zoom = re.search(r"; Global zoom: ([0-9.]+)%", text)
    points = re.search(r"; (\d+) / (\d+) MAY source points;", text)
    assert zoom and float(zoom[1]) >= 90, text
    assert points and 0 < int(points[1]) <= int(points[2]), text
    # These are source-scope counts. They are deliberately never compared to
    # the number of Native states or regional receipts in the retained report.
    page.mouse.move(2, 2)
    rendered_frame(page)
    png = canvas.screenshot(path=str(output / f"{name}.png"))
    ink = layout_ink(page, png, (left+4, top+4, right-4, bottom-4))
    assert ink["preview_pixels"] > 0, (card, ink)
    return {"plot": plot, "entry": card, "zoom_percent": float(zoom[1]), "ink": ink,
            "shown_source_points": int(points[1]), "total_source_points": int(points[2])}


def source_content_snapshot(page, canvas, program, output: Path, name: str, *, ssa=None):
    """Check actual Code/SSA rows and stable pixels in their observed clip."""
    role = "code-row" if ssa is None else "ssa-row"
    if ssa is None:
        first = program["blocks"][0]
        header = f"Block 0x{first['start_pc']:04x}"
        instruction = first["instructions"][0]
        prefix = f"0x{instruction['pc']:04x}  {instruction['name']}"
        ui_control(page, "Copy bytecode", role="action")
    else:
        assert ssa["coverage"] in {"Current", "Stale"}, ssa
        header = (f"{symbol('σ', ssa['state'])}  {ssa['coverage']} · "
                  f"{'complete' if ssa['incoming_complete'] else 'open'} incoming")
        prefix = f"{ssa['instructions'][0]['pc']:04x}  "
        ui_control(page, "Selected state", role="tab", selected=True)
    previous = None
    page.mouse.move(2, 2)
    for _ in range(12):
        rendered_frame(page)
        ui_control(page, header, role=role, selected=True)
        rows = [control for control in ui_controls(page) if control["role"] == role and control["enabled"]]
        assert any(row["label"].startswith(prefix) for row in rows), (prefix, rows)
        region = (min(row["rect"][0] for row in rows), min(row["rect"][1] for row in rows),
                  max(row["rect"][2] for row in rows), max(row["rect"][3] for row in rows))
        png = canvas.screenshot()
        metrics = screenshot_metrics(page, png, region)
        assert metrics["selected_pixels"] > 100, (name, rows, metrics)
        signature = (region, metrics["hash"])
        if signature == previous:
            (output / f"{name}.png").write_bytes(png)
            return {**metrics, "role": role, "rows": rows}
        previous = signature
    raise AssertionError(f"{name}: source rows and painted content did not settle")


def pick_may_point(page, canvas, report, *, summary: int, procedure: int, region: int, location):
    """Pick a visible retained MAY point, with its original regional receipt.

    Source-window and virtual-list reachability remain real UI prerequisites;
    an absent/ambiguous target fails instead of choosing another receipt.
    """
    model = report["minimum"]
    assert model is not None, "MAY selection requires an actual Minimum publication"
    assert type(summary) is int and 0 <= summary < len(model["summaries"]), summary
    relation = model["summaries"][summary]["relation"]
    assert type(procedure) is int and 0 <= procedure < len(relation["procedures"]), procedure
    assert type(region) is int and region >= 0, region
    regional = relation["regional_cfg"]["nodes"]
    assert all(type(node["id"]) is int and node["id"] >= 0 for node in regional), regional
    nodes = [node for node in regional if node["id"] == region]
    assert (len(nodes) == 1 and type(nodes[0]["procedure"]) is int
            and nodes[0]["procedure"] == procedure), (summary, procedure, region, nodes)
    node, = nodes
    key = node["frame"]["process"]
    assert key is not None and node["transfer"] is not None, node
    process = node["transfer"]["process"]
    assert process is not None and process["control"] is not None, node
    control = process["control"]
    certificate = process_control_relation(report, key, control)
    if type(location) is dict:
        assert set(location) == {"Block"} and type(location["Block"]) is int, location
        assert 0 <= location["Block"] < len(certificate["blocks"]), location
        assert certificate["program"] is not None, "uncaptured source has no decoded block title"
        block = source_program(report, certificate["program"])["blocks"][location["Block"]]
        last = block["instructions"][-1]
        end = last["pc"] + max(last["size"] - 1, 0)
        title = f"Block 0x{block['start_pc']:04x}" + (f"–0x{end:04x}" if end != block["start_pc"] else "")
        point_label = f"block index {location['Block']}"
    else:
        assert type(location) is str and location in {"JumpTargets", "End"}, location
        title, point_label = {"JumpTargets": ("Unknown jump", "unknown-jump hub · all legal JUMPDEST targets"),
                              "End": ("End of code", "end of source · no instruction")}[location]
    assert location in [control["entry"], *control["through"], *control["boundaries"]], (location, control)
    select_report_context(page, report, "Baseline", summary=summary, procedure=procedure)
    workspace_route(page, "CFG")
    set_control(page, canvas, "Model overview", False, role="checkbox")
    click_control(page, canvas, title, role="graph-node")
    origins = [item for item in ui_controls(page) if item["role"] == "graph-origin" and item["enabled"]]
    if origins:
        # The producer represents the source entry by its Body origin. Other
        # through-points keep their independent Some(ProcessLocation) receipt.
        kind = "Body" if location == key["location"] else "MAY point"
        prefix = f"r{region} {kind} · "
        matches = [item for item in origins if item["label"].startswith(prefix)]
        assert len(matches) == 1, (prefix, origins)
        chosen, = matches
        click_control(page, canvas, chosen["label"], role="graph-origin", id=chosen["id"])
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("View: Global model")
    expect(status).to_contain_text(re.compile(rf"; Selected regional r{region}(?=;|$)"))
    expect(status).to_contain_text(f"Selected MAY source: {point_label}")
    return ui_control(page, title, role="graph-node", selected=True)


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
    if not any(control["label"] == "cfg_instance_search" and control["role"] == "text"
               and control["enabled"] for control in ui_controls(page)):
        click_control(page, canvas, "Instances", role="action")
    edit_control(page, canvas, "cfg_instance_search", query if query is not None else symbol("σ", state))
    rows = [control for control in ui_controls(page)
            if control["role"] == "graph-instance" and control["enabled"]
            and control["label"].startswith(symbol("σ", state) + " · ")]
    assert len(rows) == 1, (state, rows)
    row, = rows
    click_control(page, canvas, row["label"], role="graph-instance", id=row["id"])
    expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ', state)}; source {symbol('P', program)}")
    settle_gesture(page)
