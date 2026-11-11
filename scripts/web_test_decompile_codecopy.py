"""CODECOPY error certificates preserve raw witnesses and reject dirty padding."""

from playwright.sync_api import expect
from web_test_decompile_alignment import copy_code
from web_test_decompile_guards import high, painted_copy, projection
from web_test_decompile_helpers import (choose_stage, inspector_control,
                                       open_decompiler, submit)
from web_test_decompile_review import source_descendant
from web_test_harness import click_control, resize_view, ui_control, webgpu_evidence
from web_test_registry import SOURCE_ROOT


MESSAGE = "UniswapV2Library: INSUFFICIENT_LIQUIDITY"
FIXTURES = "crates/evm-decompile/tests/fixtures/error-payloads/"


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 1800)
    results = []
    for dirty in [False, True]:
        name = "router-codecopy-liquidity-" + ("dirty" if dirty else "guard")
        bytecode = (SOURCE_ROOT / (FIXTURES + name + ".hex")).read_text().strip()
        artifact, _ = submit(context, page, canvas, bytecode, name, procedures="Inline")
        assert artifact["completion"] == "complete"
        current, program = projection(artifact)
        assert bytes(current["ir"]["runtime_bytecode"]) == bytes.fromhex(bytecode)
        high(page, canvas)
        text = painted_copy(page, canvas)
        if dirty:
            assert not program.get("reverts") and not program.get("payloads")
            data, = program["revert_data"]
            assert "segments" in data["content"], data
            assert all(plan["revert"] == data["id"] for plan in program.get("constructions", []))
            assert text.count("evm.codecopy(") == 1 and "evm.revert(offset =" in text
            assert "require(" not in text and MESSAGE not in text
            source_descendant(page, canvas, artifact, current["id"], data["id"])
            inspector_control(page, canvas, "Decoded format: Unrecognized format",
                              "decompile-entity-detail")
            inspector_control(page, canvas, "Rendered data: Revert(data = concat(",
                              "decompile-entity-detail")
            high(page, canvas)
            assert painted_copy(page, canvas) == text
        else:
            payload, = program["payloads"]
            proof, = program["reverts"]
            capsule, = program["constructions"]
            data = bytes(payload["bytes"])
            assert payload["message"] == MESSAGE
            assert len(data) == 132 and data[68:108] == MESSAGE.encode() and data[108:] == bytes(24)
            assert proof["code_hash"] and len(proof["code_hash"]) == 66
            assert MESSAGE in text and "require(" in text and "evm.codecopy(" not in text
            operations = {op["id"]: op for block in current["ir"]["blocks"] for op in block["operations"]}
            body = next(block for block in program["blocks"] if block["block"] == proof["block"])
            statements = body["statements"][capsule["start"]:capsule["end"]]
            copied = next(statement for statement in statements if operations[statement["operation"]]["opcode"] == 0x39)
            assert copied["operation"] in proof["writes"]
            source_descendant(page, canvas, artifact, current["id"], copied["id"])
            ui_control(page, "IR", role="tab", selected=True)
            source_descendant(page, canvas, artifact, current["id"], proof["id"])
            inspector_control(page, canvas, "Captured runtime code hash: " + proof["code_hash"],
                              "decompile-entity-detail")
        choose_stage(page, canvas, artifact, "structured")
        click_control(page, canvas, "Structured code", role="tab")
        raw = copy_code(page, canvas)
        assert raw.count("evm.codecopy(") == 1 and "evm.revert(" in raw
        assert "Revert(data =" not in raw and 'evm.revert("' not in raw
        choose_stage(page, canvas, artifact, "lifted")
        high(page, canvas)
        assert painted_copy(page, canvas) == text
        expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + artifact["id"])
        canvas.screenshot(path=str(context.output / (name + "-review.png")))
        results.append({"dirty": dirty, "artifact": artifact["id"], "text": text})
    evidence = webgpu_evidence(page)
    page.close()
    return {"cases": results, "renderer": evidence}
