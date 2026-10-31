"""Full same-app decompiler workspace with real stages, graphs, IR and origins."""

from playwright.sync_api import expect
from web_test_decompile_helpers import (BRANCH, LOOP, choose_stage, contains_ast,
                                       inspector_control, observed, open_decompiler,
                                       prepare, stage, submit)
from web_test_harness import click_control, resize_view, ui_control, webgpu_evidence
from web_test_pixels import text_pixels


def run(context):
    page, canvas, _, analysis_status = open_decompiler(context)
    status = page.locator("#analysis-status")
    submissions = []
    page.on("request", lambda request: submissions.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    prepare(page, canvas, "this is not hexadecimal")
    expect(status).to_contain_text("Input validation: Runtime bytecode:")
    ui_control(page, "Decompile", role="action", enabled=False)
    canvas.screenshot(path=str(context.output / "invalid-local-draft.png"))
    artifact, request = submit(context, page, canvas, BRANCH, "branch")
    assert request["fork"] == "Osaka" and request["strategy"] == "dispatcher"
    assert contains_ast(stage(artifact, "structured")["ir"]["structured"], "if")
    code = observed(page, "decompile-code")
    assert code and any("if (" in control["label"] for control in code), code
    screenshot = canvas.screenshot(path=str(context.output / "structured-painted.png"))
    band = [min(control["rect"][0] for control in code), min(control["rect"][1] for control in code),
            max(control["rect"][2] for control in code), max(control["rect"][3] for control in code)]
    pixels = text_pixels(page, screenshot, band)
    assert len(pixels["rows"]) >= 3, pixels
    event = next(event for event in reversed(artifact["events"]) if event["from_stage"] is not None)
    inspector_control(page, canvas, f"Transform {event['id']}:", "decompile-transform")
    inspector_control(page, canvas, "Compare transform stages", "action")
    expect(status).to_contain_text(f"Before stage: {event['from_stage']}")
    expect(status).to_contain_text(f"Current stage: {event['to_stage']}")
    expect(status).to_contain_text("Route: Changes")
    canvas.screenshot(path=str(context.output / "transform-stage-differences.png"))
    choose_stage(page, canvas, artifact, "baseline", before=True)
    click_control(page, canvas, "Graphs", role="tab")
    for title in ["Before CFG", "Current CFG"]:
        plot = ui_control(page, title, role="graph-canvas")
        assert plot["rect"][2] - plot["rect"][0] > 300
        assert observed(page, "decompile-block"), "no actual recovered graph cards were painted"
    canvas.screenshot(path=str(context.output / "baseline-current-cfg.png"))
    choose_stage(page, canvas, artifact, "decoded", before=True)
    canvas.screenshot(path=str(context.output / "source-current-cfg.png"))
    choose_stage(page, canvas, artifact, "baseline", before=True)
    click_control(page, canvas, "IR", role="tab")
    assert observed(page, "decompile-ir"), "baseline/current IR is not selectable"
    canvas.screenshot(path=str(context.output / "baseline-current-ir.png"))
    click_control(page, canvas, "Original bytecode", role="tab")
    source = observed(page, "decompile-source")[0]
    click_control(page, canvas, source["label"], role="decompile-source", id=source["id"])
    descendants = observed(page, "decompile-entity-link", "Source descendant")
    assert descendants, "an original instruction has no navigable descendants"
    selected = next((item for item in descendants if f"s{artifact['current_stage']}/" in item["label"]), descendants[0])
    click_control(page, canvas, selected["label"], role="decompile-entity-link", id=selected["id"])
    expect(status).to_contain_text("Selected entity: " + artifact["id"] + "/stage:")
    inspector_control(page, canvas, "Original ", "decompile-origin")
    expect(status).to_contain_text("Route: Original bytecode")
    canvas.screenshot(path=str(context.output / "original-descendant-navigation.png"))
    click_control(page, canvas, "Analysis", role="tab")
    assert page.locator("#analysis-status").text_content().startswith(analysis_status.split("; Workspace:")[0])
    with page.expect_response(lambda response: response.request.method == "GET"
                              and response.url.endswith("/result") and "/api/decompile/" in response.url,
                              timeout=60000) as transferred:
        click_control(page, canvas, "Decompile captured source", role="action")
        click_control(page, canvas, "Decompile", role="action")
    transferred_artifact = transferred.value.json()["result"]["Ok"]["artifact"]
    expect(status).to_contain_text("Artifact: " + transferred_artifact["id"])
    loop, loop_request = submit(context, page, canvas, LOOP, "natural-loop", fork="Cancun")
    assert loop_request["fork"] == "Cancun" and loop["input"]["fork"] == "Cancun"
    assert contains_ast(stage(loop, "structured")["ir"]["structured"], "while")
    click_control(page, canvas, "Structured code", role="tab")
    assert any("while (" in item["label"] for item in observed(page, "decompile-code"))
    resize_view(page, 640, 760)
    click_control(page, canvas, "Graphs", role="tab")
    click_control(page, canvas, "Before graph", role="tab")
    ui_control(page, "Before CFG", role="graph-canvas")
    click_control(page, canvas, "Current graph", role="tab")
    ui_control(page, "Current CFG", role="graph-canvas")
    canvas.screenshot(path=str(context.output / "compact-stage-comparison.png"))
    assert not submissions, "decompiler interactions triggered a new analysis"
    result = {"branch_artifact": artifact["id"], "loop_artifact": loop["id"],
              "source_transfer": transferred_artifact["id"], "structured_pixels": pixels,
              "renderer": webgpu_evidence(page)}
    page.close()
    return result
