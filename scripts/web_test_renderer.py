"""Startup failures retain their actual phase and cause without a renderer fallback."""

from pathlib import Path
from playwright.sync_api import expect
import re
from web_test_harness import observe_webgpu


def startup_failure(browser, url: str, output: Path, failure: str):
    page = browser.new_page(viewport={"width": 840, "height": 540})
    observe_webgpu(page, failure)
    if failure == "adapter-info":
        # API-present older browsers lack the getter read unconditionally by
        # wgpu 30 before requestDevice; detect this before entering Wasm.
        page.add_init_script("delete GPUAdapter.prototype.info")
    if failure == "text-agent":
        page.add_init_script("""(() => {
            const add = EventTarget.prototype.addEventListener;
            EventTarget.prototype.addEventListener = function(kind, listener, options) {
                if (this instanceof HTMLInputElement &&
                    this.previousElementSibling?.id === 'evm-canvas' &&
                    kind === 'keydown' && options === true)
                    throw new Error('Injected text-agent listener failure',
                                    {cause: new Error('Injected listener root cause')});
                return add.call(this, kind, listener, options);
            };
        })()""")
    if failure == "module-load":
        page.route("**/evm_abstract_web.js", lambda route: route.fulfill(status=503, body="Unavailable"))
    if failure == "wasm-init":
        page.route("**/evm_abstract_web_bg.wasm", lambda route: route.fulfill(
            content_type="application/wasm", body=b"invalid WebAssembly bytes"))
    requests = []
    page.on("request", lambda request: requests.append(request.url)
            if request.url.endswith("/api/tasks") else None)
    page.goto(url, wait_until="networkidle")
    boot = page.locator("#boot")
    status = page.locator("#analysis-status")
    expect(boot).to_be_visible()
    expect(boot).to_have_attribute("role", "alert", timeout=60000)
    phase, reason = {
        "missing-api": ("WebGPU availability check", "The WebGPU API is unavailable"),
        "adapter-info": ("WebGPU compatibility check", "GPUAdapter.info is unavailable"),
        "adapter-null": ("browser workspace initialization", re.compile(r"adapter|WebGPU|wgpu", re.IGNORECASE)),
        "device-rejected": ("browser workspace initialization", "Injected WebGPU device rejection"),
        "text-agent": ("browser workspace initialization", "Injected text-agent listener failure"),
        "module-load": ("JavaScript module loading", "Failed to fetch dynamically imported module"),
        "wasm-init": ("WebAssembly initialization", "WebAssembly"),
    }[failure]
    # Check the real failure before cosmetic wording: the old catch hid the
    # text-agent error even after a real GPU device had been created.
    expect(boot.locator("span")).to_contain_text(reason)
    expect(boot.locator("span")).to_contain_text(phase)
    expect(status).to_contain_text("Error: Unable to start workspace")
    expect(boot).not_to_contain_text("Check WebGPU support and graphics acceleration")
    details = page.locator("#startup-context")
    expect(details).to_contain_text(f"Stage: {phase}")
    expect(details).to_contain_text("WebGPU API: absent" if failure == "missing-api" else "WebGPU API: present")
    expect(details).to_contain_text(page.evaluate("navigator.userAgent"))
    if failure == "text-agent":
        expect(details).to_contain_text("Caused by: Error: Injected listener root cause")
    if failure == "adapter-info":
        expect(details).to_contain_text("GPUAdapter.info: absent")
    assert not requests, "analysis started despite workspace initialization failure"
    evidence = page.evaluate("window.__webgpuEvidence")
    if failure == "text-agent":
        assert evidence["devices"] > 0, "exercise input setup after actual GPU creation"
    else:
        assert evidence["devices"] == 0
    if failure == "adapter-null":
        assert evidence["adapter_requests"] > 0 and not evidence["adapters"]
    if failure == "device-rejected":
        assert evidence["adapters"] and evidence["device_requests"] > 0
    if failure in ["missing-api", "adapter-info", "module-load", "wasm-init"]:
        assert evidence["adapter_requests"] == 0
    assert not any(item["kind"] in ["webgl", "webgl2", "experimental-webgl"]
                   for item in evidence["contexts"]), f"failure attempted a WebGL fallback: {evidence}"
    page.locator("#startup-diagnostics summary").click()
    expect(page.locator("#copy-startup-context")).to_be_visible()
    if failure == "text-agent":
        page.context.grant_permissions(["clipboard-read", "clipboard-write"])
        page.locator("#copy-startup-context").click()
        expect(page.locator("#copy-startup-context")).to_have_text("Copied")
        assert page.evaluate("navigator.clipboard.readText()") == details.text_content()
    page.screenshot(path=str(output / f"startup-{failure}.png"))
    result = {"status": status.text_content(), "context": details.text_content(), "renderer": evidence}
    page.close()
    return result


def webgpu_unavailable(browser, url: str, output: Path):
    return {failure: startup_failure(browser, url, output, failure)
            for failure in ["missing-api", "adapter-info", "adapter-null", "device-rejected",
                            "text-agent", "module-load", "wasm-init"]}


def run(context):
    return webgpu_unavailable(context.browser, context.url, context.output)
