"""Bound SSA word RHS over actual publications, painted rows and source links."""

from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_helpers import (inspector_control, observed,
                                       open_decompiler, stage, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, resize_view, set_control,
                              ui_control, webgpu_evidence)


MASK160 = (1 << 160) - 1
CROSS = ("600a5473ffffffffffffffffffffffffffffffffffffffff1673"
         "ffffffffffffffffffffffffffffffffffffffff16610033565b5f5500")
DEPENDENCIES = "Show state dependencies"
ANNOTATIONS = "Show structure annotations"


class Runtime:
    def __init__(self):
        self.code, self.labels, self.jumps = bytearray(), {}, []

    def emit(self, code):
        self.code.extend(bytes.fromhex(code))

    def label(self, name):
        assert name not in self.labels
        self.labels[name] = len(self.code)
        self.emit("5b")

    def address(self, name):
        self.emit("6000")
        self.jumps.append((len(self.code) - 1, name))

    def jump(self, name):
        self.address(name)
        self.emit("56")

    def mask(self, value):
        width = 20 if value <= MASK160 else 32
        self.code.extend(bytes([0x5f + width]) + value.to_bytes(width, "big"))

    def finish(self):
        for position, name in self.jumps:
            target = self.labels[name]
            assert target < 256
            self.code[position] = target
        return self.code.hex()


def cross(left, right, memory=False):
    if not memory and left == right == MASK160:
        return CROSS
    code = Runtime()
    code.emit("5f355f525f51" if memory else "600a54")
    code.mask(left)
    code.emit("16")
    code.mask(right)
    code.emit("16")
    code.jump("store")
    code.label("store")
    code.emit("5f5500")
    return code.finish()


def shared_mask(left, right):
    code = Runtime()
    code.address("first-return")
    code.emit("5f35")
    code.jump("helper")
    code.label("first-return")
    code.emit("600155")
    code.address("second-return")
    code.emit("602035")
    code.jump("helper")
    code.label("second-return")
    code.emit("60025500")
    code.label("helper")
    code.emit("54")
    code.mask(left)
    code.emit("16")
    code.mask(right)
    code.emit("16")
    code.emit("805f525f5150805f5560205fa0")
    code.jump("return-value")
    code.label("return-value")
    code.emit("6001019056")
    return code.finish()


def publication(artifact):
    assert artifact["completion"] == "complete", artifact["diagnostics"]
    assert all(value["validation"]["status"] == "passed" for value in artifact["stages"])
    current, raw = stage(artifact, "lifted"), stage(artifact, "structured")
    primitive = lambda ir: {key: value for key, value in ir.items() if key != "lifted"}
    assert primitive(current["ir"]) == primitive(raw["ir"])
    program = current["ir"]["lifted"]
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    statements = {statement["id"]: statement for block in program["blocks"]
                  for statement in block["statements"]}
    return current, program, operations, statements


def painted_copy(page, canvas):
    text = copy_code(page, canvas)
    rows = [code_label(row["label"])[1] for row in observed(page, "decompile-code")]
    assert text.splitlines() == rows, (text, rows)
    return text


def high(page, canvas):
    click_control(page, canvas, "Simplified structured code", role="tab")
    ui_control(page, "Simplified structured code", role="tab", selected=True)


def selected_statement(page, canvas, artifact, current, statement, operation, needle):
    source_descendant(page, canvas, artifact, current["id"], statement["id"], operation["source"]["pc"])
    ui_control(page, "Simplified structured code", role="tab", selected=True)
    row = selected_row(page, "decompile-code", needle)
    return code_label(row["label"])[1]


def single_mask(text, output, read, mask):
    expected = [f"v{output} = (0x{mask:x} & v{read});",
                f"v{output} = (v{read} & 0x{mask:x});"]
    assert text.strip() in expected, text
    assert text.count(f"0x{mask:x}") == 1 and text.count("&") == 1, text
    assert "evm.and(" not in text, text


def metadata_invariance(page, canvas, text, bound_row, reads):
    for dependencies, annotations in [(True, False), (True, True), (False, True), (False, False)]:
        set_control(page, canvas, DEPENDENCIES, dependencies)
        set_control(page, canvas, ANNOTATIONS, annotations)
        detailed = painted_copy(page, canvas)
        semantic = lambda line: line.split(" // memory", 1)[0].split(" // effect", 1)[0]
        assert [semantic(line) for line in detailed.splitlines()].count(semantic(bound_row)) == 1, detailed
        for primitive, count in reads.items():
            assert detailed.count(primitive) == count, detailed
    assert painted_copy(page, canvas) == text


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 2200)
    results, posts, publications = [], [], []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and "/api/decompile/" in request.url else None)
    page.on("response", lambda response: publications.append(response.url)
            if response.request.method == "GET" and "/api/decompile/tasks/" in response.url
            and response.url.endswith("/result") else None)
    for name, left, right, memory in [("same", MASK160, MASK160, False),
                                     ("different", 0xf0, 0xcc, False),
                                     ("memory", MASK160, MASK160, True)]:
        artifact, _ = submit(context, page, canvas, cross(left, right, memory), "bound-word-" + name)
        current, program, operations, statements = publication(artifact)
        result_url, admitted = publications[-1], len(posts)
        bound, = [statement for statement in statements.values()
                  if operations[statement["operation"]]["opcode"] == 0x16 and statement["outputs"]]
        read_opcode, spelling = (0x51, "evm.mload(") if memory else (0x54, "evm.sload(")
        read, = [statement for statement in statements.values()
                 if operations[statement["operation"]]["opcode"] == read_opcode]
        store, = [statement for statement in statements.values()
                  if operations[statement["operation"]]["opcode"] == 0x55]
        assert set(bound) == {"id", "operation", "inputs", "outputs"}, bound
        assert len(bound["inputs"]) == 2 and len(bound["outputs"]) == len(read["outputs"]) == 1
        if name == "same":
            assert bound["outputs"] == ["16"] and read["outputs"] == ["7"], bound
        expressions = {expression["id"]: expression for expression in program["expressions"]}
        assert expressions[store["inputs"][1]]["kind"] == {"value": bound["outputs"][0]}
        high(page, canvas)
        text = painted_copy(page, canvas)
        target = selected_statement(page, canvas, artifact, current, bound, operations[bound["operation"]], f"v{bound['outputs'][0]} = ")
        single_mask(target, bound["outputs"][0], read["outputs"][0], left & right)
        assert text.count(spelling) == text.count("evm.sstore(") == 1, text
        assert text.index(spelling) < text.index(target) < text.index("evm.sstore("), text
        if memory:
            assert text.count("evm.mstore(") == 1 and "evm.sload(" not in text, text
        detail = inspector_control(page, canvas, f"Statement h{bound['id']} ", "decompile-entity-detail")
        assert target.strip() in detail["label"], detail
        for entity in [bound["operation"], *bound["inputs"]]:
            # Each reference follows real navigation, so restore the source
            # statement before checking the next one in its own inspector.
            assert selected_statement(page, canvas, artifact, current, bound,
                                      operations[bound["operation"]], f"v{bound['outputs'][0]} = ") == target
            set_control(page, canvas, "Entity references", True, role="section")
            inspector_control(page, canvas, f"Reference s{current['id']}/e{entity} ", "decompile-entity-link")
        assert selected_statement(page, canvas, artifact, current, bound,
                                  operations[bound["operation"]], f"v{bound['outputs'][0]} = ") == target
        metadata_invariance(page, canvas, text, target, {spelling: 1, "evm.sstore(": 1})
        retained = page.request.get(result_url)
        assert retained.ok and retained.json()["result"]["Ok"]["artifact"] == artifact
        assert len(posts) == admitted, posts
        click_control(page, canvas, "Structured code", role="tab")
        raw = copy_code(page, canvas)
        assert raw.count("evm.and(") == 2, raw
        high(page, canvas)
        assert painted_copy(page, canvas) == text
        results.append({"shape": name, "artifact": artifact["id"], "statement": bound["id"],
                        "output": bound["outputs"][0], "source_pc": operations[bound["operation"]]["source"]["pc"],
                        "row": target})

    artifact, _ = submit(context, page, canvas, cross(0, MASK160), "bound-word-zero")
    current, program, operations, statements = publication(artifact)
    high(page, canvas)
    text = painted_copy(page, canvas)
    assert text.count("evm.sload(") == text.count("evm.sstore(") == 1, text
    assert "evm.sstore(slot = 0x0, value = 0x0);" in text, text
    outer = [operation for block in current["ir"]["blocks"] for operation in block["operations"]
             if operation["opcode"] == 0x16][-1]
    assert outer["replacement"] == "0x0", outer
    assert not any(statement["operation"] == outer["id"] for statement in statements.values())
    store, = [statement for statement in statements.values() if operations[statement["operation"]]["opcode"] == 0x55]
    row = selected_statement(page, canvas, artifact, current, store, operations[store["operation"]], "evm.sstore(")
    metadata_invariance(page, canvas, text, row, {"evm.sload(": 1, "evm.sstore(": 1})
    results.append({"shape": "zero", "artifact": artifact["id"], "store": store["id"], "row": row})

    artifact, _ = submit(context, page, canvas, shared_mask(MASK160, MASK160), "bound-word-shared")
    current, program, operations, statements = publication(artifact)
    assert program["procedures"] and len(program["invocations"]) >= 2
    high(page, canvas)
    text = painted_copy(page, canvas)
    selected = None
    for statement in statements.values():
        if operations[statement["operation"]]["opcode"] != 0x16 or not statement["outputs"]:
            continue
        source_descendant(page, canvas, artifact, current["id"], statement["id"], operations[statement["operation"]]["source"]["pc"])
        if ui_control(page, "Simplified structured code", role="tab")["selected"]:
            row = code_label(selected_row(page, "decompile-code", f"v{statement['outputs'][0]} = ")["label"])[1]
            selected = statement, row
            break
    assert selected is not None, "canonical shared definition must retain its actual bound AND row"
    bound, row = selected
    assert row.count(f"0x{MASK160:x}") == 1 and row.count("&") == 1 and "evm.and(" not in row, row
    assert text.count("evm.sload(") == text.count("evm.mload(") == 1, text
    assert text.count("evm.sstore(") == 3 and text.count("evm.mstore(") == 1, text
    metadata_invariance(page, canvas, text, row, {"evm.sload(": 1, "evm.mload(": 1, "evm.sstore(": 3})
    results.append({"shape": "shared", "artifact": artifact["id"], "statement": bound["id"], "row": row,
                    "procedures": len(program["procedures"]), "calls": len(program["invocations"])})
    canvas.screenshot(path=str(context.output / "bound-word-shared-review.png"))
    renderer = webgpu_evidence(page)
    page.close()
    return {"cases": results, "renderer": renderer}
