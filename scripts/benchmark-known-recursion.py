#!/usr/bin/env python3
"""Alternate frozen CLI and isolated RPC API binaries on identical case inputs.

Build both revisions before invoking this script. External process guards leave
analysis policy unchanged; an aborted attempt or export error is never recorded
as a delivered analysis report. See --help for required binary provenance.
"""

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import statistics
import sys

from benchmark_known_recursion.identity import source, cpus
from benchmark_known_recursion.process import ProtectedProcess, available_memory, sha256, utc, write_json
from benchmark_known_recursion.report import (metrics, strict_json, NATIVE_VERSION, WIRE_VERSION,
                                              effective_request_limits, cli_detail_region_request)
from benchmark_known_recursion import workbench
from benchmark_known_recursion import probe


def fixture(base, value):
    path = Path(value)
    return path.resolve() if path.is_absolute() else (base / path).resolve()


def load_case(base, name, raw, modes):
    record = {"name": name, "files": []}
    if "offline" in modes:
        if "offline_cli_arguments" in raw:
            arguments = list(raw["offline_cli_arguments"])
        else:
            command = raw["offline_cli_command"]
            if command[:2] != ["BINARY", "analyze"]:
                raise ValueError(f"{name}: expected offline BINARY analyze command")
            arguments = list(command[2:])
        if arguments.count("--world") != 1:
            raise ValueError(f"{name}: exactly one explicit --world is required")
        offset = arguments.index("--world") + 1
        world = fixture(base, arguments[offset])
        arguments[offset] = str(world)
        digest = sha256(world)
        if raw.get("world_sha256") and digest != raw["world_sha256"]:
            raise ValueError(f"{name}: world digest mismatch")
        record.update(offline_arguments=arguments, world=world, world_sha256=digest)
        record["files"].append(world)
    if "rpc" in modes:
        request = fixture(base, raw.get("rpc_request", raw.get("original_request_file")))
        digest = sha256(request)
        if raw.get("original_request_sha256") and digest != raw["original_request_sha256"]:
            raise ValueError(f"{name}: RPC request digest mismatch")
        body = request.read_bytes()
        value = strict_json(body)
        effective_request_limits(value["limits"])
        if "Hash" not in value["input"]["Rpc"]["block"]:
            raise ValueError(f"{name}: a fixed block hash is required")
        record.update(request=request, request_bytes=body, request_sha256=digest,
                      request_policy=value["limits"], request_environment=value["environment"],
                      rpc_input=value["input"]["Rpc"])
        record["files"].append(request)
    return record


def offline(binary, directory, root, guard, case, environment):
    command = ["taskset", "-c", ",".join(map(str, guard["cpus"])), str(binary), "analyze", *case["offline_arguments"]]
    record = {"command": command, "report_delivered": False,
              "precision_policy_request": cli_detail_region_request(case["offline_arguments"]),
              "scope": "Frozen observed initial world; excludes RPC acquisition and dynamically fetched unobserved facts"}
    process = ProtectedProcess(command, directory, root, guard, environment)
    try:
        record["process"] = process.finish()
    except KeyboardInterrupt:
        process.finish("external_keyboard_interrupt")
        raise
    output = directory / "stdout.log"
    error = directory / "stderr.log"
    record.update(stdout_bytes=output.stat().st_size, stdout_sha256=sha256(output),
                  stderr_bytes=error.stat().st_size, stderr_sha256=sha256(error),
                  termination=record["process"]["termination"])
    if record["process"]["exit_code"] in (0, 2) and record["termination"] in ("normal_exit", "process_error"):
        try:
            report = strict_json(output.read_bytes())
            observed = metrics(report, "offline")
            assert observed["precision_policy"]["coverage"]["requested_detail_regions"] == record["precision_policy_request"]["max_detail_regions"]
            record.update(metrics=observed, report_delivered=True, termination="cli_report_delivered")
        except (AssertionError, ValueError, KeyError, TypeError) as failure:
            record.update(termination="cli_invalid_report", report_error_type=type(failure).__name__)
    elif record["termination"] == "process_error":
        message = error.read_text(errors="replace")
        match = re.search(r"native JSON export requires at least (\d+) bytes, limit (\d+) \(retained (\d+)\)", message)
        if match:
            required, limit, retained = map(int, match.groups())
            record.update(termination="native_json_export_error", export_error={
                "required_bytes_lower_bound": required, "limit_bytes": limit, "retained_bytes": retained})
    return record


def aggregates(samples):
    output = []
    groups = sorted({(sample["case"], sample["mode"], sample["side"]) for sample in samples})
    for name, mode, side in groups:
        records = [s for s in samples if (s["case"], s["mode"], s["side"]) == (name, mode, side)]
        delivered = [s for s in records if s["report_delivered"]]
        group = {"case": name, "mode": mode, "side": side, "attempts": len(records),
                 "delivered_reports": len(delivered), "termination_counts": dict(Counter(s["termination"] for s in records)),
                 "coverage_note": "Returned status/frontiers remain separate from report delivery; no equivalence or speedup claim is inferred"}
        for prefix, selected in (("all_attempts", records), ("delivered_reports", delivered)):
            if not selected:
                continue
            group[prefix + "_process_medians"] = {key: statistics.median(s["process"][key] for s in selected)
                for key in ("wall_seconds", "user_seconds", "system_seconds", "peak_rss_kib")}
        if mode == "rpc":
            closed = [s["first_closed_model"] for s in records if s.get("first_closed_model") is not None]
            group["closed_model_observations"] = len(closed)
            if closed:
                group["first_closed_model_median_seconds"] = statistics.median(
                    item["submit_to_usable_model_seconds"] for item in closed)
            group["task_state_counts"] = dict(Counter(s.get("task_state") for s in records))
            values = [s["terminal_observed_seconds"] for s in records if "terminal_observed_seconds" in s]
            if values:
                group["terminal_median_seconds"] = statistics.median(values)
            for field in ("final_result", "cached-result", "cached-result-gzip"):
                values = [s[field]["http"]["elapsed_seconds"] for s in records if s.get(field) is not None]
                if values:
                    group[field + "_get_median_seconds"] = statistics.median(values)
        output.append(group)
    return output


def policy_comparisons(samples):
    output = []
    for name, mode in sorted({(s["case"], s["mode"]) for s in samples}):
        delivered = [s for s in samples if (s["case"], s["mode"]) == (name, mode) and s["report_delivered"]]
        policies = ([{"config": s["metrics"]["graphs"][s["metrics"]["selected_view"]]["config"],
                      "requested_detail_regions": s["metrics"]["precision_policy"]["coverage"]["requested_detail_regions"]}
                     for s in delivered]
                    if mode == "offline" else [s["request_identity"]["effective_policy"] for s in delivered])
        output.append({"case": name, "mode": mode, "delivered_reports": len(delivered),
                       "observed_policy_consistent": all(p == policies[0] for p in policies) if policies else None,
                       "policy_kind": "native-config-and-requested-cap" if mode == "offline" else "effective-rpc-limits",
                       "scope": "Compare effective policy; exact raw RPC maps and omission-only defaults are recorded separately; minimum has no transport-policy echo"})
    return output


def parser():
    result = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    result.add_argument("--manifest", type=Path, required=True, help="JSON cases with offline_cli_arguments and/or rpc_request paths relative to manifest")
    result.add_argument("--output", type=Path, required=True, help="New evidence directory; existing directories are rejected")
    result.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    result.add_argument("--mode", choices=("offline", "rpc", "both"), default="both")
    result.add_argument("--side", choices=("baseline", "candidate", "both"), default="both")
    result.add_argument("--case", action="append", dest="cases")
    result.add_argument("--runs", type=int, default=3)
    for side in ("baseline", "candidate"):
        result.add_argument(f"--{side}-cli", type=Path)
        result.add_argument(f"--{side}-server", type=Path)
        result.add_argument(f"--{side}-build-source", help="Actual build source commit; never inferred from current HEAD")
        result.add_argument(f"--{side}-metadata", type=Path, help="Optional provenance JSON; preserve dirty-build/equivalent-source caveats")
    result.add_argument("--assets", type=Path, help="Workbench assets directory; API evidence does not claim a browser UI test")
    result.add_argument("--rpc-config", help="Server provider URL or config file; persisted evidence redacts this argument")
    result.add_argument("--cpus", required=True, help="Linux CPU list, at most 8; leave at least 5 free, e.g. 48-55")
    result.add_argument("--timeout-seconds", type=float, default=120)
    result.add_argument("--rss-limit-gib", type=float, default=8)
    result.add_argument("--host-available-floor-gib", type=float, default=20)
    result.add_argument("--poll-seconds", type=float, default=1)
    result.add_argument("--allow-dirty", action="store_true", help="Preliminary diagnostics only; no frozen-source performance claim")
    result.add_argument("--probe-library", type=Path, help="Optional external Z3 preload; use a separate batch, never combine its wall with uninstrumented measurements")
    result.add_argument("--probe-verification", type=Path, help="Matching JSON containing library SHA and independent sat/unsat smoke counters")
    result.add_argument("--environment-note", required=True, help="Record other loads/builds or their absence; this script cannot prove host isolation")
    return result


def main():
    arguments = parser().parse_args()
    root, manifest = arguments.root.resolve(), arguments.manifest.resolve()
    modes = ("offline", "rpc") if arguments.mode == "both" else (arguments.mode,)
    sides = ("baseline", "candidate") if arguments.side == "both" else (arguments.side,)
    if arguments.runs < 1 or min(arguments.timeout_seconds, arguments.rss_limit_gib, arguments.poll_seconds) <= 0 or arguments.host_available_floor_gib < 0:
        raise ValueError("runs and process guard values must be positive")
    guard = {"cpus": cpus(arguments.cpus), "timeout_seconds": arguments.timeout_seconds,
             "rss_limit_kib": int(arguments.rss_limit_gib * 1024 * 1024),
             "host_available_floor_kib": int(arguments.host_available_floor_gib * 1024 * 1024),
             "poll_seconds": arguments.poll_seconds}
    if available_memory() < guard["host_available_floor_kib"]:
        raise ValueError("host memory is already below guard floor; no process started")
    initial = source(root)
    if initial["dirty"] and not arguments.allow_dirty:
        raise ValueError("commit the final source first, or explicitly mark --allow-dirty preliminary diagnostics")
    binaries = {}
    for side in sides:
        build_source = getattr(arguments, side + "_build_source")
        if not build_source:
            raise ValueError(f"--{side}-build-source is required")
        entry = {"declared_actual_build_source": build_source}
        metadata_path = getattr(arguments, side + "_metadata")
        if metadata_path:
            entry.update(provenance_metadata=json.loads(metadata_path.read_bytes()), provenance_metadata_sha256=sha256(metadata_path))
        for mode in modes:
            kind = "cli" if mode == "offline" else "server"
            path = getattr(arguments, side + "_" + kind)
            if path is None or not os.access(path, os.X_OK):
                raise ValueError(f"executable --{side}-{kind} is required")
            entry[kind] = {"path": str(path.resolve()), "sha256": sha256(path)}
        binaries[side] = entry
    if "rpc" in modes and (not arguments.rpc_config or not arguments.assets or not arguments.assets.is_dir()):
        raise ValueError("RPC mode needs --rpc-config and an existing --assets directory")
    rpc_configuration = None
    if "rpc" in modes:
        workbench.private_tokens(arguments.rpc_config)
        rpc_configuration = {"argument_sha256": hashlib.sha256(arguments.rpc_config.encode()).hexdigest(),
                             "argument": "[REDACTED_RPC_ENDPOINT]"}
        if not re.match(r"https?://", arguments.rpc_config, re.IGNORECASE):
            rpc_configuration["file_sha256"] = sha256(arguments.rpc_config)
    raw_manifest = json.loads(manifest.read_bytes())
    verified_probe = None
    if bool(arguments.probe_library) != bool(arguments.probe_verification):
        raise ValueError("--probe-library and --probe-verification must be supplied together")
    if arguments.probe_library:
        verified_probe = probe.verify(arguments.probe_library, arguments.probe_verification,
            [Path(item["path"]) for entry in binaries.values() for key, item in entry.items() if key in ("cli", "server")])
    names = arguments.cases or list(raw_manifest["cases"])
    if len(names) != len(set(names)):
        raise ValueError("duplicate case names")
    cases = {name: load_case(manifest.parent, name, raw_manifest["cases"][name], modes) for name in names}
    inputs = {str(manifest): sha256(manifest)}
    if verified_probe:
        inputs[str(arguments.probe_library.resolve())] = sha256(arguments.probe_library)
        inputs[str(arguments.probe_verification.resolve())] = sha256(arguments.probe_verification)
    if rpc_configuration and "file_sha256" in rpc_configuration:
        inputs[str(Path(arguments.rpc_config).resolve())] = rpc_configuration["file_sha256"]
    for case in cases.values():
        inputs.update({str(path): sha256(path) for path in case["files"]})
    output = arguments.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    for name, case in cases.items():
        copied = output / "inputs" / name
        copied.mkdir(parents=True)
        for key, path_key in (("world.json", "world"), ("request.json", "request")):
            if path_key in case:
                (copied / key).write_bytes(case[path_key].read_bytes())
    (output / "manifest.json").write_bytes(manifest.read_bytes())
    if verified_probe:
        (output / "probe-verification.json").write_bytes(arguments.probe_verification.read_bytes())
    environment = os.environ.copy()
    removed = {key: environment.pop(key) for key in ("LD_PRELOAD", "LD_AUDIT") if key in environment}
    evidence = {"schema": "known-recursion-benchmark-v2", "started_utc": utc(), "source_before": initial,
                "precision_policy_contract": "D5-effective-detail-regions",
                "consumer_formats": {"offline": {"format": "native-hybrid-report", "schema_version": NATIVE_VERSION},
                                     "rpc": {"format": "web-job-result-reply", "schema_version": WIRE_VERSION}},
                "frozen_source_required": not arguments.allow_dirty, "preliminary": arguments.allow_dirty,
                "platform": platform.platform(), "allowed_cpus": sorted(os.sched_getaffinity(0)), "guard": guard,
                "environment_note": arguments.environment_note, "binaries": binaries, "input_sha256_before": inputs,
                "instrumented": bool(verified_probe),
                "native_solver_cost": {"instrumentation_verified": bool(verified_probe), "reason": "Separate probe batch; per-sample availability remains explicit" if verified_probe else "Uninstrumented batch; solver counts/time are unavailable, not zero. Use a separate verified probe batch."},
                "preload_environment_removed": list(removed),
                "rpc_configuration": rpc_configuration,
                "scope": "Same input bytes and explicit policy per case/mode; offline and RPC modes have different acquisition boundaries and are not pooled",
                "order": "Alternate baseline/candidate order each repetition, sequential processes; no analysis budget/input changes",
                "samples": []}
    write_json(output / "measurements.json", evidence)
    success = True
    try:
        for repetition in range(arguments.runs):
            order = sides if repetition % 2 == 0 else tuple(reversed(sides))
            for name, case in cases.items():
                for mode in modes:
                    for side in order:
                        for path, digest in inputs.items():
                            if sha256(path) != digest:
                                raise ValueError("frozen input changed during measurement")
                        kind = "cli" if mode == "offline" else "server"
                        binary = Path(binaries[side][kind]["path"])
                        if sha256(binary) != binaries[side][kind]["sha256"]:
                            raise ValueError("frozen binary changed during measurement")
                        directory = output / f"{name}-{mode}-{repetition + 1}-{side}"
                        directory.mkdir()
                        sample = {"case": name, "mode": mode, "side": side, "repetition": repetition + 1,
                                  "binary_sha256": binaries[side][kind]["sha256"], "directory": directory.name}
                        sample_environment = environment.copy()
                        if verified_probe:
                            sample_environment.update(LD_PRELOAD=str(arguments.probe_library.resolve()),
                                                      EVM_Z3_CHECK_PROBE_OUTPUT=str(directory / "native-checks.json"))
                        if mode == "offline":
                            sample["world_sha256"] = case["world_sha256"]
                            sample.update(offline(binary, directory, root, guard, case, sample_environment))
                        else:
                            sample.update(request_sha256=case["request_sha256"], request_policy=case["request_policy"],
                                          request_environment=case["request_environment"], rpc_input=case["rpc_input"])
                            sample.update(workbench.run(binary, directory, root, guard, case["request_bytes"],
                                                        arguments.rpc_config, arguments.assets.resolve(), sample_environment))
                            sample.setdefault("termination", sample["process"]["termination"])
                        if verified_probe:
                            sample["native_solver_cost"] = probe.collect(directory / "native-checks.json", sample["process"])
                        write_json(directory / "measurement.json", sample)
                        evidence["samples"].append(sample)
                        evidence["aggregates"] = aggregates(evidence["samples"])
                        evidence["policy_comparisons"] = policy_comparisons(evidence["samples"])
                        write_json(output / "measurements.json", evidence)
                        print(name, mode, side, sample["termination"], "exit=" + str(sample["process"]["exit_code"]), flush=True)
        evidence["planned_batch_completed"] = True
    except (Exception, KeyboardInterrupt) as failure:
        success = False
        evidence.update(planned_batch_completed=False, runner_error_type=type(failure).__name__)
        print("measurement batch interrupted:", type(failure).__name__, file=sys.stderr)
    finally:
        evidence.update(finished_utc=utc(), source_after=source(root),
                        input_sha256_after={path: sha256(path) if Path(path).exists() else None for path in inputs})
        evidence["source_unchanged"] = evidence["source_after"] == initial
        evidence["input_unchanged"] = evidence["input_sha256_after"] == inputs
        evidence["binary_unchanged"] = all(sha256(item["path"]) == item["sha256"]
            for side in binaries.values() for key, item in side.items() if key in ("cli", "server"))
        evidence["valid_frozen_batch"] = bool(success and not arguments.allow_dirty and evidence["source_unchanged"]
                                               and evidence["input_unchanged"] and evidence["binary_unchanged"])
        if any(item["observed_policy_consistent"] is False for item in evidence.get("policy_comparisons", [])):
            success = False
            evidence.update(valid_frozen_batch=False, policy_mismatch=True)
        write_json(output / "measurements.json", evidence)
    return 0 if success and evidence["input_unchanged"] and evidence["binary_unchanged"] and (
        arguments.allow_dirty or evidence["source_unchanged"]) else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, KeyError, OSError) as error:
        # Configuration errors are local and must occur before any analysis.
        print("benchmark configuration error:", str(error), file=sys.stderr)
        sys.exit(2)
