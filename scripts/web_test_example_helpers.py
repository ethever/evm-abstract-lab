"""Example helpers browser acceptance helpers."""

from web_test_harness import capture_console_errors, complete_submission, hide_world_directory, observe_webgpu


def example_page(browser, url, *, namespace="Native", close_directory=True):
    page = browser.new_page(viewport={"width": 1440, "height": 1000}, color_scheme="light")
    observe_webgpu(page)
    capture_console_errors(page)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as admitted:
        page.goto(url, wait_until="networkidle")
    reply = complete_submission(page, admitted.value, namespace=namespace)
    assert reply.status == 200, f"initial analysis failed: {reply.status}"
    if close_directory:
        hide_world_directory(page, page.locator("#evm-canvas"))
    return page, page.locator("#evm-canvas"), reply
