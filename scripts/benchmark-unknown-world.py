#!/usr/bin/env python3
"""Compare complete CLI requests with identical inputs and summary reuse on/off.

Linux wait4 measures each child independently. Reports and stderr stay beside
the aggregate evidence, including unsuccessful requests; incomplete coverage
never becomes a successful timing sample.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time


ROOT = Path(__file__).resolve().parent.parent
CASES = ["static-reuse", "busy-x", "busy-x-nested", "external-change"]


def constants(value):
    assert isinstance(value, dict) and "Constants" in value, value
    return {int(item, 0) for item in value["Constants"]}


def word(data, offset, allowed):
    # Require explicit sound byte facts. The sparse cache's omitted entries
    # cannot be replaced with the array default when a symbolic root exists.
    for index in range(offset, offset + 31):
        assert constants(data["bytes"][str(index)]) == {0}
    values = constants(data["bytes"][str(offset + 31)])
    assert values and values <= set(allowed), (values, allowed)
    return values


def slot(store, owner, key, allowed):
    address = f"0x{owner:040x}"
    plane = store["persistent"]
    for entry in plane["slots"]:
        if entry["address"] == address and int(entry["slot"], 0) == key:
            values = constants(entry["value"])
            break
    else:
        # Missing cache entries need an actual root proof, not the default
        # value of a potentially modified symbolic array.
        graph = plane["roots"][address]
        index = graph["root"]
        while True:
            node = graph["nodes"][index]
            if node == "SlotsZero":
                values = {0}
                break
            assert isinstance(node, dict) and "Store" in node, node
            write = node["Store"]
            stored_key = graph["nodes"][write["key"]]
            assert isinstance(stored_key, dict) and "Constant" in stored_key
            if int(stored_key["Constant"], 0) == key:
                value = graph["nodes"][write["value"]]
                assert isinstance(value, dict) and "Constant" in value
                values = {int(value["Constant"], 0)}
                break
            index = write["previous"]
    assert values and values <= allowed, (address, key, values, allowed)
    return values


def precision(report, case):
    assert report["status"] == "Converged" and not report["frontiers"]
    summaries = report["procedure_summaries"]
    assert summaries and all(item["closed"] and item["procedures"] for item in summaries)
    returns = [item for item in report["outcomes"] if item["kind"] == "Return"]
    assert returns
    slots = ([(0x101, 0, {42})] if case == "static-reuse" else
             [(0x101, 0, {0}), (0x101, 1, {0, 1, 2})] if case.startswith("busy-x") else
             [(0x301, 0, {5, 9}), (0x301, 1, {1})])
    store_values = {f"{owner:#x}:{key}": set() for owner, key, _ in slots}
    first, second = set(), set()
    for outcome in returns:
        data = outcome["data"]
        for owner, key, allowed in slots:
            store_values[f"{owner:#x}:{key}"] |= slot(outcome["store"], owner, key, allowed)
        if case == "static-reuse":
            assert constants(data["length"]) == {32}
            first |= word(data, 0, {42})
        elif case.startswith("busy-x"):
            assert constants(data["length"]) == {64}
            first |= word(data, 0, {0, 1, 2})
            second |= word(data, 32, {1})
        else:
            assert constants(data["length"]) == {64}
            first |= word(data, 0, {5})
            second |= word(data, 32, {5, 9})
    expected = ({42}, set()) if case == "static-reuse" else (
        ({0, 1, 2}, {1}) if case.startswith("busy-x") else ({5}, {5, 9})
    )
    assert (first, second) == expected, (case, first, second)
    for owner, key, expected in slots:
        assert store_values[f"{owner:#x}:{key}"] == expected
    return {"return_word_0": sorted(first), "return_word_1": sorted(second),
            "storage": {key: sorted(values) for key, values in store_values.items()}}


def graph_sizes(report):
    pending = [report]
    roots, nodes, largest = 0, 0, 0
    while pending:
        value = pending.pop()
        if isinstance(value, dict):
            if "identity_scope" in value and isinstance(value.get("nodes"), list):
                size = len(value["nodes"])
                roots += 1
                nodes += size
                largest = max(largest, size)
            pending.extend(value.values())
        elif isinstance(value, list):
            pending.extend(value)
    # Native JSON repeats roots in observations. These are serialized sizes,
    # not counts of globally unique live DAG nodes or solver formulas.
    return {"serialized_roots": roots, "serialized_nodes": nodes,
            "largest_serialized_graph": largest}


def run(binary, output, case, reuse, repetition, max_work):
    fixture = ROOT / "examples" / "unknown-world" / f"{case}.json"
    name = f"{case}-{'on' if reuse else 'off'}-{repetition}"
    report_path, error_path = output / f"{name}.json", output / f"{name}.stderr"
    command = [
        str(binary), "analyze", "--world", str(fixture),
        "--evm.to", "0x0000000000000000000000000000000000000101",
        "--evm.caller", "0x0000000000000000000000000000000000000900",
        "--evm.value", "0", "--evm.calldata", "0x", "--format", "json",
        "--max-work", str(max_work),
    ]
    if not reuse:
        command.append("--no-summaries")
    started = time.monotonic()
    with report_path.open("wb") as stdout, error_path.open("wb") as stderr:
        process = subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=stderr)
        _, status, usage = os.wait4(process.pid, 0)
        process.returncode = os.waitstatus_to_exitcode(status)
    elapsed = time.monotonic() - started
    if process.returncode != 0:
        raise RuntimeError(f"{name}: exit {process.returncode}; inspect {report_path} and {error_path}")
    report = json.loads(report_path.read_text())
    checked = precision(report, case)
    record = {
        "case": case, "reuse": reuse, "repetition": repetition,
        "wall_seconds": elapsed, "user_seconds": usage.ru_utime,
        "system_seconds": usage.ru_stime, "peak_rss_kib": usage.ru_maxrss,
        "report_bytes": report_path.stat().st_size,
        "status": report["status"], "frontiers": len(report["frontiers"]),
        "states": len(report["states"]), "transfers": report["transfers"],
        "work": report["work"], "summary_requests": len(report["procedure_summaries"]),
        "relation_states": sum(item["state_count"] for item in report["procedure_summaries"]),
        "propagations": sum(item["propagations"] for item in report["procedure_summaries"]),
        "reused": sum(item["reused"] for item in report["procedure_summaries"]),
        "checked_precision": checked, "config": report["config"],
        "graph_sizes": graph_sizes(report),
        "fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
    }
    print(f"{name}: {elapsed:.3f}s, {usage.ru_maxrss} KiB, work={report['work']}", flush=True)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--max-work", type=int, default=3_000_000_000)
    parser.add_argument("--case", choices=CASES, action="append")
    args = parser.parse_args()
    if platform.system() != "Linux" or args.runs < 1 or args.max_work < 1:
        parser.error("requires Linux, positive runs and positive max-work")
    def git(*arguments):
        return subprocess.check_output(["git", *arguments], cwd=ROOT, text=True).strip()

    if git("status", "--porcelain"):
        parser.error("commit the source before recording measurements")
    revision = git("rev-parse", "HEAD")
    tree = git("rev-parse", "HEAD^{tree}")
    binary = args.binary.resolve(strict=True)
    binary_sha256 = hashlib.sha256(binary.read_bytes()).hexdigest()
    if args.output.exists():
        parser.error("use a new output directory to preserve previous evidence")
    args.output.mkdir(parents=True)
    started_at = datetime.now(timezone.utc).isoformat()
    records, aggregates = [], []
    for case in args.case or CASES:
        for repetition in range(args.runs):
            for reuse in ([True, False] if repetition % 2 == 0 else [False, True]):
                records.append(run(binary, args.output, case, reuse, repetition, args.max_work))
        samples = [item for item in records if item["case"] == case]
        normalized_configs = []
        for sample in samples:
            config = dict(sample["config"])
            config.pop("use_summaries")
            normalized_configs.append(config)
        assert all(config == normalized_configs[0] for config in normalized_configs)
        assert all(item["checked_precision"] == samples[0]["checked_precision"] for item in samples)
        for reuse in [True, False]:
            group = [item for item in samples if item["reuse"] == reuse]
            aggregates.append({
                "case": case, "reuse": reuse,
                "median_seconds": statistics.median(item["wall_seconds"] for item in group),
                "median_peak_rss_kib": statistics.median(item["peak_rss_kib"] for item in group),
                "work": sorted({item["work"] for item in group}),
                "checked_precision": group[0]["checked_precision"],
            })
    assert not git("status", "--porcelain") and git("rev-parse", "HEAD") == revision
    assert hashlib.sha256(binary.read_bytes()).hexdigest() == binary_sha256
    evidence = {
        "source_commit": revision, "source_tree": tree,
        "started_at": started_at, "finished_at": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(), "logical_cpus": os.cpu_count(),
        "binary_sha256": binary_sha256,
        "command_template": "analyze --world CASE --evm.to 0x101 --evm.caller 0x900 --evm.value 0 --evm.calldata 0x --format json --max-work LIMIT [--no-summaries]",
        "scope": "CLI analysis and JSON export; alternating order; same specified return-word and key-storage numeric precision, and closed coverage; no claim of full relational equivalence",
        "samples": records, "aggregates": aggregates,
    }
    (args.output / "measurements.json").write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
