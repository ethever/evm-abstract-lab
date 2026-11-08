"""Opcode display keeps operators, named operands and source identities."""

import re

from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_helpers import (inspector_control, observed,
                                       open_decompiler, stage, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, resize_view, set_control, ui_control,
                              webgpu_evidence)


# Byte-for-byte equal to the native Web fixture. Each arithmetic result crosses
# a real JUMP, so these are retained statements rather than inline-only probes.
RETAINED_OPERATORS = ("600360025f3760025f5101600e565b60031b6015565b60041c601c565b19"
                      "6021565b6002900a6029565b5f5560035ffd")


def painted_copy(page, canvas):
    text = copy_code(page, canvas)
    labels = [code_label(row["label"])[1] for row in observed(page, "decompile-code")]
    assert text.splitlines() == labels, (text, labels)
    return text


def invoke(opcode, count):
    # Pushing in reverse makes sentinel 1 the first popped argument. Expected
    # labels below are written independently of the production signature table.
    return "".join(f"60{value:02x}" for value in range(count, 0, -1)) + f"{opcode:02x}5f5500"


def retained_operators(context, page, canvas, posts, publications):
    resize_view(page, 1920, 2200)
    artifact, _ = submit(context, page, canvas, RETAINED_OPERATORS, "retained-opcode-operators")
    result_url, admitted = publications[-1], len(posts)
    current = stage(artifact, "lifted")
    assert artifact["completion"] == "complete", artifact["diagnostics"]
    assert all(item["validation"]["status"] == "passed" for item in artifact["stages"])
    raw = lambda ir: {key: value for key, value in ir.items() if key != "lifted"}
    assert raw(current["ir"]) == raw(stage(artifact, "structured")["ir"])
    program = current["ir"]["lifted"]
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    statements = {operations[statement["operation"]]["opcode"]: statement
                  for block in program["blocks"] for statement in block["statements"]}
    expressions = {expression["id"]: expression for expression in program["expressions"]}
    word = lambda opcode, index: "v" + expressions[statements[opcode]["inputs"][index]]["kind"]["value"]
    output = lambda opcode: "v" + statements[opcode]["outputs"][0]
    for opcode, index, expected in [(0x01, 1, "0x2"), (0x1b, 0, "0x3"),
                                    (0x1c, 0, "0x4"), (0x0a, 1, "0x2")]:
        assert expressions[statements[opcode]["inputs"][index]]["kind"] == {"word": expected}
    expected = [
        (0x37, "evm.calldatacopy(memory_offset = 0x0, calldata_offset = 0x2, size = 0x3);"),
        (0x51, f"{output(0x51)} = evm.mload(offset = 0x0);"),
        (0x01, f"{output(0x01)} = ({word(0x01, 0)} + 0x2);"),
        (0x1b, f"{output(0x1b)} = ({word(0x1b, 1)} << 0x3);"),
        (0x1c, f"{output(0x1c)} = ({word(0x1c, 1)} >> 0x4);"),
        (0x19, f"{output(0x19)} = (~{word(0x19, 0)});"),
        (0x0a, f"{output(0x0a)} = ({word(0x0a, 0)} ** 0x2);"),
        (0xfd, "evm.revert(offset = 0x0, size = 0x3);"),
    ]
    text = painted_copy(page, canvas)
    for opcode, row_text in expected:
        statement = statements[opcode]
        operation = operations[statement["operation"]]
        assert set(statement) == {"id", "operation", "inputs", "outputs"}, statement
        assert statement["outputs"] == operation["outputs"], statement
        if opcode in {0x01, 0x1b, 0x1c, 0x19, 0x0a}:
            block = next(block for block in current["ir"]["blocks"] if operation in block["operations"])
            assert any(edge["from"] == block["id"] and operation["outputs"][0] in edge["arguments"]
                       for edge in current["ir"]["edges"]), operation
        assert text.splitlines().count(row_text) == 1, text
        source_descendant(page, canvas, artifact, current["id"], statement["id"], operation["source"]["pc"])
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        assert code_label(selected_row(page, "decompile-code", row_text)["label"])[1] == row_text
        detail = inspector_control(page, canvas, f"Statement h{statement['id']} ", "decompile-entity-detail")
        assert row_text in detail["label"], detail
        if opcode == 0x01:
            for entity in [statement["operation"], *statement["inputs"], *statement["outputs"]]:
                # The reference itself navigates away. Restore the statement
                # before inspecting every subsequent reference in its panel.
                source_descendant(page, canvas, artifact, current["id"], statement["id"], operation["source"]["pc"])
                set_control(page, canvas, "Entity references", True, role="section")
                inspector_control(page, canvas, f"Reference s{current['id']}/e{entity} ", "decompile-entity-link")
    click_control(page, canvas, "Simplified structured code", role="tab")
    semantic = lambda line: re.split(r" // (?:memory|effect)", line, maxsplit=1)[0]
    for dependencies, annotations in [(True, False), (True, True), (False, True), (False, False)]:
        set_control(page, canvas, "Show state dependencies", dependencies)
        set_control(page, canvas, "Show structure annotations", annotations)
        detailed = painted_copy(page, canvas)
        rows = [semantic(line) for line in detailed.splitlines()]
        for _, row_text in expected:
            assert rows.count(row_text) == 1, detailed
        assert (" // memory" in detailed) == dependencies, detailed
    assert painted_copy(page, canvas) == text
    click_control(page, canvas, "Structured code", role="tab")
    raw_text = copy_code(page, canvas)
    for opcode, name in [(0x01, "add"), (0x1b, "shl"), (0x1c, "shr"), (0x19, "not"), (0x0a, "exp")]:
        operation = operations[statements[opcode]["operation"]]
        arguments = ", ".join("v" + value for value in operation["inputs"])
        assert f"v{operation['outputs'][0]} = evm.{name}({arguments});" in raw_text, raw_text
    click_control(page, canvas, "Simplified structured code", role="tab")
    assert painted_copy(page, canvas) == text
    retained = page.request.get(result_url)
    assert retained.ok and retained.json()["result"]["Ok"]["artifact"] == artifact
    assert len(posts) == admitted, posts
    canvas.screenshot(path=str(context.output / "retained-opcode-operators-code.png"))
    return {"artifact": artifact["id"], "rows": dict(expected),
            "statements": {opcode: statements[opcode]["id"] for opcode, _ in expected}}


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
    loaded = re.search(r"(v\d+) = evm\.calldataload\(offset = 0x0\);", text)
    assert loaded, text
    invocation = (f"evm.call(gas = ({loaded[1]} - 0x2c6), target = 0x2, value = 0x0, "
                  "input_offset = 0x80, input_size = 0x84, output_offset = 0x80, output_size = 0x0);")
    assert invocation in text, text
    canvas.screenshot(path=str(context.output / "named-call-dynamic-gas-code.png"))
    results.append({"artifact": artifact["id"], "invocation": invocation})

    # Copies, topics and high-level terminal rendering have separate ownership
    # paths, but use the same fixed operand naming convention.
    code = ("600360025f3760015f5160025e600960086007600660056004a4"
            "60026001205f5560113160015560025f351260025560035ffd")
    artifact, _ = submit(context, page, canvas, code, "named-memory-log-revert")
    text = painted_copy(page, canvas)
    for expected in [
        "evm.calldatacopy(memory_offset = 0x0, calldata_offset = 0x2, size = 0x3);",
        "evm.mload(offset = 0x0)",
        "evm.mcopy(dest_offset = 0x2, src_offset = ",
        "evm.log4(offset = 0x4, size = 0x5, topic0 = 0x6, topic1 = 0x7, topic2 = 0x8, topic3 = 0x9);",
        "evm.keccak256(0x1, 0x2)",
        "evm.balance(0x11)",
        "evm.revert(offset = 0x0, size = 0x3);",
    ]:
        assert expected in text, (expected, text)
    assert re.search(r"evm\.mcopy\(dest_offset = 0x2, src_offset = v\d+, size = 0x1\);", text), text
    assert re.search(r"word\(evm\.slt\(v\d+, 0x2\)\)", text), text
    canvas.screenshot(path=str(context.output / "named-memory-log-revert-code.png"))
    results.append({"artifact": artifact["id"], "terminal": "revert"})
    results.append(retained_operators(context, page, canvas, posts, publications))
    gpu = webgpu_evidence(page)
    page.close()
    return {"cases": results, "webgpu": gpu}
