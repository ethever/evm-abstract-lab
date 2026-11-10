"""Source-bound common continuations through HTTP, Wasm and painted code."""

import json

from web_test_decompile_alignment import code_label
from web_test_decompile_guards import (ANNOTATIONS, DEPENDENCIES, high,
                                       painted_copy, projection)
from web_test_decompile_helpers import (choose_stage, open_decompiler, stage,
                                        submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, resize_view, set_control, ui_control,
                              webgpu_evidence)
from web_test_registry import SOURCE_ROOT


# Each branch may terminate before reaching one shared SSTORE/STOP block.
# PUSH2 destinations deliberately remain fixed in the INVALID variant; its
# trailing STOP bytes are unreachable source instructions, not invented exits.
REVERT = "5f356100115760403561001d57610021565b60203561001d57610021565b5f5ffd5b60775f5500"
INVALID = REVERT.replace("5b5f5ffd5b6077", "5bfe00005b6077")
LOOP_FIXTURE = "crates/evm-decompile/tests/fixtures/structure/regions/loop-normal-join"


def semantic_lines(text):
    return [line.split(" // ", 1)[0].strip() for line in text.splitlines()
            if line.strip() and not line.lstrip().startswith("//")
            and not (" token on e" in line)]


def loop_normal_join(context, page, canvas):
    bytecode = (SOURCE_ROOT / (LOOP_FIXTURE + ".hex")).read_text().strip()
    metadata = json.loads((SOURCE_ROOT / (LOOP_FIXTURE + ".json")).read_text())
    assert len(bytes.fromhex(bytecode)) == metadata["byte_length"]
    artifact, _ = submit(context, page, canvas, bytecode, "planned-loop-normal-join")
    assert artifact["completion"] == "complete", artifact["diagnostics"]
    assert not artifact["diagnostics"], artifact["diagnostics"]
    current, program = projection(artifact)
    ir = current["ir"]
    join, = [block for block in ir["blocks"]
             if block["source"]["pc"] == metadata["labels"]["join"]]
    nodes = stage(artifact, "structured")["ir"]["structured"]["nodes"]
    assert not any(isinstance(node["kind"], dict) and "dispatcher" in node["kind"]
                   for node in nodes)
    assert sum(isinstance(node["kind"], dict) and "while" in node["kind"]
               for node in nodes) == 1
    assert sum(node["kind"] == {"block": {"block": join["id"]}} for node in nodes) == 1
    operations = {operation["id"]: operation for block in ir["blocks"]
                  for operation in block["operations"]}
    suffix = [statement for block in program["blocks"] if block["block"] == join["id"]
              for statement in block["statements"]
              if operations[statement["operation"]]["opcode"] in (0x55, 0xa0)]
    assert sorted(operations[statement["operation"]]["opcode"] for statement in suffix) == [0x55, 0x55, 0xa0]
    failure, = [statement for block in program["blocks"] for statement in block["statements"]
                if operations[statement["operation"]]["opcode"] == 0xfd]

    high(page, canvas)
    text = painted_copy(page, canvas)
    assert text.count("evm.sstore(") == 3, text
    assert text.count("evm.log0(") == 1, text
    assert text.count("evm.revert(") == 1, text
    for statement in [*suffix, failure]:
        source_descendant(page, canvas, artifact, current["id"], statement["id"])
    for dependencies, annotations in [(True, False), (False, True), (True, True), (False, False)]:
        set_control(page, canvas, DEPENDENCIES, dependencies)
        set_control(page, canvas, ANNOTATIONS, annotations)
        assert semantic_lines(painted_copy(page, canvas)) == semantic_lines(text)
    # Raw stack/token rows are longer than the high projection. Keep its final
    # loop brace visible so painted_copy can compare every row with the clipboard.
    resize_view(page, 1920, 3400)
    choose_stage(page, canvas, artifact, "structured")
    click_control(page, canvas, "Structured code", role="tab")
    assert painted_copy(page, canvas).count("evm.sstore(") == 3
    choose_stage(page, canvas, artifact, "lifted")
    high(page, canvas)
    assert painted_copy(page, canvas) == text
    canvas.screenshot(path=str(context.output / "planned-loop-normal-join-review.png"))
    return {"case": "loop-normal-join", "artifact": artifact["id"],
            "join_pc": metadata["labels"]["join"], "join_block": join["id"],
            "suffix_statements": [statement["id"] for statement in suffix],
            "failure_statement": failure["id"], "text": text}


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 2600)
    reports = []
    for name, bytecode, terminal in [("revert", REVERT, "require("),
                                      ("invalid", INVALID, "evm.invalid();")]:
        artifact, _ = submit(context, page, canvas, bytecode, "planned-" + name)
        current, program = projection(artifact)
        operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                      for operation in block["operations"]}
        stores = [(block, statement) for block in program["blocks"]
                  for statement in block["statements"]
                  if operations[statement["operation"]]["opcode"] == 0x55]
        (block, statement), = stores
        source_store = operations[statement["operation"]]
        assert source_store["source"]["pc"] == 37, source_store
        structured = stage(artifact, "structured")["ir"]["structured"]
        occurrences = [node for node in structured["nodes"]
                       if node["kind"] == {"block": {"block": block["block"]}}]
        assert len(occurrences) == 1, occurrences

        high(page, canvas)
        text = painted_copy(page, canvas)
        assert text.count("evm.sstore(") == 1, text
        assert text.count(terminal) == 2, text
        assert text.index(terminal) < text.index("evm.sstore("), text
        assert text.rindex(terminal) < text.index("evm.sstore("), text
        if name == "invalid":
            assert "trap(" not in text and "exception(" not in text, text
        source_descendant(page, canvas, artifact, current["id"], statement["id"])
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        assert "evm.sstore(" in code_label(selected_row(page, "decompile-code", "evm.sstore(")["label"])[1]

        for dependencies, annotations in [(True, False), (False, True), (True, True), (False, False)]:
            set_control(page, canvas, DEPENDENCIES, dependencies)
            set_control(page, canvas, ANNOTATIONS, annotations)
            displayed = painted_copy(page, canvas)
            assert semantic_lines(displayed) == semantic_lines(text), (text, displayed)
        choose_stage(page, canvas, artifact, "structured")
        click_control(page, canvas, "Structured code", role="tab")
        ui_control(page, "Structured code", role="tab", selected=True)
        assert painted_copy(page, canvas).count("evm.sstore(") == 1
        choose_stage(page, canvas, artifact, "lifted")
        high(page, canvas)
        assert painted_copy(page, canvas) == text
        canvas.screenshot(path=str(context.output / ("planned-" + name + "-review.png")))
        reports.append({"case": name, "artifact": artifact["id"], "completion": artifact["completion"],
                        "store_operation": source_store["id"], "source_pc": source_store["source"]["pc"],
                        "store_occurrences": len(occurrences), "text": text})
    reports.append(loop_normal_join(context, page, canvas))
    result = {"fixtures": reports, "renderer": webgpu_evidence(page)}
    page.close()
    return result
