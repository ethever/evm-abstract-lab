"""Rpc entry browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import json
import re
from web_test_harness import complete_submission, observe_webgpu, rendered_frame, settle_gesture, webgpu_evidence
from web_test_rpc_helpers import CUSTOM_ENTRY, RPC_ENTRY_EXAMPLES, RPC_PROVIDERS, automatic_rpc_entry, configured_rpc_server, open_rpc_input, select_rpc_provider
from web_test_hybrid import graph


def entry_example_interactions(browser, url: str, output: Path, providers, rpc):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    baseline = automatic_rpc_entry(page, url, providers[0], output, "examples-startup")
    canvas = page.locator("#evm-canvas")
    submitted = []
    page.on("request", lambda request: submitted.append(request.post_data_json)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    expected_submissions = []
    results = {"automatic_entry": baseline["input"], "selected_addresses": [],
               "selected_examples": [], "snapshot_pins": {}}
    # Keep the original token transitions, then exercise every new mainnet row.
    # The expanded popup retains 20-point controls and a 23-point row stride;
    # its 300-point height displays all eight presets and Custom without scrolling.
    for ordinal, menu_index in enumerate([1, 2, 0, *range(3, len(RPC_ENTRY_EXAMPLES))]):
        ui_label, address = RPC_ENTRY_EXAMPLES[menu_index]
        label = re.sub(r"[^a-z0-9]+", "-", ui_label.lower()).strip("-")
        open_rpc_input(page, canvas)
        if ordinal == 0:
            # The entry choice must preserve an independently chosen provider.
            select_rpc_provider(page, canvas, 1, output, "entry-weth")
            expect(page.locator("#analysis-status")).to_contain_text(
                f"RPC provider: {providers[1]['name']} ({providers[1]['endpoint']})")
        canvas.click(position={"x": 454, "y": 425})
        rendered_frame(page)
        canvas.screenshot(path=str(output / f"entry-{label}-options.png"))
        request_offset = len(rpc.requests)
        canvas.click(position={"x": 460, "y": 464 + menu_index * 23})
        settle_gesture(page)
        assert submitted == expected_submissions, f"choosing {ui_label} unexpectedly submitted analysis"
        assert len(rpc.requests) == request_offset, f"choosing {ui_label} unexpectedly contacted the provider"
        canvas.screenshot(path=str(output / f"entry-{label}-selected.png"))
        request_offset = len(rpc.requests)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
            page.keyboard.press("Control+Enter")
        reply = complete_submission(page, response.value, namespace="Native")
        assert reply.status == 200 and "Ok" in reply.json()["result"], reply.json()
        requests = rpc.requests[request_offset:]
        pins = sum(request["method"] == "eth_getBlockByNumber" and request["params"][0] == "latest" for request in requests)
        assert pins == 1, f"{label} must pin latest once: {requests}"
        results["snapshot_pins"][label] = pins
        expected = json.loads(json.dumps(baseline))
        expected["input"]["Rpc"]["provider_id"] = providers[1]["id"]
        expected["input"]["Rpc"]["address"] = address
        assert reply.request.post_data_json == expected, "entry selection reset unrelated request fields"
        expected_submissions.append(expected)
        assert submitted == expected_submissions, f"{ui_label} did not submit exactly once to the chosen provider"
        assert graph(reply.report, "Native")["metadata"]["entry_address"].lower() == address.lower()
        results["selected_addresses"].append(address)
        results["selected_examples"].append({"label": ui_label, "address": address})
    open_rpc_input(page, canvas)
    canvas.click(position={"x": 454, "y": 425})
    rendered_frame(page)
    canvas.screenshot(path=str(output / "entry-custom-options.png"))
    # Custom follows all presets plus the popup's separator.
    canvas.click(position={"x": 460, "y": 464 + len(RPC_ENTRY_EXAMPLES) * 23 + 9})
    settle_gesture(page)
    assert submitted == expected_submissions, "choosing Custom unexpectedly submitted analysis"
    canvas.click(position={"x": 600, "y": 448})
    page.keyboard.press("Control+A")
    page.keyboard.type(CUSTOM_ENTRY, delay=3)
    rendered_frame(page)
    canvas.screenshot(path=str(output / "entry-custom-edited.png"))
    page.keyboard.press("Escape")
    settle_gesture(page)
    open_rpc_input(page, canvas)
    canvas.screenshot(path=str(output / "entry-custom-reopened.png"))
    assert submitted == expected_submissions, "editing or reopening Custom unexpectedly submitted analysis"
    request_offset = len(rpc.requests)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
        page.keyboard.press("Control+Enter")
    reply = complete_submission(page, response.value, namespace="Native")
    assert reply.status == 200 and "Ok" in reply.json()["result"], reply.json()
    requests = rpc.requests[request_offset:]
    pins = sum(request["method"] == "eth_getBlockByNumber" and request["params"][0] == "latest" for request in requests)
    assert pins == 1, f"custom entry must pin latest once: {requests}"
    results["snapshot_pins"]["custom"] = pins
    expected = json.loads(json.dumps(baseline))
    expected["input"]["Rpc"]["provider_id"] = providers[1]["id"]
    expected["input"]["Rpc"]["address"] = CUSTOM_ENTRY
    assert reply.request.post_data_json == expected, "reopening the form replaced the hand-edited address"
    expected_submissions.append(expected)
    assert submitted == expected_submissions, "Custom did not submit exactly once to the chosen provider"
    assert graph(reply.report, "Native")["metadata"]["entry_address"].lower() == CUSTOM_ENTRY.lower()
    results["custom_reopened_address"] = CUSTOM_ENTRY
    results["provider_submissions"] = [request["input"] for request in submitted]
    results["renderer"] = webgpu_evidence(page)
    page.close()
    return results


def run(context):
    with configured_rpc_server(context.args) as (url, pending, rpc, providers):
        page = context.browser.new_page()
        catalogue = page.request.get(url + "/api/rpc-providers")
        assert catalogue.ok, catalogue.text()
        assert catalogue.json() == {"result": {"Ok": providers}}
        page.close()
        result = entry_example_interactions(context.browser, url, context.output, providers, rpc)
        result.update(providers=providers, selected_provider_id=RPC_PROVIDERS[1]["id"],
                      endpoint_displayed_in_selector=True, submission_uses_provider_id=True)
        return result
