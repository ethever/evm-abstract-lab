"""URL and cache witnesses through real browser navigation and native replies."""

from urllib.parse import parse_qs, urlencode, urlsplit

from playwright.sync_api import expect
from web_test_decompile_helpers import (assert_decompiler_isolated, decompile_admission,
                                       decompiler_page, origin)
from web_test_harness import rendered_frame


def source_url(base, source):
    return origin(base) + "/decompiler?" + urlencode({
        "sha256": source["sha256"], "config": source["config_sha256"],
    })


def assert_source_url(page, source):
    page.wait_for_function("""source => {
        const url = new URL(window.location.href);
        return url.pathname === '/decompiler' &&
            url.searchParams.get('sha256') === source.sha256 &&
            url.searchParams.get('config') === source.config_sha256;
    }""", arg=source)
    parsed = urlsplit(page.url)
    assert parse_qs(parsed.query) == {
        "sha256": [source["sha256"]], "config": [source["config_sha256"]],
    }, page.url
    assert_decompiler_isolated(page)


def opened_result(page, action, *, cache_hit):
    """A full document load must restore its state through the public open API."""
    with page.expect_response(lambda response: response.request.method == "GET"
                              and "/api/decompile/tasks/" in response.url
                              and response.url.endswith("/result") and response.status == 200,
                              timeout=60000) as publication:
        with page.expect_response(lambda response: response.request.method == "POST"
                                  and urlsplit(response.url).path == "/api/decompile/open",
                                  timeout=60000) as admitted:
            action()
    request, job, source = decompile_admission(admitted.value)
    result = publication.value.json()["result"]["Ok"]
    assert result["id"] == job["id"]
    assert source["cache_hit"] is cache_hit, source
    if cache_hit:
        assert job["state"] == "Completed", job
    artifact = result["artifact"]
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Workspace: Decompiler")
    expect(status).to_contain_text("Decompile task: Ready", timeout=60000)
    expect(status).to_contain_text("Artifact: " + artifact["id"])
    expect(status).to_contain_text("Code SHA-256: " + source["sha256"])
    expect(status).to_contain_text("Config SHA-256: " + source["config_sha256"])
    expect(status).to_contain_text("Result cache: " + ("hit" if cache_hit else "miss"))
    assert artifact["input"]["fork"] == request["fork"].lower()
    assert_source_url(page, source)
    rendered_frame(page)
    return artifact, source, {"request": request, "job": job, "source": source,
                              "publication": result["publication"]}


def open_cached(context, base, source, *, expected_artifact=None):
    page = decompiler_page(context)
    artifact, restored, evidence = opened_result(
        page, lambda: page.goto(source_url(base, source), wait_until="networkidle"),
        cache_hit=True,
    )
    assert restored["sha256"] == source["sha256"]
    assert restored["config_sha256"] == source["config_sha256"]
    if expected_artifact is not None:
        assert artifact == expected_artifact
    return page, artifact, restored, evidence


def history_result(page, action, artifact, source):
    """History may reuse retained state; any server reopen must be a cache hit."""
    replies = []

    def receive(response):
        if (response.request.method == "POST"
                and urlsplit(response.url).path == "/api/decompile/open"):
            replies.append(response)

    page.on("response", receive)
    try:
        action()
        expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + artifact["id"], timeout=60000)
        expect(page.locator("#analysis-status")).to_contain_text("Decompile task: Ready")
        assert_source_url(page, source)
        rendered_frame(page)
        admissions = [decompile_admission(reply) for reply in replies]
        assert all(item[2]["cache_hit"] for item in admissions), admissions
        assert all(item[2]["sha256"] == source["sha256"]
                   and item[2]["config_sha256"] == source["config_sha256"]
                   for item in admissions), admissions
        return {"server_reopens": len(admissions), "artifact": artifact["id"], "source": source}
    finally:
        page.remove_listener("response", receive)
