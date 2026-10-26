#!/usr/bin/env bash
set -euo pipefail

task_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$task_root"

task_jobs=${STATE_BACKEND_JOBS:-}
task_build_args=()
if [[ -n "$task_jobs" ]]; then
  task_build_args=(--jobs "$task_jobs")
fi
task_slots=${STATE_BACKEND_SLOTS:-64,4096}
task_iterations=${STATE_BACKEND_ITERATIONS:-100}
task_repeats=${STATE_BACKEND_REPEATS:-5}
task_output=${STATE_BACKEND_OUTPUT:-target/state-backend-comparison}

python3 - "$task_jobs" "$task_slots" "$task_iterations" "$task_repeats" <<'PY'
import sys

jobs, sizes, iterations, repeats = sys.argv[1:]
try:
    if jobs and (not jobs.isdecimal() or int(jobs) < 1):
        raise ValueError("STATE_BACKEND_JOBS must be a positive integer")
    if not sizes or not all(1 <= int(size) <= 65536 for size in sizes.split(",")):
        raise ValueError("STATE_BACKEND_SLOTS must list integers between 1 and 65536")
    if not 1 <= int(iterations) <= 10000:
        raise ValueError("STATE_BACKEND_ITERATIONS must be between 1 and 10000")
    if not 1 <= int(repeats) <= 25:
        raise ValueError("STATE_BACKEND_REPEATS must be between 1 and 25")
except ValueError as error:
    sys.exit(str(error))
PY

for task_backend in std imbl; do
  task_features=()
  if [[ "$task_backend" == imbl ]]; then
    task_features=(--features evm-abstract/imbl,evm-abstract-cli/imbl)
  fi
  cargo build --release --locked --no-default-features \
    -p evm-abstract -p evm-abstract-cli \
    --bin evm-abstract --example state-backends \
    --target-dir "target/state-backends-$task_backend" \
    "${task_build_args[@]}" "${task_features[@]}"
done

python3 - "$task_root" "$task_slots" "$task_iterations" "$task_repeats" "$task_output" "$task_jobs" <<'PY'
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile

root = Path(sys.argv[1])
sys.path.insert(0, str(root / "scripts"))
from normalize_identity_scopes import canonical_json
from verify_logical_memory import assert_logical_memory, assert_retained_partial

sizes = list(dict.fromkeys(int(size) for size in sys.argv[2].split(",")))
iterations, repeats = map(int, sys.argv[3:5])
output_base = Path(sys.argv[5]).resolve()
output_base.mkdir(parents=True, exist_ok=True)
timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
output = Path(tempfile.mkdtemp(prefix=f"{timestamp}-", dir=output_base))
cli = {backend: root / f"target/state-backends-{backend}/release/evm-abstract"
       for backend in ("std", "imbl")}
bench = {backend: root / f"target/state-backends-{backend}/release/examples/state-backends"
         for backend in cli}

def command_output(*arguments):
    return subprocess.check_output(arguments, cwd=root)

source_diff = command_output("git", "diff", "--binary", "HEAD")
untracked = command_output("git", "ls-files", "--others", "--exclude-standard", "-z")
untracked_digest = hashlib.sha256()
for filename in sorted(path for path in untracked.split(b"\0") if path):
    path = root / os.fsdecode(filename)
    untracked_digest.update(filename + b"\0")
    untracked_digest.update(path.read_bytes())
status = command_output("git", "status", "--porcelain=v1").decode()
metadata = {
    "started_at_utc": timestamp,
    "completed": False,
    "git_sha": command_output("git", "rev-parse", "HEAD").decode().strip(),
    "git_dirty": bool(status),
    "git_status": status,
    "git_diff_sha256": hashlib.sha256(source_diff).hexdigest(),
    "untracked_source_sha256": untracked_digest.hexdigest(),
    "rustc": command_output("rustc", "--version", "--verbose").decode().strip(),
    "cargo": command_output("cargo", "--version").decode().strip(),
    "host": platform.platform(),
    "machine": platform.machine(),
    "cpu_count": os.cpu_count(),
    "build_jobs": int(sys.argv[6]) if sys.argv[6] else None,
    "slots": sizes,
    "iterations": iterations,
    "repeats": repeats,
    "storage_validation_scope": "all listed slots and one unlisted default read; complete numeric payloads",
    "restoration_validation_scope": "full Store equality within each benchmark process",
    "binaries": {
        backend: {
            "cli_sha256": hashlib.sha256(cli[backend].read_bytes()).hexdigest(),
            "benchmark_sha256": hashlib.sha256(bench[backend].read_bytes()).hexdigest(),
        } for backend in cli
    },
}
(output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
print(json.dumps({"run_started": True, "output": str(output)}), flush=True)


entry = "0x0000000000000000000000000000000000000101"
cases = []
for fixture in sorted((root / "examples").glob("*.hex")):
    for command in ("disasm", "cfg", "ssa"):
        # Logical MSTORE/MLOAD completes even with a full-width calldata offset.
        expected = 0
        arguments = [command, "--file", str(fixture), "--format", "json"]
        cases.append((f"{fixture.stem}-{command}",
                      arguments, expected))
# Keep exit-2 and retained partial SSA coverage on an actual expression refusal.
cases.append(("expression-limit-ssa",
              ["ssa", "--hex", "600160005200", "--evm.to", entry, "--evm.calldata", "0x",
               "--max-symbolic-nodes", "2", "--format", "json", "--allow-partial-ssa"], 2))
for fixture in sorted((root / "examples/worlds").glob("*.json")):
    expected = 0
    fixture_entry = ("0x1111111111111111111111111111111111111111"
                     if fixture.name == "nested-detail-regions.json" else entry)
    arguments = ["analyze", "--world", str(fixture), "--evm.to", fixture_entry, "--evm.caller", "0x0000000000000000000000000000000000001000", "--evm.value", "0", "--evm.calldata", "0x", "--format", "json"]
    cases.append((f"{fixture.stem}-analyze", arguments, expected))
    cases.append((f"{fixture.stem}-ssa", arguments + ["--ssa"], expected))
summary_world = root / "examples/worlds/summary-reuse.json"
cases.append(("summary-reuse-no-summaries",
              ["analyze", "--world", str(summary_world), "--evm.to", entry, "--evm.caller", "0x0000000000000000000000000000000000001000", "--evm.value", "0", "--evm.calldata", "0x",
               "--format", "json", "--no-summaries"], 0))

def completed_analysis(payload, arguments):
    """Keep standalone SSA and Hybrid Native evidence in their real slots."""
    wrapped = arguments[0] == "ssa" or "--ssa" in arguments
    if arguments[0] == "analyze":
        global_graph, native_graph = payload.get("global"), payload.get("native")
        coverage = payload.get("hybrid", {}).get("coverage", {})
        if (payload.get("schema_version") != 16
                or not isinstance(global_graph, dict)
                or not isinstance(native_graph, dict)
                or payload.get("status") != "Converged"
                or coverage.get("baseline_current") is not True
                or coverage.get("detail_current") is not True
                or global_graph.get("analysis", {}).get("status") != "Converged"):
            raise ValueError("expected current Global and Native graphs in native16 Hybrid report")
        analysis, ir = native_graph.get("analysis", {}), native_graph.get("ssa", {})
    else:
        analysis = payload.get("analysis", {}) if wrapped else payload
        ir = payload.get("ssa", {})
    if (not isinstance(analysis, dict) or analysis.get("status") != "Converged"
            or analysis.get("frontiers") != []):
        raise ValueError("expected converged Native analysis without frontiers")
    if wrapped and (not isinstance(ir, dict) or ir.get("status") != "Converged"
                    or ir.get("complete") is not True):
        raise ValueError("expected complete SSA in the analysis graph's own wrapper")
    return analysis


parity = []
for label, arguments, expected in cases:
    canonical = {}
    for backend, executable in cli.items():
        result = subprocess.run([str(executable), *arguments], capture_output=True, text=True,
                                check=False)
        (output / f"{label}-{backend}.stderr").write_text(result.stderr)
        (output / f"{label}-{backend}.json").write_text(result.stdout)
        if result.returncode != expected:
            sys.exit(f"{label}/{backend}: expected exit {expected}, got {result.returncode}; "
                     f"see {output / f'{label}-{backend}.stderr'}")
        try:
            payload = json.loads(result.stdout)
            canonical[backend] = canonical_json(result.stdout)
        except ValueError as error:
            sys.exit(f"{label}/{backend}: invalid JSON: {error}")
        if label == "expression-limit-ssa":
            assert_retained_partial(payload)
        elif arguments[0] != "disasm":
            try:
                analysis = completed_analysis(payload, arguments)
            except ValueError as error:
                sys.exit(f"{label}/{backend}: {error}")
            if label.startswith("missing-code-"):
                summaries = analysis.get("procedure_summaries", [])
                if not summaries or not all(summary.get("closed") is True
                                            and summary.get("procedures")
                                            for summary in summaries):
                    sys.exit(f"{label}/{backend}: expected nonempty closed unknown relations")
        if label in ("memory-symbolic-offset-cfg", "memory-symbolic-offset-ssa"):
            assert_logical_memory(payload)
        # Whole-report producer renaming retains every other identity and member/array order.
    if canonical["std"] != canonical["imbl"]:
        sys.exit(f"{label}: ordered JSON differs; see {output}")
    parity.append({"case": label, "exit_code": expected,
                   "sha256": hashlib.sha256(canonical["std"].encode()).hexdigest()})

(output / "parity.json").write_text(json.dumps(parity, indent=2) + "\n")
print(json.dumps({"cli_reports_match": True, "cli_cases": len(parity), "output": str(output)}),
      flush=True)

samples = []
signatures = {}
for size in sizes:
    for repeat in range(repeats):
        # Reverse process order on each repeat to reduce a consistent warm-cache bias.
        order = ("std", "imbl") if repeat % 2 == 0 else ("imbl", "std")
        for backend in order:
            result = subprocess.run([str(bench[backend]), str(size), str(iterations)],
                                    capture_output=True, text=True, check=False)
            capture = f"store-{size}-{repeat}-{backend}"
            (output / f"{capture}.stdout.jsonl").write_text(result.stdout)
            (output / f"{capture}.stderr").write_text(result.stderr)
            if result.returncode != 0:
                metadata["failure"] = {"stage": "store", "backend": backend,
                                       "slots": size, "repeat": repeat,
                                       "exit_code": result.returncode}
                (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
                sys.exit(f"{backend}/{size}: Store example exited {result.returncode}; "
                         f"see {output / f'{capture}.stderr'}")
            batch = [json.loads(line) for line in result.stdout.splitlines()]
            expected_workloads = {"checkpoint_drop", "sparse_write_restore",
                                  "unknown_alias_write_restore", "join_drop"}
            if len(batch) != 4 or {item["workload"] for item in batch} != expected_workloads:
                sys.exit(f"{backend}/{size}: unexpected benchmark workloads")
            for sample in batch:
                if set(sample) != {
                    "backend", "workload", "slots", "iterations", "writes_per_iteration",
                    "relation_limits", "elapsed_ns", "final_slot_count", "final_work_size",
                    "storage_observation_scope", "storage_observation_count", "unlisted_slot",
                    "baseline_storage_observation_checksum", "effect_storage_observation_checksum",
                    "restored_storage_observation_checksum", "restore_verified",
                }:
                    sys.exit(f"{backend}/{size}: unsupported storage observation sample")
                if sample["backend"] != backend or sample["slots"] != size or sample["iterations"] != iterations:
                    sys.exit(f"{backend}/{size}: benchmark configuration differs")
                writes = min(size, 8)
                if sample["relation_limits"] != {
                    "enabled": True, "max_constraints": 128,
                    "max_nodes": max(1024, 6 * size + 3 * writes + 6),
                    "max_depth": max(64, size + writes + 3),
                }:
                    sys.exit(f"{backend}/{size}: bounded Store construction policy differs")
                if (sample["storage_observation_scope"] != "all-listed-slots-plus-unlisted-default-read"
                        or sample["storage_observation_count"] != size + 1
                        or int(sample["unlisted_slot"], 16) != size
                        or sample["restore_verified"] is not True
                        or sample["final_slot_count"] != size
                        or sample["baseline_storage_observation_checksum"] != sample["restored_storage_observation_checksum"]):
                    sys.exit(f"{backend}/{size}: incomplete storage observation or restoration evidence")
                unchanged = (sample["baseline_storage_observation_checksum"] ==
                             sample["effect_storage_observation_checksum"])
                if unchanged != (sample["workload"] == "checkpoint_drop"):
                    sys.exit(f"{backend}/{size}/{sample['workload']}: expected storage effect is missing")
                key = (size, sample["workload"])
                signature = {name: value for name, value in sample.items()
                             if name not in ("backend", "elapsed_ns")}
                if key in signatures and signatures[key] != signature:
                    sys.exit(f"{backend}/{size}/{sample['workload']}: tested storage observations differ")
                signatures[key] = signature
                sample["repeat"] = repeat
                samples.append(sample)

(output / "samples.jsonl").write_text("".join(json.dumps(sample) + "\n" for sample in samples))
report = []
for size, workload in sorted(signatures):
    medians = {
        backend: statistics.median(sample["elapsed_ns"] for sample in samples
                                   if sample["backend"] == backend and sample["slots"] == size
                                   and sample["workload"] == workload)
        for backend in cli
    }
    row = {"slots": size, "workload": workload, "iterations": iterations, "repeats": repeats,
           "std_median_ns": medians["std"], "imbl_median_ns": medians["imbl"],
           "std_over_imbl": medians["std"] / medians["imbl"] if medians["imbl"] else None}
    report.append(row)
    print(json.dumps(row), flush=True)
(output / "medians.json").write_text(json.dumps(report, indent=2) + "\n")
metadata["completed"] = True
metadata["cli_reports_match"] = True
metadata["storage_observations_match"] = True
metadata["restore_verified"] = True
(output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
PY
