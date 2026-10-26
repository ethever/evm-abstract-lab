"""Responsive input browser acceptance cases."""

from playwright.sync_api import expect
from web_test_example_helpers import example_page
from web_test_harness import (complete_submission, edit_control, focus_text_control, open_input,
                              posted_bytecode, rendered_frame, resize_view, submit_bytecode,
                              ui_control, webgpu_evidence)
from web_test_hybrid import oracle_graph


def run_responsive_input(context):
    page, canvas, _ = example_page(context.browser, context.url)
    status = page.locator("#analysis-status")
    responsive_input = {}
    for width, height in [(390, 844), (844, 390), (320, 480)]:
        resize_view(page, width, height)
        reply = submit_bytecode(page, canvas, "600160020100",
                                context.output / f"editor-{width}x{height}.png", close_directory=True)
        assert reply.status == 200
        expect(status).to_contain_text("Converged")
        native = oracle_graph(reply.report, "Native")
        assert any(block["exit_stack"] == ["{0x3}"] for block in native["cfg"]), native["cfg"]
        responsive_input[f"{width}x{height}"] = posted_bytecode(reply)
    # One bounded synthetic Chromium IME/touch exercise complements the three
    # original responsive admissions. It does not claim physical OS IME coverage.
    submissions = []
    page.on("request", lambda request: submissions.append(request.post_data_json)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    open_input(page, mode="Bytecode")
    edit_control(page, canvas, "Runtime bytecode", "")
    editor = focus_text_control(page, canvas, "Runtime bytecode", touch=True)
    session = page.context.new_cdp_session(page)
    code = "600160020100"
    session.send("Input.imeSetComposition", {"text": code, "selectionStart": len(code),
                                            "selectionEnd": len(code)})
    rendered_frame(page)
    page.keyboard.press("Control+Enter")
    rendered_frame(page)
    expect(status).to_contain_text("Input editor: Bytecode")
    ui_control(page, "Runtime bytecode", role="text")
    assert submissions == [], "composing Ctrl+Enter submitted the input"
    session.send("Input.insertText", {"text": code})
    rendered_frame(page)
    expect(status).to_contain_text("Input validation: Ready")
    canvas.screenshot(path=str(context.output / "editor-320-ime-committed.png"))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as admitted:
        page.keyboard.press("Control+Enter")
    reply = complete_submission(page, admitted.value, namespace="Native", close_directory=True)
    assert reply.status == 200 and posted_bytecode(reply) == code, reply.json()
    assert len(submissions) == 1 and submissions[0]["input"]["Bytecode"]["bytecode"] == code
    assert submissions[0]["limits"]["max_work"] == 2_000_000_000
    assert submissions[0]["limits"]["max_detail_regions"] == 1
    assert submissions[0]["environment"]["calldata"] == "Unknown"
    assert submissions[0]["environment"]["call_value"] == "Unknown"
    session.detach()
    result = {"responsive_input": responsive_input, "synthetic_chromium_ime": {
        "touch_focused_editor": editor, "composition_submissions": 0,
        "committed_submissions": 1, "bytecode": posted_bytecode(reply)}, "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context):
    return run_responsive_input(context)
