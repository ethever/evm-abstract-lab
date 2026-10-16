"""Renderer browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
from web_test_harness import observe_webgpu


def webgpu_unavailable(browser, url: str, output: Path):
    results = {}
    for failure in ["missing-api", "adapter-null"]:
        page = browser.new_page(viewport={"width": 840, "height": 540})
        observe_webgpu(page, failure)
        requests = []
        page.on("request", lambda request: requests.append(request.url)
                if request.url.endswith("/api/tasks") else None)
        page.goto(url, wait_until="networkidle")
        boot = page.locator("#boot")
        status = page.locator("#analysis-status")
        expect(boot).to_be_visible()
        expect(boot).to_have_attribute("role", "alert", timeout=60000)
        expect(boot).to_contain_text("Unable to start WebGPU")
        expect(status).to_contain_text("Error: Unable to start WebGPU")
        if failure == "adapter-null":
            expect(boot).to_contain_text("could not create a usable WebGPU graphics device")
            expect(boot).not_to_contain_text("not requested")
        assert not requests, "analysis started despite WebGPU initialization failure"
        evidence = page.evaluate("window.__webgpuEvidence")
        assert evidence["devices"] == 0
        assert not any(item["kind"] in ["webgl", "webgl2", "experimental-webgl"]
                       for item in evidence["contexts"]), f"failure attempted a WebGL fallback: {evidence}"
        page.screenshot(path=str(output / f"webgpu-{failure}.png"))
        results[failure] = {"status": status.text_content(), "renderer": evidence}
        page.close()
    return results


def run(context):
    return webgpu_unavailable(context.browser, context.url, context.output)
