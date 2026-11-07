"""Explicit failure branches through publications, painted code and source links."""

import re

from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_helpers import (choose_stage, inspector_control, observed,
                                       open_decompiler, stage, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import click_control, set_control, ui_control, webgpu_evidence


POLARITIES = [("5f35600a57602a5f55005b5f5ffd", False),
              ("5f356008575f5ffd5b602a5f5500", True)]
JOINED_REVERT = "5f35600857600d565b602a5f555b5f5ffd"
DEPENDENCIES = "Show state dependencies"
ANNOTATIONS = "Show structure annotations"


class Runtime:
    """Small label assembler, matching the native guard fixtures' real EVM."""

    def __init__(self):
        self.code, self.labels, self.jumps = bytearray(), {}, []

    def emit(self, code):
        self.code.extend(bytes.fromhex(code))

    def label(self, name):
        assert name not in self.labels
        self.labels[name] = len(self.code)
        self.emit("5b")

    def jump(self, name, conditional=True):
        self.emit("600057" if conditional else "600056")
        self.jumps.append((len(self.code) - 2, name))

    def finish(self):
        for position, name in self.jumps:
            self.code[position] = self.labels[name]
        return self.code.hex()


def decoder():
    # evm-decompile/tests/lifted/guards/fixtures.rs::decoder(false): signed
    # wrapping size check, canonical bool, dynamic offset/length, stores, CALL.
    runtime = Runtime()
    runtime.emit("60406004360312")
    runtime.jump("fail")
    runtime.emit("60043580151514")
    runtime.jump("bool-pass")
    runtime.jump("fail", False)
    runtime.label("bool-pass")
    runtime.emit("602435600401602081360312")
    runtime.jump("fail")
    runtime.emit("80356020820136038111")
    runtime.jump("fail")
    runtime.emit("5f55600155" + "5f" * 7 + "f15000")
    runtime.label("fail")
    runtime.emit("5f5ffd")
    return runtime.finish()


def rejected_failure(failure):
    # Native Web dependencies/guards.rs negative fixtures, with an explicit
    # successful STOP so no implicit-EOF exit is mistaken for a guard.
    runtime = Runtime()
    runtime.emit("5f35")
    runtime.jump("pass")
    runtime.emit(failure)
    runtime.label("pass")
    runtime.emit("00")
    return runtime.finish()


def painted_copy(page, canvas):
    text = copy_code(page, canvas)
    rows = [code_label(row["label"])[1] for row in observed(page, "decompile-code")]
    assert text.splitlines() == rows, (text, rows)
    return text


def high(page, canvas):
    click_control(page, canvas, "Simplified structured code", role="tab")
    ui_control(page, "Simplified structured code", role="tab", selected=True)


def projection(artifact):
    current = stage(artifact, "lifted")
    assert artifact["completion"] == "complete", artifact["diagnostics"]
    assert all(item["validation"]["status"] == "passed" for item in artifact["stages"])
    raw = lambda ir: {key: value for key, value in ir.items() if key != "lifted"}
    assert raw(current["ir"]) == raw(stage(artifact, "structured")["ir"])
    return current, current["ir"]["lifted"]


def guard_rows(text):
    return [line for line in text.splitlines() if line.lstrip().startswith("if (")]


def normal_join_witness(current, program):
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    expressions = {expression["id"]: expression for expression in program["expressions"]}
    stores = [(block, statement) for block in program["blocks"] for statement in block["statements"]
              if operations[statement["operation"]]["opcode"] == 0x55]
    (join, store), = stores
    parameter, = join["parameters"]
    assert expressions[store["inputs"][0]]["kind"] == {"word": "0x0"}
    assert expressions[store["inputs"][1]]["kind"] == {"value": parameter}
    edges = {edge["id"]: edge for edge in current["ir"]["edges"]}
    transfers = {transfer["edge"]: transfer for transfer in program["transfers"]}
    nodes = {node["id"]: node["kind"] for node in current["ir"]["structured"]["nodes"]}
    branch, = [kind["if"] for kind in nodes.values() if isinstance(kind, dict) and "if" in kind]
    arms = []
    for arm in [branch["then_branch"], branch["else_branch"]]:
        pending, incoming = [arm], []
        while pending:
            kind = nodes[pending.pop()]
            if "sequence" in kind:
                pending.extend(kind["sequence"])
            elif "transfer" in kind and edges[kind["transfer"]["edge"]]["to"] == join["block"]:
                incoming.append(transfers[kind["transfer"]["edge"]])
        transfer, = incoming
        argument, = transfer["arguments"]
        literal = expressions[argument]["kind"]["word"]
        arms.append((transfer, f"(v{parameter}) = ({literal});"))
    assert {expressions[transfer["arguments"][0]]["kind"]["word"] for transfer, _ in arms} == {"0x1", "0x2"}
    return parameter, store, arms


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    posts, publications, results = [], [], []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and "/api/" in request.url else None)
    page.on("response", lambda response: publications.append(response.url)
            if "/api/decompile/" in response.url and response.url.endswith("/result") else None)

    for bytecode, nonzero in POLARITIES:
        name = "guard-pass-" + ("nonzero" if nonzero else "zero")
        artifact, _ = submit(context, page, canvas, bytecode, name)
        current, program = projection(artifact)
        result_url, admitted = publications[-1], len(posts)
        controls = [control for control in program["controls"] if control.get("guard")]
        assert len(controls) == 1, controls
        control, = controls
        guard = control["guard"]
        assert guard["pass_nonzero"] == nonzero and guard["payload"] == "empty_revert", guard
        if nonzero:
            assert guard["continuation"] and not guard["continuation"]["pass_falls_through"], guard
        else:
            assert guard["continuation"] is None, guard
        high(page, canvas)
        text = painted_copy(page, canvas)
        loaded = re.search(r"(?m)^(v\d+) = evm\.calldataload\(offset = 0x0\);$", text)
        assert loaded, text
        predicate = "(" + loaded[1] + " != 0)"
        expected = f"if (!({predicate})) {{" if nonzero else f"if ({predicate}) {{"
        assert guard_rows(text) == [expected], text
        assert "require(" not in text and "    evm.revert(" in text and "} else {" not in text, text
        lines = text.splitlines()
        assert lines.index(expected) < lines.index("evm.sstore(slot = 0x0, value = 0x2a);")
        assert lines[-1] == "evm.stop();", text
        row = next(row for row in observed(page, "decompile-code")
                   if code_label(row["label"])[1] == expected)
        click_control(page, canvas, row["label"], role="decompile-code", id=row["id"])
        selected_row(page, "decompile-code", expected)
        detail = inspector_control(page, canvas, "Certified empty-REVERT guard h" + control["id"],
                                   "decompile-entity-detail")
        assert expected in detail["label"], detail
        polarity = inspector_control(page, canvas, "Predicate:", "decompile-entity-detail")
        assert "successful source truth: " + ("nonzero" if nonzero else "zero") in polarity["label"]
        edges = {edge["id"]: edge for edge in current["ir"]["edges"]}
        failure = edges[guard["failure_edge"]]["to"]
        rank = next(item["rank"] for item in program["failures"] if item["block"] == failure)
        proof = inspector_control(page, canvas, "Shared empty-failure proof:", "decompile-entity-detail")
        assert f"b{failure} · decreasing rank {rank}" in proof["label"], proof
        if guard["continuation"]:
            plan = inspector_control(page, canvas, "Failure continuation:", "decompile-entity-detail")
            assert "moved into explicit failed arm" in plan["label"], plan

        # Each bounded direct proof reference is an actual navigable entity.
        references = [control["node"], control["condition"], guard["block"],
                      guard["pass_edge"], guard["failure_edge"],
                      guard["pass_node"], guard["failure_node"]]
        if guard["continuation"]:
            references.append(guard["continuation"]["sequence"])
        for entity in references:
            source_descendant(page, canvas, artifact, current["id"], control["id"])
            selected_row(page, "decompile-code", expected)
            set_control(page, canvas, "Entity references", True, role="section")
            inspector_control(page, canvas, f"Reference s{current['id']}/e{entity} ", "decompile-entity-link")

        # The guard is a descendant of the actual JUMPI and opens its high row.
        block = next(block for block in current["ir"]["blocks"] if block["id"] == guard["block"])
        pc = next(op["source"]["pc"] for op in block["operations"] if op["opcode"] == 0x57)
        source_descendant(page, canvas, artifact, current["id"], control["id"], pc)
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        selected_row(page, "decompile-code", expected)

        for dependencies in [False, True]:
            for annotations in [False, True]:
                set_control(page, canvas, DEPENDENCIES, dependencies)
                set_control(page, canvas, ANNOTATIONS, annotations)
                detailed = painted_copy(page, canvas)
                assert guard_rows(detailed) == [expected], detailed
                assert "require(" not in detailed and "    evm.revert(" in detailed, detailed
                failure_tokens = [line for line in detailed.splitlines()
                                  if "token on e" + guard["failure_edge"] in line]
                assert len(failure_tokens) == 2 * int(dependencies), detailed
                assert all(line.startswith("    ") for line in failure_tokens), detailed
                if dependencies:
                    tokens = [i for i, line in enumerate(detailed.splitlines())
                              if "token on e" + guard["pass_edge"] in line]
                    assert len(tokens) == 2 and all(i > detailed.splitlines().index("}") for i in tokens)
        set_control(page, canvas, DEPENDENCIES, False)
        set_control(page, canvas, ANNOTATIONS, False)
        assert painted_copy(page, canvas) == text

        # The retained REVERT statement owns its explicit high terminal row.
        operations = {op["id"]: op for block in current["ir"]["blocks"] for op in block["operations"]}
        absorbed = next(statement for block in program["blocks"] for statement in block["statements"]
                        if operations[statement["operation"]]["opcode"] == 0xfd)
        source_descendant(page, canvas, artifact, current["id"], absorbed["id"])
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        selected_row(page, "decompile-code", "evm.revert(")
        source_descendant(page, canvas, artifact, current["id"], guard["failure_node"])
        ui_control(page, "Structured code", role="tab", selected=True)
        selected_row(page, "decompile-code", "")
        raw_text = copy_code(page, canvas)
        assert "if (" in raw_text and "evm.revert(" in raw_text and "require(" not in raw_text, raw_text
        choose_stage(page, canvas, artifact, "structured")
        click_control(page, canvas, "Structured code", role="tab")
        assert copy_code(page, canvas) == raw_text
        choose_stage(page, canvas, artifact, "lifted")
        high(page, canvas)
        assert painted_copy(page, canvas) == text
        retained = page.request.get(result_url)
        assert retained.ok and retained.json()["result"]["Ok"]["artifact"] == artifact
        assert len(posts) == admitted, posts
        canvas.screenshot(path=str(context.output / (name + "-navigation.png")))
        results.append({"artifact": artifact["id"], "guard": control["id"], "failed_if": expected,
                        "failure_rank": rank, "proof_references": references})

    artifact, _ = submit(context, page, canvas, decoder(), "guard-abi-decoder")
    current, program = projection(artifact)
    high(page, canvas)
    text = painted_copy(page, canvas)
    controls = [control for control in program["controls"] if control.get("guard")]
    assert len(controls) >= 4 and len(guard_rows(text)) == len(controls), text
    assert all(line.startswith("if (") for line in guard_rows(text)), text
    assert "require(" not in text and "evm.revert(" in text, text
    assert "evm.slt(" in text and " - 0x4)" in text, text
    assert text.count("evm.sstore(") == 2 and text.count("evm.call(") == 1, text
    assert text.rindex("if (") < text.index("evm.sstore(") < text.index("evm.call("), text
    # This is the native decoder's actual nested If/following-tail shape.
    nodes = {node["id"]: node["kind"] for node in current["ir"]["structured"]["nodes"]}
    assert sum("if" in kind for kind in nodes.values() if isinstance(kind, dict)) >= 4
    assert any(plan and not plan["pass_falls_through"]
               for plan in (control["guard"]["continuation"] for control in controls))
    for dependencies, annotations in [(True, False), (True, True), (False, True), (False, False)]:
        set_control(page, canvas, DEPENDENCIES, dependencies)
        set_control(page, canvas, ANNOTATIONS, annotations)
        detailed = copy_code(page, canvas)
        assert guard_rows(detailed) == guard_rows(text), detailed
        assert "require(" not in detailed and "evm.revert(" in detailed, detailed
    click_control(page, canvas, "Structured code", role="tab")
    raw = copy_code(page, canvas)
    assert raw.count("if (") >= 4 and "evm.revert(" in raw and "require(" not in raw, raw
    high(page, canvas)
    assert painted_copy(page, canvas) == text
    results.append({"artifact": artifact["id"], "decoder_guards": len(controls)})

    artifact, _ = submit(context, page, canvas, JOINED_REVERT, "guard-shared-joined-tail")
    current, program = projection(artifact)
    control, = [control for control in program["controls"] if control.get("guard")]
    assert control["guard"]["continuation"]["pass_falls_through"], control
    high(page, canvas)
    text = painted_copy(page, canvas)
    lines = text.splitlines()
    failed_if, = guard_rows(text)
    store = "    evm.sstore(slot = 0x0, value = 0x2a);"
    revert = "evm.revert(offset = 0x0, size = 0x0);"
    assert lines.index(failed_if) < lines.index(store) < lines.index(revert), text
    assert text.count("evm.revert(") == 1 and "require(" not in text, text
    row = next(row for row in observed(page, "decompile-code") if failed_if in row["label"])
    click_control(page, canvas, row["label"], role="decompile-code", id=row["id"])
    plan = inspector_control(page, canvas, "Failure continuation:", "decompile-entity-detail")
    assert "pass falls through: true · retained after pass" in plan["label"], plan
    set_control(page, canvas, DEPENDENCIES, True)
    detailed = painted_copy(page, canvas)
    assert guard_rows(detailed) == [failed_if] and detailed.count(revert) == 1, detailed
    assert detailed.index(failed_if) < detailed.index(store[:-1]) < detailed.index(revert[:-1]), detailed
    set_control(page, canvas, DEPENDENCIES, False)
    results.append({"artifact": artifact["id"], "retained_join_revert": True})

    for name, failure, operation in [("payload", "60015ffd", "size = 0x1"),
                                      ("store", "602a5f555f5ffd", "evm.sstore("),
                                      ("observation", "33505f5ffd", "evm.caller("),
                                      ("call-memory", "5f" * 7 + "f150604051602a905260205ffd", "evm.call(")]:
        artifact, _ = submit(context, page, canvas, rejected_failure(failure), "failure-with-" + name)
        current, program = projection(artifact)
        assert not any(control.get("guard") for control in program["controls"]), program
        high(page, canvas)
        for dependencies in [False, True]:
            set_control(page, canvas, DEPENDENCIES, dependencies)
            text = painted_copy(page, canvas)
            assert "require(" not in text and "if (" in text and "evm.revert(" in text, text
            assert operation in text and "} else {" not in text, text
            assert text.splitlines()[-1] == "evm.stop();", text
            assert any(line.startswith("    ") and operation in line for line in text.splitlines()), text
            if name != "payload":
                assert text.index(operation) < text.index("evm.revert("), text
            if name == "call-memory":
                assert text.index("evm.call(") < text.index("evm.mload(") < text.index("evm.mstore(") < text.index("evm.revert("), text
        set_control(page, canvas, DEPENDENCIES, False)
        results.append({"artifact": artifact["id"], "retained_failure_effect": name})

    # Exceptional halt retains INVALID identity and never becomes REVERT.
    artifact, _ = submit(context, page, canvas, rejected_failure("fe"), "failure-invalid")
    current, program = projection(artifact)
    high(page, canvas)
    invalid_text = painted_copy(page, canvas)
    assert len(guard_rows(invalid_text)) == 1 and "require(" not in invalid_text, invalid_text
    assert "exception(" in invalid_text and "evm.revert(" not in invalid_text, invalid_text
    assert "} else {" not in invalid_text and invalid_text.splitlines()[-1] == "evm.stop();", invalid_text
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    terminal = next(statement for block in program["blocks"] for statement in block["statements"]
                    if operations[statement["operation"]]["opcode"] == 0xfe)
    source_descendant(page, canvas, artifact, current["id"], terminal["id"])
    ui_control(page, "Simplified structured code", role="tab", selected=True)
    selected_row(page, "decompile-code", "exception(")
    results.append({"artifact": artifact["id"], "invalid_statement": terminal["id"]})

    # Two normal alternatives keep their live branch tuple and else.
    artifact, _ = submit(context, page, canvas,
                         "5f35600a576001600d565b60025b5f5500", "failure-normal-branch")
    current, program = projection(artifact)
    high(page, canvas)
    ordinary = painted_copy(page, canvas)
    assert "} else {" in ordinary, ordinary
    assert ordinary.count("evm.sstore(") == 1 and "evm.revert(" not in ordinary, ordinary
    parameter, store, arms = normal_join_witness(current, program)
    rows = [line.strip() for line in ordinary.splitlines()]
    separator = rows.index("} else {")
    stored = rows.index(f"evm.sstore(slot = 0x0, value = v{parameter});")
    assert rows.index(arms[0][1]) < separator < rows.index(arms[1][1]) < stored, ordinary
    for transfer, assignment in arms:
        assert rows.count(assignment) == 1, ordinary
        source_descendant(page, canvas, artifact, current["id"], transfer["id"])
        ui_control(page, "Simplified structured code", role="tab", selected=True)
        selected_row(page, "decompile-code", assignment)
    source_descendant(page, canvas, artifact, current["id"], store["id"])
    ui_control(page, "Simplified structured code", role="tab", selected=True)
    selected_row(page, "decompile-code", f"value = v{parameter}")
    assert painted_copy(page, canvas) == ordinary
    results.append({"artifact": artifact["id"], "ordinary_branch": True,
                    "join_parameter": parameter, "store_statement": store["id"],
                    "arm_transfers": [transfer["id"] for transfer, _ in arms]})

    gpu = webgpu_evidence(page)
    page.close()
    return {"cases": results, "renderer": gpu}
