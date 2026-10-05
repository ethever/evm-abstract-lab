#!/usr/bin/env bash
set -euo pipefail

task_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$task_root"

task_cpu_count=$(nproc)
task_job_limit=$((task_cpu_count - 5))
if ((task_job_limit < 1)); then
  task_job_limit=1
fi
task_default_jobs=$task_job_limit
if ((task_default_jobs > 8)); then
  task_default_jobs=8
fi
task_jobs=${STATE_BACKEND_JOBS:-$task_default_jobs}
task_slots=${STATE_BACKEND_SLOTS:-64,4096}
task_iterations=${STATE_BACKEND_ITERATIONS:-100}
task_repeats=${STATE_BACKEND_REPEATS:-5}
task_output=${STATE_BACKEND_OUTPUT:-target/state-backend-comparison}

python3 - "$task_jobs" "$task_job_limit" "$task_slots" "$task_iterations" "$task_repeats" <<'PY'
import sys

jobs, job_limit, sizes, iterations, repeats = sys.argv[1:]
try:
    if not 1 <= int(jobs) <= int(job_limit):
        raise ValueError(f"STATE_BACKEND_JOBS must be between 1 and {job_limit}")
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
    --jobs "$task_jobs" "${task_features[@]}"
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
    "build_jobs": int(sys.argv[6]),
    "slots": sizes,
    "iterations": iterations,
    "repeats": repeats,
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
        cases.append((f"{fixture.stem}-{command}",
                      [command, "--file", str(fixture), "--format", "json"], 0))
for fixture in sorted((root / "examples/worlds").glob("*.json")):
    expected = 2 if fixture.stem == "missing-code" else 0
    arguments = ["analyze", "--world", str(fixture), "--entry", entry, "--format", "json"]
    cases.append((f"{fixture.stem}-analyze", arguments, expected))
    cases.append((f"{fixture.stem}-ssa", arguments + ["--ssa"], expected))
summary_world = root / "examples/worlds/summary-reuse.json"
cases.append(("summary-reuse-no-summaries",
              ["analyze", "--world", str(summary_world), "--entry", entry,
               "--format", "json", "--no-summaries"], 0))

parity = []
for label, arguments, expected in cases:
    canonical = {}
    for backend, executable in cli.items():
        result = subprocess.run([str(executable), *arguments], capture_output=True, text=True,
                                check=False)
        (output / f"{label}-{backend}.stderr").write_text(result.stderr)
        if result.returncode != expected:
            sys.exit(f"{label}/{backend}: expected exit {expected}, got {result.returncode}; "
                     f"see {output / f'{label}-{backend}.stderr'}")
        try:
            payload = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            sys.exit(f"{label}/{backend}: invalid JSON: {error}")
        if expected == 2:
            if payload.get("status") != "Incomplete" or not any(
                "MissingCode" in frontier["reason"] for frontier in payload["frontiers"]
            ):
                sys.exit(f"{label}/{backend}: missing typed MissingCode frontier")
        # Dict insertion order and array order are both retained: only whitespace is removed.
        canonical[backend] = json.dumps(payload, separators=(",", ":"), ensure_ascii=False)
        (output / f"{label}-{backend}.json").write_text(result.stdout)
    if canonical["std"] != canonical["imbl"]:
        sys.exit(f"{label}: ordered JSON differs; see {output}")
    parity.append({"case": label, "exit_code": expected,
                   "sha256": hashlib.sha256(canonical["std"].encode()).hexdigest()})

(output / "parity.json").write_text(json.dumps(parity, indent=2) + "\n")
print(json.dumps({"semantic_parity": True, "cli_cases": len(parity), "output": str(output)}),
      flush=True)

samples = []
signatures = {}
for size in sizes:
    for repeat in range(repeats):
        # Reverse process order on each repeat to reduce a consistent warm-cache bias.
        order = ("std", "imbl") if repeat % 2 == 0 else ("imbl", "std")
        for backend in order:
            result = subprocess.run([str(bench[backend]), str(size), str(iterations)],
                                    capture_output=True, text=True, check=True)
            batch = [json.loads(line) for line in result.stdout.splitlines()]
            expected_workloads = {"checkpoint_drop", "sparse_write_restore",
                                  "unknown_alias_write_restore", "join_drop"}
            if len(batch) != 4 or {item["workload"] for item in batch} != expected_workloads:
                sys.exit(f"{backend}/{size}: unexpected benchmark workloads")
            for sample in batch:
                if sample["backend"] != backend or sample["slots"] != size or sample["iterations"] != iterations:
                    sys.exit(f"{backend}/{size}: benchmark configuration differs")
                key = (size, sample["workload"])
                signature = {name: value for name, value in sample.items()
                             if name not in ("backend", "elapsed_ns")}
                if key in signatures and signatures[key] != signature:
                    sys.exit(f"{backend}/{size}/{sample['workload']}: semantic checksum differs")
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
metadata["semantic_parity"] = True
(output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
PY
