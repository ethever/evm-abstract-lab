"""Source-bound common continuations through HTTP, Wasm and painted code."""

from web_test_decompile_alignment import code_label
from web_test_decompile_guards import (ANNOTATIONS, DEPENDENCIES, high,
                                       painted_copy, projection)
from web_test_decompile_helpers import (choose_stage, open_decompiler, stage,
                                        submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, resize_view, set_control, ui_control,
                              webgpu_evidence)


# Each branch may terminate before reaching one shared SSTORE/STOP block.
# PUSH2 destinations deliberately remain fixed in the INVALID variant; its
# trailing STOP bytes are unreachable source instructions, not invented exits.
REVERT = "5f356100115760403561001d57610021565b60203561001d57610021565b5f5ffd5b60775f5500"
INVALID = REVERT.replace("5b5f5ffd5b6077", "5bfe00005b6077")


def semantic_lines(text):
    return [line.split(" // ", 1)[0].strip() for line in text.splitlines()
            if line.strip() and not line.lstrip().startswith("//")
            and not (" token on e" in line)]


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
    result = {"fixtures": reports, "renderer": webgpu_evidence(page)}
    page.close()
    return result
