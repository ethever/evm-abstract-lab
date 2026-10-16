"""Responsive input browser acceptance cases."""

from playwright.sync_api import expect
from web_test_example_helpers import example_page
from web_test_harness import posted_bytecode, rendered_frame, submit_bytecode, webgpu_evidence


def run_responsive_input(context):
    page, canvas, _ = example_page(context.browser, context.url)
    status = page.locator("#analysis-status")
    responsive_input = {}
    for width, height in [(390, 844), (844, 390), (320, 480)]:
        page.set_viewport_size({"width": width, "height": height})
        rendered_frame(page)
        reply = submit_bytecode(page, canvas, "600160020100",
                                context.output / f"editor-{width}x{height}.png")
        assert reply.status == 200
        expect(status).to_contain_text("Converged")
        responsive_input[f"{width}x{height}"] = posted_bytecode(reply)
    result = {"responsive_input": responsive_input, "renderer": webgpu_evidence(page)}
    page.close()
    return result


def run(context):
    return run_responsive_input(context)
