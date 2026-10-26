"""Responsive views browser acceptance cases."""

from playwright.sync_api import expect
import hashlib
from web_test_example_helpers import example_page
from web_test_graph_helpers import compare_tab, default_entry_cfg, source_content_snapshot, workspace_route
from web_test_harness import capture_console_errors, complete_submission, measured_backend_timings, observe_webgpu, resize_view, select_report_namespace, set_control, ui_control, webgpu_evidence
from web_test_hybrid import native_entry_context, oracle_graph, source_program


def run_responsive_views(context):
    page, canvas, reply = example_page(context.browser, context.url, namespace=None, close_directory=False)
    report = reply.report
    entries = {"desktop": default_entry_cfg(page, canvas, report, context.output, "responsive-default-desktop")}
    # A fresh phone entry exercises initial panel defaults. Resizing a desktop
    # session must retain explicit panel preferences and cannot prove this.
    narrow = context.browser.new_page(viewport={"width": 320, "height": 480})
    observe_webgpu(narrow)
    capture_console_errors(narrow)
    try:
        with narrow.expect_response(lambda response: response.url.endswith("/api/tasks")
                                    and response.request.method == "POST") as initial:
            narrow.goto(context.url, wait_until="networkidle")
        narrow_reply = complete_submission(narrow, initial.value)
        assert narrow_reply.status == 200
        entries["320x480"] = default_entry_cfg(narrow, narrow.locator("#evm-canvas"), narrow_reply.report,
                                             context.output, "responsive-default-320x480")
    finally:
        narrow.close()
    status = page.locator("#analysis-status")
    resize_requests = []
    page.on("request", lambda request: resize_requests.append(request.url)
            if request.url.endswith("/api/tasks") else None)
    select_report_namespace(page, report, "Native")
    measured_backend_timings(report)
    set_control(page, canvas, "Inspect", False)
    native = oracle_graph(report, "Native")
    root = native_entry_context(report)
    program = source_program(report, root["program"])
    block = next(block for block in native["ssa"]["blocks"] if block["state"] == root["id"])
    viewports = [(1920, 1080), (1440, 900), (1024, 768), (768, 600),
                 (390, 844), (844, 390), (320, 480), (1440, 1000)]
    viewport_screenshots = {}
    for width, height in viewports:
        resize_view(page, width, height)
        for route, name in [("Compare", "workspace"), ("Code", "disassembly"),
                            ("CFG", "cfg"), ("SSA", "ssa")]:
            workspace_route(page, route)
            if route == "Compare" and width < 1120:
                compare_tab(page, canvas, "SSA")
            if route == "Code" or (route == "Compare" and width >= 1120):
                source_content_snapshot(page, canvas, program, context.output, f"resize-{width}x{height}-{name}-code")
            if route in {"Compare", "SSA"}:
                source_content_snapshot(page, canvas, program, context.output, f"resize-{width}x{height}-{name}-ssa", ssa=block)
            if route == "CFG":
                ui_control(page, "CFG canvas", role="graph-canvas")
            screenshot = canvas.screenshot(path=str(context.output / f"resize-{width}x{height}-{name}.png"))
            viewport_screenshots[f"{width}x{height}-{name}"] = hashlib.sha256(screenshot).hexdigest()
        if width == 320:
            set_control(page, canvas, "Sources", True)
            ui_control(page, "Inspect", role="toggle", selected=False)
            set_control(page, canvas, "Inspect", True)
            ui_control(page, "Sources", role="toggle", selected=False)
            set_control(page, canvas, "Inspect", False)
        expect(status).to_contain_text("Converged")
        assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
        assert page.evaluate("document.documentElement.scrollHeight <= innerHeight")
    assert not resize_requests, "layout resizing unexpectedly reran analysis"
    result = {"viewport_screenshots": viewport_screenshots, "default_entries": entries,
              "analysis_requests_after_startup": resize_requests,
              "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context):
    return run_responsive_views(context)
