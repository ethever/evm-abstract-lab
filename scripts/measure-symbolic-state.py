#!/usr/bin/env python3
"""Measure each compiled acceptance case, excluding Cargo/compiler memory."""
import argparse
import datetime
import json
import hashlib
import platform
import shutil
import os
from pathlib import Path
import re
import subprocess
import sys
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary", type=Path, help="freshly built symbolic_state test executable")
parser.add_argument("source", type=Path, help="clean source tree used to build the executable")
parser.add_argument("output", type=Path, help="new directory for the frozen executable and results")
args = parser.parse_args()
binary = args.binary.resolve()
source = args.source.resolve()
out = args.output.resolve()
out.mkdir(parents=True, exist_ok=False)
sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
if subprocess.check_output(["git", "status", "--porcelain"], cwd=source, text=True).strip():
    raise SystemExit("measurement requires a clean source tree")
original_binary = binary
binary = out / "acceptance-test-binary"
shutil.copy2(original_binary, binary)
with binary.open("rb") as compiled:
    binary_sha = hashlib.file_digest(compiled, "sha256").hexdigest()
listing = subprocess.check_output([str(binary), "--list"], text=True)
names = [line.removesuffix(": test") for line in listing.splitlines() if line.endswith(": test")]
report = {
    "source_sha": sha,
    "binary": str(binary),
    "original_binary": str(original_binary),
    "binary_sha256": binary_sha,
    "platform": platform.platform(),
    "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "measurement": "one fresh compiled Rust test process per case; peak RSS from wait4 excludes Cargo/compiler; wall includes process startup and test fixture/oracle work",
    "cases": [],
}
for i, name in enumerate(names):
    path = out / f"{i:02d}-{name.replace('::', '_')}.log"
    start = time.monotonic()
    with path.open("w") as log:
        process = subprocess.Popen([str(binary), name, "--exact", "--nocapture", "--test-threads=1"], stdout=log, stderr=subprocess.STDOUT)
        _, status, usage = os.wait4(process.pid, 0)
        process.returncode = os.waitstatus_to_exitcode(status)
    case = {"name": name, "exit": process.returncode, "wall_seconds": time.monotonic() - start, "peak_rss_kib": usage.ru_maxrss, "user_seconds": usage.ru_utime, "system_seconds": usage.ru_stime, "log": path.name, "analyses": []}
    for line in path.read_text().splitlines():
        if "issue111 evidence:" in line:
            fields = dict(re.findall(r"(\w+)=([^ ]+)", line.split(" frontiers=", 1)[0]))
            case["analyses"].append({k: int(v) if v.isdigit() else v for k, v in fields.items()})
    report["cases"].append(case)
    (out / "measurements.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"case": name, "exit": process.returncode, "wall_seconds": round(case["wall_seconds"], 3), "peak_rss_kib": case["peak_rss_kib"], "analyses": len(case["analyses"])}), flush=True)
report["all_passed"] = all(case["exit"] == 0 for case in report["cases"])
report["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
(out / "measurements.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
sys.exit(0 if report["all_passed"] else 1)
