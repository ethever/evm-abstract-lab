"""Compression browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import json
from web_test_hybrid import assert_hybrid, graph, job_result_report
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
        fetched = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.on("response", lambda response: fetched.append(response)
                if response.url.endswith("/result") and response.request.method == "GET" else None)
        try:
            with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                      and response.request.method == "POST") as submitted:
                page.goto(url, wait_until="networkidle")
            completed = complete_submission(page, submitted.value, namespace="Native")
            # The app can receive a closed Global publication before the final
            # report. Check the actual final browser response, including its
            # compression and resource timing, against the final task identity.
            publications = [(response, response.json()) for response in fetched if response.status == 200]
            final = [(response, envelope) for response, envelope in publications
                     if envelope["result"]["Ok"]["id"] == completed.job["id"]
                     and envelope["result"]["Ok"]["publication"] == completed.job["publication"]]
            assert final, "the Wasm app never fetched the final publication"
            response, envelope = final[-1]
            assert response.status == 200, response.text()
            request_headers = response.request.all_headers()
            assert request_headers.get("accept-encoding") == requested, request_headers
            headers = response.all_headers()
            observed = headers.get("content-encoding", "identity")
            assert observed == expected, (expected, headers)
            assert "accept-encoding" in headers.get("vary", "").lower(), headers
            assert headers["content-type"].startswith("application/json"), headers
            assert headers["cache-control"] == "no-store", headers
            report = job_result_report(envelope, job_id=completed.job["id"],
                                       publication=completed.job["publication"])
            assert report == completed.report
            assert_hybrid(report)
            assert report["schema_version"] == 21
            native = graph(report, "Native")
            assert "max_output_bytes" not in native["metadata"]["export_limits"]
            assert native["cfg"] and native["ssa"]["blocks"]
            expect(page.locator("#analysis-status")).to_contain_text("Ready:")
            resource = {"url": response.url, "decoded": len(response.body())}
            page.wait_for_function(
                "input => performance.getEntriesByName(input.url).some(entry => entry.responseEnd > 0 && entry.decodedBodySize === input.decoded)",
                arg=resource)
            sizes = page.evaluate("""input => {
                const entry = performance.getEntriesByName(input.url).find(entry => entry.responseEnd > 0 && entry.decodedBodySize === input.decoded);
                return {encoded: entry.encodedBodySize, decoded: entry.decodedBodySize};
            }""", resource)
            assert sizes["decoded"] == len(response.body()), sizes
            if expected == "identity":
                assert sizes["encoded"] == sizes["decoded"] > 0, sizes
            else:
                assert 0 < sizes["encoded"] < sizes["decoded"], sizes
            assert not errors, errors
            results[expected] = {"accepted": accepted, "request_accept_encoding": requested,
                                 "content_encoding": observed,
                                 "encoded_bytes": sizes["encoded"], "decoded_bytes": sizes["decoded"],
                                 "status": report["status"], "wasm_installed_report": True,
                                 "job_id": completed.job["id"], "publication": completed.job["publication"],
                                 "browser_publications": [item["result"]["Ok"]["publication"]
                                                          for _, item in publications]}
            (output / "compression-report.json").write_text(json.dumps(results, indent=2) + "\n")
            print(f"HTTP compression {expected}: {sizes['encoded']} wire bytes, "
                  f"{sizes['decoded']} decoded bytes; Wasm report ready", flush=True)
        finally:
            page.close()
    return results


def run(context):
    return compression_interactions(context.browser, context.url, context.output)
