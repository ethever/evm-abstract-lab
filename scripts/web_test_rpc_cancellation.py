"""Rpc cancellation browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import re
from web_test_harness import click_control, observe_webgpu
from web_test_rpc_helpers import automatic_rpc_entry, configured_rpc_server, submit_rpc, visible_seconds, wait_for_job


def cancellation_interactions(browser, url: str, output: Path, rpc, providers):
    results = {}
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    startup = automatic_rpc_entry(page, url, providers[0], output, "cancel-startup")
    rpc.block_method = "eth_getCode"
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    page.evaluate("""() => {
        window.taskStatusHistory = [];
        new MutationObserver(() => window.taskStatusHistory.push(document.querySelector('#analysis-status').textContent))
            .observe(document.querySelector('#analysis-status'), {childList:true,subtree:true,characterData:true});
    }""")
    held_polls = []
    aborted = []
    def hold_first_poll(route):
        if route.request.method == "GET" and re.search(r"/api/tasks/\d+$", route.request.url) and not held_polls:
            held_polls.append(route)
        else:
            route.continue_()
    page.route("**/api/tasks/*", hold_first_poll)
    page.on("requestfailed", lambda request: aborted.append({"url": request.url, "failure": request.failure}))
    admitted = submit_rpc(page, canvas, rpc, output, "cancel", provider_index=0, providers=providers)
    assert admitted.status == 202, admitted.text()
    assert rpc.started.wait(5), "fixture did not enter a real pending RPC request"
    expect(status).to_contain_text("showing previous result")
    for _ in range(50):
        if held_polls:
            break
        page.wait_for_timeout(100)
    assert held_polls, "fixture did not hold an actual browser status fetch"
    canvas.screenshot(path=str(output / "cancel-pending.png"))
    with page.expect_response(lambda response: response.request.method == "DELETE" and "/api/tasks/" in response.url) as cancelled:
        click_control(page, canvas, "Cancel", role="action")
    assert cancelled.value.ok
    assert cancelled.value.json()["result"]["Ok"]["state"] == "Cancelling"
    assert rpc.disconnected.wait(5), "cooperative cancellation did not close pending acquisition"
    expect(status).to_contain_text("Cancelled", timeout=22000)
    assert any("/api/tasks/" in failure["url"] for failure in aborted), "hung HTTP poll was never aborted"
    history = page.evaluate("window.taskStatusHistory")
    assert any("Cancelling" in text for text in history), history
    assert "showing previous result" in status.text_content()
    cancelled_wait = visible_seconds(page, "Total wait")
    cancelled_id = admitted.json()["result"]["Ok"]["id"]
    frozen_cancelled = wait_for_job(page, url, cancelled_id, lambda job: job["state"] == "Cancelled")
    page.wait_for_timeout(300)
    assert visible_seconds(page, "Total wait") == cancelled_wait, "cancelled user wait kept advancing"
    assert wait_for_job(page, url, cancelled_id, lambda job: job["state"] == "Cancelled")["timings"] == frozen_cancelled["timings"]
    canvas.screenshot(path=str(output / "cancel-acknowledged.png"))
    results["cancellation"] = {"history": history, "pending_rpc_closed": rpc.disconnected.is_set(), "aborted_http": aborted, "status": status.text_content(), "total_wait_s": cancelled_wait, "backend_timings": frozen_cancelled["timings"]}
    # Resolve the test harness interception after the application itself
    # proved it aborted/recovered, so Playwright has no pending handler.
    held_polls[0].abort()
    page.unroute_all(behavior="wait")
    page.close()
    results["automatic_entry"] = startup["input"]
    return results


def run(context):
    with configured_rpc_server(context.args) as (url, pending, rpc, providers):
        return cancellation_interactions(context.browser, url, context.output, pending, providers)
