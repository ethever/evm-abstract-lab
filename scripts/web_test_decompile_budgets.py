"""Exercise exact u64 budgets through real wasm input and HTTP publication."""

import json

from playwright.sync_api import expect
from web_test_decompile_helpers import (observed, open_decompiler, prepare, submit)
from web_test_harness import (edit_control, focus_text_control,
                              rendered_frame, set_control, ui_control,
                              webgpu_evidence)


MAX = 18446744073709551615
RESOURCE_FIELDS = ["max_bytecode_bytes", "max_blocks", "max_operations", "max_values",
                   "max_work", "max_iterations", "max_cloned_blocks", "max_history_entities"]
FIELDS = [("Work limit", "max_work"), ("Block limit", "max_blocks"),
          ("Operation limit", "max_operations"), ("Clone limit", "max_cloned_blocks"),
          ("History limit", "max_history_entities"), ("Recovery iteration limit", "max_iterations")]


def editor_text(page, canvas, label):
    # Copy the actual native editor buffer, never read a JS numeric model or
    # assume Chromium's transient input agent contains the complete draft.
    page.context.grant_permissions(["clipboard-read", "clipboard-write"])
    focus_text_control(page, canvas, label)
    page.evaluate("navigator.clipboard.writeText('')")
    page.keyboard.press("Control+A")
    rendered_frame(page)
    page.keyboard.press("Control+C")
    rendered_frame(page)
    page.wait_for_function("() => navigator.clipboard.readText().then(value => value.length > 0)")
    return page.evaluate("navigator.clipboard.readText()")


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    requests, responses, results = [], [], []
    page.on("request", lambda request: requests.append(request)
            if request.method == "POST" and request.url.endswith("/api/decompile/tasks") else None)
    page.on("response", lambda response: responses.append(response)
            if response.request.method == "GET" and "/api/decompile/tasks/" in response.url
            and response.url.endswith("/result") and response.status == 200 else None)

    # A tiny actual job with maximum configured ceilings must complete cheaply;
    # successful wasm adoption also proves its returned config decodes above u32.
    artifact, _ = submit(context, page, canvas, "00", "budgets-default-maximum")
    request = json.loads(requests[-1].post_data)
    reply = json.loads(responses[-1].text())["result"]["Ok"]
    assert artifact == reply["artifact"]
    assert all(request["limits"][field] == MAX for field in RESOURCE_FIELDS), request
    assert request["limits"]["max_structuring_depth"] == 512
    assert request["limits"]["max_constant_set"] == 8
    assert artifact["config"]["limits"] == request["limits"]
    assert artifact["completion"] == "complete" and artifact["statistics"]["work"] < 1_000_000
    prepare(page, canvas, "00")
    set_control(page, canvas, "Decompile budgets", True, role="section")
    for label, _ in FIELDS:
        assert editor_text(page, canvas, label) == str(MAX), label
    canvas.screenshot(path=str(context.output / "budgets-default-max-editors.png"))
    results.append({"artifact": artifact["id"], "default_limits": request["limits"]})

    # Distinct boundary values must survive draft -> raw request -> frozen
    # result config -> actual editor readback, without a JavaScript Number hop.
    for value in [4294967296, 10_000_000_000, 9_007_199_254_740_993, MAX]:
        prepare(page, canvas, "00")
        set_control(page, canvas, "Decompile budgets", True, role="section")
        edit_control(page, canvas, "Work limit", str(value))
        assert editor_text(page, canvas, "Work limit") == str(value)
        artifact, _ = submit(context, page, canvas, "00", "budgets-work-" + str(value))
        raw = requests[-1].post_data
        request = json.loads(raw)
        reply = json.loads(responses[-1].text())["result"]["Ok"]
        assert f'"max_work":{value}' in raw, raw
        assert request["limits"]["max_work"] == value
        assert reply["artifact"]["config"]["limits"]["max_work"] == value
        assert artifact == reply["artifact"] and artifact["completion"] == "complete"
        expect(page.locator("#analysis-status")).to_contain_text("Decompile task: Ready")
        prepare(page, canvas, "00")
        assert editor_text(page, canvas, "Work limit") == str(value)
        results.append({"artifact": artifact["id"], "work_limit": value,
                        "actual_work": artifact["statistics"]["work"]})

    # Invalid drafts must block both real activation paths, even though the
    # typed config still has its previous valid value. Correcting the same
    # editor restores submission and preserves every decimal digit.
    for draft in ["18446744073709551616", "-1", "1.5", ""]:
        admitted = len(requests)
        edit_control(page, canvas, "Work limit", draft)
        button = ui_control(page, "Decompile", role="action", enabled=False)
        expect(page.locator("#analysis-status")).to_contain_text("Correct invalid decompile budgets")
        page.keyboard.press("Control+Enter")
        rendered_frame(page)
        left, top, right, bottom = button["rect"]
        page.mouse.click((left + right) / 2, (top + bottom) / 2)
        rendered_frame(page)
        assert len(requests) == admitted, (draft, requests)
    canvas.screenshot(path=str(context.output / "budgets-invalid-draft-blocks-submit.png"))
    edit_control(page, canvas, "Work limit", "9007199254740993")
    ui_control(page, "Decompile", role="action", enabled=True)
    artifact, _ = submit(context, page, canvas, "00", "budgets-corrected-draft")
    assert json.loads(requests[-1].post_data)["limits"]["max_work"] == 9_007_199_254_740_993
    assert artifact["config"]["limits"]["max_work"] == 9_007_199_254_740_993

    # Exercise progress and final-statistics decoding at the u64 boundary
    # without pretending the tiny runtime performed billions of work units.
    # This is a transport-only synthetic counter overlay on real HTTP replies;
    # request/config/artifact structure and backend-retained results stay real.
    def wide_counters(route):
        response = route.fetch()
        payload = json.loads(response.text())
        success = payload.get("result", {}).get("Ok")
        if success is not None:
            if "progress" in success:
                success["progress"]["work"] = MAX
            if "artifact" in success:
                success["artifact"]["statistics"]["work"] = MAX
        route.fulfill(response=response, body=json.dumps(payload))

    prepare(page, canvas, "00")
    edit_control(page, canvas, "Work limit", str(MAX))
    pattern = "**/api/decompile/tasks**"
    page.route(pattern, wide_counters)
    try:
        artifact, _ = submit(context, page, canvas, "00", "budgets-synthetic-wide-counters")
        expect(page.locator("#analysis-status")).to_contain_text("Work: " + str(MAX))
        assert artifact["statistics"]["work"] == MAX
        assert artifact["config"]["limits"]["max_work"] == MAX
        assert any("evm.stop();" in row["label"] for row in observed(page, "decompile-code"))
        canvas.screenshot(path=str(context.output / "budgets-wide-counters-adopted.png"))
        backend = page.request.get(responses[-1].url)
        original = json.loads(backend.text())["result"]["Ok"]["artifact"]
        assert original["statistics"]["work"] < 1_000_000
        assert original["config"] == artifact["config"]
        results.append({"artifact": artifact["id"], "transport_only_work": MAX,
                        "actual_work": original["statistics"]["work"]})
    finally:
        page.unroute(pattern, wide_counters)
    gpu = webgpu_evidence(page)
    page.close()
    return {"cases": results, "webgpu": gpu}
