"""Replay exact RPC request bytes against only the server created by this run."""

import hashlib
from functools import lru_cache
import json
from pathlib import Path
import re
import time
import urllib.error
import urllib.request

from .process import ProtectedProcess, write_json
from .report import metrics


@lru_cache(maxsize=8)
def private_tokens(argument):
    tokens = [argument]
    if not re.match(r"https?://", argument, re.IGNORECASE):
        configuration = json.loads(Path(argument).read_bytes())
        tokens.extend(provider["endpoint"] for provider in configuration["providers"])
    return tuple(tokens)


def safe_bytes(value, endpoint):
    for token in private_tokens(endpoint):
        for form in (token.encode(), json.dumps(token)[1:-1].encode()):
            value = value.replace(form, b"[REDACTED_RPC_ENDPOINT]")
    return value


def redact(value, endpoint):
    return json.loads(safe_bytes(json.dumps(value).encode(), endpoint))


def http(opener, directory, name, method, url, endpoint, body=None, timeout=5, limit=128 * 1024 * 1024):
    headers = {"Content-Type": "application/json"} if body is not None else {}
    request = urllib.request.Request(url, data=body, method=method, headers=headers)
    started = time.monotonic()
    try:
        response = opener.open(request, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read(limit + 1)
        if len(raw) > limit:
            raise RuntimeError("external HTTP response capture limit reached")
        status = response.status
    received = time.monotonic()
    # Preserve exact raw hashes/lengths. A private endpoint may appear in a typed
    # error; persist a redacted body instead of disclosing that configuration.
    safe = safe_bytes(raw, endpoint)
    (directory / f"{name}.json").write_bytes(safe)
    detail = {"http_status": status, "response_bytes": len(raw),
              "raw_response_sha256": hashlib.sha256(raw).hexdigest(),
              "persisted_response_sha256": hashlib.sha256(safe).hexdigest(),
              "endpoint_redacted": raw != safe, "elapsed_seconds": received - started,
              "body_received_monotonic": received, "capture_limit_bytes": limit}
    write_json(directory / f"{name}.http.json", detail)
    return detail, json.loads(raw)


def result_value(envelope):
    result = envelope["result"]
    if "Ok" not in result:
        return None, result.get("Err")
    return result["Ok"], None


def run(server, directory, root, guard, request_bytes, endpoint, assets, environment=None):
    command = ["taskset", "-c", ",".join(map(str, guard["cpus"])), str(server),
               "--bind", "127.0.0.1:0", "--assets", str(assets),
               "--rpc-config", endpoint, "--workers", "1", "--queue-capacity", "1", "--retained-jobs", "2"]
    record = {"command": ["[REDACTED_RPC_ENDPOINT]" if arg == endpoint else arg for arg in command],
              "scope": "Original fixed-block RPC request, isolated workbench HTTP protocol; not a browser screenshot test",
              "report_delivered": False}
    process = ProtectedProcess(command, directory, root, guard, environment)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    task_id, base_url = None, None
    started = None
    try:
        startup_deadline = time.monotonic() + 10
        while not process.done.is_set() and time.monotonic() < startup_deadline:
            stdout = (directory / "stdout.log").read_text(errors="replace")
            match = re.search(r"EVM workbench: (http://127\.0\.0\.1:(\d+))", stdout)
            if match:
                assert int(match[2]) != 1111
                base_url = match[1]
                break
            time.sleep(0.05)
        if base_url is None:
            record["termination"] = "private_server_startup_failed"
            return record
        record["private_base_url"] = base_url
        (directory / "request.json").write_bytes(request_bytes)
        started = time.monotonic()
        detail, envelope = http(opener, directory, "submit", "POST", base_url + "/api/tasks", endpoint, request_bytes)
        record["submit_http"] = detail
        snapshot, error = result_value(envelope)
        if error is not None or detail["http_status"] != 202:
            record.update(termination="http_submission_error", api_error=redact(error, endpoint))
            return record
        task_id = snapshot["id"]  # Python preserves u64 task IDs exactly.
        record["task_id_decimal_string"] = str(task_id)

        def cancel():
            detail, envelope = http(opener, directory, "cancel", "DELETE", base_url + f"/api/tasks/{task_id}", endpoint, timeout=0.2)
            record["cancel_http"] = detail
            record["cancel_snapshot"] = redact(envelope, endpoint)

        process.before_stop = cancel
        with (directory / "progress.jsonl").open("w", buffering=1) as progress:
            poll = 0
            while not process.done.is_set() and not process.stop_reason:
                record["last_snapshot"] = redact(snapshot, endpoint)
                progress.write(json.dumps({"elapsed_seconds": time.monotonic() - started, "snapshot": record["last_snapshot"]}) + "\n")
                state = snapshot["state"]
                kind = state if isinstance(state, str) else next(iter(state))
                if kind == "Completed":
                    record["task_state"] = "Completed"
                    record["completed_progress"] = snapshot["progress"]
                    detail, envelope = http(opener, directory, "result", "GET", base_url + f"/api/tasks/{task_id}/result", endpoint, timeout=15)
                    record["result_http"] = detail
                    record["submit_to_body_received_seconds"] = detail["body_received_monotonic"] - started
                    report, error = result_value(envelope)
                    if error is not None or detail["http_status"] != 200:
                        record.update(termination="http_report_error", api_error=redact(error, endpoint))
                    else:
                        request = json.loads(request_bytes)
                        assert report["metadata"]["limits"] == request["limits"], "returned policy differs from exact request"
                        assert report["metadata"]["snapshot"]["block_hash"].lower() == request["input"]["Rpc"]["block"]["Hash"].lower()
                        record.update(report_delivered=True, termination="http_report_delivered", metrics=redact(metrics(report, "rpc"), endpoint))
                    break
                if kind in ("Failed", "Cancelled"):
                    record.update(task_state=kind, termination="http_task_" + kind.lower())
                    if kind == "Failed":
                        record["api_error"] = redact(state["Failed"], endpoint)
                    break
                time.sleep(0.5)
                detail, envelope = http(opener, directory, f"poll-{poll:04d}", "GET", base_url + f"/api/tasks/{task_id}", endpoint, timeout=2)
                snapshot, error = result_value(envelope)
                if error is not None:
                    record.update(termination="http_status_error", api_error=redact(error, endpoint))
                    break
                poll += 1
        record["request_wall_seconds"] = time.monotonic() - started
        if process.stop_reason:
            record["termination"] = process.stop_reason
    except Exception as error:
        record.update(termination="workbench_runner_error", runner_error_type=type(error).__name__)
    finally:
        record["process"] = process.finish("capture_complete")
        if process.record["termination"].startswith("external_"):
            record["termination"] = process.record["termination"]
        if started is not None:
            record.setdefault("request_wall_seconds", time.monotonic() - started)
        for filename in ("stdout.log", "stderr.log"):
            path = directory / filename
            path.write_bytes(safe_bytes(path.read_bytes(), endpoint))
    return record
