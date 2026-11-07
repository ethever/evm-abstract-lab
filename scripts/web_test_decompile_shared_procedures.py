"""Certified procedures through real HTTP, Wasm painting and exact source IDs."""

import json
import re

from playwright.sync_api import expect
from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_helpers import (choose_stage, inspector_control, observed,
                                       open_decompiler, prepare, stage, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, rendered_frame, resize_view,
                              set_control, ui_control, webgpu_evidence)
from web_test_registry import SOURCE_ROOT


# Matches the independent native Web fixture, with two distinct continuations.
SHARED = ("60075f35601e565b6001556002556016602035601e565b60035560045500"
          "5b80604052805f5560206040a0600554600101600555600654600755603d565b806001019156")
UNSHARED = "5f356040525f546001015f5500"
DEPENDENCIES = "Show state dependencies"
ANNOTATIONS = "Show structure annotations"
STRUCTURE = re.compile(r"^\s*// (?:sequence ast|b\d+\b|block b\d+\b|on e\d+\b)")


def painted_copy(page, canvas):
    text = copy_code(page, canvas)
    needed = 900 + 28 * len(text.splitlines())
    if page.viewport_size["height"] < needed <= 3800:
        resize_view(page, 1920, needed)
    rows = [code_label(row["label"])[1] for row in observed(page, "decompile-code")]
    assert text.splitlines() == rows, "the complete fixture must fit the bounded viewport"
    return text


def high(page, canvas):
    click_control(page, canvas, "Simplified structured code", role="tab")
    ui_control(page, "Simplified structured code", role="tab", selected=True)


def raw_witness(artifact):
    current, source = stage(artifact, "lifted"), stage(artifact, "structured")
    raw = lambda ir: {name: value for name, value in ir.items() if name != "lifted"}
    assert raw(current["ir"]) == raw(source["ir"]), "sharing changed raw IR or Structured"
    assert all(item["validation"]["status"] == "passed" for item in artifact["stages"])
    return current, current["ir"]["lifted"]


def without_structure(text):
    return [line for line in text.splitlines() if not STRUCTURE.match(line)]


def without_state(text, current):
    blocks = {block["id"]: block for block in current["ir"]["blocks"]}
    tokens = set()
    for edge in current["ir"]["edges"]:
        target = blocks[edge["to"]]
        tokens.add(f"m{target['memory_parameter']} = m{edge['memory']}; // memory token on e{edge['id']}")
        tokens.add(f"q{target['effect_parameter']} = q{edge['effect']}; // effect token on e{edge['id']}")
    return [re.split(r" // (?:memory|effect)", line, maxsplit=1)[0]
            for line in text.splitlines() if line.lstrip() not in tokens]


def actual_statement_pair(current, program):
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    statements = {statement["operation"]: statement for block in program["blocks"]
                  for statement in block["statements"]}
    pairs = [(statements[binding["formal"]], statements[binding["actual"]])
             for invocation in program["invocations"] for binding in invocation["bindings"]
             if binding["formal"] != binding["actual"]
             and binding["formal"] in statements and binding["actual"] in statements
             and operations[binding["formal"]]["opcode"] == 0x55]
    assert pairs, "positive fixture must exercise a noncanonical caller's store"
    return pairs[0]


def definition_edges(current, program):
    nodes = {node["id"]: node["kind"] for node in current["ir"]["structured"]["nodes"]}
    result = set()
    for procedure in program["procedures"]:
        pending, seen, blocks, edges = [procedure["body"]], set(), set(), set()
        while pending:
            identity = pending.pop()
            assert identity not in seen, "fixture source must be a finite tree"
            seen.add(identity)
            kind = nodes[identity]
            if not isinstance(kind, dict):
                continue
            if "sequence" in kind:
                pending.extend(kind["sequence"])
            elif "block" in kind:
                blocks.add(kind["block"]["block"])
            elif "transfer" in kind:
                edges.add(kind["transfer"]["edge"])
            elif "if" in kind:
                pending.extend([kind["if"]["then_branch"], kind["if"]["else_branch"]])
            elif "while" in kind:
                pending.extend([kind["while"]["header"], kind["while"]["body"]])
                if kind["while"]["exit_edge"] is not None:
                    edges.add(kind["while"]["exit_edge"])
        if len(blocks) >= 2:
            result.update(edges)
    assert len(result) >= 2, "fixture must share real multi-block control with state transfers"
    return result


def corrupt_publication(context, page, canvas, retained, retained_text, mutation):
    delivered = []

    def corrupt(route):
        response = route.fetch()
        envelope = response.json()
        artifact = envelope["result"]["Ok"]["artifact"]
        current = stage(artifact, "lifted")
        program = current["ir"]["lifted"]
        assert program["procedures"] and len(program["invocations"]) >= 2
        missing = "18446744073709551615"
        if mutation == "body":
            program["procedures"][0]["body"] = missing
        else:
            program["invocations"][0]["continuation"] = missing
        delivered.append(envelope)
        (context.output / ("corrupt-" + mutation + "-publication.json")).write_text(
            json.dumps(envelope, indent=2) + "\n")
        headers = {key: value for key, value in response.headers.items()
                   if key.lower() not in {"content-encoding", "content-length"}}
        route.fulfill(status=response.status, headers=headers, body=json.dumps(envelope))

    pattern = "**/api/decompile/tasks/*/result"
    page.route(pattern, corrupt)
    try:
        prepare(page, canvas, SHARED)
        with page.expect_response(lambda response: response.request.method == "GET"
                                  and "/api/decompile/tasks/" in response.url
                                  and response.url.endswith("/result")):
            click_control(page, canvas, "Decompile", role="action")
        expect(page.locator("#analysis-status")).to_contain_text("Invalid decompiler artifact:")
        expect(page.locator("#analysis-status")).to_contain_text("shared procedure sources")
        expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + retained["id"])
        assert len(delivered) == 1, "a rejected immutable publication must not retry forever"
        high(page, canvas)
        assert painted_copy(page, canvas) == retained_text
    finally:
        page.unroute(pattern, corrupt)


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    # Native canvas rows remain virtualized. This compact fixture fits even
    # annotated mode, permitting full row equality without repeated scrolling.
    resize_view(page, 1920, 2300)
    artifact, _ = submit(context, page, canvas, SHARED, "shared-procedures-positive")
    assert artifact["completion"] == "complete", artifact["diagnostics"]
    current, program = raw_witness(artifact)
    assert program["procedures"], "sharing is mandatory for this fixture"
    assert len(program["invocations"]) >= 2, "the helper has two distinct callers"
    assert any(binding["formal"] != binding["actual"]
               for call in program["invocations"] for binding in call["bindings"])
    assert ui_control(page, "Simplified structured code", role="tab")["selected"]
    ui_control(page, DEPENDENCIES, role="toggle", selected=False)
    ui_control(page, ANNOTATIONS, role="toggle", selected=False)
    snapshots = {}
    for dependencies, annotations in [(False, False), (True, False), (True, True), (False, True), (False, False)]:
        set_control(page, canvas, DEPENDENCIES, dependencies)
        set_control(page, canvas, ANNOTATIONS, annotations)
        text = painted_copy(page, canvas)
        key = (dependencies, annotations)
        assert key not in snapshots or snapshots[key] == text
        snapshots[key] = text
    for dependencies in [False, True]:
        assert without_structure(snapshots[dependencies, True]) == snapshots[dependencies, False].splitlines()
    for annotations in [False, True]:
        assert without_state(snapshots[True, annotations], current) == snapshots[False, annotations].splitlines()
    reading = snapshots[False, False]
    assert reading.count("log0(") == 1 and reading.count("mstore(") == 1, reading
    (context.output / "shared-toggle-text.json").write_text(json.dumps(
        {f"dependencies={dependencies},annotations={annotations}": text
         for (dependencies, annotations), text in snapshots.items()}, indent=2) + "\n")

    # Source samples cover definitions/calls and both original caller contexts.
    for entity, prefix in [
        (program["procedures"][0]["id"], "Certified shared procedure p"),
        (program["invocations"][-1]["id"], "Certified shared invocation h"),
    ]:
        source_descendant(page, canvas, artifact, current["id"], entity)
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        selected_row(page, "decompile-code", "")
        inspector_control(page, canvas, prefix + entity, "decompile-entity-detail")
        if entity == program["invocations"][-1]["id"]:
            bindings = program["invocations"][-1]["bindings"]
            assert bindings, "noncanonical caller must expose actual source substitutions"
            set_control(page, canvas, "Procedure source bindings", True, role="section")
            first = bindings[0]
            ui_control(page, f"e{first['formal']} → e{first['actual']}", role="decompile-procedure-binding")
    canonical, noncanonical = actual_statement_pair(current, program)
    source_descendant(page, canvas, artifact, current["id"], noncanonical["id"])
    ui_control(page, "IR", role="tab", selected=True)
    selected_row(page, "decompile-ir", "Statement h" + noncanonical["id"])
    inspector_control(page, canvas, "Statement h" + noncanonical["id"], "decompile-entity-detail")
    set_control(page, canvas, "Entity references", True, role="section")
    inspector_control(page, canvas,
                      f"Reference s{current['id']}/e{noncanonical['operation']} ", "decompile-entity-link")
    selected_row(page, "decompile-ir", "o" + noncanonical["operation"] + ":")
    inspector_control(page, canvas, "Raw IR o" + noncanonical["operation"], "decompile-entity-detail")
    source_descendant(page, canvas, artifact, current["id"], canonical["operation"])
    ui_control(page, "IR", role="tab", selected=True)
    selected_row(page, "decompile-ir", "o" + canonical["operation"] + ":")
    inspector_control(page, canvas, "Raw IR o" + canonical["operation"], "decompile-entity-detail")
    assert not observed(page, "action", "Copy structured text"), "IR does not expose a structured copy action"

    choose_stage(page, canvas, artifact, "structured")
    click_control(page, canvas, "Structured code", role="tab")
    raw_text = painted_copy(page, canvas)
    assert raw_text.count("evm.sstore(") >= 6 and raw_text.count("evm.log0(") == 2, raw_text
    choose_stage(page, canvas, artifact, "lifted")
    high(page, canvas)
    resize_view(page, 1920, 2300)
    assert painted_copy(page, canvas) == reading
    high_bytes, raw_bytes = len(reading.encode("utf-8")), len(raw_text.encode("utf-8"))
    assert high_bytes < raw_bytes, "positive sharing must reduce the rendered helper witness"
    canvas.screenshot(path=str(context.output / "shared-procedures-source-roundtrip.png"))

    # Corrupt source and return identities are rejected at the real wire boundary.
    for mutation in ["body", "continuation"]:
        corrupt_publication(context, page, canvas, artifact, reading, mutation)

    loop_code = (SOURCE_ROOT / "crates/evm-abstract-web/tests/fixtures/decompile/shared-loop.hex").read_text().strip()
    loop_artifact, _ = submit(context, page, canvas, loop_code, "shared-procedures-loop-state")
    loop_current, loop_program = raw_witness(loop_artifact)
    assert loop_program["procedures"] and len(loop_program["invocations"]) >= 2
    edges = definition_edges(loop_current, loop_program)
    high(page, canvas)
    loop_reading = painted_copy(page, canvas)
    set_control(page, canvas, DEPENDENCIES, True)
    loop_detailed = painted_copy(page, canvas)
    assert without_state(loop_detailed, loop_current) == loop_reading.splitlines()
    blocks = {block["id"]: block for block in loop_current["ir"]["blocks"]}
    state_tokens = []
    for edge in loop_current["ir"]["edges"]:
        if edge["id"] not in edges:
            continue
        target = blocks[edge["to"]]
        state_tokens.extend([
            f"m{target['memory_parameter']} = m{edge['memory']}; // memory token on e{edge['id']}",
            f"q{target['effect_parameter']} = q{edge['effect']}; // effect token on e{edge['id']}",
        ])
    assert len(state_tokens) >= 4
    assert all(token in [row.strip() for row in loop_detailed.splitlines()] for token in state_tokens)
    assert all(token not in [row.strip() for row in loop_reading.splitlines()] for token in state_tokens)
    set_control(page, canvas, DEPENDENCIES, False)
    assert painted_copy(page, canvas) == loop_reading
    canvas.screenshot(path=str(context.output / "shared-procedures-loop-state.png"))

    rejected, _ = submit(context, page, canvas, UNSHARED, "shared-procedures-no-repeated-region")
    _, rejected_program = raw_witness(rejected)
    assert rejected_program.get("procedures", []) == [] and rejected_program.get("invocations", []) == []
    text = painted_copy(page, canvas)
    assert text.count("mstore(") == text.count("sload(") == text.count("sstore(") == 1, text
    rendered_frame(page)
    gpu = webgpu_evidence(page)
    page.close()
    return {"artifact": artifact["id"], "procedures": len(program["procedures"]),
            "invocations": len(program["invocations"]), "raw_bytes": raw_bytes,
            "high_bytes": high_bytes, "loop_artifact": loop_artifact["id"],
            "definition_state_rows": len(state_tokens),
            "rejected_artifact": rejected["id"], "webgpu": gpu}
