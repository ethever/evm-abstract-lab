"""Optional verified external native Z3 instrumentation, separate from wall runs."""

import json
from pathlib import Path
import re
import subprocess
import time

from .process import sha256


def verify(library, proof_path, binaries):
    proof = json.loads(proof_path.read_bytes())
    counters = proof["smoke_counters"]
    if (proof["schema"] != "z3-check-probe-verification-v1" or proof["library_sha256"] != sha256(library)
            or proof["smoke_exit_code"] != 0 or counters["outer_checks"] != 2
            or counters["sat"] != 1 or counters["unsat"] != 1 or counters["unknown"] != 0
            or counters["clock_failures"] != 0):
        raise ValueError("native probe needs a matching successful independent sat/unsat interception smoke")
    for path in binaries:
        symbols = subprocess.check_output(["readelf", "--dyn-syms", "--wide", str(path)], text=True)
        if not re.search(r"\bUND\s+Z3_solver_check\b", symbols):
            raise ValueError("target has no dynamic Z3 check import; cannot claim that this preload observes its checks")
    return proof


def collect(path, process):
    if not path.exists():
        return {"available": False, "reason": "No native snapshot; unavailable is not zero"}
    try:
        counters = json.loads(path.read_bytes())
        if (counters["schema"] != "z3-check-live-probe-v1" or counters["pid"] != process["pid"]
                or counters["clock_failures"] != 0
                or counters["outer_checks"] != counters["sat"] + counters["unsat"] + counters["unknown"]
                or counters["outer_entered"] != counters["outer_checks"] + counters["in_flight_outer_checks"]):
            raise ValueError("native snapshot identity or accounting mismatch")
        return {"available": True, "counters": counters, "snapshot_sha256": sha256(path),
                "snapshot_age_seconds": (time.monotonic_ns() - counters["snapshot_monotonic_ns"]) / 1e9,
                "scope": "Completed intercepted native Z3 checks only; encoding, model extraction and context cleanup excluded. Interrupted/in-flight calls and activity after the last snapshot are excluded; cost is then a lower bound."}
    except (ValueError, KeyError, TypeError):
        return {"available": False, "reason": "Invalid or mismatched native snapshot; no zero inferred"}
