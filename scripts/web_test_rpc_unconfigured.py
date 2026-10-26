"""Rpc unconfigured browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
from web_test_harness import click_control, observe_webgpu, settle_gesture, submit_bytecode, ui_control
from web_test_rpc_helpers import open_rpc_input


def assert_rpc_disabled(page, canvas, output: Path, name: str):
    submissions = []
    def observe_submission(request):
        if request.method == "POST" and request.url.endswith("/api/tasks"):
            submissions.append(request.post_data_json)
    page.on("request", observe_submission)
    try:
        page.keyboard.press("Control+Enter")
        settle_gesture(page)
        ui_control(page, "Analyze", role="action", enabled=False)
        click_control(page, canvas, "Analyze", role="action", enabled=False)
        assert not submissions, f"{name} allowed an RPC task without an available provider: {submissions}"
        canvas.screenshot(path=str(output / f"{name}.png"))
    finally:
        page.remove_listener("request", observe_submission)

def unconfigured_rpc_interactions(browser, url: str, output: Path):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    with page.expect_response(lambda response: response.url.endswith("/api/rpc-providers")) as catalogue:
        page.goto(url, wait_until="networkidle")
    assert catalogue.value.ok
    assert catalogue.value.json() == {"result": {"Ok": []}}
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    expect(status).to_contain_text("No RPC providers are configured on the server.")
    canvas = page.locator("#evm-canvas")
    bytecode = submit_bytecode(page, canvas, "600160020100", output / "rpc-empty-bytecode.png")
    assert bytecode.status == 200 and bytecode.report["status"] == "Converged"
    open_rpc_input(page, canvas, from_bytecode=True)
    assert_rpc_disabled(page, canvas, output, "rpc-empty-disabled")
    with page.expect_response(lambda response: response.url.endswith("/api/rpc-providers")) as retry:
        click_control(page, canvas, "Retry", role="action")
    assert retry.value.json() == {"result": {"Ok": []}}
    expect(status).to_contain_text("No RPC providers are configured on the server.")
    page.close()

    # Fail the initial catalogue, then delay a retry after bytecode is ready.
    # This starts the production fetch deadline immediately before the loading
    # assertions, rather than letting initial analysis consume that deadline.
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    held_catalogues = []
    catalogue_attempts = 0
    def delay_catalogue(route):
        nonlocal catalogue_attempts
        catalogue_attempts += 1
        if catalogue_attempts == 1:
            route.fulfill(status=503, content_type="application/json", body='{"result":{"Ok":[]}}')
        else:
            held_catalogues.append(route)
    page.route("**/api/rpc-providers", delay_catalogue)
    page.goto(url, wait_until="networkidle")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    expect(status).to_contain_text("Could not load RPC providers:")
    canvas = page.locator("#evm-canvas")
    bytecode = submit_bytecode(page, canvas, "600160020100", output / "rpc-failed-bytecode.png")
    assert bytecode.status == 200
    open_rpc_input(page, canvas, from_bytecode=True)
    assert_rpc_disabled(page, canvas, output, "rpc-failed-disabled")
    click_control(page, canvas, "Retry", role="action")
    expect(status).to_contain_text("Loading RPC providers")
    assert len(held_catalogues) == 1
    assert_rpc_disabled(page, canvas, output, "rpc-loading-disabled")
    expect(status).to_contain_text("Loading RPC providers")
    held_catalogues[0].fulfill(status=503, content_type="application/json", body='{"result":{"Ok":[]}}')
    expect(status).to_contain_text("Could not load RPC providers:")
    page.unroute("**/api/rpc-providers", delay_catalogue)
    with page.expect_response(lambda response: response.url.endswith("/api/rpc-providers")) as retry:
        click_control(page, canvas, "Retry", role="action")
    assert retry.value.ok and retry.value.json() == {"result": {"Ok": []}}
    expect(status).to_contain_text("No RPC providers are configured on the server.")
    canvas.screenshot(path=str(output / "rpc-retry-recovered.png"))
    page.close()
    return {"unconfigured_catalogue": [], "bytecode_available": True,
            "disabled_rpc_states": ["empty", "loading", "failed"],
            "checked_submission_paths": ["Analyze", "Control+Enter"],
            "catalogue_retry": "real server returned empty catalogue"}


def run(context):
    return unconfigured_rpc_interactions(context.browser, context.url, context.output)
