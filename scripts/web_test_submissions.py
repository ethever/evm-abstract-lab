"""Submissions browser acceptance cases."""

from web_test_hybrid import oracle_graph
from playwright.sync_api import expect
import json
from web_test_example_helpers import example_page
from web_test_harness import open_input, rendered_frame, submit_bytecode, webgpu_evidence


def run_submissions(context):
    page, canvas, _ = example_page(context.browser, context.url)
    status = page.locator("#analysis-status")
    page.keyboard.press("0")
    rendered_frame(page)
    submissions = []
    page.on("request", lambda request: submissions.append(request.post_data)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    open_input(page)
    page.keyboard.press("Control+A")
    page.keyboard.insert_text("this is not bytecode")
    expect(status).to_contain_text("Input validation: Runtime bytecode:")
    page.keyboard.press("Control+Enter")
    rendered_frame(page)
    expect(status).to_contain_text("Input editor: Bytecode")
    assert submissions == [], "invalid local draft reached the task API"
    canvas.screenshot(path=str(context.output / "invalid-local-input.png"))
    page.keyboard.press("Escape")
    expect(status).not_to_contain_text("Input editor:")
    # Preserve the typed backend decode-failure oracle using valid hex that
    # needs actual bytecode admission (EOF), separately from local syntax.
    invalid = submit_bytecode(page, canvas, "ef00", context.output / "invalid-input.png")
    assert invalid.admission_status == 202
    assert invalid.status in (200, 202), "worker failures belong to typed task status"
    assert invalid.json()["result"]["Err"]["code"] == "InvalidBytecode"
    expect(status).to_contain_text("Error: InvalidBytecode")
    canvas.screenshot(path=str(context.output / "invalid-bytecode.png"))

    arithmetic = submit_bytecode(page, canvas, "600160020100", context.output / "arithmetic-input.png")
    assert arithmetic.status == 200
    assert arithmetic.report["status"] == "Converged"
    expect(status).to_contain_text("Ready:")
    expect(status).to_contain_text("Converged")

    partial = submit_bytecode(page, canvas, "5a5f5ff0", context.output / "uncovered-creation-input.png")
    assert partial.status == 200
    partial_reply = partial.json()
    (context.output / "incomplete.json").write_text(json.dumps(partial_reply, indent=2) + "\n")
    global_report = partial.report
    assert global_report["status"] == "Converged"
    assert global_report["hybrid"]["coverage"]["baseline_current"] is True
    assert global_report["hybrid"]["coverage"]["chosen"] == "ProcessWithDetail"
    incomplete = oracle_graph(global_report, "Native")
    assert incomplete["status"] == "Incomplete"
    assert any(frontier["kind"] == "Creation" and "UnknownInitCode" in frontier["detail"] for frontier in incomplete["frontiers"])
    assert not incomplete["ssa"]["complete"]
    expect(status).to_contain_text("Incomplete")
    expect(status).to_contain_text("SSA partial")
    expect(status).to_contain_text("Global coverage closed")
    expect(status).to_contain_text("View: Detailed paths")
    canvas.screenshot(path=str(context.output / "incomplete.png"))
    result = {"invalid_local_draft_submitted": False,
              "invalid_bytecode": invalid.json()["result"]["Err"]["code"],
              "arithmetic": arithmetic.report["status"],
              "uncovered_creation": incomplete["status"],
              "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context):
    return run_submissions(context)
