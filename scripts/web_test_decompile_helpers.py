"""Real decompiler HTTP publications and painted control interactions."""

import json
import re
import time
from playwright.sync_api import expect
from web_test_harness import (capture_console_errors, click_control, edit_control,
                              observe_webgpu, rendered_frame, ui_control, ui_controls,
                              focus_text_control, set_control, settle_gesture,
                              settle_control_geometry)


BRANCH = "5f35600a576001600d565b60025b60030100"
LOOP = "5f5b5f35600d576001016001565b5000"
# Two distinct entries into the A/B cycle. A performs a visible storage write.
IRREDUCIBLE = "5f356008576011565b60016000556011565b5f3560085700"


def open_decompiler(context, url=None):
    page = context.browser.new_page(viewport={"width": 1920, "height": 1080})
    page.decompile_output = context.output
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
    if any(character in bytecode for character in "\n\r\t"):
        # Chromium's single-line text agent sanitizes insertText newlines.
        # A real paste reaches egui's multiline Paste event without that loss.
        focus_text_control(page, canvas, "Runtime bytecode")
        page.context.grant_permissions(["clipboard-read", "clipboard-write"])
        page.evaluate("text => navigator.clipboard.writeText(text)", bytecode)
        page.keyboard.press("Control+A")
        rendered_frame(page)
        page.keyboard.press("Control+V")
        rendered_frame(page)
    else:
        edit_control(page, canvas, "Runtime bytecode", bytecode)
    record_input(page, "draft-before-submit", expected_draft=bytecode)
    ui_control(page, bytecode, role="bytecode-value")
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
    def scroll(direction, inner=False):
        def geometry():
            return [(control["label"], control["role"], control["rect"])
                    for control in ui_controls(page) if control["enabled"]]
        previous = geometry()
        controls = observed(page, "decompile-entity-link", "Source descendant") if inner else []
        panel = observed(page, "decompile-inspector")[0]["rect"]
        if controls:
            first, last = controls[0]["rect"], controls[-1]["rect"]
            point = [(first[0] + first[2]) / 2, (first[1] + last[3]) / 2]
            distance = max(25, (last[3] - first[1]) * 0.6)
        else:
            # Wheel over a real disclosure header in the OUTER document;
            # the panel border/scrollbar may sit outside its wheel viewport.
            headers = [control for control in observed(page, "section")
                       if panel[0] <= control["rect"][0] < control["rect"][2] <= panel[2]
                       and panel[1] <= control["rect"][1] < control["rect"][3] <= panel[3]
                       and control["rect"][3] - control["rect"][1] >= 16]
            area = headers[0]["rect"] if headers else panel
            point = [(area[0] + area[2]) / 2, (area[1] + area[3]) / 2]
            distance = 200
        before = page.evaluate("window.__webgpuEvidence.submissions")
        page.mouse.move(*point)
        page.wait_for_function("before => window.__webgpuEvidence.submissions > before", arg=before, polling=25)
        record_input(page, "before-inspector-scroll", direction=direction, inner=inner, point=point)
        # Overlap virtual-list windows:200px skips complete rows in a160px
        # viewport, especially a target first observed as a clipped final row.
        page.mouse.wheel(0, direction * distance)
        settle_gesture(page)
        settle_control_geometry(page)
        record_input(page, "after-inspector-scroll", direction=direction, inner=inner, point=point)
        return geometry() != previous

    if role == "decompile-transform":
        for direction in [-1, 1]:
            for _ in range(16):
                sections = observed(page, "section", "Transformation history")
                if sections:
                    set_control(page, canvas, "Transformation history", True, role="section")
                    break
                if not scroll(direction):
                    break
            if sections:
                break

    settle_gesture(page)
    settle_control_geometry(page)
    for direction in [-1, 1]:
        for _ in range(64):
            minimum_height = 16 if role in {"decompile-entity-link", "decompile-origin", "decompile-transform", "action"} else 15
            matches = [control for control in observed(page, role, prefix)
                       if control["rect"][3] - control["rect"][1] >= minimum_height]
            if matches:
                item = matches[0]
                if role == "decompile-entity-detail":
                    # Ordinary native labels are15px high and need no click.
                    record_input(page, "inspector-detail-visible", target=item)
                    return item
                # Wheel smoothing and accepted geometry must settle BEFORE
                # press, not after a click using a stale virtual-row rectangle.
                point = [(item["rect"][0] + item["rect"][2]) / 2,
                         (item["rect"][1] + item["rect"][3]) / 2]
                before = page.evaluate("window.__webgpuEvidence.submissions")
                page.mouse.move(*point)
                page.wait_for_function("before => window.__webgpuEvidence.submissions > before", arg=before, polling=25)
                record_input(page, "before-inspector-click", target=item, point=point)
                click_control(page, canvas, item["label"], role=role, id=item["id"])
                record_input(page, "after-inspector-click", target=item, point=point)
                reference = re.search(r" s(\d+)/e(\d+) ", item["label"])
                if role == "decompile-entity-link" and reference:
                    stage_id, entity_id = reference.groups()
                    expect(page.locator("#analysis-status")).to_contain_text(f"/stage:{stage_id}/entity:{entity_id}")
                return item
            if not scroll(direction, prefix.startswith("Source descendant")):
                break
    record_input(page, "inspector-target-missing", prefix=prefix, role=role)
    page.locator("#evm-canvas").screenshot(path=str(page.decompile_output / "missing-inspector-target.png"))
    raise AssertionError(f"inspector never rendered {role} {prefix!r}")


def record_input(page, event, **fields):
    """Read-only evidence of actual controls, focus, buffers and paint frames."""
    output = getattr(page, "decompile_output", None)
    if output is None:
        return
    state = page.evaluate("""() => {
        const status = document.querySelector('#analysis-status');
        const active = document.activeElement;
        return {status: status?.textContent, controls: JSON.parse(status?.getAttribute('data-controls') || '[]'),
            active: active ? {tag: active.tagName, id: active.id, value: active.value,
                rect: [active.getBoundingClientRect().x, active.getBoundingClientRect().y,
                       active.getBoundingClientRect().width, active.getBoundingClientRect().height]} : null,
            frames: window.__webgpuEvidence?.submissions, input_frame: window.__webTestInputFrame,
            last_wheel: window.__webTestLastWheel, now: performance.now()};
    }""")
    with (output / "interaction-evidence.jsonl").open("a") as stream:
        stream.write(json.dumps({"event": event, **fields, "state": state}) + "\n")


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
