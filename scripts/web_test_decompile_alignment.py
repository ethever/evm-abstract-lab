"""Measure actual decompiler columns, glyph ink and navigation on real artifacts."""

import base64
from collections import defaultdict
import json
from statistics import median

from playwright.sync_api import expect
from web_test_decompile_helpers import (BRANCH, IRREDUCIBLE, LOOP, choose_stage,
                                       inspector_control, observed, open_decompiler,
                                       stage, submit)
from web_test_harness import (click_control, edit_control, rendered_frame,
                              resize_view, settle_control_geometry, settle_gesture,
                              webgpu_evidence)
from web_test_pixels import canvas_screenshot, save_png_crop


MARKERS = {"New", "Copied", "Changed", "Removed", "Redirected"}


def code_label(label):
    markers = []
    while " · " in label and label.split(" · ", 1)[0] in MARKERS:
        marker, label = label.split(" · ", 1)
        markers.append(marker)
    spaces = len(label) - len(label.lstrip(" "))
    assert spaces % 4 == 0, label
    return markers, label, spaces // 4


def copy_code(page, canvas, expected=None):
    page.context.grant_permissions(["clipboard-read", "clipboard-write"])
    # Clear the clipboard first: an earlier successful copy cannot prove this
    # click preserved the full text after selection, clipping or stage changes.
    page.evaluate("navigator.clipboard.writeText('')")
    click_control(page, canvas, "Copy structured text", role="action")
    rendered_frame(page)
    page.wait_for_function("() => navigator.clipboard.readText().then(text => text.length > 0)")
    text = page.evaluate("navigator.clipboard.readText()")
    assert all(line.split(" · ", 1)[0] not in MARKERS for line in text.splitlines()), text
    if expected is not None:
        assert text == expected, "display interaction changed the complete copied code"
    return text


def ast_anchors(current):
    """Qualify expected depths by server AST ownership and real source blocks."""
    program = current["ir"]["structured"]
    nodes = {node["id"]: node for node in program["nodes"]}
    blocks = {block["id"]: block for block in current["ir"]["blocks"]}
    anchors = []

    def add(entity, prefix, depth):
        anchors.append({"entity": entity, "prefix": prefix, "depth": depth})

    def block_body(owner, block_id, depth):
        block = blocks[block_id]
        source = block["source"]
        suffix = f" · source 0x{source['pc']:04x}" if source else ""
        add(owner, f"// b{block_id}{suffix}", depth)
        for operation in block["operations"]:
            if operation["outputs"] and operation["source"]:
                add(operation["id"], f"v{operation['outputs'][0]} =", depth)

    def visit(identity, depth):
        node = nodes[identity]
        kind = node["kind"]
        if isinstance(kind, str):
            return
        if "sequence" in kind:
            add(identity, f"// sequence ast{identity} ·", depth)
            for child in kind["sequence"]:
                visit(child, depth)
        elif "block" in kind:
            block_body(identity, kind["block"]["block"], depth)
        elif "if" in kind:
            branch = kind["if"]
            add(identity, f"if (v{branch['condition']} != 0) {{", depth)
            visit(branch["then_branch"], depth + 1)
            visit(branch["else_branch"], depth + 1)
        elif "while" in kind:
            loop = kind["while"]
            add(identity, f"'loop{identity}: while (true) {{", depth)
            visit(loop["header"], depth + 1)
            visit(loop["body"], depth + 1)
        elif "dispatcher" in kind:
            add(identity, f"'dispatch{identity}: while (true) {{", depth)
            add(identity, f"switch (dispatch_state_{identity}) {{", depth + 1)
            for case in kind["dispatcher"]["cases"]:
                add(case["block"], f"case b{case['block']}: {{", depth + 2)
                block_body(identity, case["block"], depth + 3)
        elif "indirect" in kind:
            for arm in kind["indirect"]["arms"]:
                visit(arm["body"], depth + 2)

    visit(program["root"], 0)
    return anchors


def brace_pairs(lines):
    stack, pairs = [], []
    for index, line in enumerate(lines):
        _, _, depth = code_label(line)
        body = line.lstrip()
        if body.startswith("}"):
            assert stack, (index, line)
            owner, owner_depth = stack.pop()
            assert depth == owner_depth, (lines[owner], line)
            pairs.append((owner, index))
        if body.endswith("{"):
            stack.append((index, depth))
    assert not stack, stack
    assert pairs, "fixture did not produce real structured braces"
    return pairs


def glyph_ink(page, screenshot, rows):
    """Decode the actual composited PNG once; measure each real glyph response."""
    return page.evaluate("""async ({png, rows}) => {
        const image = new Image(); image.src = `data:image/png;base64,${png}`;
        await image.decode();
        const canvas = document.createElement('canvas');
        canvas.width = image.width; canvas.height = image.height;
        const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
        const scale = image.width / innerWidth;
        return rows.map(row => {
            const [left, top, right, bottom] = row.glyph.rect.map(v => Math.round(v * scale));
            const width = right - left, height = bottom - top;
            const pixels = context.getImageData(left, top, width, height).data;
            let minX = width, minY = height, maxX = -1, maxY = -1, count = 0;
            for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
                const at = (y * width + x) * 4;
                const r = pixels[at], g = pixels[at + 1], b = pixels[at + 2];
                // Both ordinary text and selected accent glyphs are bright;
                // the dark row, pane and scrollbar fills do not satisfy this.
                if (Math.max(r, g, b) < 110) continue;
                minX = Math.min(minX, x); minY = Math.min(minY, y);
                maxX = Math.max(maxX, x); maxY = Math.max(maxY, y); count++;
            }
            return {count, bounds: count ? [(left + minX) / scale, (top + minY) / scale,
                (left + maxX + 1) / scale, (top + maxY + 1) / scale] : null};
        });
    }""", {"png": base64.b64encode(screenshot).decode(), "rows": rows})


def snapshot(context, page, lines, name, reference=None):
    settle_gesture(page)
    settle_control_geometry(page)
    rendered_frame(page)
    controls = observed(page, "decompile-code")
    glyphs = observed(page, "decompile-code-glyphs")
    gutters = observed(page, "decompile-code-gutter")
    screenshot = canvas_screenshot(page, path=str(context.output / (name + ".png")))
    # Keep diagnostic geometry even when occurrence matching fails before the
    # stronger pixel/column assertions have enough evidence to run.
    evidence_path = context.output / (name + "-geometry.json")
    evidence_path.write_text(json.dumps({"controls": controls, "glyphs": glyphs,
                                         "gutters": gutters}, indent=2) + "\n")
    # Root rows have no leading whitespace, so their existing native Label
    # rectangles independently expose marker-induced drift, even on the old UI
    # before the companion glyph observations were introduced.
    roots = [control for control in controls if code_label(control["label"])[2] == 0]
    if len(roots) > 1:
        starts = [control["rect"][0] for control in roots]
        assert max(starts) - min(starts) <= 0.75, (
            "root-level code starts differ across diff markers",
            [(control["label"], control["rect"][0]) for control in roots])
    assert controls and glyphs, "real code and positioned glyph responses must both be observed"
    rows = []
    for control in controls:
        matches = [glyph for glyph in glyphs
                   if (glyph["id"], glyph["label"]) == (control["id"], control["label"])]
        assert len(matches) <= 1, (control, matches)
        if not matches:
            continue  # Horizontally clipped text can leave its clickable row exposed.
        glyph = matches[0]
        gutter = next((item for item in gutters if item["id"] == control["id"]), None)
        markers, source, depth = code_label(control["label"])
        rows.append({"control": control, "glyph": glyph, "markers": markers,
                     "gutter": gutter, "source": source, "body": source.lstrip(), "depth": depth})
    rows.sort(key=lambda row: row["control"]["rect"][1])
    # Logical line height is independent of whether a glyph is a short dash or
    # a tall brace. Only the scroll pane's first/last partial row may be dropped.
    height = max(row["control"]["rect"][3] - row["control"]["rect"][1] for row in rows)
    rows = [row for row in rows
            if row["control"]["rect"][3] - row["control"]["rect"][1] >= height - 0.5]
    assert len(rows) >= 3, rows
    if reference is None:
        # Before horizontal scrolling, real virtual rows form one contiguous
        # window. Repeated braces are qualified by their complete neighboring rows.
        offsets = [offset for offset in range(len(lines) - len(rows) + 1)
                   if all(lines[offset + index] == row["source"] for index, row in enumerate(rows))]
        assert len(offsets) == 1, (name, offsets, [row["source"] for row in rows])
        for index, row in enumerate(rows):
            row["line"] = offsets[0] + index
    else:
        # A scrolled short brace can disappear completely to the left. Its
        # neighbors retain their actual native response IDs, not ordinal aliases.
        previous = {row["control"]["id"]: row for row in reference["rows"]}
        for row in rows:
            before = previous[row["control"]["id"]]
            assert row["source"] == before["source"], (row, before)
            row["line"] = before["line"]
    ink = glyph_ink(page, screenshot, rows)
    for row, pixels in zip(rows, ink, strict=True):
        row["ink"] = pixels
        assert pixels["count"] >= 3 and pixels["bounds"], (name, row)
    region = [min(row["control"]["rect"][0] for row in rows),
              rows[0]["glyph"]["rect"][1],
              max(row["glyph"]["rect"][2] for row in rows),
              rows[-1]["glyph"]["rect"][3]]
    save_png_crop(page, screenshot, region, context.output / (name + "-code.png"))
    evidence = {"name": name, "viewport": page.viewport_size, "rows": rows}
    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n")
    return evidence


def assert_columns(sample):
    rows = sample["rows"]
    by_depth = defaultdict(list)
    for row in rows:
        by_depth[row["depth"]].append(row["control"]["rect"][0])
    for depth, starts in by_depth.items():
        assert max(starts) - min(starts) <= 0.75, (sample["name"], depth, starts)
    columns = sorted((depth, median(starts)) for depth, starts in by_depth.items())
    assert len(columns) >= 2, "alignment window needs multiple real nesting levels"
    # Label bounds include the last glyph's overhang; width / character count
    # therefore is not a font advance. Measure actual nesting-column spacing.
    # Native glyph tests independently require exactly four space advances.
    steps = [(right - left) / (after - before)
             for (before, left), (after, right) in zip(columns, columns[1:])]
    step = median(steps)
    assert max(steps) - min(steps) <= 0.75, (sample["name"], columns, steps)
    advance = step / 4
    assert 5 <= advance <= 14, (sample["name"], advance)
    origins = origins_for_rows(rows, advance)
    assert max(origins) - min(origins) <= 1.25, (sample["name"], origins)
    gutters = [row["gutter"]["rect"] for row in rows if row["gutter"]]
    assert gutters, "the real fixed gutter must be visible before horizontal scrolling"
    assert max(rect[0] for rect in gutters) - min(rect[0] for rect in gutters) <= 0.75
    assert max(rect[2] for rect in gutters) - min(rect[2] for rect in gutters) <= 0.75
    assert min(origins) > max(rect[2] for rect in gutters), (origins, gutters)
    by_first = defaultdict(list)
    for row in rows:
        left = row["control"]["rect"][0]
        assert -1 <= row["ink"]["bounds"][0] - left <= advance + 1, row
        by_first[row["body"][0]].append(row)
    # Same first characters remove the glyph's intrinsic left bearing. This
    # checks painted offsets independently of labels and response rectangles.
    for group in by_first.values():
        origins = [row["ink"]["bounds"][0] - row["depth"] * 4 * advance for row in group]
        assert max(origins) - min(origins) <= 2, (sample["name"], group)
    return {"origin": median(origins_for_rows(rows, advance)), "advance": advance}


def origins_for_rows(rows, advance):
    return [row["control"]["rect"][0] - row["depth"] * 4 * advance for row in rows]


def wheel_code(page, sample, dx=0, dy=0):
    row = sample["rows"][len(sample["rows"]) // 2]
    left, top, right, bottom = row["glyph"]["rect"]
    before = page.evaluate("window.__webgpuEvidence.submissions")
    page.mouse.move((left + right) / 2, (top + bottom) / 2)
    page.wait_for_function("before => window.__webgpuEvidence.submissions > before", arg=before, polling=25)
    page.mouse.wheel(dx, dy)
    settle_gesture(page)
    rendered_frame(page)


def scan_document(context, page, lines, name):
    samples, complete = [], {}
    # Scroll state is stage-specific and can survive a new publication. Move
    # the actual pane to its start before measuring all overlapping windows.
    controls = observed(page, "decompile-code")
    assert controls
    wheel_code(page, {"rows": [{"glyph": control} for control in controls]}, dy=-10000)
    for window in range(8):
        sample = snapshot(context, page, lines, f"{name}-{window}")
        sample["columns"] = assert_columns(sample)
        samples.append(sample)
        complete.update((row["line"], row) for row in sample["rows"])
        if max(complete) == len(lines) - 1:
            break
        height = sample["rows"][-1]["glyph"]["rect"][3] - sample["rows"][0]["glyph"]["rect"][1]
        wheel_code(page, sample, dy=height * 0.6)
    assert set(complete) == set(range(len(lines))), (name, sorted(set(range(len(lines))) - complete.keys()))
    origins = [sample["columns"]["origin"] for sample in samples]
    assert max(origins) - min(origins) <= 1.25, (name, origins)
    for owner, close in brace_pairs(lines):
        opened, closed = complete[owner], complete[close]
        assert abs(opened["control"]["rect"][0] - closed["control"]["rect"][0]) <= 1, (opened, closed)
        assert abs(opened["ink"]["bounds"][0] - closed["ink"]["bounds"][0]) <= samples[0]["columns"]["advance"], (opened, closed)
    return samples, complete


def navigate_entity(page, canvas, artifact, current, entity):
    origin = next(item for item in artifact["mappings"]["origins"]
                  if item["entity"] == {"stage": current["id"], "entity": entity})
    assert origin["source"], origin
    pc = origin["source"][0]["pc"]
    click_control(page, canvas, "Original bytecode", role="tab")
    edit_control(page, canvas, "Original PC", f"0x{pc:x}")
    click_control(page, canvas, "Go to original PC", role="action")
    selected = [row for row in observed(page, "decompile-source") if row["selected"]]
    assert any(f"Source 0x{pc:04x}:" in row["label"] for row in selected), selected
    # Select the destination view before following the operation occurrence.
    # Operation links opened from Source intentionally navigate to IR.
    click_control(page, canvas, "Structured code", role="tab")
    inspector_control(page, canvas, f"Source descendant s{current['id']}/e{entity} ", "decompile-entity-link")
    expect(page.locator("#analysis-status")).to_contain_text(f"{artifact['id']}/stage:{current['id']}/entity:{entity}")


def check_fixture(context, page, canvas, bytecode, name, variant, strategy="Dispatcher"):
    artifact, _ = submit(context, page, canvas, bytecode, name, strategy)
    current = choose_stage(page, canvas, artifact, "structured")
    choose_stage(page, canvas, artifact, "baseline", before=True)
    click_control(page, canvas, "Structured code", role="tab")
    assert any(isinstance(node["kind"], dict) and variant in node["kind"]
               for node in current["ir"]["structured"]["nodes"]), variant
    copied = copy_code(page, canvas)
    (context.output / (name + "-copied.txt")).write_text(copied + "\n")
    lines = copied.splitlines()
    anchors = ast_anchors(current)
    for anchor in anchors:
        assert any(line.startswith("    " * anchor["depth"] + anchor["prefix"])
                   for line in lines), anchor
    samples, rows = scan_document(context, page, lines, name)
    assert any(row["markers"] for row in rows.values())
    assert any(not row["markers"] for row in rows.values())
    assert any(marked["depth"] == plain["depth"] for marked in rows.values() if marked["markers"]
               for plain in rows.values() if not plain["markers"])
    return artifact, current, copied, samples, rows


def horizontal_scroll(context, page, canvas, artifact, current, text, samples):
    # A real dispatcher transition contains a long scoped-control line. Its
    # typed occurrence exposes that same line in the narrow virtual viewport.
    dispatcher = next(node for node in current["ir"]["structured"]["nodes"]
                      if isinstance(node["kind"], dict) and "dispatcher" in node["kind"])
    transition = next(transition for case in dispatcher["kind"]["dispatcher"]["cases"] for transition in case["transitions"])
    resize_view(page, 640, 900)
    navigate_entity(page, canvas, artifact, current, transition["id"])
    narrow = snapshot(context, page, text.splitlines(), "alignment-narrow")
    columns = assert_columns(narrow)
    assert abs(columns["origin"] - samples[0]["columns"]["origin"]) <= 1.25
    needle = f"dispatch_state_{dispatcher['id']} = b{transition['target']};"
    long_row = next(row for row in narrow["rows"] if row["body"].startswith(needle))
    expected_width = len(long_row["body"]) * columns["advance"]
    assert expected_width > long_row["control"]["rect"][2] - long_row["control"]["rect"][0] + 40, long_row
    wheel_code(page, narrow, dx=120)
    scrolled = snapshot(context, page, text.splitlines(), "alignment-narrow-scrolled", reference=narrow)
    shifted = next(row for row in scrolled["rows"] if row["line"] == long_row["line"])
    # The chosen nested transition's prefix must remain exposed; clipped
    # logical left edges cannot stand in for a glyph's true translation.
    assert shifted["control"]["rect"][0] > 10, shifted
    delta = shifted["control"]["rect"][0] - long_row["control"]["rect"][0]
    assert delta < -40, (long_row, shifted)
    assert abs(shifted["ink"]["bounds"][0] - long_row["ink"]["bounds"][0] - delta) <= 2
    copy_code(page, canvas, text)
    choose_stage(page, canvas, artifact, "structured", before=True)
    no_markers = snapshot(context, page, text.splitlines(), "alignment-scrolled-without-markers", reference=narrow)
    assert not any(row["markers"] for row in no_markers["rows"])
    stable = next(row for row in no_markers["rows"] if row["line"] == long_row["line"])
    assert stable["control"]["id"] == shifted["control"]["id"]
    assert abs(stable["control"]["rect"][0] - shifted["control"]["rect"][0]) <= 0.75
    assert abs(stable["ink"]["bounds"][0] - shifted["ink"]["bounds"][0]) <= 1
    wheel_code(page, no_markers, dx=-120)
    restored = snapshot(context, page, text.splitlines(), "alignment-narrow-restored", reference=narrow)
    assert_columns(restored)
    copy_code(page, canvas, text)
    resize_view(page, 1920, 1080)
    return delta


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    branch, current, copied, samples, rows = check_fixture(context, page, canvas, BRANCH, "alignment-branch", "if")
    # Before==current removes every difference marker without changing code.
    # Return to the first window through a real source-qualified operation.
    operation = next(op for block in current["ir"]["blocks"] for op in block["operations"]
                     if op["source"] and op["source"]["pc"] == 1 and op["name"] == "CALLDATALOAD")
    branch_node = next(node for node in current["ir"]["structured"]["nodes"]
                       if isinstance(node["kind"], dict) and "if" in node["kind"])
    close = next(row for row in rows.values() if row["body"] == "}")
    click_control(page, canvas, close["control"]["label"], role="decompile-code", id=close["control"]["id"])
    expect(page.locator("#analysis-status")).to_contain_text(f"/stage:{current['id']}/entity:{branch_node['id']}")
    op_row = next(row for row in observed(page, "decompile-code")
                  if code_label(row["label"])[1].lstrip().startswith(f"v{operation['outputs'][0]} ="))
    click_control(page, canvas, op_row["label"], role="decompile-code", id=op_row["id"])
    expect(page.locator("#analysis-status")).to_contain_text(f"/stage:{current['id']}/entity:{operation['id']}")
    navigate_entity(page, canvas, branch, current, operation["id"])
    selected = snapshot(context, page, copied.splitlines(), "alignment-selected-operation")
    assert any(row["control"]["selected"] and row["body"].startswith(f"v{operation['outputs'][0]} =")
               for row in selected["rows"]), operation
    assert_columns(selected)
    choose_stage(page, canvas, branch, "structured", before=True)
    unmarked = snapshot(context, page, copied.splitlines(), "alignment-without-markers")
    assert not any(row["markers"] for row in unmarked["rows"])
    marked_by_id = {row["control"]["id"]: row for row in selected["rows"]}
    compared = 0
    for row in unmarked["rows"]:
        previous = marked_by_id.get(row["control"]["id"])
        if previous:
            assert row["source"] == previous["source"]
            assert row["control"]["selected"] == previous["control"]["selected"]
            assert abs(row["control"]["rect"][0] - previous["control"]["rect"][0]) <= 0.75
            assert abs(row["ink"]["bounds"][0] - previous["ink"]["bounds"][0]) <= 1
            compared += bool(previous["markers"])
    assert compared >= 3, "before-stage change did not exercise visible marked code"
    assert any(row["control"]["selected"] for row in unmarked["rows"]), "marker comparison cleared entity selection"
    copy_code(page, canvas, copied)
    loop, _, _, loop_samples, _ = check_fixture(context, page, canvas, LOOP, "alignment-loop", "while")
    dispatched, dispatch_stage, dispatch_text, dispatch_samples, _ = check_fixture(
        context, page, canvas, IRREDUCIBLE, "alignment-dispatcher", "dispatcher")
    delta = horizontal_scroll(context, page, canvas, dispatched, dispatch_stage, dispatch_text, dispatch_samples)
    duplicated, duplicate_stage, duplicate_text, duplicate_samples, duplicate_rows = check_fixture(
        context, page, canvas, IRREDUCIBLE, "alignment-copies", "while", "Code duplication")
    assert any("Copied" in row["markers"] for row in duplicate_rows.values()), "real copied operations were not painted"
    assert len(duplicate_samples) > 1, "copy fixture must exercise real virtual windows"
    clone_ids = {copy["copy"]["entity"] for event in duplicated["events"] for copy in event["clones"]}
    cloned = next(op for block in duplicate_stage["ir"]["blocks"] for op in block["operations"]
                  if op["id"] in clone_ids and op["outputs"] and op["source"])
    navigate_entity(page, canvas, duplicated, duplicate_stage, cloned["id"])
    copy_sample = snapshot(context, page, duplicate_text.splitlines(), "alignment-copied-source-roundtrip")
    assert any(row["control"]["selected"] and "Copied" in row["markers"]
               and row["body"].startswith(f"v{cloned['outputs'][0]} =") for row in copy_sample["rows"])
    assert_columns(copy_sample)
    copy_code(page, canvas, duplicate_text)
    result = {"branch": branch["id"], "loop": loop["id"], "dispatcher": dispatched["id"],
              "copies": duplicated["id"], "marker_comparisons": compared,
              "windows": {"branch": len(samples), "loop": len(loop_samples),
                          "dispatcher": len(dispatch_samples), "copies": len(duplicate_samples)},
              "horizontal_translation": delta, "renderer": webgpu_evidence(page)}
    page.close()
    return result
