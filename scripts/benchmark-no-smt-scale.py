#!/usr/bin/env python3
"""Check fixed offline inputs or run explicit commands under external guards.

No build, RPC, shell command, policy translation, or speedup calculation occurs
here. Candidate interfaces supply their own reviewed command profile. See
examples/no-smt-scale/README.md for the input and report-delivery contracts.
"""

import argparse
from collections import Counter
from copy import deepcopy
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re

from benchmark_known_recursion.identity import cpus, source
from benchmark_known_recursion.process import ProtectedProcess, available_memory, sha256, utc, write_json
from benchmark_known_recursion.report import metrics


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                                     allow_nan=False).encode()).hexdigest()


def read_json(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    def nonfinite(value):
        raise ValueError(f"nonfinite JSON number: {value}")

    return json.loads(Path(path).read_bytes(), object_pairs_hook=unique, parse_constant=nonfinite)


def checked_file(base, record):
    path = (base / record["path"]).resolve(strict=True)
    if sha256(path) != record["sha256"]:
        raise ValueError(f"input SHA256 mismatch: {path}")
    return path


WIRE14_REMOVALS = ("/limits/smt_provider", "/limits/smt_rlimit")


def derive_wire14_request(original):
    """Explicit schema-only derivation; acquisition/entry semantics stay intact."""
    derived = deepcopy(original)
    for pointer in WIRE14_REMOVALS:
        parent, key = pointer.strip("/").split("/")
        if key not in derived[parent]:
            raise ValueError(f"original request lacks expected schema field {pointer}")
        del derived[parent][key]
    return derived


def validate_request_derivations(manifest, base, files):
    for name, spec in manifest.get("request_derivations", {}).items():
        if (spec["removed_json_pointers"] != list(WIRE14_REMOVALS)
                or spec["versions"] != {"native": 9, "wire": 14, "cost": 3}):
            raise ValueError(f"{name}: unreviewed schema derivation")
        original_path = checked_file(base, spec["original"])
        derived_path = checked_file(base, spec["derived"])
        if digest(read_json(derived_path)) != digest(derive_wire14_request(read_json(original_path))):
            raise ValueError(f"{name}: derivation changed more than the two SMT fields")
        files.update({str(path): sha256(path) for path in (original_path, derived_path)})


def program_identities(world):
    output = []
    for account in world["accounts"]:
        if "code" not in account:
            continue
        code = account["code"]
        if not isinstance(code, str) or not code.startswith("0x"):
            raise ValueError("fixed program must have 0x-prefixed bytecode")
        raw = bytes.fromhex(code[2:])
        output.append({"address": account["address"].lower(), "mode": "Runtime",
                       "bytecode_bytes": len(raw), "raw_sha256": hashlib.sha256(raw).hexdigest(),
                       "declared_code_hash": account.get("code_hash")})
    return sorted(output, key=lambda item: item["address"])


def initial_facts(world):
    """Record observations only; omitted fields remain explicitly unknown."""
    return sorted([{"address": account["address"].lower(),
                    "balance": account.get("balance"), "nonce": account.get("nonce"),
                    "existence": account.get("existence", "unknown"),
                    "storage": account.get("storage", {}),
                    "storage_unknown": account.get("storage_unknown", True)}
                   for account in world["accounts"]], key=lambda item: item["address"])


def check_guard(guard):
    for key in ("timeout_seconds", "rss_limit_kib", "poll_seconds", "host_available_floor_kib"):
        value = guard[key]
        if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
            raise ValueError(f"invalid {key}")
        if key != "host_available_floor_kib" and value == 0:
            raise ValueError(f"invalid {key}")
    if not 30 <= guard["timeout_seconds"] <= 120:
        raise ValueError("this first-stage corpus only plans 30–120 second screens")
    if guard["poll_seconds"] > 1:
        raise ValueError("resource sampling must occur at least once per second")
    if type(guard["report_parse_limit_bytes"]) is not int or guard["report_parse_limit_bytes"] <= 0:
        raise ValueError("declare a positive harness report parsing byte limit")


def load_inputs(path, names=None):
    manifest = read_json(path)
    if manifest["schema"] != "evm-no-smt-scale-inputs-v1":
        raise ValueError("unsupported input manifest schema")
    names = list(manifest["cases"]) if names is None else names
    if not names or len(names) != len(set(names)):
        raise ValueError("select nonempty, distinct cases")
    cases, files = {}, {str(path): sha256(path)}
    for name in names:
        if not re.fullmatch(r"[a-z0-9-]+", name):
            raise ValueError(f"unsafe case name: {name}")
        raw = manifest["cases"][name]
        if raw["profile"] not in ("code-only", "fixed-offline-snapshot"):
            raise ValueError("the batch runner accepts offline profiles only")
        world_path = checked_file(path.parent, raw["world"])
        world = read_json(world_path)
        programs = program_identities(world)
        if (world["fork"] != raw["fork"] or programs != raw["programs"]
                or initial_facts(world) != raw["initial_facts"]):
            raise ValueError(f"{name}: fork/program/initial-fact identity mismatch")
        check_guard(raw["planned_guard"])
        if raw["entry"]["address"].lower() not in [program["address"] for program in programs]:
            raise ValueError(f"{name}: entry program is not fixed")
        # Never infer observed zero or absent accounts from missing JSON members.
        if any(account.get("storage_unknown", True) is not True for account in world["accounts"]):
            raise ValueError(f"{name}: this corpus requires unknown unobserved storage")
        if raw["profile"] == "code-only":
            for account in world["accounts"]:
                if (account.get("storage") or account.get("balance") is not None
                        or account.get("nonce") is not None
                        or account.get("existence", "unknown") != "unknown"):
                    raise ValueError(f"{name}: code-only profile contains state observations")
        files[str(world_path)] = sha256(world_path)
        provenance = raw.get("provenance_files", [])
        for record in provenance:
            reference = checked_file(path.parent, record)
            files[str(reference)] = sha256(reference)
        identity = {"profile": raw["profile"], "world_raw_sha256": raw["world"]["sha256"],
                    "programs": programs, "fork": raw["fork"], "entry": raw["entry"],
                    "environment": raw["environment"], "fact_boundary": raw["fact_boundary"],
                    "initial_facts": raw["initial_facts"]}
        cases[name] = {"spec": raw, "world_path": world_path, "identity": identity,
                       "provenance_paths": [checked_file(path.parent, record) for record in provenance],
                       "input_identity_sha256": digest(identity)}
    validate_request_derivations(manifest, path.parent, files)
    return manifest, cases, files


def load_commands(path, cases):
    profile = read_json(path)
    if (profile["schema"] != "evm-no-smt-scale-commands-v1"
            or profile["boundary"] != "offline-no-acquisition"
            or profile["report_adapter"] != "native-world-v1"):
        raise ValueError("unsupported command profile boundary/report adapter")
    if any(not isinstance(profile[key], dict) or not profile[key] for key in
           ("policy", "interface", "policy_mapping", "expected_native_config")):
        raise ValueError("declare interface and the complete effective policy/mapping")
    if not re.fullmatch(r"[0-9a-f]{40}", profile["interface"]["source_commit"]):
        raise ValueError("bind the command profile to its reviewed interface source commit")
    if (not isinstance(profile["report_exit_codes"], list) or not profile["report_exit_codes"]
            or any(type(code) is not int or code < 0 for code in profile["report_exit_codes"])):
        raise ValueError("declare nonnegative final-report exit codes")
    if profile["interface"].get("versions", {}).get("native") in {12, 13, 14, 15}:
        if (profile.get("expected_native_numeric_model") not in ("product", "finite-set")
                or not isinstance(profile.get("expected_native_domain_spec"), dict)
                or not profile["expected_native_domain_spec"]):
            raise ValueError("native 12/13/14/15 requires the selected model and complete algebra limits")
        allowance = profile.get("expected_native_postprocessing_max_work")
        if type(allowance) is not int or allowance <= 0:
            raise ValueError("native 12/13/14/15 requires a positive application postprocessing allowance")
    for name, case in cases.items():
        entry = profile["cases"][name]
        if entry["input_identity_sha256"] != case["input_identity_sha256"]:
            raise ValueError(f"{name}: commands refer to a different input boundary")
        if not entry["input_mapping"]:
            raise ValueError(f"{name}: declare how argv binds the full input boundary")
        command = entry["argv"]
        if (not isinstance(command, list) or not command or command[0] != "BINARY"
                or any(not isinstance(arg, str) or not arg for arg in command)
                or command.count("BINARY") != 1 or command.count("WORLD") != 1):
            raise ValueError(f"{name}: argv must start with BINARY and bind WORLD exactly once")
    return profile


def native_final(report, process, accepted_exits):
    """External stops cannot deliver a native final, even if stdout is JSON."""
    if process["termination"] not in ("normal_exit", "process_error"):
        return None
    if process["exit_code"] not in accepted_exits:
        return None
    if not isinstance(report, dict) or report.get("status") not in ("Converged", "Incomplete"):
        raise ValueError("stdout is not a final native world Report")
    for key in ("states", "frontiers", "outcomes"):
        if not isinstance(report.get(key), list):
            raise ValueError(f"final Report lacks {key}")
        if any(not isinstance(item, dict) for item in report[key]):
            raise ValueError(f"final Report has malformed {key}")
    if not isinstance(report.get("config"), dict):
        raise ValueError("final Report lacks effective config")
    if not isinstance(report.get("world"), dict) or not isinstance(report.get("entry"), dict):
        raise ValueError("final Report lacks world/entry evidence")
    for key in ("transfers", "work"):
        if type(report.get(key)) is not int or report[key] < 0:
            raise ValueError(f"final Report lacks nonnegative {key}")
    result = metrics(report, "offline")
    closed = result["summary_closed"]
    result["closure"] = {"reported_summaries": len(closed), "closed": closed,
                         "all_closed": None if not closed or None in closed else all(closed),
                         "scope": "Summary flags only; root coverage still depends on status/frontiers and effects"}
    result["outcomes_sha256"] = digest(report["outcomes"])
    result["outcome_kind_counts"] = dict(Counter(str(outcome.get("kind")) for outcome in report["outcomes"]))
    result["frontier_reason_counts"] = dict(Counter(
        json.dumps(frontier.get("reason", frontier.get("kind")), sort_keys=True)
        for frontier in report["frontiers"]))
    result["entry_sha256"] = digest(report["entry"])
    result["world_sha256"] = digest(report["world"])
    # A native budget-only config does not describe numeric precision. Retain
    # the actual algebra limits and application label as independent evidence.
    for field in ("domain_spec", "numeric_model", "postprocessing_max_work", "postprocessing_work"):
        if field in report:
            result[field] = report[field]
    result["effective_policy_sha256"] = digest({key: report[key] for key in
        ("config", "domain_spec", "numeric_model", "postprocessing_max_work") if key in report})
    result["semantic_acceptance"] = "pending joint output/effect inclusion review"
    return result


def native_view(delivered):
    """Keep wrapper coverage authoritative while measuring its selected graph.

    Raw stdout retains both graphs and every prefix. Historical native captures
    have no wrapper and continue through the unchanged native path.
    """
    if not isinstance(delivered, dict) or "hybrid" not in delivered:
        return delivered, None
    hybrid, graph = delivered["hybrid"], delivered.get("analysis", delivered)
    if not isinstance(hybrid, dict) or not isinstance(graph, dict):
        raise ValueError("invalid Hybrid application envelope")
    expected = "Converged" if hybrid["coverage"]["baseline_current"] else "Incomplete"
    if delivered.get("status") != expected:
        raise ValueError("Hybrid wrapper status contradicts baseline coverage")
    view = {key: value for key, value in graph.items() if key != "hybrid"}
    # The flat CLI carries application totals at the root. Retain the actual
    # selected relation's source counters independently of those wrapper totals.
    if "selected_graph" in hybrid:
        view.update(hybrid["selected_graph"])
    view.update({key: delivered[key] for key in
        ("numeric_model", "postprocessing_max_work", "postprocessing_work") if key in delivered})
    return view, hybrid


def collect_report(directory, process, commands, report_parse_limit_bytes):
    accepted_exits = commands["report_exit_codes"]
    record = {"process": process, "exit_code": process["exit_code"],
              "termination": process["termination"], "report_delivered": False,
              "nativefinal": None, "native_phase": None,
              "phase": "process_exited_without_final_report",
              "phase_scope": "Harness observation; internal engine phase is unavailable",
              "report_error": None, "rawhash": {}}
    if process["termination"].startswith("external_"):
        record["phase"] = "external_stop_without_final_report"
    elif process["termination"] == "spawn_error":
        record["phase"] = "spawn_failed"
    for name in ("stdout.log", "stderr.log", "resources.jsonl", "process.json", "process-start.json"):
        path = directory / name
        if path.exists():
            record["rawhash"][name] = {"sha256": sha256(path), "bytes": path.stat().st_size}
    if (process["termination"] not in ("normal_exit", "process_error")
            or process["exit_code"] not in accepted_exits):
        return record
    try:
        if (directory / "stdout.log").stat().st_size > report_parse_limit_bytes:
            record["phase"] = "output_unvalidated_parse_guard"
            raise ValueError("stdout exceeds harness parsing limit; retain raw output without inferring a Report")
        delivered = read_json(directory / "stdout.log")
        report, hybrid = native_view(delivered)
        native_version = commands.get("interface", {}).get("versions", {}).get("native")
        if native_version in {12, 13, 14, 15} and report.get("schema_version") != native_version:
            raise ValueError(f"expected native report schema {native_version}")
        if native_version in {12, 13, 14, 15}:
            allowance, used = report.get("postprocessing_max_work"), report.get("postprocessing_work")
            if (type(allowance) is not int or allowance <= 0
                    or type(used) is not int or not 0 <= used <= allowance):
                raise ValueError("invalid native application postprocessing metadata")
        final = native_final(report, process, accepted_exits)
        if hybrid is not None:
            final["application"] = {key: delivered[key] for key in ("status", "work")}
            final["hybrid"] = {key: hybrid.get(key) for key in ("coverage", "precision", "rpc", "local_detail", "selected_graph")}
            final["graph_namespace"] = "Native" if hybrid["coverage"]["chosen"] == "Native" else "Baseline"
        record.update(nativefinal=final, report_delivered=True, phase="native_final_received")
        record["policy_matches"] = final["config"] == commands["expected_native_config"]
        if native_version in {12, 13, 14, 15}:
            record["policy_matches"] = record["policy_matches"] and (
                final.get("domain_spec") == commands["expected_native_domain_spec"]
                and final.get("numeric_model") == commands["expected_native_numeric_model"]
                and final["postprocessing_max_work"] == commands["expected_native_postprocessing_max_work"])
    except (ValueError, KeyError, TypeError, AttributeError, OSError, OverflowError, RecursionError) as error:
        record["report_error"] = {"type": type(error).__name__, "message": str(error)}
    return record


def executable_identity(path, build_path):
    with path.open("rb") as stream:
        header = stream.read(20)
    if len(header) < 20 or header[:4] != b"\x7fELF" or header[4] not in (1, 2) or header[5] not in (1, 2):
        raise ValueError("BINARY must be the actual built ELF, not a build/wrapper command")
    build = read_json(build_path)
    if build["schema"] != "evm-no-smt-scale-build-v1":
        raise ValueError("unsupported build metadata")
    for field in ("source_commit", "source_tree"):
        if not re.fullmatch(r"[0-9a-f]{40}", build[field]):
            raise ValueError(f"declare actual build {field}")
    if (type(build["source_dirty"]) is not bool or not isinstance(build["build_command"], list)
            or not build["build_command"]
            or any(not isinstance(arg, str) or not arg for arg in build["build_command"])):
        raise ValueError("declare build cleanliness and actual build argv")
    identity = {"path": str(path), "sha256": sha256(path), "bytes": path.stat().st_size,
                "elf_class_bits": 32 if header[4] == 1 else 64,
                "elf_byte_order": "little" if header[5] == 1 else "big",
                "elf_machine": int.from_bytes(header[18:20], "little" if header[5] == 1 else "big"),
                "build_metadata": build, "build_metadata_raw_sha256": sha256(build_path),
                "binding_scope": "Artifact hash checked; build-source statement is supplied by the builder"}
    if identity["sha256"] != build["artifact_sha256"]:
        raise ValueError("ELF hash does not match actual build metadata")
    if not os.access(path, os.X_OK):
        raise ValueError("ELF is not executable")
    return identity


def aggregate(samples):
    # No ratios: delivery, policy equality and semantic inclusion are separate obligations.
    return [{"case": name, "attempts": len(group),
             "delivered_reports": sum(sample["report_delivered"] for sample in group),
             "termination_counts": dict(Counter(sample["termination"] for sample in group)),
             "semantic_acceptance": "pending; counts/timings do not establish coverage"}
            for name in dict.fromkeys(sample["case"] for sample in samples)
            for group in [[sample for sample in samples if sample["case"] == name]]]


def run(arguments, manifest, cases, inputs, commands):
    if not all((arguments.binary, arguments.build_metadata, arguments.output, arguments.cpus,
                arguments.environment_note)):
        raise ValueError("--execute requires --binary, --build-metadata, --output, --cpus, --environment-note")
    binary, build_path = arguments.binary.resolve(strict=True), arguments.build_metadata.resolve(strict=True)
    artifact = executable_identity(binary, build_path)
    selected_cpus = cpus(arguments.cpus)
    root = arguments.root.resolve(strict=True)
    initial = source(root)
    build = artifact["build_metadata"]
    if any(build[key] != initial[actual] for key, actual in (
            ("source_commit", "commit"), ("source_tree", "tree"), ("source_dirty", "dirty"))):
        raise ValueError("--root source does not match the builder's declared source")
    if commands["interface"]["source_commit"] != build["source_commit"]:
        raise ValueError("command profile was not reviewed for this build's interface source")
    if initial["dirty"] and build.get("source_identity") != initial:
        raise ValueError("dirty builds require the complete matching source_identity in build metadata")
    if (initial["dirty"] or artifact["build_metadata"]["source_dirty"]) and not arguments.allow_dirty:
        raise ValueError("dirty sources/builds require --allow-dirty and remain preliminary")
    inputs.update({str(binary): sha256(binary), str(build_path): sha256(build_path)})
    harness = Path(__file__).resolve()
    helpers = [harness.parent / "benchmark_known_recursion" / name
               for name in ("identity.py", "process.py", "report.py")]
    for path in [harness, *helpers]:
        inputs[str(path)] = sha256(path)
    if any(sha256(Path(path)) != expected for path, expected in inputs.items()):
        raise ValueError("an input/artifact changed after plan validation")
    output = arguments.output.resolve()
    if output.is_relative_to(root):
        raise ValueError("store run evidence outside --root to keep source identity fixed")
    output.mkdir(parents=True, exist_ok=False)
    (output / "input-manifest.json").write_bytes(arguments.manifest.read_bytes())
    (output / "commands.json").write_bytes(arguments.commands.read_bytes())
    (output / "build-metadata.json").write_bytes(build_path.read_bytes())
    for name, case in cases.items():
        saved = output / "inputs" / name
        saved.mkdir(parents=True)
        (saved / "world.json").write_bytes(case["world_path"].read_bytes())
        if sha256(saved / "world.json") != case["spec"]["world"]["sha256"]:
            raise ValueError(f"{name}: copied world changed after identity validation")
        for index, path in enumerate(case["provenance_paths"]):
            (saved / f"provenance-{index:02d}-{path.name}").write_bytes(path.read_bytes())
    environment = os.environ.copy()
    removed = [key for key in ("LD_PRELOAD", "LD_AUDIT") if environment.pop(key, None) is not None]
    evidence = {"schema": "evm-no-smt-scale-results-v1", "started_utc": utc(),
                "baseline_source": manifest["baseline_source"], "source_before": initial,
                "artifact": artifact, "platform": platform.platform(),
                "environment_note": arguments.environment_note, "preload_environment_removed": removed,
                "preliminary": bool(initial["dirty"] or artifact["build_metadata"]["source_dirty"]),
                "command_profile_raw_sha256": sha256(arguments.commands),
                "requested_policy": commands["policy"], "requested_policy_sha256": digest(commands["policy"]),
                "expected_native_config_sha256": digest(commands["expected_native_config"]),
                "interface": commands["interface"], "policy_mapping": commands["policy_mapping"],
                "input_sha256_before": inputs.copy(), "samples": [],
                "timing_boundary": "Process exec through reaping, including native serialization; excludes builds, acquisition and Python Report parsing",
                "scope": "Offline screens; external stop is NoReport, never a speedup or semantic acceptance"}
    result_path = output / "measurements.json"
    write_json(result_path, evidence)
    interrupted = False
    try:
        for repetition in range(arguments.runs):
            for name, case in cases.items():
                guard = {**case["spec"]["planned_guard"], "cpus": selected_cpus}
                if available_memory() < guard["host_available_floor_kib"]:
                    raise RuntimeError("host memory below guard floor; no next process started")
                directory = output / f"{repetition:02d}-{name}"
                directory.mkdir()
                saved_world = output / "inputs" / name / "world.json"
                command = [str(binary) if arg == "BINARY" else str(saved_world) if arg == "WORLD" else arg
                           for arg in commands["cases"][name]["argv"]]
                command = ["taskset", "-c", ",".join(map(str, selected_cpus)), *command]
                try:
                    process = ProtectedProcess(command, directory, root, guard, environment)
                except OSError as error:
                    process_record = {"exit_code": None, "termination": "spawn_error",
                                      "error_type": type(error).__name__, "error": str(error)}
                    write_json(directory / "process.json", process_record)
                else:
                    try:
                        process_record = process.finish()
                    except KeyboardInterrupt:
                        process_record = process.finish("external_keyboard_interrupt")
                        interrupted = True
                sample = collect_report(directory, process_record, commands, guard["report_parse_limit_bytes"])
                sample.update(case=name, repetition=repetition, command=command,
                              command_sha256=digest(command), guard=guard, guard_sha256=digest(guard),
                              input_mapping=commands["cases"][name]["input_mapping"],
                              input_identity=case["identity"], input_identity_sha256=case["input_identity_sha256"])
                evidence["samples"].append(sample)
                write_json(directory / "sample.json", sample)
                write_json(result_path, evidence)
                print(f"{name}: exit={sample['exit_code']} {sample['termination']} Report={sample['report_delivered']} policy_matches={sample.get('policy_matches')}", flush=True)
                if interrupted:
                    raise KeyboardInterrupt
    except KeyboardInterrupt:
        interrupted = True
        evidence["batch_error"] = {"type": "KeyboardInterrupt"}
    except Exception as error:
        evidence["batch_error"] = {"type": type(error).__name__, "message": str(error)}
    finally:
        evidence.update(finished_utc=utc(), source_after=source(root),
                        input_sha256_after={path: sha256(path) if Path(path).is_file() else None for path in inputs},
                        aggregates=aggregate(evidence["samples"]))
        evidence["identities_unchanged"] = (evidence["source_before"] == evidence["source_after"]
                                             and evidence["input_sha256_before"] == evidence["input_sha256_after"])
        write_json(result_path, evidence)
    if interrupted:
        return 130
    return 0 if ("batch_error" not in evidence and evidence["identities_unchanged"]
                 and len(evidence["samples"]) == arguments.runs * len(cases)
                 and all(sample["report_delivered"] and sample.get("policy_matches") is True
                         for sample in evidence["samples"])) else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--commands", type=Path, required=True)
    parser.add_argument("--case", action="append", dest="cases")
    parser.add_argument("--execute", action="store_true", help="Start sequential guarded native runs; default only checks the plan")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--build-metadata", type=Path)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path)
    parser.add_argument("--cpus")
    parser.add_argument("--environment-note")
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--allow-dirty", action="store_true", help="Preliminary evidence only; never a frozen-build comparison")
    arguments = parser.parse_args()
    if arguments.runs < 1:
        parser.error("--runs must be positive")
    arguments.manifest, arguments.commands = arguments.manifest.resolve(), arguments.commands.resolve()
    manifest, cases, inputs = load_inputs(arguments.manifest, arguments.cases)
    commands = load_commands(arguments.commands, cases)
    inputs[str(arguments.commands)] = sha256(arguments.commands)
    if not arguments.execute:
        print(json.dumps({"phase": "plan_checked_no_process_started", "baseline_source": manifest["baseline_source"],
                          "interface": commands["interface"], "policy_sha256": digest(commands["policy"]),
                          "cases": {name: {"input_identity_sha256": case["input_identity_sha256"],
                                           "planned_guard": case["spec"]["planned_guard"]} for name, case in cases.items()},
                          "input_sha256": inputs}, indent=2))
        return 0
    return run(arguments, manifest, cases, inputs, commands)


if __name__ == "__main__":
    raise SystemExit(main())
