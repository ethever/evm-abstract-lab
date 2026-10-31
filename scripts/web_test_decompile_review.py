"""Review regressions use real tasks, typed artifacts and painted occurrences."""

import time
from playwright.sync_api import expect
from web_test_decompile_helpers import (BRANCH, IRREDUCIBLE, LOOP, choose_stage,
                                       inspector_control, observed, open_decompiler,
                                       prepare, stage, submit)
from web_test_harness import (click_control, edit_control, focus_text_control,
                              open_input, rendered_frame, resize_view, ui_control,
                              set_control,
                              webgpu_evidence)


def selected_row(page, role, needle):
    rows = [row for row in observed(page, role) if row["selected"] and needle in row["label"]]
    assert rows, (role, needle, observed(page, role))
    height = page.viewport_size["height"]
    assert all(0 <= row["rect"][1] < row["rect"][3] <= height for row in rows)
    return rows[0]


def source_descendant(page, canvas, artifact, at_stage, entity, pc=None):
    origin = next(origin for origin in artifact["mappings"]["origins"]
                  if origin["entity"] == {"stage": at_stage, "entity": entity})
    assert origin["source"], origin
    pc = origin["source"][0]["pc"] if pc is None else pc
    assert any(span["pc"] <= pc < span["end_pc"] for span in origin["source"])
    click_control(page, canvas, "Original bytecode", role="tab")
    edit_control(page, canvas, "Original PC", f"0x{pc:x}")
    click_control(page, canvas, "Go to original PC", role="action")
    selected_row(page, "decompile-source", f"Source 0x{pc:04x}:")
    inspector_control(page, canvas, f"Source descendant s{at_stage}/e{entity} ", "decompile-entity-link")
    expect(page.locator("#analysis-status")).to_contain_text(f"{artifact['id']}/stage:{at_stage}/entity:{entity}")


def recovery(context):
    page, canvas, _, _ = open_decompiler(context)
    artifacts = []
    for name, draft, expected in [
        ("prefix-lower", "0x600160020100", "0x600160020100"),
        ("prefix-upper-space", " \n0X60 01\t60 02 01 00 ", "0x600160020100"),
        ("unprefixed-space", "60 01\n60 02 01 00", "600160020100"),
    ]:
        artifact, request = submit(context, page, canvas, draft, name)
        assert request["bytecode"] == expected and artifact["input"]["runtime_bytecode"] == list(bytes.fromhex("600160020100"))
        click_control(page, canvas, "Edit bytecode", role="action")
        ui_control(page, draft, role="bytecode-value")
        artifacts.append(artifact["id"])
    for name, code in [("mixed-targets", "5f35600a576001600d565b600f5b565b00"),
                       ("known-targets", "5f35600a576011600d565b600f5b565b005b00"),
                       ("unknown-target", "5f3556"),
                       ("jumpi-falloff", "5b5f35600057"),
                       ("fault-add", "01"), ("fault-sstore", "55")]:
        artifact, _ = submit(context, page, canvas, code, name)
        click_control(page, canvas, "Structured code", role="tab")
        code_rows = observed(page, "decompile-code")
        text = "\n".join(row["label"] for row in code_rows)
        if name in {"mixed-targets", "known-targets"}:
            structured = stage(artifact, "structured")
            node = next(node for node in structured["ir"]["structured"]["nodes"] if "indirect" in node["kind"])
            block = next(block for block in structured["ir"]["blocks"] if block["id"] == node["kind"]["indirect"]["block"])
            jump_pc = next(op["source"]["pc"] for op in block["operations"] if op["opcode"] == 0x56)
            source_descendant(page, canvas, artifact, structured["id"], node["id"], jump_pc)
            selected_row(page, "decompile-code", "indirect_jump")
            # Position the actual indirect region in the virtual code pane
            # before checking its complete alternatives and exceptional exit.
            text = "\n".join(row["label"] for row in observed(page, "decompile-code"))
            assert "target 0xf:" in text and 'otherwise: exception("invalid jump destination")' in text, text
            click_control(page, canvas, "Original bytecode", role="tab")
            edit_control(page, canvas, "Original PC", f"0x{jump_pc:x}")
            click_control(page, canvas, "Go to original PC", role="action")
            selected_row(page, "decompile-source", "JUMP ")
        elif name == "unknown-target":
            assert "partial" in text or "unresolved_target" in text, text
        elif name == "jumpi-falloff":
            assert "end of code" in text and "else end" not in text, text
        else:
            mnemonic = "ADD" if name == "fault-add" else "SSTORE"
            assert "fault o" in text and mnemonic in text and "source 0x0000" in text, text
            assert "evm.add(" not in text and "evm.sstore(" not in text, text
            fault = next(row for row in code_rows if "fault o" in row["label"])
            click_control(page, canvas, fault["label"], role="decompile-code", id=fault["id"])
            detail = inspector_control(page, canvas, "Raw IR o", "decompile-entity-detail")
            assert "fault o" in detail["label"] and "not completed" in detail["label"] and "raw opcode" in detail["label"]
            assert "evm.add(" not in detail["label"] and "evm.sstore(" not in detail["label"]
            click_control(page, canvas, "IR", role="tab")
            selected_row(page, "decompile-ir", "fault o")
            click_control(page, canvas, "Graphs", role="tab")
            plot = ui_control(page, "Current CFG", role="graph-canvas")["rect"]
            preview = [row for row in observed(page, "graph-preview")
                       if "fault o" in row["label"] and "not completed" in row["label"]
                       and plot[0] <= row["rect"][0] < row["rect"][2] <= plot[2]
                       and plot[1] <= row["rect"][1] < row["rect"][3] <= plot[3]]
            assert preview, "current graph did not paint the fault occurrence"
            canvas.screenshot(path=str(context.output / (name + "-graph-ir-inspector-fault.png")))
            inspector_control(page, canvas, "Original ", "decompile-origin")
            selected_row(page, "decompile-source", mnemonic)
        canvas.screenshot(path=str(context.output / f"{name}-semantic-content.png"))
        artifacts.append(artifact["id"])
    # A real recovery context copy that faults after one entry supplied too
    # few stack values; the retained fault identity must resolve to source.
    copied, _ = submit(context, page, canvas, "5f35600a5760096012565b600260036012565b015f5500", "faulted-context-copy")
    current = stage(copied, "structured")
    fault_block = next(block for block in current["ir"]["blocks"] if "trap" in block["terminator"])
    fault = fault_block["operations"][-1]
    assert fault["name"] == "ADD" and fault["source"]["pc"] == 19
    source_descendant(page, canvas, copied, current["id"], fault["id"])
    selected_row(page, "decompile-ir", "o" + fault["id"] + ":")
    click_control(page, canvas, "Structured code", role="tab")
    inspector_control(page, canvas, f"Source descendant s{current['id']}/e{fault['id']} ", "decompile-entity-link")
    selected_row(page, "decompile-code", "fault o" + fault["id"] + " ADD at source 0x0013")
    canvas.screenshot(path=str(context.output / "faulted-copy-source-attribution.png"))
    prepare(page, canvas, "0155")
    set_control(page, canvas, "Decompile budgets", True, role="section")
    number = ui_control(page, "Recovery iteration limit", role="number")
    left, top, right, bottom = number["rect"]
    page.mouse.dblclick((left + right) / 2, (top + bottom) / 2)
    page.wait_for_function("() => document.activeElement === document.querySelector('#evm-canvas')?.nextElementSibling")
    page.keyboard.press("Control+a")
    page.keyboard.insert_text("0")
    page.keyboard.press("Enter")
    rendered_frame(page)
    limited, request = submit(context, page, canvas, "0155", "decoded-only-limited")
    assert request["limits"]["max_iterations"] == 0
    assert limited["completion"] == "limited" and [item["kind"] for item in limited["stages"]] == ["decoded"]
    click_control(page, canvas, "Structured code", role="tab")
    assert observed(page, "decompile-recovery-unavailable")
    assert not observed(page, "decompile-code")
    assert any("ADD 01" in row["label"] for row in observed(page, "decompile-source"))
    click_control(page, canvas, "IR", role="tab")
    assert all(" = " not in row["label"] for row in observed(page, "decompile-ir"))
    canvas.screenshot(path=str(context.output / "decoded-only-source-and-ir.png"))
    artifacts.extend([copied["id"], limited["id"]])
    page.close()
    return {"artifacts": artifacts}


def navigation(context):
    page, canvas, _, _ = open_decompiler(context)
    for name, code, variant, needle in [("if", BRANCH, "if", "if ("),
                                        ("while", LOOP, "while", "while ("),
                                        ("dispatcher", IRREDUCIBLE, "dispatcher", "dispatch_state")]:
        artifact, _ = submit(context, page, canvas, code, "typed-" + name)
        current = stage(artifact, "structured")
        program = current["ir"]["structured"]
        node = next(node for node in program["nodes"] if variant in node["kind"])
        source_descendant(page, canvas, artifact, current["id"], node["id"])
        selected_row(page, "decompile-code", needle)
        if name == "dispatcher":
            transition = next(transition for case in node["kind"]["dispatcher"]["cases"] for transition in case["transitions"])
            source_descendant(page, canvas, artifact, current["id"], transition["id"])
            selected_row(page, "decompile-code", "on e" + transition["edge"])
            inspector_control(page, canvas, "Transition t", "decompile-entity-detail")
        else:
            inspector_control(page, canvas, name.title() + " ast", "decompile-entity-detail")
            if name == "if":
                sequence = next(node for node in program["nodes"] if "sequence" in node["kind"])
                source_descendant(page, canvas, artifact, current["id"], sequence["id"])
                selected_row(page, "decompile-code", "sequence ast" + sequence["id"])
        canvas.screenshot(path=str(context.output / f"typed-{name}-selected.png"))

    # Same effectful A/B cycle and common end as core's irreducible fixture.
    # The join at PC48 is outside the residual dispatch case set.
    common_end = "5f3561001b575b5f54600101805f556004901061001b57610030565b5f54600101805f556004901061000657610030565b00"
    escaping, _ = submit(context, page, canvas, common_end, "dispatcher-common-end")
    current = stage(escaping, "structured")
    candidate = next((node, transition) for node in current["ir"]["structured"]["nodes"]
                     if "dispatcher" in node["kind"]
                     for case in node["kind"]["dispatcher"]["cases"]
                     for transition in case["transitions"]
                     if transition["target"] not in {case["block"] for case in node["kind"]["dispatcher"]["cases"]})
    dispatcher, external = candidate
    target = next(block for block in current["ir"]["blocks"] if block["id"] == external["target"])
    assert target["source"]["pc"] == 48
    source_descendant(page, canvas, escaping, current["id"], external["id"])
    selected_row(page, "decompile-code", f"= b{external['target']}; break 'dispatch{dispatcher['id']};")
    selected_row(page, "decompile-code", "memory token on e" + external["edge"])
    selected_row(page, "decompile-code", "effect token on e" + external["edge"])
    page.context.grant_permissions(["clipboard-read", "clipboard-write"])
    click_control(page, canvas, "Copy structured text", role="action")
    page.wait_for_function("() => navigator.clipboard.readText().then(text => text.includes(\"break 'dispatch\"))")
    copied_text = page.evaluate("navigator.clipboard.readText()")
    assert "if (" in copied_text and "match v" in copied_text and "is active" not in copied_text
    assert copied_text.rfind("evm.stop();") > copied_text.find(f"break 'dispatch{dispatcher['id']}")
    canvas.screenshot(path=str(context.output / "dispatcher-common-end-scoped-exit.png"))

    code = "".join(f"60{index % 256:02x}50" for index in range(1300)) + "00"
    artifact, _ = submit(context, page, canvas, code, "large-push-stream")
    baseline = stage(artifact, "baseline")
    pc = 3 * 1297
    operation = next(op for block in baseline["ir"]["blocks"] for op in block["operations"] if op["source"] and op["source"]["pc"] == pc)
    value = operation["outputs"][0]
    timings = []
    for width, height in [(1920, 1080), (900, 620)]:
        started = time.monotonic()
        resize_view(page, width, height)
        source_descendant(page, canvas, artifact, baseline["id"], operation["id"])
        selected_row(page, "decompile-ir", "o" + operation["id"] + ":")
        click_control(page, canvas, "Graphs", role="tab")
        # The origin's source-descendant list is still qualified by the late PC.
        inspector_control(page, canvas, f"Source descendant s{baseline['id']}/e{value} ", "decompile-entity-link")
        selected_row(page, "decompile-ir", "v" + value + ":")
        click_control(page, canvas, "Back", role="action")
        selected_row(page, "decompile-ir", "o" + operation["id"] + ":")
        click_control(page, canvas, "Forward", role="action")
        selected_row(page, "decompile-ir", "v" + value + ":")
        choose_stage(page, canvas, artifact, "structured")
        click_control(page, canvas, "Structured code", role="tab")
        final = stage(artifact, "structured")
        op = next(op for block in final["ir"]["blocks"] for op in block["operations"] if op["source"] and op["source"]["pc"] == pc)
        inspector_control(page, canvas, f"Source descendant s{final['id']}/e{op['id']} ", "decompile-entity-link")
        selected_row(page, "decompile-code", "v" + op["outputs"][0] + " =")
        canvas.screenshot(path=str(context.output / f"late-code-ir-{width}.png"))
        timings.append({"width": width, "navigation_seconds": time.monotonic() - started})
    click_control(page, canvas, "Graphs", role="tab")
    choose_stage(page, canvas, artifact, "baseline")
    ui_control(page, "Current CFG", role="graph-canvas")
    canvas.screenshot(path=str(context.output / "large-bounded-graph-preview.png"))
    result = {"large_artifact": artifact["id"], "dispatcher_escape": escaping["id"], "late_pc": pc, "timings": timings, "renderer": webgpu_evidence(page)}
    page.close()
    return result


def editing(context):
    page, canvas, _, _ = open_decompiler(context)
    requests = []
    page.on("request", lambda request: requests.append(request) if request.method == "POST" and "/api/" in request.url else None)

    def draft(expected):
        ui_control(page, expected, role="bytecode-value")

    def checkpoint():
        # egui's pinned Undoer uses a one-second stable-text checkpoint. This
        # wait exercises that documented edit debounce, not paint readiness.
        page.wait_for_timeout(1100)
        page.keyboard.press("End")
        rendered_frame(page)

    a, b = "600160020100", "600360040100"
    click_control(page, canvas, "Analysis", role="tab")
    open_input(page, mode="Bytecode")
    edit_control(page, canvas, "Runtime bytecode", a)
    checkpoint()
    page.keyboard.insert_text("00")
    rendered_frame(page)
    draft(a + "00")
    checkpoint()
    page.keyboard.press("Escape")
    click_control(page, canvas, "Decompiler", role="tab")
    prepare(page, canvas, b)
    focus_text_control(page, canvas, "Runtime bytecode")
    checkpoint()
    page.keyboard.insert_text("00")
    rendered_frame(page)
    draft(b + "00")
    checkpoint()
    page.keyboard.press("Control+z")
    rendered_frame(page)
    draft(b)
    page.keyboard.press("Control+y")
    rendered_frame(page)
    draft(b + "00")
    click_control(page, canvas, "Hide input", role="action")
    click_control(page, canvas, "Edit bytecode", role="action")
    draft(b + "00")
    click_control(page, canvas, "Analysis", role="tab")
    open_input(page, mode="Bytecode")
    # open_input already establishes focus; avoid another selection-changing
    # activation before the exact single Undo/Redo assertions.
    page.keyboard.press("Control+z")
    rendered_frame(page)
    draft(a)
    page.keyboard.press("Control+y")
    rendered_frame(page)
    draft(a + "00")
    page.keyboard.press("Escape")
    click_control(page, canvas, "Decompiler", role="tab")
    draft(b + "00")
    focus_text_control(page, canvas, "Runtime bytecode")
    page.keyboard.press("Control+a")
    rendered_frame(page)
    session = page.context.new_cdp_session(page)
    session.send("Input.imeSetComposition", {"text": b, "selectionStart": len(b), "selectionEnd": len(b)})
    rendered_frame(page)
    page.keyboard.press("Control+Enter")
    rendered_frame(page)
    assert not requests, "composition submitted a task"
    click_control(page, canvas, "Hide input", role="action")
    click_control(page, canvas, "Analysis", role="tab")
    open_input(page, mode="Bytecode")
    draft(a + "00")
    session.send("Input.imeSetComposition", {"text": "", "selectionStart": 0, "selectionEnd": 0})
    session.detach()
    page.keyboard.press("Escape")
    click_control(page, canvas, "Decompiler", role="tab")
    prepare(page, canvas, b)
    with page.expect_response(lambda response: response.request.method == "POST" and response.url.endswith("/api/decompile/tasks")) as admission:
        page.keyboard.down("Control")
        page.keyboard.down("Enter")
        page.keyboard.down("Enter")
        page.keyboard.up("Enter")
        page.keyboard.up("Control")
    assert admission.value.request.post_data_json["bytecode"] == b
    expect(page.locator("#analysis-status")).to_contain_text("Decompile task: Ready", timeout=60000)
    assert len(requests) == 1, "held submit chord or workspace switch submitted another task"
    page.keyboard.press("Control+j")
    expect(page.locator("#analysis-status")).to_contain_text("Transform inspector: closed")
    click_control(page, canvas, "Analysis", role="tab")
    click_control(page, canvas, "Decompile captured source", role="action")
    report = page.decompile_analysis_report
    graph = report["native"] or report["global"]
    original = next(program for program in report["programs"] if program["id"] == graph["metadata"]["root_program"])["bytecode"]
    draft(original)
    focus_text_control(page, canvas, "Runtime bytecode")
    page.keyboard.press("Control+z")
    rendered_frame(page)
    draft(original)
    canvas.screenshot(path=str(context.output / "isolated-edit-history-and-source-replacement.png"))
    result = {"analysis_draft": a + "00", "decompiler_draft": b, "captured": original, "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context, scenario):
    return {"recovery": recovery, "navigation": navigation, "editing": editing}[scenario](context)
