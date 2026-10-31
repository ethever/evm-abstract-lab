"""Budgets browser acceptance cases."""

from pathlib import Path
import json
from web_test_harness import complete_submission, edit_control, observe_webgpu, open_input, set_control, webgpu_evidence
from web_test_hybrid import assert_hybrid, graph, job_result_report, oracle_graph


def budget_interactions(browser, url: str, output: Path):
    """Real decimal editing preserves u64 values through WASM, HTTP and metadata."""
    defaults = {
        "max_states": 100_000,
        "max_transfers": 10_000_000,
        "max_work": 2_000_000_000,
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
        "root_terminal_coverage": True,
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
        report = job_result_report(envelope)
        rewritten = False
        for namespace in ["Baseline", "Native"]:
            selected = graph(report, namespace)
            if selected is not None:
                metadata = selected["metadata"]
                assert "max_work" not in metadata["export_limits"], metadata
                metadata["export_usage"]["work"] = reported_export_work
                rewritten = True
        # Minimum export usage is paid and bounded by its actual application
        # allowance. This u64 control edits only informational rich counters.
        # The intercepted body was decoded before editing. Do not label the
        # replacement plain JSON with the upstream response's content coding.
        headers = dict(response.headers)
        headers.pop("content-encoding", None)
        headers.pop("content-length", None)
        route.fulfill(status=response.status, headers=headers, body=json.dumps(envelope))
        if rewritten:
            decoded_export_work.append(reported_export_work)

    page.route("**/api/tasks/*/result", large_export_work)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    initial_reply = complete_submission(page, initial.value, namespace="Native")
    assert initial_reply.status == 200, initial_reply.json()
    initial_request = json.loads(initial_reply.request.post_data)
    initial_report = initial_reply.report
    assert_hybrid(initial_report)
    assert initial_report["schema_version"] == 22
    initial_report = oracle_graph(initial_report, "Native")
    assert initial_report["metadata"]["postprocessing_max_work"] == 2_000_000_000
    assert 0 <= initial_report["metadata"]["postprocessing_work"] <= 2_000_000_000
    assert "max_work" not in initial_report["metadata"]["export_limits"]
    for field, value in defaults.items():
        assert initial_request["limits"][field] == value, (field, initial_request["limits"])
        assert initial_report["metadata"]["limits"][field] == value

    canvas = page.locator("#evm-canvas")
    open_input(page, mode="Bytecode")
    edit_control(page, canvas, "Runtime bytecode", "600160020100")
    geometry = {"section": set_control(page, canvas, "Execution budget", True, role="section")}
    # Above 2^53, DragValue/f64 and JS Number would round these. The real
    # native editors submit decimal text; Python compares the raw HTTP ints.
    edited = {"max_work": 9_007_199_254_740_993,
              "max_transfers": 9_007_199_254_740_995,
              "max_states": 500_000}
    for field, label in [("max_work", "Logical work"), ("max_transfers", "Transfers"),
                         ("max_states", "States")]:
        geometry[field] = edit_control(page, canvas, label, edited[field])
    canvas.screenshot(path=str(output / "large-budget-input.png"))
    (output / "large-budget-geometry.json").write_text(json.dumps(geometry, indent=2) + "\n")
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as submitted:
        page.keyboard.press("Control+Enter")
    reply = complete_submission(page, submitted.value, namespace="Native")
    assert reply.admission_status == 202, reply.json()
    assert reply.status == 200, reply.json()
    request = json.loads(reply.request.post_data)
    report = reply.report
    assert request["input"]["Bytecode"]["bytecode"] == "600160020100"
    assert_hybrid(report)
    assert report["schema_version"] == 22
    report = oracle_graph(report, "Native")
    # This independent finite allowance does not inherit the edited execution
    # budget or the deliberately huge informational export work counter.
    assert report["metadata"]["postprocessing_max_work"] == 2_000_000_000
    assert 0 <= report["metadata"]["postprocessing_work"] <= 2_000_000_000
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
