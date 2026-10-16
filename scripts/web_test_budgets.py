"""Budgets browser acceptance cases."""

from pathlib import Path
import json
from web_test_harness import complete_submission, observe_webgpu, open_input, rendered_frame, settle_gesture, webgpu_evidence


def budget_interactions(browser, url: str, output: Path):
    """Real decimal editing preserves u64 values through WASM, HTTP and metadata."""
    defaults = {
        "max_states": 100_000,
        "max_transfers": 10_000_000,
        "max_work": 1_000_000_000_000,
        "max_call_depth": 1025,
        "max_memory_bytes": 64 * 1024 * 1024,
        "context_depth": 128,
        "max_constants": 512,
        "reduction_rounds": 16,
        "max_facts": 4096,
        "max_constraints": 2048,
        "max_expression_nodes": 16_384,
        "max_expression_depth": 256,
        "rpc_max_accounts": 4096,
        "rpc_max_requests": 1_000_000,
        "rpc_max_response_bytes": 64 * 1024 * 1024,
        "rpc_timeout_ms": 120_000,
    }
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    # Unbounded export accounting can exceed Wasm's usize and JavaScript's
    # exact Number range. Let the real Wasm client decode an exact u64 total.
    reported_export_work = 9_007_199_254_740_993
    decoded_export_work = []

    def large_export_work(route):
        response = route.fetch()
        envelope = response.json()
        metadata = envelope["result"]["Ok"]["metadata"]
        assert "max_work" not in metadata["export_limits"], metadata
        metadata["export_usage"]["work"] = reported_export_work
        # The intercepted body was decoded before editing. Do not label the
        # replacement plain JSON with the upstream response's content coding.
        headers = dict(response.headers)
        headers.pop("content-encoding", None)
        headers.pop("content-length", None)
        route.fulfill(status=response.status, headers=headers, body=json.dumps(envelope))
        decoded_export_work.append(reported_export_work)

    page.route("**/api/tasks/*/result", large_export_work)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    initial_reply = complete_submission(page, initial.value)
    assert initial_reply.status == 200, initial_reply.json()
    initial_request = json.loads(initial_reply.request.post_data)
    initial_report = initial_reply.json()["result"]["Ok"]
    assert initial_report["schema_version"] == 14
    assert "max_work" not in initial_report["metadata"]["export_limits"]
    for field, value in defaults.items():
        assert initial_request["limits"][field] == value, (field, initial_request["limits"])
        assert initial_report["metadata"]["limits"][field] == value

    canvas = page.locator("#evm-canvas")
    open_input(page)
    page.keyboard.press("Control+A")
    page.keyboard.type("600160020100", delay=5)
    rendered_frame(page)
    canvas.click(position={"x": 419, "y": 559})  # Execution budget
    settle_gesture(page)
    # Edit actual single-line controls. Values above 2^53 would be rounded
    # by DragValue/f64 or JavaScript Number, so compare raw HTTP with Python ints.
    edited = {"max_work": 9_007_199_254_740_993,
              "max_transfers": 9_007_199_254_740_995,
              "max_states": 500_000}
    for field, y in [("max_work", 559), ("max_transfers", 536), ("max_states", 513)]:
        canvas.click(position={"x": 510, "y": y})
        page.keyboard.press("Control+A")
        rendered_frame(page)
        page.keyboard.type(str(edited[field]), delay=5)
        rendered_frame(page)
    canvas.screenshot(path=str(output / "large-budget-input.png"))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as submitted:
        page.keyboard.press("Control+Enter")
    reply = complete_submission(page, submitted.value)
    assert reply.admission_status == 202, reply.json()
    assert reply.status == 200, reply.json()
    request = json.loads(reply.request.post_data)
    report = reply.json()["result"]["Ok"]
    assert request["input"]["Bytecode"]["bytecode"] == "600160020100"
    assert report["schema_version"] == 14
    assert report["status"] == "Converged", report["frontiers"]
    assert "max_work" not in report["metadata"]["export_limits"]
    expected = {**defaults, **edited}
    for field, value in expected.items():
        assert request["limits"][field] == value, (field, request["limits"])
        assert report["metadata"]["limits"][field] == value, (field, report["metadata"])
    assert not errors, errors
    assert len(decoded_export_work) >= 2, decoded_export_work
    canvas.screenshot(path=str(output / "large-budget-result.png"))
    (output / "large-budget-request.json").write_text(reply.request.post_data + "\n")
    (output / "large-budget-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    result = {"defaults": defaults, "edited": edited, "status": report["status"],
              "metadata_preserves_exact_integers": True,
              "wasm_decoded_export_work": reported_export_work,
              "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context):
    return budget_interactions(context.browser, context.url, context.output)
