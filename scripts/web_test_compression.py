"""Compression browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import json
from web_test_harness import complete_submission, observe_webgpu


def compression_interactions(browser, url: str, output: Path):
    """Exercise real HTTP negotiation and transparent decoding in the Wasm app."""
    cases = [("zstd", ["zstd", "br", "gzip"]),
             ("br", ["br", "gzip"]), ("gzip", ["gzip"]), ("identity", [])]
    results = {}
    for expected, accepted in cases:
        page = browser.new_page(viewport={"width": 1440, "height": 1000})
        requested = ", ".join(accepted) if accepted else "identity"
        page.set_extra_http_headers({"Accept-Encoding": requested})
        observe_webgpu(page)
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        try:
            with page.expect_response(lambda response: response.url.endswith("/result")
                                      and response.request.method == "GET") as fetched:
                with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                          and response.request.method == "POST") as submitted:
                    page.goto(url, wait_until="networkidle")
                completed = complete_submission(page, submitted.value)
            response = fetched.value
            assert response.status == 200, response.text()
            request_headers = response.request.all_headers()
            assert request_headers.get("accept-encoding") == requested, request_headers
            headers = response.all_headers()
            observed = headers.get("content-encoding", "identity")
            assert observed == expected, (expected, headers)
            assert "accept-encoding" in headers.get("vary", "").lower(), headers
            assert headers["content-type"].startswith("application/json"), headers
            assert headers["cache-control"] == "no-store", headers
            envelope = response.json()
            report = envelope["result"]["Ok"]
            assert report == completed.json()["result"]["Ok"]
            assert report["schema_version"] == 15
            assert "max_output_bytes" not in report["metadata"]["export_limits"]
            assert report["cfg"] and report["ssa"]["blocks"]
            expect(page.locator("#analysis-status")).to_contain_text("Ready:")
            page.wait_for_function(
                "url => performance.getEntriesByName(url).some(entry => entry.responseEnd > 0)",
                arg=response.url)
            sizes = page.evaluate("""url => {
                const entry = performance.getEntriesByName(url).find(entry => entry.responseEnd > 0);
                return {encoded: entry.encodedBodySize, decoded: entry.decodedBodySize};
            }""", response.url)
            assert sizes["decoded"] == len(response.body()), sizes
            if expected == "identity":
                assert sizes["encoded"] == sizes["decoded"] > 0, sizes
            else:
                assert 0 < sizes["encoded"] < sizes["decoded"], sizes
            assert not errors, errors
            results[expected] = {"accepted": accepted, "request_accept_encoding": requested,
                                 "content_encoding": observed,
                                 "encoded_bytes": sizes["encoded"], "decoded_bytes": sizes["decoded"],
                                 "status": report["status"], "wasm_installed_report": True}
            (output / "compression-report.json").write_text(json.dumps(results, indent=2) + "\n")
            print(f"HTTP compression {expected}: {sizes['encoded']} wire bytes, "
                  f"{sizes['decoded']} decoded bytes; Wasm report ready", flush=True)
        finally:
            page.close()
    return results


def run(context):
    return compression_interactions(context.browser, context.url, context.output)
