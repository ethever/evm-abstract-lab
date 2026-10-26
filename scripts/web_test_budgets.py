"""Budgets browser acceptance cases."""

from pathlib import Path
import json
from web_test_harness import complete_submission, observe_webgpu, open_input, rendered_frame, settle_gesture, webgpu_evidence
from web_test_layout_helpers import layout_modal
from web_test_hybrid import assert_hybrid, graph, job_result_report, oracle_graph
from web_test_pixels import text_pixels


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
    assert initial_report["schema_version"] == 21
    initial_report = oracle_graph(initial_report, "Native")
    assert initial_report["metadata"]["postprocessing_max_work"] == 2_000_000_000
    assert 0 <= initial_report["metadata"]["postprocessing_work"] <= 2_000_000_000
    assert "max_work" not in initial_report["metadata"]["export_limits"]
    for field, value in defaults.items():
        assert initial_request["limits"][field] == value, (field, initial_request["limits"])
        assert initial_report["metadata"]["limits"][field] == value

    canvas = page.locator("#evm-canvas")
    open_input(page)
    page.keyboard.press("Control+A")
    page.keyboard.type("600160020100", delay=5)
    rendered_frame(page)
    geometry, header_offset = {}, 10

    def save_geometry():
        (output / "large-budget-geometry.json").write_text(json.dumps(geometry, indent=2) + "\n")

    def capture(stage):
        page.mouse.move(2, 2)
        settle_gesture(page)
        page.wait_for_timeout(200)  # Finish native collapse/recentering animation.
        png = canvas.screenshot(path=str(output / f"large-budget-{stage}.png"))
        bounds = layout_modal(page, canvas)
        form = geometry[stage] = {"modal": bounds}
        save_geometry()
        assert (0 < bounds[0] < bounds[2] < 1440 and 0 < bounds[1] < bounds[3] < 1000
                and 750 <= bounds[2] - bounds[0] <= 780), (stage, bounds)
        region = (bounds[0] + 29, bounds[1] + header_offset, bounds[0] + 95, bounds[3] - 40)
        form.update(region=region, **text_pixels(page, png, region))
        save_geometry()
        return form

    def click(form, row, x):
        top, bottom = form["rows"][row]
        target = {"x": form["modal"][0] + x, "y": (top + bottom) / 2}
        form["target"] = {"row": row, **target}
        save_geometry()
        canvas.click(position=target)
        settle_gesture(page)

    before = capture("before")
    headers = before["rows"][-6:]
    assert len(headers) == 6 and all(20 <= second[0] - first[0] <= 26
                                   for first, second in zip(headers, headers[1:])), before
    header_offset = headers[0][0] - before["modal"][1] - 2
    header_positions = [row[0] - before["modal"][1] for row in headers[:4]]
    click(before, len(before["rows"]) - 3, 70)
    opened = capture("open")
    # Three environment headers, Execution, five numeric labels, summaries,
    # coverage, explanation, Precision and RPC; the anchored strip excludes Source.
    assert len(opened["rows"]) == 14, opened
    assert all(abs(row[0] - opened["modal"][1] - expected) <= 2
               for row, expected in zip(opened["rows"][:4], header_positions)), opened
    assert opened["rows"][-2][0] - opened["rows"][3][1] > 150, opened
    # Edit actual single-line controls. Values above 2^53 would be rounded
    # by DragValue/f64 or JavaScript Number, so compare raw HTTP with Python ints.
    edited = {"max_work": 9_007_199_254_740_993,
              "max_transfers": 9_007_199_254_740_995,
              "max_states": 500_000}
    for field, row in [("max_work", 6), ("max_transfers", 5), ("max_states", 4)]:
        form = capture(f"field-{field}")
        assert len(form["rows"]) == 14 and all(20 <= second[0] - first[0] <= 26
            for first, second in zip(form["rows"][4:8], form["rows"][5:9])), form
        click(form, row, 180)
        page.keyboard.press("Control+A")
        rendered_frame(page)
        page.keyboard.type(str(edited[field]), delay=5)
        rendered_frame(page)
    final = capture("input")
    assert len(final["rows"]) == 14 and all(abs(sum(after) / 2 - sum(before) / 2) <= 2
        for before, after in zip(opened["rows"], final["rows"])), final
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
    assert report["schema_version"] == 21
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
