"""Example helpers browser acceptance helpers."""

from web_test_harness import capture_console_errors, complete_submission, observe_webgpu


def example_page(browser, url):
    page = browser.new_page(viewport={"width": 1440, "height": 1000}, color_scheme="light")
    observe_webgpu(page)
    capture_console_errors(page)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as admitted:
        page.goto(url, wait_until="networkidle")
    reply = complete_submission(page, admitted.value)
    assert reply.status == 200, f"initial analysis failed: {reply.status}"
    return page, page.locator("#evm-canvas"), reply
