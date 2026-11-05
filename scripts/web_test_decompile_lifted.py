"""Typed high-level publication, code, raw witness and source navigation."""

import re

from playwright.sync_api import expect
from web_test_decompile_alignment import copy_code
from web_test_decompile_helpers import (choose_stage, inspector_control, observed,
                                       open_decompiler, stage, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import click_control, rendered_frame, ui_control, webgpu_evidence


TRIPLE_ZERO = "6020355f3510151515600f575f5ffd5b00"
NUMERIC_TRUTH = "5f3515155f5500"
RETURN_CLEANUP = "60096007600b5600005b005b5056"


def projection(artifact):
    before, current = stage(artifact, "structured"), stage(artifact, "lifted")
    assert artifact["current_stage"] == current["id"]
    assert all(item["validation"]["status"] == "passed" for item in artifact["stages"])
    raw = lambda ir: {key: value for key, value in ir.items() if key != "lifted"}
    assert raw(before["ir"]) == raw(current["ir"]), "high-level publication changed its raw witness"
    program = current["ir"]["lifted"]
    assert program and all(name in program for name in ["expressions", "blocks", "transfers", "controls"])
    event = next(event for event in artifact["events"] if event["to_stage"] == current["id"])
    assert event["from_stage"] == before["id"] and event["rule"] == "lift_semantic_expressions"
    return before, current, program, event


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    artifacts = []
    artifact, _ = submit(context, page, canvas, TRIPLE_ZERO, "lifted-unsigned-predicate")
    assert artifact["completion"] == "complete", artifact["diagnostics"]
    before, current, program, event = projection(artifact)
    expressions = {expression["id"]: expression for expression in program["expressions"]}
    controls = [control for control in program["controls"]
                if expressions[control["condition"]]["kind"].get("compare", {}).get("relation") == "uge"]
    assert len(controls) == 1, program
    control = controls[0]
    assert expressions[control["condition"]]["ty"] == "predicate"
    assert ui_control(page, "Simplified structured code", role="tab")["selected"]
    high_text = copy_code(page, canvas)
    assert " >= " in high_text and "iszero(" not in high_text, high_text
    assert any("if (" in row["label"] and " >= " in row["label"]
               for row in observed(page, "decompile-code"))
    canvas.screenshot(path=str(context.output / "lifted-unsigned-predicate-code.png"))

    # The original Structured snapshot remains a separate navigable representation.
    choose_stage(page, canvas, artifact, "structured")
    click_control(page, canvas, "Structured code", role="tab")
    raw_text = copy_code(page, canvas)
    assert raw_text.count("evm.iszero(") == 3 and "evm.lt(" in raw_text, raw_text
    canvas.screenshot(path=str(context.output / "lifted-retained-structured-witness.png"))

    choose_stage(page, canvas, artifact, "lifted")
    click_control(page, canvas, "Simplified structured code", role="tab")
    source_descendant(page, canvas, artifact, current["id"], control["id"])
    selected_row(page, "decompile-code", " >= ")
    inspector_control(page, canvas, "Control h" + control["id"], "decompile-entity-detail")
    canvas.screenshot(path=str(context.output / "lifted-predicate-source-roundtrip.png"))
    assert {"stage": current["id"], "entity": control["id"]} in event["outputs"]
    origin = next(origin for origin in artifact["mappings"]["origins"]
                  if origin["entity"] == {"stage": current["id"], "entity": control["id"]})
    assert origin["source"] and origin["parents"] and origin["introduced_by"] == event["id"], origin

    choose_stage(page, canvas, artifact, "structured", before=True)
    click_control(page, canvas, "Changes", role="tab")
    assert any("New" in row["label"] for row in observed(page, "decompile-difference"))
    inspector_control(page, canvas, f"Transform {event['id']}:", "decompile-transform")
    canvas.screenshot(path=str(context.output / "lifted-transform-history.png"))
    artifacts.append(artifact["id"])

    # A word consumer must retain the 0/1 cast; double ISZERO is not word identity.
    artifact, _ = submit(context, page, canvas, NUMERIC_TRUTH, "lifted-numeric-truth")
    _, current, program, _ = projection(artifact)
    expressions = {expression["id"]: expression for expression in program["expressions"]}
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    stores = [statement for block in program["blocks"] for statement in block["statements"]
              if operations[statement["operation"]]["opcode"] == 0x55]
    assert len(stores) == 1 and len(stores[0]["inputs"]) == 2, stores
    value = expressions[stores[0]["inputs"][1]]
    assert value["ty"] == "word256" and "word_from_predicate" in value["kind"], value
    click_control(page, canvas, "Simplified structured code", role="tab")
    text = copy_code(page, canvas)
    assert "word(" in text and " != 0" in text and "iszero(" not in text, text
    canvas.screenshot(path=str(context.output / "lifted-numeric-boolean-cast.png"))
    artifacts.append(artifact["id"])

    # A known return target and a POP-only parameter have no high-level consumer.
    artifact, _ = submit(context, page, canvas, RETURN_CLEANUP, "lifted-return-cleanup")
    before, current, program, _ = projection(artifact)
    blocks = [block for block in before["ir"]["blocks"] if block["source"]["pc"] == 11]
    assert len(blocks) == 1 and len(blocks[0]["parameters"]) == 2, blocks
    block = next(block for block in program["blocks"] if block["block"] == blocks[0]["id"])
    assert block["parameters"] == [] and block["target"] is None, block
    edges = [edge for edge in current["ir"]["edges"] if edge["to"] == blocks[0]["id"]]
    assert edges and all(edge["arguments"] for edge in edges)
    for edge in edges:
        transfer = next(transfer for transfer in program["transfers"] if transfer["edge"] == edge["id"])
        assert transfer["arguments"] == [], transfer
    click_control(page, canvas, "Simplified structured code", role="tab")
    text = copy_code(page, canvas)
    assert "parallel assignment" not in text and "stop(" in text, text
    for parameter in blocks[0]["parameters"]:
        assert f"v{parameter}" not in text, text
    canvas.screenshot(path=str(context.output / "lifted-return-and-pop-parameters.png"))
    artifacts.append(artifact["id"])

    # Residual exits must use high expressions even when raw PUSH/ADD/ISZERO
    # bindings have disappeared; an opaque CALLDATALOAD target alone misses this.
    for name, code, completion in [
        ("opaque-target", "5f3556", "partial"),
        ("computed-target", "5f3560010156", "partial"),
        ("invalid-target", "60ff56", "complete"),
        ("conditional-target", "5f3515156020356001015700", "partial"),
        ("final-unknown-branch", "5f3560203557", "partial"),
        ("final-invalid-branch", "5f3560ff57", "complete"),
    ]:
        artifact, _ = submit(context, page, canvas, code, "lifted-" + name)
        assert artifact["completion"] == completion, artifact["diagnostics"]
        _, current, program, _ = projection(artifact)
        expressions = {expression["id"]: expression for expression in program["expressions"]}
        raw_blocks = {block["id"]: block for block in current["ir"]["blocks"]}
        residuals = [block for block in program["blocks"] if block["target"] is not None]
        assert residuals, program
        click_control(page, canvas, "Simplified structured code", role="tab")
        text = copy_code(page, canvas)
        for block in residuals:
            raw_term = raw_blocks[block["block"]]["terminator"]
            target = next(iter(raw_term.values()))["target"]
            expression = expressions[block["target"]]
            if expression["kind"].get("value") != target:
                assert not re.search(r"\bv" + target + r"\b", text), text
        if name in {"invalid-target", "final-invalid-branch"}:
            assert "0xff" in text and "invalid" in text, text
        else:
            assert "unresolved" in text or "partial" in text, text
        if name in {"computed-target", "conditional-target"}:
            assert " + " in text and "0x1" in text, text
        if name == "conditional-target":
            assert " != 0" in text and "iszero(" not in text, text
        canvas.screenshot(path=str(context.output / ("lifted-residual-" + name + ".png")))
        if name.startswith("final-"):
            # Only the false continuation of a terminal JUMPI stops. Its true
            # partial/invalid exit must remain explicit in both source views.
            assert text.count("stop(") == 1, text
            click_control(page, canvas, "Structured code", role="tab")
            raw_text = copy_code(page, canvas)
            assert raw_text.count("stop(") == 1, raw_text
            assert ("unresolved" in raw_text or "partial" in raw_text) if completion == "partial" else "invalid" in raw_text, raw_text
            click_control(page, canvas, "Simplified structured code", role="tab")
        artifacts.append(artifact["id"])
    rendered_frame(page)
    expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + artifact["id"])
    gpu = webgpu_evidence(page)
    page.close()
    return {"artifacts": artifacts, "webgpu": gpu}
