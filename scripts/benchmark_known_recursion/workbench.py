"""Replay exact RPC request bytes against only the server created by this run."""

import hashlib
import gzip
from functools import lru_cache
import io
import json
from pathlib import Path
import re
import time
import urllib.error
import urllib.request

from .process import ProtectedProcess, write_json
from .report import (job_snapshot, metrics, request_qualification, result_value,
                     strict_json, wire_report, effective_request_limits)


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


def http(opener, directory, name, method, url, endpoint, body=None, timeout=5,
         limit=128 * 1024 * 1024, encoding="identity"):
    headers = {"Content-Type": "application/json"} if body is not None else {}
    headers["Accept-Encoding"] = encoding
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
        content_encoding = response.headers.get("Content-Encoding", "identity")
    received = time.monotonic()
    assert content_encoding in ("identity", "gzip")
    assert content_encoding == "identity" or encoding == "gzip"
    decoded = raw
    if content_encoding == "gzip":
        with gzip.GzipFile(fileobj=io.BytesIO(raw)) as source:
            decoded = source.read(limit + 1)
        if len(decoded) > limit:
            raise RuntimeError("external decoded HTTP response capture limit reached")
    decompressed = time.monotonic()
    # Preserve exact raw hashes/lengths. A private endpoint may appear in a typed
    # error; persist a redacted body instead of disclosing that configuration.
    safe = safe_bytes(decoded, endpoint)
    (directory / f"{name}.json").write_bytes(safe)
    detail = {"http_status": status, "response_bytes": len(raw),
              "raw_response_sha256": hashlib.sha256(raw).hexdigest(),
              "persisted_response_sha256": hashlib.sha256(safe).hexdigest(),
              "endpoint_redacted": decoded != safe, "elapsed_seconds": received - started,
              "body_received_monotonic": received, "capture_limit_bytes": limit,
              "requested_encoding": encoding, "content_encoding": content_encoding,
              "decoded_response_bytes": len(decoded), "decompression_seconds": decompressed - received,
              "persisted_representation": "redacted decoded JSON"}
    write_json(directory / f"{name}.http.json", detail)
    value = strict_json(decoded)
    parsed = time.monotonic()
    detail.update(json_decoded_monotonic=parsed, json_parse_seconds=parsed - decompressed)
    write_json(directory / f"{name}.http.json", detail)
    return detail, value


def observe(opener, directory, base_url, endpoint, request_bytes, process, record, started, snapshot):
    """Observe publications independently of lifecycle; all requests stay owned."""
    request = strict_json(request_bytes)
    effective_policy, applied_defaults = effective_request_limits(request["limits"])
    task_id = snapshot["id"]
    record["request_identity"] = {
        "sha256": hashlib.sha256(request_bytes).hexdigest(), "bytes": len(request_bytes),
        "input": request["input"], "fork": request["fork"], "environment": request["environment"],
        "policy": request["limits"], "effective_policy": effective_policy,
        "applied_policy_defaults": applied_defaults}
    record.update(first_closed_model=None, final_result=None, publications=[],
                  task_state=None, termination="no_terminal_observation",
                  timing_scope="External observation includes polling, capture I/O and validation; backend phases remain separate")
    retained = None

    def fetch(name, advertised, state, *, encoding="identity"):
        nonlocal retained
        detail, envelope = http(opener, directory, name, "GET", base_url + f"/api/tasks/{task_id}/result",
                                endpoint, timeout=15, encoding=encoding)
        result, error = result_value(envelope)
        if error is not None or detail["http_status"] != 200:
            record[name + "_error"] = {"http": detail, "api_error": redact(error, endpoint)}
            return None
        result = wire_report(envelope, job_id=task_id, advertised=advertised)
        report, identity = result["report"], result["publication"]
        policy_checks = request_qualification(report, request)
        if retained is not None:
            previous = retained["publication"]
            assert int(identity["version"]) >= int(previous["version"])
            for field, slot in (("minimum_version", "minimum"), ("global_version", "global"), ("native_version", "native")):
                if previous[field] is not None:
                    assert identity[field] is not None and int(identity[field]) >= int(previous[field])
                    if identity[field] == previous[field]:
                        assert report[slot] == retained["report"][slot], "same model version changed"
            if identity["version"] == previous["version"]:
                assert result == retained, "same publication changed"
            elif identity["minimum_version"] is not None and identity["minimum_version"] == previous["minimum_version"]:
                for source_directory in ("programs", "process_controls"):
                    current_sources = {source["id"]: source for source in report[source_directory]}
                    assert len(current_sources) == len(report[source_directory]), f"duplicate {source_directory} ID"
                    assert all(current_sources.get(source["id"]) == source
                               for source in retained["report"][source_directory]), f"same minimum version changed {source_directory}"
        summary = {"publication": identity, "task_state_at_observation": state,
                   "advertised_publication": advertised, "http": detail, "artifact": name + ".json",
                   "submit_to_body_received_seconds": detail["body_received_monotonic"] - started,
                   "submit_to_usable_model_seconds": time.monotonic() - started,
                   "policy_checks": policy_checks, "metrics": redact(metrics(report, "rpc"), endpoint)}
        record["publications"].append({key: value for key, value in summary.items() if key != "metrics"})
        if identity["model_closed"] and record["first_closed_model"] is None:
            record["first_closed_model"] = summary
        record.update(report_delivered=True, metrics=summary["metrics"], delivered_publication=identity)
        retained = result
        return summary

    with (directory / "progress.jsonl").open("w", buffering=1) as progress:
        poll = 0
        while not process.done.is_set() and not process.stop_reason:
            record["last_snapshot"] = redact(snapshot, endpoint)
            state = snapshot["state"]
            kind = state if type(state) is str else "Failed"
            record["task_state"] = kind
            progress.write(json.dumps({"elapsed_seconds": time.monotonic() - started, "snapshot": record["last_snapshot"]}) + "\n")
            advertised = snapshot["publication"]
            if retained is not None:
                assert advertised is not None, "a retained publication disappeared"
                assert int(advertised["version"]) >= int(retained["publication"]["version"]), "stale status"
                # A result may race ahead of the poll which triggered its GET.
                if advertised["version"] == retained["publication"]["version"]:
                    assert advertised == retained["publication"]
            terminal = kind in ("Completed", "Failed", "Cancelled")
            if terminal:
                record.update(terminal_observed_seconds=time.monotonic() - started,
                              final_snapshot=redact(snapshot, endpoint), termination="http_task_" + kind.lower())
                if kind == "Failed":
                    record["api_error"] = redact(state["Failed"], endpoint)
                if kind == "Completed":
                    assert advertised is not None, "Completed without a publication"
                    record["completed_progress"] = snapshot["progress"]
                if advertised is not None:
                    final = fetch("result", advertised, kind)
                    record["final_result"] = final
                    if final is not None:
                        assert final["publication"] == advertised, "terminal result differs from final status"
                        record["final_export"] = {"outcome": final["metrics"]["rich_export"],
                                                  "timings": final["metrics"]["timings"],
                                                  "result_body_observed_seconds": final["submit_to_body_received_seconds"]}
                        record["final_detail"] = {"native_version": final["publication"]["native_version"],
                                                  "qualification": final["metrics"]["hybrid"],
                                                  "graph": final["metrics"]["graphs"]["Native"],
                                                  "job_terminal_observed_seconds": record["terminal_observed_seconds"],
                                                  "latency_scope": "No separate Native completion clock; terminal observation includes export and polling"}
                        for name, encoding in (("cached-result", "identity"), ("cached-result-gzip", "gzip")):
                            cached = fetch(name, advertised, kind, encoding=encoding)
                            record[name] = cached
                        if kind == "Completed":
                            record["termination"] = "http_report_delivered"
                break
            if advertised is not None and (retained is None or int(advertised["version"]) > int(retained["publication"]["version"])):
                fetch(f"published-{poll:04d}", advertised, kind)
            time.sleep(0.5)
            detail, envelope = http(opener, directory, f"poll-{poll:04d}", "GET", base_url + f"/api/tasks/{task_id}", endpoint, timeout=2)
            record["last_poll_http"] = detail
            snapshot, error = job_snapshot(envelope, task_id)
            if error is not None or detail["http_status"] != 200:
                record.update(termination="http_status_error", api_error=redact(error, endpoint))
                break
            poll += 1
    if record["first_closed_model"] is not None and "terminal_observed_seconds" in record:
        record["first_model_to_terminal_seconds"] = record["terminal_observed_seconds"] - record["first_closed_model"]["submit_to_usable_model_seconds"]


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
        snapshot, error = job_snapshot(envelope)
        if error is not None or detail["http_status"] != 202:
            record.update(termination="http_submission_error", api_error=redact(error, endpoint))
            return record
        task_id = snapshot["id"]  # Python preserves u64 task IDs exactly.
        record["task_id_decimal_string"] = str(task_id)

        def cancel():
            detail, envelope = http(opener, directory, "cancel", "DELETE", base_url + f"/api/tasks/{task_id}", endpoint, timeout=0.2)
            record["cancel_http"] = detail
            record["cancel_snapshot"] = redact(envelope, endpoint)
            job_snapshot(envelope, task_id)

        process.before_stop = cancel
        observe(opener, directory, base_url, endpoint, request_bytes, process, record, started, snapshot)
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
