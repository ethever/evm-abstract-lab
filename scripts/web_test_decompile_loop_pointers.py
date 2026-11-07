"""Changing, source-bounded loop pointers through HTTP, Wasm and painted code."""

import re

from web_test_decompile_alignment import code_label
from web_test_decompile_error_payloads import (MESSAGE, abi_bytes, error_tail,
                                             navigate_witnesses)
from web_test_decompile_guards import (ANNOTATIONS, DEPENDENCIES, Runtime, high,
                                       painted_copy, projection)
from web_test_decompile_helpers import observed, open_decompiler, submit
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, resize_view, set_control, ui_control,
                              webgpu_evidence)


def runtime(coefficient=0x60, step=0x67, guard=True, mutation=None):
    """Native error_payloads/loop_pointers.rs uses the same real source program.

    The success edge rejects n > 7 OR n < 2. B=0xe0+48*n holds n; the initial
    F=B+64+coefficient*n grows by step while i < memory[B]-1. Neither allocation
    nor pointer increment uses Router02's constants. No concrete n is supplied
    to the decompiler, and the loop reloads its protected array header.
    """
    code = Runtime()
    code.emit("5f358060079011816002901017")
    code.jump("reject")
    code.emit("8060300260e001818152")
    if mutation == "wrap":
        # For admitted n=2, the first F update reaches 0x47, overlapping slot64.
        code.emit("8160100260405f0301604052")
    else:
        code.emit(f"8160{coefficient:02x}028101604001604052")
    code.emit("5f")
    code.label("header")
    code.emit("6001825103811015")
    code.jump("exit")
    # Retain the saved F across both stores: first F+43, then F+step.
    code.emit(f"60405180602b016040526037816020015260{step:02x}01604052")
    code.emit("602a82602083600101020152")  # memory[B+32*(i+1)] = 42
    code.emit("60406040515f5f60015afa50")  # 64-byte output at the updated F
    if mutation == "alias":
        code.emit("5f60403552")  # arbitrary address may overwrite B or slot64
    if guard:
        # A second pass predecessor retains a real word phi at the joined store.
        code.emit("80606035")
        code.jump("other-pass")
        code.emit("602035")
        code.jump("pass")
        code.emit(error_tail(MESSAGE))
        code.label("other-pass")
        code.emit("600101")
        code.jump("pass", False)
        code.label("pass")
        code.emit("5f55")  # guard passes i; alternate predecessor passes i+1
    code.emit("600101")
    code.jump("header", False)
    code.label("exit")
    if guard:
        code.emit("50505000")
    else:
        code.emit("805f55")  # observable prefix remains before the loop-exit construction
        code.emit(error_tail(MESSAGE))
    code.label("reject")
    code.emit("5f5ffd")
    return code.finish()


def semantic_rows(text):
    """Remove only display metadata; keep word assignments, control and order."""
    rows = []
    for line in text.splitlines():
        if line.lstrip().startswith("//"):
            continue
        if re.fullmatch(r"\s*[mq]\d+ = [mq]\d+; // (memory|effect) token on e\d+", line):
            continue
        rows.append(line.split(" // memory", 1)[0].split(" // effect", 1)[0])
    return rows


def live_rows(page, program, current, absorbed):
    plan, = program["constructions"]
    block = next(block for block in program["blocks"] if block["block"] == plan["block"])
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    hidden = {statement["id"] for statement in block["statements"][plan["start"]:plan["end"]]} if absorbed else set()
    retained = [statement for block in program["blocks"] for statement in block["statements"]
                if statement["id"] not in hidden and operations[statement["operation"]]["opcode"] in {0x52, 0xfa}]
    assert len(retained) >= 7, retained
    text = "\n".join(code_label(row["label"])[1] for row in observed(page, "decompile-code"))
    for opcode, spelling in [(0x52, "evm.mstore("), (0xfa, "evm.staticcall(")]:
        expected = sum(operations[statement["operation"]]["opcode"] == opcode for statement in retained)
        assert text.count(spelling) == expected, (retained, text)
    return retained


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 3200)
    posts, publications, results = [], [], []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and "/api/" in request.url else None)
    page.on("response", lambda response: publications.append(response.url)
            if "/api/decompile/" in response.url and response.url.endswith("/result") else None)
    for coefficient, step, guarded in [(0x60, 0x67, True), (0x70, 0x59, False)]:
        name = "loop-pointer-" + ("require" if guarded else "revert")
        artifact, _ = submit(context, page, canvas, runtime(coefficient, step, guarded), name)
        current, program = projection(artifact)
        admitted, result_url = len(posts), publications[-1]
        payloads = program.get("payloads", [])
        assert len(payloads) == 1, f"{name} must certify its source-bounded payload: {payloads}"
        payload, = payloads
        assert payload["message"] == MESSAGE and bytes(payload["bytes"]) == abi_bytes(MESSAGE), payload
        plans, terminals = program.get("constructions", []), program.get("reverts", [])
        assert len(plans) == len(terminals) == 1, (name, plans, terminals)
        plan, = plans
        terminal, = terminals
        if_nodes = {node["id"] for node in current["ir"]["structured"]["nodes"] if "if" in node["kind"]}
        guards = [control for control in program["controls"] if control["node"] in if_nodes and control.get("guard")
                  and control["guard"]["payload"] == {"error_string": payload["id"]}]
        assert len(guards) == int(guarded), guards
        loops = [node for node in current["ir"]["structured"]["nodes"] if "while" in node["kind"]]
        assert len(loops) == 1, loops
        high(page, canvas)
        text = painted_copy(page, canvas)
        assert "while (" in text and "evm.staticcall(" in text, text
        assert text.count("evm.mstore(offset = 0x40,") == 3, text
        assert ("evm.revert(offset =" in text) != guarded and ("require(" in text) == guarded, text
        if guarded:
            assert guards[0]["guard"]["pass_nonzero"], guards
            source_descendant(page, canvas, artifact, current["id"], guards[0]["id"])
            ui_control(page, "Simplified structured code", role="tab", selected=True)
            required = code_label(selected_row(page, "decompile-code", "require(")["label"])[1]
            store_at = text.index("evm.sstore(", text.index(required))
            transfer = next(transfer for transfer in program["transfers"]
                            if transfer["edge"] == guards[0]["guard"]["pass_edge"])
            assert transfer["arguments"], transfer
            source_descendant(page, canvas, artifact, current["id"], transfer["id"])
            ui_control(page, "Simplified structured code", role="tab", selected=True)
            row = selected_row(page, "decompile-code", " = ")
            tuple_text = code_label(row["label"])[1]
            assert text.index(required) < text.index(tuple_text) < store_at, text
        else:
            assert "evm.revert(offset =" in text, text
            terminal_statement = next(statement for block in program["blocks"] for statement in block["statements"]
                                      if statement["operation"] == terminal["operation"])
            source_descendant(page, canvas, artifact, current["id"], terminal_statement["id"])
            ui_control(page, "Simplified structured code", role="tab", selected=True)
            terminal_row = code_label(selected_row(page, "decompile-code", "evm.revert(offset =")["label"])[1]
            assert text.index("evm.sstore(") < text.index(terminal_row), text
        retained = live_rows(page, program, current, guarded)
        for dependencies, annotations in [(True, False), (False, True), (True, True), (False, False)]:
            set_control(page, canvas, DEPENDENCIES, dependencies)
            set_control(page, canvas, ANNOTATIONS, annotations)
            displayed = painted_copy(page, canvas)
            assert semantic_rows(displayed) == semantic_rows(text), (text, displayed)
            live_rows(page, program, current, guarded)
        assert painted_copy(page, canvas) == text
        navigate_witnesses(page, canvas, artifact, current, program, text, not guarded)
        # Ordinary F writes remain source-selectable high-level operations.
        for statement in [retained[0], retained[-1]]:
            source_descendant(page, canvas, artifact, current["id"], statement["id"])
            ui_control(page, "Simplified structured code", role="tab", selected=True)
            operation = next(operation for block in current["ir"]["blocks"]
                             for operation in block["operations"] if operation["id"] == statement["operation"])
            selected_row(page, "decompile-code", operation["name"].lower() + "(")
        click_control(page, canvas, "IR", role="tab")
        ui_control(page, "IR", role="tab", selected=True)
        ir_before = [(row["label"], row["selected"]) for row in observed(page, "decompile-ir")]
        assert ir_before
        high(page, canvas)
        assert painted_copy(page, canvas) == text
        click_control(page, canvas, "IR", role="tab")
        ui_control(page, "IR", role="tab", selected=True)
        assert [(row["label"], row["selected"]) for row in observed(page, "decompile-ir")] == ir_before
        high(page, canvas)
        response = page.request.get(result_url)
        assert response.ok and response.json()["result"]["Ok"]["artifact"] == artifact
        assert len(posts) == admitted, posts
        canvas.screenshot(path=str(context.output / (name + "-review.png")))
        results.append({"shape": name, "artifact": artifact["id"], "code": text,
                        "payload": payload, "construction": plan, "terminal": terminal,
                        "retained_statements": [statement["id"] for statement in retained],
                        "coefficient": coefficient, "step": step, "source_n_bounds": [2, 7]})

    # Raw failure construction plus every state/structure row is taller than
    # its certified positive. Keep the complete annotated fixture in view.
    resize_view(page, 1920, 3200)
    for mutation in ["alias", "wrap"]:
        name = "loop-pointer-rejected-" + mutation
        artifact, _ = submit(context, page, canvas, runtime(mutation=mutation), name)
        _, program = projection(artifact)
        assert not program.get("payloads") and not program.get("constructions"), program
        high(page, canvas)
        for dependencies, annotations in [(False, False), (True, True), (False, False)]:
            set_control(page, canvas, DEPENDENCIES, dependencies)
            set_control(page, canvas, ANNOTATIONS, annotations)
            text = painted_copy(page, canvas)
            assert "while (" in text and "evm.mstore(" in text and "evm.revert(offset =" in text, text
            assert f', "{MESSAGE}");' not in text and f'evm.revert("{MESSAGE}");' not in text, text
        results.append({"rejected": mutation, "artifact": artifact["id"], "code": text})
    gpu = webgpu_evidence(page)
    page.close()
    return {"cases": results, "renderer": gpu}
