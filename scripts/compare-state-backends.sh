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
    "binaries": {
        backend: {
            "cli_sha256": hashlib.sha256(cli[backend].read_bytes()).hexdigest(),
            "benchmark_sha256": hashlib.sha256(bench[backend].read_bytes()).hexdigest(),
        } for backend in cli
    },
}
(output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
print(json.dumps({"run_started": True, "output": str(output)}), flush=True)

def only(items):
    (item,) = items
    return item


def memory_word_seven(word):
    # The numeric abstraction can be coarse; the typed root proves this word.
    assert word["provenance"]["origins"] == {"Sources": ["Memory"]}
    bounds = word["interval"]
    assert int(bounds["unsigned_lo"], 16) <= 7 <= int(bounds["unsigned_hi"], 16)
    biased = 7 ^ (1 << 255)
    assert int(bounds["signed_lo"], 16) <= biased <= int(bounds["signed_hi"], 16)
    congruence = word["congruence"]
    if congruence != "Top":
        if set(congruence) == {"Exact"}:
            assert int(congruence["Exact"], 16) == 7
        else:
            assert set(congruence) == {"Modulo"}
            modulo = congruence["Modulo"]
            modulus = int(modulo["modulus"], 16)
            assert modulus > 0 and 7 % modulus == int(modulo["residue"], 16)
    assert int(word["known_bits"]["zero"], 16) & 7 == 0
    assert int(word["known_bits"]["one"], 16) & 7 == int(word["known_bits"]["one"], 16)
    if "Constants" in word:
        assert "0x7" in word["Constants"]
    graph = word["expression"]
    assert graph["identity_scope"]
    nodes = graph["nodes"]
    read = nodes[graph["root"]]["WordRead"]
    assert nodes[read["offset"]] == {"Constant": "0x0"}
    view = nodes[read["view"]]["View"]
    assert nodes[view["length"]] == {"Constant": "0x20"}
    write = nodes[view["state"]]["WriteWord"]
    assert nodes[write["previous"]] == "BytesZero"
    assert nodes[write["value"]] == {"Constant": "0x7"}
    # Equality is between ExprIds in this graph, never two unrelated offsets.
    assert view["offset"] == write["offset"]
    return graph, view["state"]


def effect_chain(ir, block):
    assert block["effect"] == {"result": 0, "inputs": []}
    for index, instruction in enumerate(block["instructions"]):
        assert instruction["fault"] is False
        assert instruction["effect_input"] == index
        assert instruction["effect_result"] == index + 1
    assert block["exit_effect"] == len(block["instructions"])
    assert ir["effect_count"] == block["exit_effect"] + 1


def assert_logical_memory(payload):
    analysis = payload["analysis"] if "analysis" in payload else payload
    assert analysis["status"] == "Converged" and analysis["frontiers"] == []
    assert analysis["program"]["fork"] == "osaka"
    local = only(analysis["states"])
    assert local["executed_pcs"] == [0, 1, 2, 3, 5, 6, 7, 8]
    word = only(local["exit_stack"])
    graph, written = memory_word_seven(word)
    if "analysis" not in payload:
        return
    assert "partial_ssa" not in payload
    machine, ir = payload["machine_analysis"], payload["ssa"]
    assert machine["status"] == ir["status"] == "Converged"
    assert machine["frontiers"] == ir["frontiers"] == [] and ir["complete"] is True
    state, block = only(machine["states"]), only(ir["blocks"])
    assert payload["state_mapping"] == [{"local_state": local["id"], "machine_state": state["id"]}]
    assert state["exit_stack"] == [word]
    frame = state["exit"]["call_stack"]["root"]["state"]
    assert frame["stack"] == [word]
    memory = frame["memory"]["root"]
    assert memory["identity_scope"] == graph["identity_scope"] and memory["root"] == written
    assert memory["nodes"] == graph["nodes"][:len(memory["nodes"])]
    assert memory["nodes"][memory["root"]] == graph["nodes"][written]
    assert block["state"] == state["id"] and block["coverage"] == "Current"
    assert block["incoming_complete"] is True and block["open_incoming"] == []
    assert ir["transitions"] == ir["deferred_edges"] == []
    instructions = block["instructions"]
    assert [item["pc"] for item in instructions] == state["executed_pcs"] == local["executed_pcs"]
    assert all(item["progress"] == "Completed" for item in instructions[:-1])
    assert instructions[-1]["opcode"] == 0 and instructions[-1]["progress"] == "Dispatched"
    effect_chain(ir, block)
    write = only([item for item in instructions if item["pc"] == 6 and item["opcode"] == 82])
    read = only([item for item in instructions if item["pc"] == 7 and item["opcode"] == 81])
    offset = only(read["operands"])
    literal = only([item for item in instructions if item["pc"] == 3 and item["opcode"] == 96])
    assert literal["operation"] == {"Constant": {"value": "0x7"}}
    assert write["results"] == [] and write["operands"] == [offset, only(literal["results"])]
    assert write["operation"] == {"Opcode": {"opcode": 82, "operands": [{"Value": offset}, {"Constant": "0x7"}]}}
    assert read["operation"] == {"Opcode": {"opcode": 81, "operands": [{"Value": offset}]}}
    result = only(read["results"])
    assert block["exit_frames"] == [[result]]
    # This complete one-block capture exports the current read in Definition scope.
    fact = only([item for item in ir["facts"] if item["value"] == result
                 and item["scope"] == "Definition" and item["validity"] == "Current"
                 and item["source"] == "Instruction"])
    assert fact["provenance"] == word["provenance"]
    assert fact["numeric"] == {key: value for key, value in word.items()
                               if key not in ("provenance", "expression")}
    assert fact["constant"] is None or fact["constant"] == "0x7"
    assert any(item["kind"] == "Failure" and item["state"] == state["id"] for item in machine["outcomes"])
    assert any(item["kind"] == "Return" and item["state"] == state["id"] for item in machine["outcomes"])


def assert_retained_partial(payload):
    analysis, machine, ir = payload["analysis"], payload["machine_analysis"], payload["partial_ssa"]
    assert "ssa" not in payload and ir["complete"] is False
    assert analysis["status"] == machine["status"] == ir["status"] == "Incomplete"
    assert analysis["program"]["fork"] == "osaka"
    local, state, block = only(analysis["states"]), only(machine["states"]), only(ir["blocks"])
    assert local["id"] == state["id"] == block["state"] == 0
    assert payload["state_mapping"] == [{"local_state": 0, "machine_state": 0}]
    reason = {"Relations": "ExpressionLimit"}
    assert analysis["frontiers"] == [{"reason": reason, "from": 0,
        "target": {"basic_block_index": 0, "stack_height": 0, "context": []}, "limit": "Model"}]
    assert machine["frontiers"] == [{"from": 0, "target": state["key"], "pc": 4, "reason": reason, "range": None}]
    assert ir["frontiers"] == machine["frontiers"]
    assert analysis["edges"] == machine["edges"] == ir["transitions"] == ir["deferred_edges"] == []
    assert local["executed_pcs"] == state["executed_pcs"] == [0, 2, 4]
    assert local["exit_stack"] == state["exit_stack"] == []
    before = state["entry"]["call_stack"]["root"]["state"]["memory"]
    frame = state["exit"]["call_stack"]["root"]["state"]
    after = frame["memory"]
    assert frame["stack"] == [] and before["length"]["Constants"] == ["0x0"]
    # Expansion precedes refusal; the retained contents are still pristine.
    assert after["length"]["Constants"] == ["0x20"] and before["root"] == after["root"]
    for memory in (before, after):
        assert memory["memory"] is True and memory["default"]["Constants"] == ["0x0"]
        assert memory["root"]["nodes"][memory["root"]["root"]] == "BytesZero"
        assert memory["bytes"] == {} and memory["operands"] == memory["conditional"] == []
        assert memory["failure"] is None
    assert machine["outcomes"] and all(item["kind"] == "Failure" for item in machine["outcomes"])
    assert block["coverage"] == "Current" and block["incoming_complete"] is False
    assert block["open_incoming"] == [] and block["exit_frames"] == [[]]
    assert [(item["pc"], item["opcode"], item["progress"]) for item in block["instructions"]] == [
        (0, 96, "Completed"), (2, 96, "Completed"), (4, 82, "OperandsConsumed")]
    one, zero, write = block["instructions"]
    operands = [only(zero["results"]), only(one["results"])]
    assert len(set(operands)) == ir["value_count"] == 2
    assert write["operands"] == operands and write["results"] == []
    assert write["operation"] == {"Opcode": {"opcode": 82, "operands": [{"Constant": "0x0"}, {"Constant": "0x1"}]}}
    for instruction, literal in ((one, "0x1"), (zero, "0x0")):
        assert instruction["operation"] == {"Constant": {"value": literal}}
        fact = only([item for item in ir["facts"] if item["value"] == only(instruction["results"])
                     and item["scope"] == {"State": {"state": 0}} and item["validity"] == "Current"
                     and item["source"] == "Instruction"])
        assert fact["constant"] == literal and fact["numeric"]["Constants"] == [literal]
        assert fact["provenance"]["origins"] == {"Sources": ["Constant"]}
    effect_chain(ir, block)

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
    arguments = ["analyze", "--world", str(fixture), "--evm.to", entry, "--evm.caller", "0x0000000000000000000000000000000000001000", "--evm.value", "0", "--evm.calldata", "0x", "--format", "json"]
    cases.append((f"{fixture.stem}-analyze", arguments, expected))
    cases.append((f"{fixture.stem}-ssa", arguments + ["--ssa"], expected))
summary_world = root / "examples/worlds/summary-reuse.json"
cases.append(("summary-reuse-no-summaries",
              ["analyze", "--world", str(summary_world), "--evm.to", entry, "--evm.caller", "0x0000000000000000000000000000000000001000", "--evm.value", "0", "--evm.calldata", "0x",
               "--format", "json", "--no-summaries"], 0))

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
            wrapped = arguments[0] == "ssa" or "--ssa" in arguments
            analysis = payload.get("analysis", {}) if wrapped else payload
            if analysis.get("status") != "Converged" or analysis.get("frontiers") != []:
                sys.exit(f"{label}/{backend}: expected converged analysis without frontiers")
            if wrapped:
                ir = payload.get("ssa", {})
                if ir.get("status") != "Converged" or ir.get("complete") is not True:
                    sys.exit(f"{label}/{backend}: expected complete SSA wrapper")
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
