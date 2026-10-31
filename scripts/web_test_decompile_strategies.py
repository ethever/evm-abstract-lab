"""Real irreducible node splitting, textual differences and copied-value origins."""

from playwright.sync_api import expect
from web_test_decompile_helpers import (IRREDUCIBLE, choose_stage, contains_ast,
                                       inspector_control, observed, open_decompiler,
                                       stage, submit)
from web_test_harness import click_control, ui_control, webgpu_evidence


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    dispatched, _ = submit(context, page, canvas, IRREDUCIBLE, "irreducible-dispatcher")
    assert contains_ast(stage(dispatched, "structured")["ir"]["structured"], "dispatcher")
    copied, request = submit(context, page, canvas, IRREDUCIBLE, "irreducible-copies", "Code duplication")
    assert request["strategy"] == "code_duplication"
    assert copied["id"] != dispatched["id"], "strategy did not qualify the artifact namespace"
    event = next(event for event in copied["events"] if event["clones"] and event["redirects"])
    candidate = next(stage for stage in copied["stages"] if stage["id"] == event["to_stage"])
    original = next(stage for stage in copied["stages"] if stage["id"] == event["from_stage"])
    blocks = {block["id"]: block for block in candidate["ir"]["blocks"]}
    relation = next(copy for copy in event["clones"] if copy["copy"]["entity"] in blocks)
    clone = blocks[relation["copy"]["entity"]]
    source_block = next(block for block in original["ir"]["blocks"] if block["id"] == relation["original"]["entity"])
    assert clone["id"] != source_block["id"] and clone["source"] == source_block["source"]
    assert clone["operations"] and all(op["id"] not in {old["id"] for old in source_block["operations"]}
                                        for op in clone["operations"]), "copied operations reused their parents' identities"
    value_ids = {value["id"] for value in candidate["ir"]["values"]}
    assert any(copy["copy"]["entity"] in value_ids for copy in event["clones"]), "copied values are not traceable"
    choose_stage(page, canvas, copied, candidate["kind"])
    click_control(page, canvas, "Changes", role="tab")
    expect(page.locator("#analysis-status")).to_contain_text("Copied:")
    differences = observed(page, "decompile-difference")
    assert any("Copied" in row["label"] for row in differences), differences
    assert any("Redirected" in row["label"] for row in differences), differences
    canvas.screenshot(path=str(context.output / "copied-redirected-differences.png"))
    click_control(page, canvas, "Graphs", role="tab")
    current = ui_control(page, "Current CFG", role="graph-canvas")
    title = f"b{clone['id']} · source 0x{clone['source']['pc']:04x}"
    click_control(page, canvas, title, role="decompile-block", within=current)
    expect(page.locator("#analysis-status")).to_contain_text(f"/stage:{candidate['id']}/entity:{clone['id']}")
    inspector_control(page, canvas, "Original ", "decompile-origin")
    expect(page.locator("#analysis-status")).to_contain_text("Route: Original bytecode")
    inspector_control(page, canvas, f"Source descendant s{candidate['id']}/e{clone['id']} ", "decompile-entity-link")
    expect(page.locator("#analysis-status")).to_contain_text(f"/stage:{candidate['id']}/entity:{clone['id']}")
    canvas.screenshot(path=str(context.output / "copied-original-roundtrip.png"))
    # Constant decisions remove real IR entities; their historical rows remain selectable.
    pruned, _ = submit(context, page, canvas, "6001600a576002600d565b60035b00", "removed-entities")
    assert any(event["deleted"] for event in pruned["events"])
    removed_event = next(event for event in pruned["events"] if event["deleted"])
    before = next(item for item in pruned["stages"] if item["id"] == removed_event["from_stage"])
    after = next(item for item in pruned["stages"] if item["id"] == removed_event["to_stage"])
    choose_stage(page, canvas, pruned, after["kind"])
    choose_stage(page, canvas, pruned, before["kind"], before=True)
    click_control(page, canvas, "Changes", role="tab")
    assert any("Removed" in row["label"] for row in observed(page, "decompile-difference"))
    canvas.screenshot(path=str(context.output / "removed-before-entities.png"))
    result = {"dispatcher": dispatched["id"], "duplication": copied["id"],
              "copy_event": event["id"], "cloned_block": clone["id"],
              "removed_artifact": pruned["id"], "renderer": webgpu_evidence(page)}
    page.close()
    return result
