"""Submissions browser acceptance cases."""

from playwright.sync_api import expect
import json
from web_test_example_helpers import example_page
from web_test_harness import rendered_frame, submit_bytecode, webgpu_evidence


def run_submissions(context):
    page, canvas, _ = example_page(context.browser, context.url)
    status = page.locator("#analysis-status")
    page.keyboard.press("0")
    rendered_frame(page)
    invalid = submit_bytecode(page, canvas, "this is not bytecode", context.output / "invalid-input.png")
    assert invalid.admission_status == 202
    assert invalid.status in (200, 202), "worker failures belong to typed task status"
    assert invalid.json()["result"]["Err"]["code"] == "InvalidBytecode"
    expect(status).to_contain_text("Error: InvalidBytecode")
    canvas.screenshot(path=str(context.output / "invalid-bytecode.png"))

    arithmetic = submit_bytecode(page, canvas, "600160020100", context.output / "arithmetic-input.png")
    assert arithmetic.status == 200
    assert arithmetic.json()["result"]["Ok"]["status"] == "Converged"
    expect(status).to_contain_text("Ready:")
    expect(status).to_contain_text("Converged")

    partial = submit_bytecode(page, canvas, "5a5f5ff0", context.output / "uncovered-creation-input.png")
    assert partial.status == 200
    partial_reply = partial.json()
    incomplete = partial_reply["result"]["Ok"]
    assert incomplete["status"] == "Incomplete"
    assert any(frontier["kind"] == "Creation" and "UnknownInitCode" in frontier["detail"] for frontier in incomplete["frontiers"])
    assert not incomplete["ssa"]["complete"]
    expect(status).to_contain_text("Incomplete")
    expect(status).to_contain_text("SSA partial")
    (context.output / "incomplete.json").write_text(json.dumps(partial_reply, indent=2) + "\n")
    canvas.screenshot(path=str(context.output / "incomplete.png"))
    result = {"invalid_bytecode": invalid.json()["result"]["Err"]["code"],
              "arithmetic": arithmetic.json()["result"]["Ok"]["status"],
              "uncovered_creation": incomplete["status"],
              "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context):
    return run_submissions(context)
