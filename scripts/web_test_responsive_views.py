"""Responsive views browser acceptance cases."""

from playwright.sync_api import expect
import hashlib
from web_test_example_helpers import example_page
from web_test_harness import rendered_frame, webgpu_evidence


def run_responsive_views(context):
    page, canvas, _ = example_page(context.browser, context.url)
    status = page.locator("#analysis-status")
    resize_requests = []
    page.on("request", lambda request: resize_requests.append(request.url)
            if request.url.endswith("/api/tasks") else None)
    viewports = [(1920, 1080), (1440, 900), (1024, 768), (768, 600),
                 (390, 844), (844, 390), (320, 480), (1440, 1000)]
    viewport_screenshots = {}
    for width, height in viewports:
        page.set_viewport_size({"width": width, "height": height})
        page.wait_for_function(
            "([w,h]) => { const c = document.querySelector('#evm-canvas'); "
            "return c.width === Math.round(w * devicePixelRatio) && "
            "c.height === Math.round(h * devicePixelRatio); }", arg=[width, height])
        for key, name in [("0", "workspace"), ("1", "disassembly"),
                          ("2", "cfg"), ("3", "ssa")]:
            page.keyboard.press(key)
            rendered_frame(page)
            screenshot = canvas.screenshot(path=str(context.output / f"resize-{width}x{height}-{name}.png"))
            assert len(screenshot) > 4000, f"blank {width}x{height} {name} view"
            viewport_screenshots[f"{width}x{height}-{name}"] = hashlib.sha256(screenshot).hexdigest()
        expect(status).to_contain_text("Converged")
        assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
        assert page.evaluate("document.documentElement.scrollHeight <= innerHeight")
    assert not resize_requests, "layout resizing unexpectedly reran analysis"
    result = {"viewport_screenshots": viewport_screenshots,
              "analysis_requests_after_startup": resize_requests,
              "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context):
    return run_responsive_views(context)
