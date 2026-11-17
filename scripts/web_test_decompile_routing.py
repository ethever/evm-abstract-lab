"""Direct routes, exact cache variants, browser history and durable restoration."""

import json
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.parse import urlencode, urlsplit
import time

from playwright.sync_api import expect
from web_test_decompile_helpers import (assert_decompiler_isolated, decompiler_page,
                                       open_decompiler, origin, submit)
from web_test_decompile_links import (assert_source_url, history_result, open_cached,
                                    opened_result)
from web_test_harness import (analysis_server, consume_http_error, expect_http_error,
                              rendered_frame, webgpu_evidence)


FIRST = "600160020100"
SECOND = "600360040100"


def trailing_slash_entry(context, base):
    page = decompiler_page(context)
    resources = []
    page.on("response", lambda response: resources.append(response)
            if urlsplit(response.url).path.endswith((".js", ".wasm")) else None)
    response = page.goto(origin(base) + "/decompiler/", wait_until="networkidle")
    assert response is not None and response.ok
    expect(page.locator("#analysis-status")).to_contain_text("Workspace: Decompiler", timeout=60000)
    rendered_frame(page)
    assert_decompiler_isolated(page)
    observed = [{"path": urlsplit(response.url).path, "status": response.status,
                 "mime": response.headers.get("content-type", "").split(";", 1)[0]}
                for response in resources]
    assert any(item["path"] == "/evm_abstract_web.js" and item["status"] == 200
               and item["mime"] in {"application/javascript", "text/javascript"} for item in observed), observed
    assert any(item["path"].endswith(".wasm") and item["status"] == 200
               and item["mime"] == "application/wasm" for item in observed), observed
    assert all(not item["path"].startswith("/decompiler/") for item in observed), observed
    page.close()
    return observed


def late_history_reply(page, first, first_source, second, second_source):
    """A completed old lookup cannot publish after the user navigates forward."""
    held, requests = [], []
    pattern = "**/api/decompile/open"

    def capture(route):
        posted = route.request.post_data_json
        if posted.get("source", {}).get("sha256", {}).get("sha256") != first_source["sha256"]:
            route.continue_()
            return
        response = route.fetch()
        value = response.json()["result"]["Ok"]
        assert value["source"]["cache_hit"]
        held.append((route, response.status, response.headers, response.body(), value["snapshot"]["id"]))

    def observe(request):
        requests.append(request.url)

    page.route(pattern, capture)
    page.on("request", observe)
    try:
        page.go_back()
        deadline = time.monotonic() + 60
        while not held:
            assert time.monotonic() < deadline, "back navigation did not issue its source lookup"
            page.wait_for_timeout(25)
        history_result(page, page.go_forward, second, second_source)
        route, status, headers, body, obsolete = held.pop()
        headers = {key: value for key, value in headers.items()
                   if key.lower() not in {"content-encoding", "content-length"}}
        with page.expect_response(lambda response: response.request.method == "POST"
                                  and urlsplit(response.url).path == "/api/decompile/open"):
            route.fulfill(status=status, headers=headers, body=body)
        rendered_frame(page)
        rendered_frame(page)
        expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + second["id"])
        expect(page.locator("#analysis-status")).to_contain_text("Decompile task: Ready")
        assert_source_url(page, second_source)
        assert not any(urlsplit(url).path == f"/api/decompile/tasks/{obsolete}/result" for url in requests), requests
        return {"obsolete_task": obsolete, "retained_artifact": second["id"],
                "ignored_artifact": first["id"], "requested_urls": requests}
    finally:
        for route, status, headers, body, _ in held:
            route.fulfill(status=status, headers={key: value for key, value in headers.items()
                          if key.lower() not in {"content-encoding", "content-length"}}, body=body)
        page.unroute(pattern, capture)
        page.remove_listener("request", observe)


def rejected_links(context, base):
    results = []
    missing = "f" * 64
    page = decompiler_page(context)
    expected = expect_http_error(page, method="POST", url=origin(base) + "/api/decompile/open", status=404)
    with page.expect_response(lambda response: response.request.method == "POST"
                              and urlsplit(response.url).path == "/api/decompile/open") as received:
        page.goto(origin(base) + "/decompiler?sha256=" + missing, wait_until="networkidle")
    error = received.value.json()["result"]["Err"]
    assert received.value.status == 404 and error["code"] == "NotFound", error
    expect(page.locator("#analysis-status")).to_contain_text("Workspace: Decompiler")
    expect(page.locator("#analysis-status")).to_contain_text(error["message"])
    assert_decompiler_isolated(page)
    results.append({"kind": "cache-miss", "error": error,
                    "http_error": consume_http_error(page, expected)})
    page.close()

    for query in ["sha256=not-a-hash", "sha256=" + missing + "&address=0x" + "11" * 20]:
        page = decompiler_page(context)
        requests = []
        page.on("request", lambda request: requests.append(request.url)
                if urlsplit(request.url).path in {"/api/decompile/open", "/api/decompile/tasks", "/api/tasks"} else None)
        page.goto(origin(base) + "/decompiler?" + query, wait_until="networkidle")
        status = page.locator("#analysis-status")
        expect(status).to_contain_text("Workspace: Decompiler")
        expect(status).to_contain_text("Invalid decompiler link:")
        rendered_frame(page)
        assert not requests, (query, requests)
        assert_decompiler_isolated(page)
        results.append({"kind": "invalid-link", "query": query, "requests": requests})
        page.close()
    return results


def run(context):
    evidence = {}
    with TemporaryDirectory(prefix="decompiler-durable-browser-") as directory:
        cache = Path(directory) / "cache"
        with analysis_server(context.args, decompile_cache_dir=cache) as first_server:
            evidence["trailing_slash_assets"] = trailing_slash_entry(context, first_server)
            page, canvas, _, _ = open_decompiler(context, first_server)
            first, _ = submit(context, page, canvas, FIRST, "routing-first")
            first_source = page.decompile_last_source
            assert first_source is not None and not first_source["cache_hit"], first_source
            assert first_source["origin"] is None
            assert_source_url(page, first_source)

            second, _ = submit(context, page, canvas, SECOND, "routing-second")
            second_source = page.decompile_last_source
            assert not second_source["cache_hit"] and first_source["sha256"] != second_source["sha256"]
            assert_source_url(page, second_source)
            evidence["back"] = history_result(page, page.go_back, first, first_source)
            evidence["forward"] = history_result(page, page.go_forward, second, second_source)
            restored, _, reload_evidence = opened_result(page, lambda: page.reload(wait_until="networkidle"), cache_hit=True)
            assert restored == second
            evidence["reload"] = reload_evidence
            # Reload clears the old document's retained A state, so Back must
            # genuinely reopen A before its response can be delayed here.
            evidence["late_reply"] = late_history_reply(page, first, first_source, second, second_source)

            # The same bytes with a different fork must have a distinct variant.
            variant, _ = submit(context, page, canvas, FIRST, "routing-cancun", fork="Cancun")
            variant_source = page.decompile_last_source
            assert not variant_source["cache_hit"]
            assert variant_source["sha256"] == first_source["sha256"]
            assert variant_source["config_sha256"] != first_source["config_sha256"]
            assert variant["input"]["fork"] == "cancun" and first["input"]["fork"] == "osaka"
            assert_source_url(page, variant_source)

            fresh, restored, _, fresh_evidence = open_cached(context, first_server, first_source, expected_artifact=first)
            assert fresh.context != page.context, "a fresh browser context must not borrow the original page's state"
            assert restored["input"]["fork"] == "osaka"
            evidence["fresh_context"] = fresh_evidence
            fresh.locator("#evm-canvas").screenshot(path=str(context.output / "fresh-context-cache-hit.png"))
            fresh.close()
            evidence["renderer"] = webgpu_evidence(page)
            assert_decompiler_isolated(page)
            page.close()

        # A different listening port ensures restoration cannot come from an old
        # task URL. Only the same binary's content cache survives this restart.
        with analysis_server(context.args, decompile_cache_dir=cache) as second_server:
            page, restored, _, restart_evidence = open_cached(context, second_server, variant_source, expected_artifact=variant)
            assert restored["input"]["fork"] == "cancun"
            evidence["server_restart"] = restart_evidence
            page.locator("#evm-canvas").screenshot(path=str(context.output / "server-restart-cached-variant.png"))
            # A bare hash selects current defaults, never the last saved variant.
            bare = origin(second_server) + "/decompiler?" + urlencode({"sha256": first_source["sha256"]})
            restored, source, bare_evidence = opened_result(page, lambda: page.goto(bare, wait_until="networkidle"), cache_hit=True)
            assert restored == first and source["config_sha256"] == first_source["config_sha256"]
            evidence["bare_hash_defaults"] = bare_evidence
            repeated, request = submit(context, page, page.locator("#evm-canvas"), FIRST, "routing-restored-settings")
            assert repeated == first and request["fork"] == "Osaka"
            assert page.decompile_last_source["cache_hit"]
            assert_source_url(page, first_source)
            assert_decompiler_isolated(page)
            page.close()
            evidence["rejected"] = rejected_links(context, second_server)

    (context.output / "routing-cache-evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    return evidence
