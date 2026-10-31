"""Startup browser acceptance cases."""

from playwright.sync_api import expect
import hashlib
import json
import re
from web_test_example_helpers import example_page
from web_test_graph_helpers import assert_availability, default_entry_cfg, source_content_snapshot, workspace_route
from web_test_hybrid import assert_hybrid, native_entry_context, oracle_graph, source_program
from web_test_harness import measured_backend_timings, select_report_namespace, set_control, ui_control, webgpu_evidence


def run_startup(context):
    page, canvas, reply = example_page(context.browser, context.url, namespace=None, close_directory=False)
    result = reply.json()
    full_analysis = reply.report
    assert_hybrid(full_analysis)
    analysis = oracle_graph(full_analysis, "Native")
    assert analysis["schema_version"] == 22
    assert analysis["metadata"]["postprocessing_max_work"] == 2_000_000_000
    assert 0 <= analysis["metadata"]["postprocessing_work"] <= 2_000_000_000
    assert sum(len(block["instructions"]) for block in analysis["disassembly"]) > 0
    assert len(analysis["cfg"]) > 1, "example did not produce a graph"
    assert {"BranchTrue", "BranchFalse"}.issubset({edge["kind"] for edge in analysis["edges"]})
    assert analysis["ssa"]["blocks"], "example did not produce SSA"
    (context.output / "analysis.json").write_text(json.dumps(result, indent=2) + "\n")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text(re.compile(r"ready", re.IGNORECASE), timeout=60000)
    entry = default_entry_cfg(page, canvas, full_analysis, context.output, "default-entry-cfg")
    canvas = page.locator("#evm-canvas")
    expect(canvas).to_be_visible()
    assert canvas.evaluate("canvas => canvas.width > 0 && canvas.height > 0")
    requests = []
    page.on("request", lambda request: requests.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    program = source_program(full_analysis, analysis["metadata"]["root_program"])
    root = native_entry_context(full_analysis)
    block = next(block for block in analysis["ssa"]["blocks"] if block["state"] == root["id"])
    screenshots = {}
    contents = {}
    # The initial MAY source can expose decoded Code. It supplies no Native SSA.
    for route in ["Compare", "Code", "CFG", "SSA"]:
        workspace_route(page, route)
        if route == "Code":
            contents["model-code"] = source_content_snapshot(
                page, canvas, program, context.output, "model-code")
        elif route == "SSA":
            contents["model-ssa"] = assert_availability(page, "MAY control has no Native instruction or SSA receipt")
        else:
            ui_control(page, "CFG canvas", role="graph-canvas")
        screenshot = canvas.screenshot(path=str(context.output / f"model-{route.lower()}.png"))
        screenshots[f"model-{route.lower()}"] = hashlib.sha256(screenshot).hexdigest()

    # Preserve the four original Native content paths after an explicit,
    # source-qualified context selection, independently of the entry route.
    select_report_namespace(page, full_analysis, "Native")
    measured_backend_timings(full_analysis)
    set_control(page, canvas, "Inspect", False)
    for route, name in [("Compare", "workspace"), ("Code", "disassembly"), ("CFG", "cfg"), ("SSA", "ssa")]:
        workspace_route(page, route)
        if route in {"Compare", "Code"}:
            contents[f"native-{name}-code"] = source_content_snapshot(
                page, canvas, program, context.output, f"native-{name}-code")
        if route in {"Compare", "SSA"}:
            contents[f"native-{name}-ssa"] = source_content_snapshot(
                page, canvas, program, context.output, f"native-{name}-ssa", ssa=block)
        if route in {"Compare", "CFG"}:
            ui_control(page, "CFG canvas", role="graph-canvas")
        screenshot = canvas.screenshot(path=str(context.output / f"{name}.png"))
        screenshots[name] = hashlib.sha256(screenshot).hexdigest()
    assert not requests, "workspace routes or context selection reran analysis"
    renderer = webgpu_evidence(page)
    result = {"branch": analysis["status"], "screenshots": screenshots, "entry": entry, "contents": contents,
              "renderer": renderer, "status": status.text_content()}
    page.close()
    return result


def run(context):
    return run_startup(context)
