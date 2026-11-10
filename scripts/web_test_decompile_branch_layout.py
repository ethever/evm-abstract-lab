"""Typed branch polarity, original source IDs and copy through a real browser."""

from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_guards import ANNOTATIONS, DEPENDENCIES, high, projection
from web_test_decompile_helpers import (choose_stage, inspector_control, observed,
                                        open_decompiler, stage, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import (click_control, resize_view, set_control, ui_control,
                              webgpu_evidence)
from web_test_registry import SOURCE_ROOT


FIXTURES = "crates/evm-abstract-web/tests/fixtures/decompile/"
LIVE_PHI = "5f35600a576001600d565b60025b5f5500"


def painted_copy(page, canvas):
    text = copy_code(page, canvas)
    needed = 850 + 28 * len(text.splitlines())
    if page.viewport_size["height"] < needed <= 3800:
        resize_view(page, 1920, needed)
    visible = [code_label(row["label"])[1] for row in observed(page, "decompile-code")]
    assert text.splitlines() == visible, "the complete compact fixture must be painted"
    return text


def semantic_rows(text):
    return [line.split(" // ", 1)[0] for line in text.splitlines()
            if not line.lstrip().startswith("//") and " token on e" not in line]


def assert_modes(page, canvas, expected_else, expected_tstore=None):
    snapshots = {}
    for dependencies, annotations in [(False, False), (True, False), (True, True),
                                      (False, True), (False, False)]:
        set_control(page, canvas, DEPENDENCIES, dependencies)
        set_control(page, canvas, ANNOTATIONS, annotations)
        text = painted_copy(page, canvas)
        key = (dependencies, annotations)
        assert key not in snapshots or snapshots[key] == text
        snapshots[key] = text
        assert text.count("} else {") == expected_else, text
        if expected_tstore is not None:
            assert text.count("evm.tstore(") == expected_tstore, text
    first = semantic_rows(snapshots[False, False])
    assert all(semantic_rows(text) == first for text in snapshots.values())
    return snapshots[False, False]


def branch_source(current, program, branch_id):
    control, = [control for control in program["controls"] if control["node"] == branch_id]
    node, = [node["kind"]["if"] for node in current["ir"]["structured"]["nodes"]
             if node["id"] == branch_id]
    operation, = [operation for block in current["ir"]["blocks"]
                  for operation in block["operations"] if node["condition"] in operation["outputs"]]
    return control, node, operation


def review_condition(page, canvas, artifact, current, control, condition):
    source_descendant(page, canvas, artifact, current["id"], control["id"],
                      pc=condition["source"]["pc"])
    ui_control(page, "Simplified structured code", role="tab", selected=True)
    selected_row(page, "decompile-code", "if (")
    inspector_control(page, canvas, "Original predicate x" + control["condition"],
                      "decompile-entity-detail")


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 3000)
    reports = []
    code = (SOURCE_ROOT / (FIXTURES + "pool-style-empty-then.hex")).read_text().strip()
    artifact, _ = submit(context, page, canvas, code, "pool-style-empty-then")
    current, program = projection(artifact)
    branch, = [node["id"] for node in current["ir"]["structured"]["nodes"]
               if isinstance(node["kind"], dict) and "if" in node["kind"]]
    control, source, condition = branch_source(current, program, branch)
    assert condition["opcode"] == 0x15, "the original true edge tests zero"
    high(page, canvas)
    text = assert_modes(page, canvas, 0, expected_tstore=1)
    assert "!= 0" in next(line for line in text.splitlines() if line.startswith("if ("))
    review_condition(page, canvas, artifact, current, control, condition)
    choose_stage(page, canvas, artifact, "structured")
    click_control(page, canvas, "Structured code", role="tab")
    raw = painted_copy(page, canvas)
    assert raw.count("} else {") == 1, "raw keeps the original zero-bypass arms"
    assert raw.count("evm.tstore(") == 1
    choose_stage(page, canvas, artifact, "lifted")
    high(page, canvas)
    assert painted_copy(page, canvas) == text
    reports.append({"case": "pool-style", "artifact": artifact["id"],
                    "control": control["id"], "source_arms": [source["then_branch"], source["else_branch"]]})

    code = (SOURCE_ROOT / (FIXTURES + "shared-empty-then.hex")).read_text().strip()
    artifact, _ = submit(context, page, canvas, code, "shared-empty-then", procedures="Shared")
    current, program = projection(artifact)
    assert program["procedures"] and len(program["invocations"]) >= 2
    nodes = {node["id"]: node["kind"] for node in current["ir"]["structured"]["nodes"]}
    procedures = [p for p in program["procedures"]
                  if p["body"] in nodes and isinstance(nodes[p["body"]], dict)
                  and "if" in nodes[p["body"]]]
    procedure, = procedures
    control, _, condition = branch_source(current, program, procedure["body"])
    high(page, canvas)
    collapsed = assert_modes(page, canvas, 0)
    assert "!= 0" in next(line for line in collapsed.splitlines() if line.lstrip().startswith("if ("))
    review_condition(page, canvas, artifact, current, control, condition)
    call = next(call for call in program["invocations"]
                if call["procedure"] == procedure["id"] and call["source"] != procedure["body"])
    source_descendant(page, canvas, artifact, current["id"], call["id"])
    click_control(page, canvas, "Expand call here", role="action")
    expanded = assert_modes(page, canvas, 0)
    assert "// Expanded:" in expanded
    assert expanded.count("if (") == collapsed.count("if (") + 1
    click_control(page, canvas, "Collapse call here", role="action")
    assert painted_copy(page, canvas) == collapsed
    reports.append({"case": "shared-expanded", "artifact": artifact["id"],
                    "procedure": procedure["id"], "call": call["id"]})

    artifact, _ = submit(context, page, canvas, LIVE_PHI, "branch-live-phi")
    current, program = projection(artifact)
    high(page, canvas)
    text = assert_modes(page, canvas, 1)
    assert "= (0x1);" in text and "= (0x2);" in text, "both live parallel transfers must remain"
    reports.append({"case": "live-phi", "artifact": artifact["id"]})
    canvas.screenshot(path=str(context.output / "branch-layout.png"))
    result = {"fixtures": reports, "renderer": webgpu_evidence(page)}
    page.close()
    return result
