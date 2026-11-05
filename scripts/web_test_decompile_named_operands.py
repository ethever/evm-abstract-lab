"""Named high-level operands retain values, ordering, copying and provenance."""

import re

from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_helpers import (inspector_control, observed,
                                       open_decompiler, stage, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, set_control, ui_control,
                              webgpu_evidence)


def painted_copy(page, canvas):
    text = copy_code(page, canvas)
    labels = [code_label(row["label"])[1] for row in observed(page, "decompile-code")]
    assert text.splitlines() == labels, (text, labels)
    return text


def invoke(opcode, count):
    # Pushing in reverse makes sentinel 1 the first popped argument. Expected
    # labels below are written independently of the production signature table.
    return "".join(f"60{value:02x}" for value in range(count, 0, -1)) + f"{opcode:02x}5f5500"


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    posts, publications, results = [], [], []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and "/api/" in request.url else None)
    page.on("response", lambda response: publications.append(response.url)
            if "/api/decompile/" in response.url and response.url.endswith("/result") else None)

    for opcode, name, count, expected in [
        (0xf1, "call", 7, "gas = 0x1, target = 0x2, value = 0x3, input_offset = 0x4, input_size = 0x5, output_offset = 0x6, output_size = 0x7"),
        (0xf2, "callcode", 7, "gas = 0x1, target = 0x2, value = 0x3, input_offset = 0x4, input_size = 0x5, output_offset = 0x6, output_size = 0x7"),
        (0xf4, "delegatecall", 6, "gas = 0x1, target = 0x2, input_offset = 0x3, input_size = 0x4, output_offset = 0x5, output_size = 0x6"),
        (0xfa, "staticcall", 6, "gas = 0x1, target = 0x2, input_offset = 0x3, input_size = 0x4, output_offset = 0x5, output_size = 0x6"),
        (0xf5, "create2", 4, "value = 0x1, init_offset = 0x2, init_size = 0x3, salt = 0x4"),
    ]:
        artifact, _ = submit(context, page, canvas, invoke(opcode, count), "named-" + name)
        current = stage(artifact, "lifted")
        result_url, admitted = publications[-1], len(posts)
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        text = painted_copy(page, canvas)
        invocation = f"{name}({expected});"
        assert text.count(invocation) == 1, text
        assert not re.search(r"(?m)^\s*// (?:sequence|b\d+)", text), text
        operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                      for operation in block["operations"]}
        statement = next(statement for block in current["ir"]["lifted"]["blocks"]
                         for statement in block["statements"]
                         if operations[statement["operation"]]["opcode"] == opcode)
        before = stage(artifact, "structured")
        raw = lambda ir: {key: value for key, value in ir.items() if key != "lifted"}
        assert raw(current["ir"]) == raw(before["ir"])
        row = next(row for row in observed(page, "decompile-code") if invocation in row["label"])
        click_control(page, canvas, row["label"], role="decompile-code", id=row["id"])
        detail = inspector_control(page, canvas, "Statement h" + statement["id"], "decompile-entity-detail")
        assert invocation in detail["label"] and "effect q" in detail["label"], detail
        source_descendant(page, canvas, artifact, current["id"], statement["id"])
        selected_row(page, "decompile-code", invocation)
        assert painted_copy(page, canvas) == text
        set_control(page, canvas, "Show state dependencies", True)
        detailed = painted_copy(page, canvas)
        assert invocation in detailed and "effect q" in detailed, detailed
        set_control(page, canvas, "Show state dependencies", False)
        assert painted_copy(page, canvas) == text
        click_control(page, canvas, "Structured code", role="tab")
        raw_text = copy_code(page, canvas)
        raw_call = next(line for line in raw_text.splitlines() if f"evm.{name}(" in line)
        assert "gas =" not in raw_call and "input_offset =" not in raw_call, raw_call
        click_control(page, canvas, "Simplified structured code", role="tab")
        assert painted_copy(page, canvas) == text
        retained = page.request.get(result_url)
        assert retained.ok and retained.json()["result"]["Ok"]["artifact"] == artifact
        assert len(posts) == admitted, posts
        canvas.screenshot(path=str(context.output / ("named-" + name + "-code.png")))
        results.append({"artifact": artifact["id"], "opcode": opcode, "invocation": invocation})

    # A dynamic gas expression and zero output length retain their original
    # values. Named output_offset must remain even when output_size is zero.
    code = "5f6080608460805f60025f356102c69003f15f5500"
    artifact, _ = submit(context, page, canvas, code, "named-call-dynamic-gas")
    text = painted_copy(page, canvas)
    loaded = re.search(r"(v\d+) = calldataload\(offset = 0x0\);", text)
    assert loaded, text
    invocation = (f"call(gas = ({loaded[1]} - 0x2c6), target = 0x2, value = 0x0, "
                  "input_offset = 0x80, input_size = 0x84, output_offset = 0x80, output_size = 0x0);")
    assert invocation in text, text
    canvas.screenshot(path=str(context.output / "named-call-dynamic-gas-code.png"))
    results.append({"artifact": artifact["id"], "invocation": invocation})

    # Copies, topics and high-level terminal rendering have separate ownership
    # paths, but use the same fixed operand naming convention.
    code = "600360025f3760015f5160025e600960086007600660056004a460035ffd"
    artifact, _ = submit(context, page, canvas, code, "named-memory-log-revert")
    text = painted_copy(page, canvas)
    for expected in [
        "calldatacopy(memory_offset = 0x0, calldata_offset = 0x2, size = 0x3);",
        "mload(offset = 0x0)",
        "mcopy(dest_offset = 0x2, src_offset = ",
        "log4(offset = 0x4, size = 0x5, topic0 = 0x6, topic1 = 0x7, topic2 = 0x8, topic3 = 0x9);",
        "evm.revert(offset = 0x0, size = 0x3);",
    ]:
        assert expected in text, (expected, text)
    assert re.search(r"mcopy\(dest_offset = 0x2, src_offset = v\d+, size = 0x1\);", text), text
    canvas.screenshot(path=str(context.output / "named-memory-log-revert-code.png"))
    results.append({"artifact": artifact["id"], "terminal": "revert"})
    gpu = webgpu_evidence(page)
    page.close()
    return {"cases": results, "webgpu": gpu}
