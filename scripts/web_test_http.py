"""Http browser acceptance cases."""

from pathlib import Path
import json
import time
from web_test_harness import complete_submission, measured_backend_timings, observe_webgpu, webgpu_evidence


def http_transport_interactions(browser, url: str, output: Path):
    """Use real page fetches for HTTP persistence and typed route boundaries."""
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    network = page.context.new_cdp_session(page)
    tagged_requests = {}
    connections = []

    def tag_request(event):
        headers = {name.lower(): value for name, value in event["request"].get("headers", {}).items()}
        tag = headers.get("x-evm-http-case")
        if tag is not None:
            tagged_requests[event["requestId"]] = tag

    def observe_response(event):
        tag = tagged_requests.get(event["requestId"])
        if tag is not None:
            response = event["response"]
            connections.append({"case": tag, "connection_id": response["connectionId"],
                                "reused": response["connectionReused"], "status": response["status"]})

    network.on("Network.requestWillBeSent", tag_request)
    network.on("Network.responseReceived", observe_response)
    network.send("Network.enable")
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    complete_submission(page, initial.value)
    request = initial.value.request.post_data_json

    def fetch(path, *, method="GET", body=None, content_type=None, tag=None):
        response = page.evaluate("""async input => {
            const headers = {};
            if (input.content_type) headers['Content-Type'] = input.content_type;
            if (input.tag) headers['X-Evm-Http-Case'] = input.tag;
            const response = await fetch(input.path, {method: input.method, headers,
                ...(input.body === null ? {} : {body: input.body})});
            const text = await response.text();
            return {status: response.status, headers: Object.fromEntries(response.headers.entries()), text};
        }""", {"path": path, "method": method, "body": body,
                 "content_type": content_type, "tag": tag})
        # Job IDs are u64; JavaScript's JSON.parse would round values above
        # 2**53 before the next task URL is constructed. Keep the wire text.
        response["reply"] = json.loads(response["text"]) \
            if response["headers"].get("content-type", "").startswith("application/json") else None
        return response

    providers = [fetch("/api/rpc-providers", tag=f"keepalive-{index}") for index in range(2)]
    assert all(response["status"] == 200 and response["reply"] == {"result": {"Ok": []}}
               for response in providers)
    assert len(connections) == 2, connections
    assert any(connection["reused"] for connection in connections), \
        f"sequential browser requests never reused an HTTP connection: {connections}"
    shell = fetch("/")
    assert shell["status"] == 200 and "evm-canvas" in shell["text"]
    assert shell["headers"]["content-type"].startswith("text/html")
    for response in [*providers, shell]:
        assert response["headers"]["cache-control"] == "no-store"
        assert response["headers"]["x-content-type-options"] == "nosniff"

    admitted = fetch("/api/tasks", method="POST", body=json.dumps(request), content_type="application/json")
    assert admitted["status"] == 202 and "Ok" in admitted["reply"]["result"], admitted
    job_id = admitted["reply"]["result"]["Ok"]["id"]
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        polled = fetch(f"/api/tasks/{job_id}")
        assert polled["status"] == 200, polled
        snapshot = polled["reply"]["result"]["Ok"]
        if snapshot["state"] == "Completed":
            break
        assert snapshot["state"] in ("Queued", "Running"), snapshot
        page.wait_for_timeout(30)
    else:
        raise AssertionError(f"browser fetch task did not complete: {snapshot}")
    result = fetch(f"/api/tasks/{job_id}/result")
    assert result["status"] == 200, result
    timing = measured_backend_timings(result["reply"]["result"]["Ok"])
    assert timing == snapshot["timings"]

    errors = {}
    for name, path, method, body, content_type, http_status, code in [
        ("invalid_json", "/api/tasks", "POST", "{", "application/json", 400, "InvalidRequest"),
        ("wrong_content_type", "/api/tasks", "POST", json.dumps(request), "text/plain", 400, "InvalidRequest"),
        ("oversized_body", "/api/tasks", "POST", " " * 262145, "application/json", 413, "RequestTooLarge"),
        ("encoded_path_traversal", "/%2e%2e%2fCargo.toml", "GET", None, None, 404, "NotFound"),
    ]:
        response = fetch(path, method=method, body=body, content_type=content_type)
        assert response["status"] == http_status, (name, response)
        error = response["reply"]["result"]["Err"]
        assert error["code"] == code, (name, error)
        assert response["headers"]["cache-control"] == "no-store"
        assert response["headers"]["x-content-type-options"] == "nosniff"
        errors[name] = {"http_status": response["status"], "code": error["code"],
                        "details": error["details"]}

    evidence = {"browser_connections": connections, "typed_errors": errors,
                "task_id": job_id, "backend_timings": timing,
                "static_headers": shell["headers"], "renderer": webgpu_evidence(page)}
    (output / "http-transport-report.json").write_text(json.dumps(evidence, indent=2) + "\n")
    page.close()
    return evidence


def run(context):
    return http_transport_interactions(context.browser, context.url, context.output)
