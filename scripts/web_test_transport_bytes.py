"""Exercise the actual Wasm decoder above the browser string-size boundary."""

import json
from playwright.sync_api import expect
from web_test_decompile_helpers import observed, prepare
from web_test_harness import capture_console_errors, click_control, observe_webgpu
from web_test_response_bytes import byte_probe_result, install_byte_probe

PADDING_BYTES = 513 * 1024 * 1024


def run(context):
    page = context.browser.new_page(viewport={"width": 1920, "height": 1080})
    page.decompile_output = context.output
    install_byte_probe(page, PADDING_BYTES)
    observe_webgpu(page)
    capture_console_errors(page)
    page.goto(context.url, wait_until="networkidle")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    click_control(page, canvas, "Decompiler", role="tab")
    prepare(page, canvas, "600160005500")
    with page.expect_response(lambda response: "/api/decompile/tasks/" in response.url
                              and response.url.endswith("/result") and response.status == 200,
                              timeout=60000) as completed:
        click_control(page, canvas, "Decompile", role="action")
    # This is the original small network body. Only the app-facing byte stream
    # is padded; the test driver never materializes a giant JSON string either.
    response = completed.value
    original = response.json()["result"]["Ok"]
    artifact = original["artifact"]
    expect(status).to_contain_text("Artifact: " + artifact["id"], timeout=60000)
    expect(status).to_contain_text("Decompile task: Ready")
    code = [row["label"] for row in observed(page, "decompile-code")]
    assert any("evm.sstore(slot = 0x0, value = 0x1);" in row for row in code), code
    assert any("evm.stop();" in row for row in code), code
    metrics = byte_probe_result(page)
    large = [entry for entry in metrics["arrays"] if entry["url"] == response.url]
    assert len(large) == 1, large
    assert large[0]["bytes"] == PADDING_BYTES + len(response.body()), large
    assert any("/api/tasks/" in entry["url"] and entry["url"].endswith("/result")
               for entry in metrics["arrays"]), metrics
    canvas.screenshot(path=str(context.output / "large-byte-result.png"))

    # A real malformed body must still produce a decoding error and retain the
    # last accepted artifact, instead of silently replacing it with empty data.
    pattern = "**/api/decompile/tasks/*/result"
    page.route(pattern, lambda route: route.fulfill(status=200,
               content_type="application/json", body=b"{"))
    prepare(page, canvas, "00")
    click_control(page, canvas, "Decompile", role="action")
    expect(status).to_contain_text("Invalid response (HTTP 200):", timeout=60000)
    expect(status).to_contain_text("Artifact: " + artifact["id"])
    final = byte_probe_result(page)
    assert any(entry["bytes"] == 1 and "/api/decompile/" in entry["url"]
               for entry in final["arrays"]), final
    page.unroute(pattern)
    result = {"padding_bytes": PADDING_BYTES, "decoded_bytes": large[0]["bytes"],
              "artifact": artifact["id"], "byte_reads": final,
              "malformed_retains_previous_artifact": True}
    (context.output / "byte-transport.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result
