"""Real decompiler HTTP publications and painted control interactions."""

import json
import time
from playwright.sync_api import expect
from web_test_harness import (capture_console_errors, click_control, edit_control,
                              observe_webgpu, rendered_frame, ui_controls)


BRANCH = "5f35600a576001600d565b60025b60030100"
LOOP = "5f5b5f35600d576001016001565b5000"
# Two distinct entries into the A/B cycle. A performs a visible storage write.
IRREDUCIBLE = "5f356008576011565b60016000556011565b5f3560085700"


def open_decompiler(context, url=None):
    page = context.browser.new_page(viewport={"width": 1920, "height": 1080})
    observe_webgpu(page)
    capture_console_errors(page)
    with page.expect_response(lambda response: response.request.method == "POST"
                              and response.url.endswith("/api/tasks")) as startup:
        page.goto(url or context.url, wait_until="networkidle")
    expect(page.locator("#analysis-status")).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    analysis_status = page.locator("#analysis-status").text_content()
    job_id = startup.value.json()["result"]["Ok"]["id"]
    retained = page.request.get((url or context.url).rstrip("/") + f"/api/tasks/{job_id}/result")
    assert retained.ok, retained.text()
    page.decompile_analysis_report = retained.json()["result"]["Ok"]["report"]
    click_control(page, canvas, "Decompiler", role="tab")
    expect(page.locator("#analysis-status")).to_contain_text("Workspace: Decompiler")
    return page, canvas, startup.value.request.post_data_json, analysis_status


def prepare(page, canvas, bytecode, strategy="Dispatcher", fork=None):
    controls = ui_controls(page)
    if not any(control["label"] == "Runtime bytecode" and control["enabled"] for control in controls):
        click_control(page, canvas, "Edit bytecode", role="action")
    edit_control(page, canvas, "Runtime bytecode", bytecode)
    click_control(page, canvas, strategy, role="tab")
    if fork is not None:
        click_control(page, canvas, "EVM fork", role="combobox")
        click_control(page, canvas, fork, role="tab")


def submit(context, page, canvas, bytecode, name, strategy="Dispatcher", fork=None):
    prepare(page, canvas, bytecode, strategy, fork)
    with page.expect_response(lambda response: response.request.method == "GET"
                              and "/api/decompile/tasks/" in response.url
                              and response.url.endswith("/result") and response.status == 200,
                              timeout=60000) as publication:
        with page.expect_response(lambda response: response.request.method == "POST"
                                  and response.url.endswith("/api/decompile/tasks")) as admitted:
            click_control(page, canvas, "Decompile", role="action")
    admission, response = admitted.value, publication.value
    assert admission.status == 202, admission.text()
    job = admission.json()["result"]["Ok"]
    result = response.json()["result"]["Ok"]
    assert result["id"] == job["id"]
    assert result["publication"].isdigit()
    artifact = result["artifact"]
    assert artifact["schema_version"] == 1
    assert artifact["config"]["strategy"] == {"Dispatcher": "dispatcher", "Code duplication": "code_duplication"}[strategy]
    selected_fork = admission.request.post_data_json["fork"]
    assert artifact["input"]["fork"] == selected_fork.lower()
    expect(page.locator("#analysis-status")).to_contain_text("Fork: " + selected_fork)
    expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + artifact["id"])
    expect(page.locator("#analysis-status")).to_contain_text("Decompile task: Ready")
    (context.output / (name + "-request.json")).write_text(admission.request.post_data + "\n")
    (context.output / (name + "-artifact.json")).write_text(json.dumps(result, indent=2) + "\n")
    rendered_frame(page)
    canvas.screenshot(path=str(context.output / (name + ".png")))
    return artifact, admission.request.post_data_json


def stage(artifact, kind):
    return next(stage for stage in artifact["stages"] if stage["kind"] == kind)


def stage_label(value):
    return f"{value['id']} · {''.join(part.title() for part in value['kind'].split('_'))} · {''.join(part.title() for part in value['validation']['status'].split('_'))}"


def choose_stage(page, canvas, artifact, kind, before=False):
    target = stage(artifact, kind)
    click_control(page, canvas, "Before stage" if before else "Current stage", role="combobox")
    click_control(page, canvas, stage_label(target), role="tab")
    expect(page.locator("#analysis-status")).to_contain_text(("Before" if before else "Current") + f" stage: {target['id']}")
    return target


def observed(page, role, prefix=None):
    return [control for control in ui_controls(page)
            if control["role"] == role and control["enabled"]
            and (prefix is None or control["label"].startswith(prefix))]


def inspector_control(page, canvas, prefix, role):
    """Scroll the real inspector, without inventing an alternate activation path."""
    for direction in [-1, 1]:
        for _ in range(16):
            matches = observed(page, role, prefix)
            if matches:
                item = matches[0]
                click_control(page, canvas, item["label"], role=role, id=item["id"])
                return item
            inner = observed(page, "decompile-entity-link", "Source descendant") if prefix.startswith("Source descendant") else []
            panel = observed(page, "decompile-inspector")[0]["rect"]
            if inner:
                first, last = inner[0]["rect"], inner[-1]["rect"]
                page.mouse.move((first[0] + first[2]) / 2, (first[1] + last[3]) / 2)
            else:
                page.mouse.move(panel[2] - 8, (panel[1] + panel[3]) / 2)
            page.mouse.wheel(0, direction * 250)
            rendered_frame(page)
    raise AssertionError(f"inspector never rendered {role} {prefix!r}")


def contains_ast(node, kind):
    if isinstance(node, dict):
        return kind in node or any(contains_ast(value, kind) for value in node.values())
    if isinstance(node, list):
        return any(contains_ast(value, kind) for value in node)
    return False


def wait_job(page, url, identity, predicate):
    deadline = time.monotonic() + 30
    while True:
        response = page.request.get(url + "/api/decompile/tasks/" + str(identity))
        assert response.status == 200, response.text()
        snapshot = response.json()["result"]["Ok"]
        if predicate(snapshot):
            return snapshot
        assert time.monotonic() < deadline, snapshot
        page.wait_for_timeout(50)
