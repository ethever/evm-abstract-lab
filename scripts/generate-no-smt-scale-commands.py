#!/usr/bin/env python3
"""Bind the frozen offline corpus to an explicitly selected no-SMT CLI source.

Only writes a command profile and optional wire 15 requests; never builds or
runs analysis. Keep outputs separate from frozen baselines. Regenerate the
profile for the final build's exact source commit.
"""

import argparse
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import re
import subprocess


def generate(repository, source):
    baseline = repository / "examples/no-smt-scale/commands-e325-native.json"
    profile = deepcopy(json.loads(baseline.read_bytes()))
    witnesses = {
        "cli": "crates/evm-abstract-cli/src/main.rs",
        "relations": "crates/evm-abstract-cli/src/symbolic.rs",
        "native": "crates/evm-abstract/src/analysis.rs",
        "wire": "crates/evm-abstract-protocol/src/lib.rs",
        "cost": "crates/evm-abstract/src/domain/spec.rs",
    }
    texts = {key: (source / path).read_text() for key, path in witnesses.items()}
    for key, expected in [("native", 10), ("wire", 15)]:
        match = re.search(r"pub const SCHEMA_VERSION: u16 = (\d+);", texts[key])
        if match is None or int(match[1]) != expected:
            raise ValueError(f"expected {key} schema {expected}")
    if not re.search(r"cost_version:\s*4\s*,", texts["cost"]):
        raise ValueError("expected logical cost version 4")
    if re.search(r"smt_provider|smt_rlimit|embedded_smt", texts["relations"]):
        raise ValueError("selected CLI still exposes native solver policy")
    commit = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
    profile["interface"] = {
        "name": "no-smt-cli-world",
        "source_commit": commit,
        "versions": {"native": 10, "wire": 15, "cost": 4},
        "source_witnesses": {
            path: hashlib.sha256((source / path).read_bytes()).hexdigest()
            for path in witnesses.values()
        },
        "scope": "Offline CLI interface only; measured ELF/build identity is supplied separately. "
                 "Logical work is not comparable across cost versions.",
    }
    # The CLI prints an Incomplete Report before returning 2. The harness still
    # validates the JSON and keeps external stops and parse failures separate.
    profile["report_exit_codes"] = [0, 2]
    # Keep the frozen 1T/coverage-off policy despite consumer defaults changing.
    profile["expected_native_config"]["root_terminal_coverage"] = False
    for key in ("smt_provider", "smt_rlimit"):
        del profile["policy"][key]
        del profile["policy_mapping"][key]
    for key in ("provider", "rlimit"):
        del profile["expected_native_config"]["analysis"]["relations"][key]
    profile["policy_mapping"]["relations_enabled"] = (
        "true: omit --no-relations; bounded local consequence propagation only")
    for case in profile["cases"].values():
        command = case["argv"]
        for flag in ("--smt.provider", "--smt.rlimit"):
            position = command.index(flag)
            del command[position:position + 2]
        command.append("--no-root-terminal-coverage")
    return profile


def derive_wire15_request(original):
    """Keep every frozen wire 14 input and limit, explicitly disabling coverage."""
    request = deepcopy(original)
    request["limits"]["root_terminal_coverage"] = False
    return request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--interface-source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--request-output-dir", type=Path,
                        help="write separate wire 15 WETH/USDC requests with the frozen 1T/off policy")
    arguments = parser.parse_args()
    repository = Path(__file__).resolve().parents[1]
    profile = generate(repository, arguments.interface_source.resolve(strict=True))
    arguments.output.write_text(json.dumps(profile, indent=2) + "\n")
    if arguments.request_output_dir is not None:
        arguments.request_output_dir.mkdir(parents=True, exist_ok=True)
        for name in ("weth", "usdc"):
            original = repository / f"examples/no-smt-scale/wire14/{name}-original-request.json"
            request = derive_wire15_request(json.loads(original.read_bytes()))
            output = arguments.request_output_dir / f"{name}-wire15-request.json"
            output.write_text(json.dumps(request, indent=2) + "\n")


if __name__ == "__main__":
    main()
