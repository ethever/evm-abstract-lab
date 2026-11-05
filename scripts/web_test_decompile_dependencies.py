"""Presentation-only dependency visibility, clipboard and source navigation."""

import re

from playwright.sync_api import expect
from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_helpers import (choose_stage, inspector_control, observed,
                                       open_decompiler, stage, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, rendered_frame, set_control,
                              ui_control, webgpu_evidence)


MEMORY = "5f356080526080515f5500"
LIVE_PHI = "5f35600a576001600d565b60025b6003015f5500"
EMPTY_TRANSFER = "60096007600b5600005b005b5056"
TOGGLE = "Show state dependencies"
ALIASES = re.compile(r"(?m)^\s*[mq]\d+\s*=")
TUPLES = re.compile(r"(?m)^\s*\(v\d+(?:, v\d+)*\) =.*$")


def clean(text):
    assert not ALIASES.search(text), text
    assert "// memory" not in text and "// effect" not in text, text


def painted_copy(page, canvas):
    text = copy_code(page, canvas)
    rows = [code_label(row["label"])[1] for row in observed(page, "decompile-code")]
    # These small fixtures fit the viewport: the button must copy the same
    # metadata policy as the actual painted Labels, not a separate export path.
    assert text.splitlines() == rows, (text, rows)
    return text


def high(page, canvas):
    click_control(page, canvas, "Simplified structured code", role="tab")
    ui_control(page, "Simplified structured code", role="tab", selected=True)


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    results, posts = [], []
    publication_urls = []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and "/api/" in request.url else None)
    page.on("response", lambda response: publication_urls.append(response.url)
            if "/api/decompile/" in response.url and response.url.endswith("/result") else None)

    artifact, _ = submit(context, page, canvas, MEMORY, "dependencies-memory")
    current = stage(artifact, "lifted")
    result_url = publication_urls[-1]
    admitted = len(posts)
    ui_control(page, TOGGLE, role="toggle", selected=False)
    reading = painted_copy(page, canvas)
    clean(reading)
    for operation in ["calldataload(", "mstore(", "mload(", "sstore(", "stop("]:
        assert reading.count(operation) == 1, reading
    canvas.screenshot(path=str(context.output / "dependencies-default-reading.png"))

    # Hidden code annotations remain available in the selected semantic details.
    raw_operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                      for operation in block["operations"]}
    store = next(statement for block in current["ir"]["lifted"]["blocks"]
                 for statement in block["statements"]
                 if raw_operations[statement["operation"]]["opcode"] == 0x52)
    row = next(row for row in observed(page, "decompile-code") if "mstore(" in row["label"])
    click_control(page, canvas, row["label"], role="decompile-code", id=row["id"])
    detail = inspector_control(page, canvas, "Statement h" + store["id"], "decompile-entity-detail")
    assert "memory m" in detail["label"] and "effect q" in detail["label"], detail
    canvas.screenshot(path=str(context.output / "dependencies-inspector-retains-state.png"))

    set_control(page, canvas, TOGGLE, True)
    detailed = painted_copy(page, canvas)
    assert "// memory" in detailed and "// effect" in detailed, detailed
    for operation in ["calldataload(", "mstore(", "mload(", "sstore(", "stop("]:
        assert detailed.count(operation) == 1, detailed
    canvas.screenshot(path=str(context.output / "dependencies-enabled-memory.png"))
    set_control(page, canvas, TOGGLE, False)
    assert painted_copy(page, canvas) == reading

    click_control(page, canvas, "Structured code", role="tab")
    raw = copy_code(page, canvas)
    assert "evm.mstore(" in raw and "effect q" in raw, raw
    click_control(page, canvas, "IR", role="tab")
    assert any("effect q" in row["label"] for row in observed(page, "decompile-ir"))
    high(page, canvas)
    ui_control(page, TOGGLE, role="toggle", selected=False)
    assert painted_copy(page, canvas) == reading
    assert len(posts) == admitted, posts
    retained = page.request.get(result_url)
    assert retained.ok and retained.json()["result"]["Ok"]["artifact"] == artifact
    results.append({"artifact": artifact["id"], "reading_lines": len(reading.splitlines()),
                    "detailed_lines": len(detailed.splitlines())})

    # The preference survives a new publication; derived rows must be rebuilt.
    set_control(page, canvas, TOGGLE, True)
    artifact, _ = submit(context, page, canvas, LIVE_PHI, "dependencies-live-transfer")
    current = stage(artifact, "lifted")
    admitted = len(posts)
    ui_control(page, TOGGLE, role="toggle", selected=True)
    detailed = painted_copy(page, canvas)
    assert ALIASES.search(detailed) and "mstore(" not in detailed, detailed
    tuples = TUPLES.findall(detailed)
    assert len(tuples) == 2 and "sstore(" in detailed, detailed
    set_control(page, canvas, TOGGLE, False)
    reading = painted_copy(page, canvas)
    clean(reading)
    assert TUPLES.findall(reading) == tuples and "sstore(" in reading, reading
    canvas.screenshot(path=str(context.output / "dependencies-live-tuples-preserved.png"))
    transfer = next(transfer for transfer in current["ir"]["lifted"]["transfers"]
                    if transfer["arguments"])
    source_descendant(page, canvas, artifact, current["id"], transfer["id"])
    ui_control(page, "Simplified structured code", role="tab", selected=True)
    selected_row(page, "decompile-code", " = ")
    choose_stage(page, canvas, artifact, "structured")
    click_control(page, canvas, "Structured code", role="tab")
    raw = copy_code(page, canvas)
    assert "memory token on e" in raw and "effect token on e" in raw, raw
    choose_stage(page, canvas, artifact, "lifted")
    high(page, canvas)
    ui_control(page, TOGGLE, role="toggle", selected=False)
    assert copy_code(page, canvas) == reading
    assert len(posts) == admitted, posts
    results.append({"artifact": artifact["id"], "word_transfers": len(tuples),
                    "hidden_alias_rows": len(ALIASES.findall(detailed))})

    # Empty semantic transfers keep identity and provenance, but have no row in
    # reading mode. Following one must land on a real IR occurrence.
    artifact, _ = submit(context, page, canvas, EMPTY_TRANSFER, "dependencies-empty-transfer")
    current = stage(artifact, "lifted")
    admitted = len(posts)
    high(page, canvas)
    ui_control(page, TOGGLE, role="toggle", selected=False)
    reading = painted_copy(page, canvas)
    clean(reading)
    assert "stop(" in reading, reading
    transfer = next(transfer for transfer in current["ir"]["lifted"]["transfers"]
                    if not transfer["arguments"])
    edge = next(edge for edge in current["ir"]["edges"] if edge["id"] == transfer["edge"])
    source_descendant(page, canvas, artifact, current["id"], transfer["id"])
    ui_control(page, "IR", role="tab", selected=True)
    selected_row(page, "decompile-ir", "Live transfer h" + transfer["id"])
    inspector_control(page, canvas, "Live transfer h" + transfer["id"], "decompile-entity-detail")
    set_control(page, canvas, "Entity references", True, role="section")
    inspector_control(page, canvas, f"Reference s{current['id']}/e{edge['memory']} ", "decompile-entity-link")
    expect(page.locator("#analysis-status")).to_contain_text(f"/entity:{edge['memory']}")
    canvas.screenshot(path=str(context.output / "dependencies-hidden-transfer-ir-fallback.png"))

    # A manual tab choice must remain effective after automatic IR fallback.
    high(page, canvas)
    rendered_frame(page)
    ui_control(page, "Simplified structured code", role="tab", selected=True)
    assert copy_code(page, canvas) == reading
    set_control(page, canvas, TOGGLE, True)
    detailed = painted_copy(page, canvas)
    assert ALIASES.search(detailed), detailed
    source_descendant(page, canvas, artifact, current["id"], transfer["id"])
    ui_control(page, "Simplified structured code", role="tab", selected=True)
    selected_row(page, "decompile-code", "memory token on e" + edge["id"])
    selected_row(page, "decompile-code", "effect token on e" + edge["id"])
    canvas.screenshot(path=str(context.output / "dependencies-enabled-transfer-navigation.png"))
    assert len(posts) == admitted, posts

    # Re-accepting an identical artifact preserves the preference, not its caches.
    repeated, _ = submit(context, page, canvas, EMPTY_TRANSFER, "dependencies-repeated-publication")
    assert repeated == artifact
    ui_control(page, TOGGLE, role="toggle", selected=True)
    assert painted_copy(page, canvas) == detailed
    set_control(page, canvas, TOGGLE, False)
    assert painted_copy(page, canvas) == reading
    results.append({"artifact": artifact["id"], "hidden_transfer": transfer["id"],
                    "reading_lines": len(reading.splitlines()),
                    "detailed_lines": len(detailed.splitlines())})
    gpu = webgpu_evidence(page)
    page.close()
    return {"publications": results, "renderer": gpu}
