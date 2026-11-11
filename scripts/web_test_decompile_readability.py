"""Reading choices cross actual HTTP plans, Wasm controls and copied code."""

import json
from collections import Counter

from playwright.sync_api import expect
from web_test_decompile_alignment import code_label, copy_code
from web_test_decompile_helpers import (inspector_control, observed, open_decompiler,
                                       stage, submit)
from web_test_decompile_review import source_descendant
from web_test_harness import (click_control, rendered_frame, resize_view, set_control,
                              ui_control, webgpu_evidence)
from web_test_registry import SOURCE_ROOT


BRANCH = "5f356100115760403561001d57610021565b60203561001d57610021565b5f5ffd5b60775f5500"
SHARED = ("60075f35601e565b6001556002556016602035601e565b60035560045500"
          "5b80604052805f5560206040a0600554600101600555600654600755603d565b806001019156")
ARITHMETIC = "crates/evm-decompile/tests/fixtures/shared-procedures/arithmetic-template-inputs.hex"
MULTICALL = "crates/evm-decompile/tests/fixtures/structure/ethereum-multicall3.hex"
RESIDUAL = "5f3561001b575b5f54600101805f556004901061001b57610030565b5f54600101805f556004901061000657610030565b00"


def choices(page, canvas):
    set_control(page, canvas, "Transform inspector", True)
    if not observed(page, "section", "Reading choices"):
        inspector_control(page, canvas, "Reading choices", "section")
    set_control(page, canvas, "Reading choices", True, role="section")
    ui_control(page, "Reading choices", role="section", selected=True)


def apply(context, page, canvas, name):
    with page.expect_response(lambda response: response.request.method == "GET"
                              and "/api/decompile/tasks/" in response.url
                              and response.url.endswith("/result"), timeout=60000) as published:
        with page.expect_response(lambda response: response.request.method == "POST"
                                  and response.url.endswith("/api/decompile/tasks")) as admitted:
            inspector_control(page, canvas, "Apply reading choices", "action")
    request = admitted.value.request.post_data_json
    result = published.value.json()["result"]["Ok"]
    artifact = result["artifact"]
    expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + artifact["id"])
    expect(page.locator("#analysis-status")).to_contain_text("Decompile task: Ready")
    (context.output / (name + ".json")).write_text(json.dumps({"request": request, "result": result}, indent=2) + "\n")
    rendered_frame(page)
    return artifact, request


def painted_copy(page, canvas):
    text = copy_code(page, canvas)
    rows = [code_label(row["label"])[1] for row in observed(page, "decompile-code")]
    assert text.splitlines() == rows, "small fixture must fit and clipboard must equal actual painted rows"
    return text


def check_loop_causes(context, page, canvas):
    residual, _ = submit(context, page, canvas, RESIDUAL, "reading-residual",
                         procedures="Inline", profile="Balanced")
    decisions = [d for d in residual["readability"] if d["selected"] == "dispatcher"]
    assert decisions and all(d["reason"] == "residual_cycle" for d in decisions)
    assert all(d["reason"] != "multi_entry_control" for d in residual["readability"])
    choices(page, canvas)
    region = decisions[0]["region"]
    blocks = {b["id"]: b for b in stage(residual, "structured")["ir"]["blocks"]}

    def label(block):
        return f"0x{blocks[block]['source']['pc']:04x} · b{block}"

    name = label(region["entry"])
    if region["continuation"] is not None:
        name += " → " + label(region["continuation"])
    if region["loop_header"] is not None:
        name += " · loop " + label(region["loop_header"])
    inspector_control(page, canvas, "Source region", "combobox")
    click_control(page, canvas, name, role="tab")
    inspector_control(page, canvas,
                      "Selected: Dispatcher. A cycle remained outside admitted natural loops.",
                      "decompile-reading-choice")
    canvas.screenshot(path=str(context.output / "reading-residual-cause.png"))

    multicall, _ = submit(context, page, canvas, (SOURCE_ROOT / MULTICALL).read_text().strip(),
                         "reading-multicall", procedures="Auto procedures", profile="Balanced",
                         retained_result=True)
    assert multicall["completion"] == "complete" and not multicall["diagnostics"]
    ir = stage(multicall, "structured")["ir"]
    nodes = {node["id"]: node["kind"] for node in ir["structured"]["nodes"]}
    blocks = {block["id"]: block for block in ir["blocks"]}
    assert not any("dispatcher" in kind for kind in nodes.values())
    headers = Counter()
    for kind in nodes.values():
        if "while" in kind:
            header = nodes[kind["while"]["header"]]
            if "block" in header:
                headers[blocks[header["block"]["block"]]["source"]["pc"]] += 1
    assert {pc: headers[pc] for pc in [0x325, 0x546, 0x714, 0x8c3]} == {
        0x325: 1, 0x546: 1, 0x714: 1, 0x8c3: 3}
    assert all(d["reason"] != "multi_entry_control" for d in multicall["readability"])
    return {"residual": residual["id"], "multicall": multicall["id"],
            "call_loop_headers": dict(headers)}


def expand(context, page, canvas, artifact, arithmetic):
    current = stage(artifact, "lifted")
    program = current["ir"]["lifted"]
    expressions = {expression["id"] for expression in program["expressions"]}
    call = next(call for call in program["invocations"]
                if (call["source"] in expressions) == arithmetic)
    click_control(page, canvas, "Simplified structured code", role="tab")
    before = painted_copy(page, canvas)
    source_descendant(page, canvas, artifact, current["id"], call["id"])
    click_control(page, canvas, "Expand call here", role="action")
    expanded = painted_copy(page, canvas)
    assert expanded != before
    if not arithmetic:
        assert "// Expanded:" in expanded
        assert expanded.count("evm.sstore(") > before.count("evm.sstore(")
    else:
        assert expanded.count("pure_arithmetic_") < before.count("pure_arithmetic_")
    click_control(page, canvas, "Collapse call here", role="action")
    assert painted_copy(page, canvas) == before
    expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + artifact["id"])
    return {"invocation": call["id"], "source": call["source"], "before": before, "expanded": expanded}


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 3200)
    initial, _ = submit(context, page, canvas, BRANCH, "reading-baseline")
    assert initial.get("readability"), "backend must publish actual source-region decisions"
    choices(page, canvas)
    inspector_control(page, canvas, "Compact", "tab")
    inspector_control(page, canvas, "Inline", "tab")
    inspector_control(page, canvas, "Dispatcher", "tab")
    dispatched, request = apply(context, page, canvas, "reading-dispatcher")
    assert request["bytecode"] == BRANCH
    reading = request["readability"]
    assert reading["profile"] == "compact" and reading["procedures"] == "inline"
    preference, = reading["regions"]
    assert preference["structure"] == "dispatcher"
    anchor = preference["region"]
    decision = next(decision for decision in dispatched["readability"] if decision["region"] == anchor)
    assert decision["selected"] == "dispatcher" and decision["reason"] == "user_preference"
    assert decision["dispatcher"] is not None
    inspector_control(page, canvas, "Selected: Dispatcher.", "decompile-reading-choice")
    inspector_control(page, canvas, "Natural", "tab")
    natural, request = apply(context, page, canvas, "reading-natural")
    assert request["readability"]["regions"][0]["region"] == anchor, "preference changes retain source-qualified region identity"
    assert next(decision for decision in natural["readability"] if decision["region"] == anchor)["selected"] == "natural"
    # Input editing clears source-specific choices while retaining global style.
    changed, changed_request = submit(context, page, canvas, "00", "reading-new-source")
    assert changed_request["readability"]["profile"] == "compact"
    assert not changed_request["readability"]["regions"]
    assert all(decision["region"]["source"] != anchor["source"] for decision in changed.get("readability", []))
    expect(page.locator("#analysis-status")).to_contain_text("Region selection cleared")

    shared, _ = submit(context, page, canvas, SHARED, "reading-shared", procedures="Shared", profile="Balanced")
    control_expansion = expand(context, page, canvas, shared, False)
    arithmetic, _ = submit(context, page, canvas, (SOURCE_ROOT / ARITHMETIC).read_text().strip(),
                           "reading-arithmetic", procedures="Shared", profile="Balanced")
    expression_expansion = expand(context, page, canvas, arithmetic, True)
    loop_causes = check_loop_causes(context, page, canvas)
    result = {"region": anchor, "dispatcher": dispatched["id"], "natural": natural["id"],
              "control_expansion": control_expansion, "expression_expansion": expression_expansion,
              "loop_causes": loop_causes,
              "renderer": webgpu_evidence(page)}
    canvas.screenshot(path=str(context.output / "reading-final.png"))
    page.close()
    return result
