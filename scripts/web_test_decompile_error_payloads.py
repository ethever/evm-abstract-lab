"""Mixed require and retained Error(string) construction through the real browser."""

import re

from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_guards import (ANNOTATIONS, DEPENDENCIES, Runtime,
                                       guard_rows, high, painted_copy, projection)
from web_test_decompile_helpers import (choose_stage, inspector_control,
                                       open_decompiler, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import click_control, resize_view, set_control, ui_control, webgpu_evidence


MESSAGE = "ds-math-mul-overflow"
ESCAPED = 'quote" slash\\ line\n雪\x00'


def abi_bytes(message):
    data = message.encode("utf-8")
    return (bytes.fromhex("08c379a0") + (32).to_bytes(32, "big")
            + len(data).to_bytes(32, "big") + data + bytes((-len(data)) % 32))


def error_tail(message, alias=False, padding=False, observation=False):
    """Real overlapping MSTOREs through p and a second slot-64 pointer load."""
    data = message.encode("utf-8")
    assert 0 < len(data) < 128
    code = bytearray.fromhex("604051")
    selector = bytes.fromhex("08c379a0") + bytes(28)
    code.extend(bytes([0x7f]) + selector + bytes.fromhex("8152"))
    code.extend(bytes.fromhex("60208160040152"))
    code.extend(bytes([0x60, len(data)]) + bytes.fromhex("8160240152"))
    for position in range(0, len(data), 32):
        word = bytearray(data[position:position + 32].ljust(32, b"\0"))
        if padding and position + 32 >= len(data):
            assert len(data) % 32
            word[-1] = 1
        code.extend(bytes([0x7f]) + word + bytes([0x81, 0x60, 68 + position, 0x01, 0x52]))
    if alias:
        # Arbitrary calldata address may overwrite payload or pointer slot.
        code.extend(bytes.fromhex("5f5f3552"))
    if observation:
        code.extend(bytes.fromhex("3350"))
    size = len(abi_bytes(message))
    code.extend(bytes([0x50, 0x60, size]) + bytes.fromhex("604051fd"))
    return code.hex()


def runtime(message=MESSAGE, shape="nonzero", **options):
    code = Runtime()
    code.emit("6080604052")
    if shape == "standalone":
        code.emit(error_tail(message, **options))
        return code.finish()
    code.emit("5f35")
    if shape == "nonzero":
        code.jump("pass")
        code.emit(error_tail(message, **options))
        code.label("pass")
        code.emit("602a5f5500")
    elif shape == "zero":
        code.jump("failure")
        code.emit("602a5f5500")
        code.label("failure")
        code.emit(error_tail(message, **options))
    elif shape == "nested":
        code.jump("failure")
        code.emit("60203515")
        code.jump("failure")
        code.emit("602a5f5500")
        code.label("failure")
        code.emit(error_tail(message, **options))
    elif shape == "separate":
        code.jump("first-pass")
        code.emit(error_tail(message, **options))
        code.label("first-pass")
        code.emit("60203515")
        code.jump("second-pass")
        code.emit(error_tail(message, **options))
        code.label("second-pass")
        code.emit("602a5f5500")
    elif shape == "join":
        code.jump("pass")
        code.jump("failure", False)
        code.label("pass")
        code.emit("602a5f55")
        code.label("failure")
        code.emit(error_tail(message, **options))
    else:
        raise AssertionError(shape)
    return code.finish()


def mixed_runtime():
    code = Runtime()
    code.emit("60806040525f35")
    code.jump("empty-pass")
    code.emit("5f5ffd")
    code.label("empty-pass")
    code.emit("602035")
    code.jump("payload-pass")
    code.emit(error_tail(MESSAGE, observation=True))
    code.label("payload-pass")
    code.emit("604035")
    code.jump("normal")
    code.emit("fe")
    code.label("normal")
    code.emit("602a5f5500")
    return code.finish()


def failure_rows(text):
    return [line for line in text.splitlines()
            if line.lstrip().startswith(("require(", "if (", "evm.revert(offset ="))]


def options_invariance(page, canvas, program, text):
    expected = failure_rows(text)
    assert program.get("constructions")
    for dependencies, annotations in [(False, True), (True, False), (True, True), (False, False)]:
        set_control(page, canvas, DEPENDENCIES, dependencies)
        set_control(page, canvas, ANNOTATIONS, annotations)
        displayed = painted_copy(page, canvas)
        assert failure_rows(displayed) == expected, displayed
        assert displayed.count("evm.mstore(") == text.count("evm.mstore("), displayed
        assert displayed.count("evm.sstore(") == text.count("evm.sstore("), displayed
    assert painted_copy(page, canvas) == text


def navigate_witnesses(page, canvas, artifact, current, program, text, standalone):
    plan, = program["constructions"]
    terminal, = program["reverts"]
    payload, = program["payloads"]
    blocks = {block["block"]: block for block in program["blocks"]}
    statements = blocks[plan["block"]]["statements"][plan["start"]:plan["end"]]
    assert statements and terminal["operation"] in {statement["operation"] for statement in statements}
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    writes = [statement for statement in statements
              if operations[statement["operation"]]["opcode"] == 0x52]
    assert writes

    for statement in [writes[0], statements[-1]]:
        source_descendant(page, canvas, artifact, current["id"], statement["id"])
        ui_control(page, "Simplified structured code" if standalone else "IR", role="tab", selected=True)
        if standalone:
            selected_row(page, "decompile-code", "evm.mstore(" if statement in writes else "evm.revert(")

    source_descendant(page, canvas, artifact, current["id"], plan["id"])
    ui_control(page, "IR", role="tab", selected=True)
    capsule = inspector_control(page, canvas, "Certified lazy failure construction h" + plan["id"],
                                "decompile-entity-detail")
    assert f"statements {plan['start']}..{plan['end']}" in capsule["label"]
    inspector_control(page, canvas, "Original operations, memory expansion and exceptional faults",
                      "decompile-entity-detail")
    set_control(page, canvas, "Entity references", True, role="section")
    inspector_control(page, canvas, f"Reference s{current['id']}/e{terminal['id']} ",
                      "decompile-entity-link")
    proof = inspector_control(page, canvas, "Terminal memory m", "decompile-entity-detail")
    assert f"m{terminal['memory']}" in proof["label"] and f"payload h{payload['id']}" in proof["label"]
    set_control(page, canvas, "Entity references", True, role="section")
    inspector_control(page, canvas, f"Reference s{current['id']}/e{payload['id']} ",
                      "decompile-entity-link")
    inspector_control(page, canvas, "Certified Error(string) payload h" + payload["id"],
                      "decompile-entity-detail")

    if_nodes = {node["id"] for node in current["ir"]["structured"]["nodes"] if "if" in node["kind"]}
    controls = [control for control in program["controls"] if control["node"] in if_nodes and control.get("guard")
                and control["guard"]["payload"] == {"error_string": payload["id"]}]
    if controls:
        control = controls[0]
        source_descendant(page, canvas, artifact, current["id"], control["id"])
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        inspector_control(page, canvas, "Certified Error(string) guard h" + control["id"],
                          "decompile-entity-detail")
        inspector_control(page, canvas, "Lazy failure construction retains original memory expansion",
                          "decompile-entity-detail")
        source_descendant(page, canvas, artifact, current["id"], control["guard"]["failure_node"])
    else:
        click_control(page, canvas, "Structured code", role="tab")
    ui_control(page, "Structured code", role="tab", selected=True)
    raw = copy_code(page, canvas)
    assert "evm.mstore(" in raw and "evm.revert(" in raw and 'evm.revert("' not in raw, raw
    choose_stage(page, canvas, artifact, "structured")
    click_control(page, canvas, "Structured code", role="tab")
    assert copy_code(page, canvas) == raw
    choose_stage(page, canvas, artifact, "lifted")
    high(page, canvas)
    assert painted_copy(page, canvas) == text


def construction_rows(page, canvas, artifact, current, program, plan, text, navigate=False):
    block = next(block for block in program["blocks"] if block["block"] == plan["block"])
    statements = block["statements"][plan["start"]:plan["end"]]
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    primitives = [statement for statement in statements
                  if operations[statement["operation"]]["opcode"] in {0x51, 0x52, 0x53, 0xfd}]
    assert any(operations[statement["operation"]]["opcode"] == 0x51 for statement in primitives)
    assert sum(operations[statement["operation"]]["opcode"] == 0x52 for statement in primitives) >= 4
    assert "evm.mload(" in text and text.count("evm.mstore(") >= 5, text
    if not navigate:
        return
    after = -1
    lines = text.splitlines()
    for statement in primitives:
        operation = operations[statement["operation"]]
        spelling = "evm." + operation["name"].lower() + "("
        source_descendant(page, canvas, artifact, current["id"], statement["id"])
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        row = selected_row(page, "decompile-code", spelling)
        inspector_control(page, canvas, "Statement h" + statement["id"] + " ", "decompile-entity-detail")
        source = code_label(row["label"])[1]
        after = next(index for index in range(after + 1, len(lines)) if lines[index] == source)
        assert source.startswith("    "), (statement, text)
        click_control(page, canvas, row["label"], role="decompile-code", id=row["id"])
        selected_row(page, "decompile-code", spelling)
    # Every original primitive was selected and occurs in source order before
    # the failed If closes; normal storage continuation is outside that body.
    closed = next(index for index in range(after + 1, len(lines)) if lines[index] == "}")
    store = lines.index("evm.sstore(slot = 0x0, value = 0x2a);")
    assert after < closed < store, text
    assert painted_copy(page, canvas) == text


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 3000)
    posts, publications, results = [], [], []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and "/api/" in request.url else None)
    page.on("response", lambda response: publications.append(response.url)
            if "/api/decompile/" in response.url and response.url.endswith("/result") else None)
    for shape, message in [("nonzero", MESSAGE), ("zero", MESSAGE),
                           ("nested", MESSAGE), ("separate", MESSAGE), ("standalone", MESSAGE),
                           ("join", MESSAGE), ("standalone", ESCAPED)]:
        name = "error-string-" + ("escaping" if message == ESCAPED else shape)
        artifact, _ = submit(context, page, canvas, runtime(message, shape), name)
        current, program = projection(artifact)
        admitted, result_url = len(posts), publications[-1]
        payload, = program["payloads"]
        assert payload["message"] == message and bytes(payload["bytes"]) == abi_bytes(message), payload
        controls = [control for control in program["controls"] if control.get("guard")]
        assert len(controls) == {"standalone": 0, "nested": 2, "separate": 2}.get(shape, 1), controls
        assert len(program["constructions"]) == (2 if shape == "separate" else 1), program
        assert all(control["guard"]["payload"] == {"error_string": payload["id"]} for control in controls)
        assert program["constructions"] and program["reverts"], program
        high(page, canvas)
        text = painted_copy(page, canvas)
        assert len(guard_rows(text)) == len(controls), text
        assert all(line.startswith("require(") for line in guard_rows(text)), text
        retained = shape in {"standalone", "join"}
        assert ("evm.revert(offset =" in text) == retained and 'evm.revert("' not in text, text
        if shape in {"nonzero", "zero"}:
            assert controls[0]["guard"]["pass_nonzero"] == (shape == "nonzero")
            value = re.search(r"(?m)^(v\d+) = evm\.calldataload\(offset = 0x0\);$", text)
            assert value, text
            predicate = "(" + value[1] + (" != 0)" if shape == "nonzero" else " == 0)")
            assert guard_rows(text) == [f'require({predicate}, "{MESSAGE}");'], text
            assert "} else {" not in text, text
        assert "\x00" not in text, text
        for plan in program["constructions"]:
            if retained:
                construction_rows(page, canvas, artifact, current, program, plan, text)
            else:
                assert text.count("evm.mstore(") == 1, text  # Original pointer initialization.
                assert "evm.mload(" not in text, text
        if shape == "join":
            guard, = controls
            assert guard["guard"]["continuation"]["pass_falls_through"], guard
            assert text.index("require(") < text.index("evm.sstore(") < text.index("evm.revert(offset ="), text
        elif shape != "standalone":
            assert text.count("evm.sstore(") == 1 and text.rindex("require(") < text.index("evm.sstore("), text
            assert "evm.sstore(slot = 0x0, value = 0x2a);" in text.splitlines(), text
        options_invariance(page, canvas, program, text)
        if shape in {"nonzero", "standalone"} and message == MESSAGE:
            navigate_witnesses(page, canvas, artifact, current, program, text, shape == "standalone")
        retained = page.request.get(result_url)
        assert retained.ok and retained.json()["result"]["Ok"]["artifact"] == artifact
        assert len(posts) == admitted, posts
        canvas.screenshot(path=str(context.output / (name + "-review.png")))
        results.append({"shape": shape, "message": message, "artifact": artifact["id"],
                        "payload": payload, "capsules": program["constructions"],
                        "guards": len(controls), "code": text})

    # One publication must expose both choices together. The observation keeps
    # every original error-building primitive in the explicit failure arm.
    artifact, _ = submit(context, page, canvas, mixed_runtime(), "hybrid-require-explicit-invalid")
    current, program = projection(artifact)
    high(page, canvas)
    text = painted_copy(page, canvas)
    requirements = [line for line in guard_rows(text) if line.startswith("require(")]
    failures = [line for line in guard_rows(text) if line.startswith("if (")]
    assert len(requirements) == 1 and len(failures) == 2, text
    assert len([control for control in program["controls"] if control.get("guard")]) == 1
    assert text.count("evm.revert(offset =") == 1 and text.count("evm.invalid();") == 1, text
    assert "exception(" not in text and "} else {" not in text and "!(!(" not in text, text
    assert text.splitlines()[-2:] == ["evm.sstore(slot = 0x0, value = 0x2a);", "evm.stop();"], text
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    body = next(block for block in program["blocks"]
                if any(operations[statement["operation"]]["opcode"] == 0x33 for statement in block["statements"]))
    retained = [statement for statement in body["statements"]
                if operations[statement["operation"]]["opcode"] in {0x51, 0x52, 0x33, 0xfd}]
    assert len(retained) >= 7
    terminal = next(statement for block in program["blocks"] for statement in block["statements"]
                    if operations[statement["operation"]]["opcode"] == 0xfe)
    previous = -1
    for statement in retained + [terminal]:
        spelling = "evm." + operations[statement["operation"]]["name"].lower() + "("
        source_descendant(page, canvas, artifact, current["id"], statement["id"])
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        row = selected_row(page, "decompile-code", spelling)
        source = code_label(row["label"])[1]
        assert source.startswith("    "), (statement, text)
        previous = next(index for index in range(previous + 1, len(text.splitlines()))
                        if text.splitlines()[index] == source)
        inspector_control(page, canvas, "Statement h" + statement["id"] + " ", "decompile-entity-detail")
    control, = [control for control in program["controls"] if control.get("guard")]
    source_descendant(page, canvas, artifact, current["id"], control["id"])
    selected_row(page, "decompile-code", requirements[0])
    source_descendant(page, canvas, artifact, current["id"], control["guard"]["failure_node"])
    ui_control(page, "Structured code", role="tab", selected=True)
    raw = copy_code(page, canvas)
    assert "require(" not in raw and "evm.invalid();" in raw and raw.count("evm.revert(") >= 2, raw
    high(page, canvas)
    for dependencies, annotations in [(True, False), (False, True), (True, True), (False, False)]:
        set_control(page, canvas, DEPENDENCIES, dependencies)
        set_control(page, canvas, ANNOTATIONS, annotations)
        displayed = painted_copy(page, canvas)
        assert guard_rows(displayed) == guard_rows(text), displayed
        assert displayed.count("evm.mstore(") == text.count("evm.mstore("), displayed
        assert displayed.count("evm.invalid();") == 1, displayed
    assert painted_copy(page, canvas) == text
    canvas.screenshot(path=str(context.output / "hybrid-require-explicit-invalid-review.png"))
    results.append({"artifact": artifact["id"], "mixed": True, "code": text,
                    "require": control["id"], "explicit_statements": [statement["id"] for statement in retained],
                    "invalid_statement": terminal["id"]})

    for name, options in [("alias", {"alias": True}), ("padding", {"padding": True})]:
        artifact, _ = submit(context, page, canvas, runtime(**options), "error-string-rejected-" + name)
        current, program = projection(artifact)
        assert not program.get("payloads") and not program.get("reverts"), program
        data, = program["revert_data"]
        if name == "alias":
            assert data["content"] == "raw" and not program.get("constructions"), program
            assert not any(control.get("guard") for control in program["controls"])
        else:
            assert "segments" in data["content"], data
            assert all(plan["revert"] == data["id"] for plan in program.get("constructions", []))
            assert all(not control.get("guard") or control["guard"]["payload"] == {"data": data["id"]}
                       for control in program["controls"])
        high(page, canvas)
        for dependencies, annotations in [(False, False), (True, True), (False, False)]:
            set_control(page, canvas, DEPENDENCIES, dependencies)
            set_control(page, canvas, ANNOTATIONS, annotations)
            text = painted_copy(page, canvas)
            terminal = "evm.revert(offset =" if name == "alias" else "evm.revert(Revert(data = concat("
            assert "if (" in text and "evm.mstore(" in text and terminal in text, text
            assert 'evm.revert("' not in text and "require(" not in text, text
        results.append({"rejected": name, "artifact": artifact["id"]})
    gpu = webgpu_evidence(page)
    page.close()
    return {"cases": results, "renderer": gpu}
