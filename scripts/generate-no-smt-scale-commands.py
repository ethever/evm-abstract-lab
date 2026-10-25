#!/usr/bin/env python3
"""Bind the frozen offline corpus to an explicitly selected no-SMT CLI source.

Only writes a command profile and optional wire 21 requests; never builds or
runs analysis. Keep outputs separate from frozen baselines. Regenerate the
profile for the final build's exact source commit.

The current interface requires native 16 / wire 21 / domain 3 / cost 9. Historical
profiles stay bound to their frozen source and generator; they are not upgraded.
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
        "numeric": "crates/evm-abstract-cli/src/numeric.rs",
        "native": "crates/evm-abstract/src/analysis.rs",
        "wire": "crates/evm-abstract-protocol/src/lib.rs",
        "cost": "crates/evm-abstract/src/domain/spec.rs",
    }
    texts = {key: (source / path).read_text() for key, path in witnesses.items()}
    for key, expected in [("native", 16), ("wire", 21)]:
        match = re.search(r"pub const SCHEMA_VERSION: u16 = (\d+);", texts[key])
        if match is None or int(match[1]) != expected:
            raise ValueError(f"expected {key} schema {expected}")
    if not re.search(r"schema_version:\s*3\s*,", texts["cost"]):
        raise ValueError("expected numeric policy schema 3")
    if not re.search(r"cost_version:\s*9\s*,", texts["cost"]):
        raise ValueError("expected logical cost version 9")
    if re.search(r"smt_provider|smt_rlimit|embedded_smt", texts["relations"]):
        raise ValueError("selected CLI still exposes native solver policy")
    commit = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
    profile["interface"] = {
        "name": "no-smt-cli-world",
        "source_commit": commit,
        "versions": {"native": 16, "wire": 21, "domain": 3, "cost": 9},
        "source_witnesses": {
            path: hashlib.sha256((source / path).read_bytes()).hexdigest()
            for path in witnesses.values()
        },
        "scope": "Offline CLI interface only; measured ELF/build identity is supplied separately. "
                 "Logical work is not comparable across cost versions; an unchanged integer "
                 "work limit does not imply equal effort.",
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
    # Numeric policy is owned by the selected algebra, not ExecutionConfig.
    # Verify it independently so a budget-only config cannot hide weaker limits.
    policy = profile["policy"]
    assert policy.pop("domain_profile") == "Product"
    policy["numeric_model"] = "Product"
    policy["postprocessing_max_work"] = 2_000_000_000
    del profile["policy_mapping"]["domain_profile"]
    profile["policy_mapping"]["numeric_model"] = (
        "--numeric-model product; native numeric_model=product; wire numeric_model=Product")
    profile["policy_mapping"]["postprocessing_max_work"] = (
        "application DEFAULT_POSTPROCESS_WORK=2000000000; independent from --max-work")
    analysis = profile["expected_native_config"]["analysis"]
    profile["expected_native_numeric_model"] = "product"
    profile["expected_native_postprocessing_max_work"] = 2_000_000_000
    profile["expected_native_domain_spec"] = {
        "schema_version": 3, "word_bits": 256,
        "capacity": analysis.pop("max_constants"),
        "reduction_rounds": analysis.pop("reduction_rounds"),
        "fact_limit": analysis.pop("max_facts"),
        "cost_version": 9, "widening_after_updates": 2,
        "provenance_policy": "scoped-expressions-and-value-identities-v3",
        "relations": analysis.pop("relations"),
    }
    assert analysis.pop("domain_profile") == "product"
    profile["policy_mapping"]["relations_enabled"] = (
        "true: omit --no-relations; bounded local consequence propagation only")
    for case in profile["cases"].values():
        command = case["argv"]
        for flag in ("--smt.provider", "--smt.rlimit"):
            position = command.index(flag)
            del command[position:position + 2]
        command[command.index("--domain")] = "--numeric-model"
        command.append("--no-root-terminal-coverage")
    return profile


def derive_wire21_request(original):
    """Migrate one frozen wire 14 request, retaining its exact input and limits."""
    request = deepcopy(original)
    # This is an explicit historical-input derivation, never a live decoder.
    assert request["limits"].pop("domain_profile") == "Product"
    request["limits"]["numeric_model"] = "Product"
    request["limits"]["root_terminal_coverage"] = False
    return request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--interface-source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--request-output-dir", type=Path,
                        help="write separate wire 21 WETH/USDC requests with the frozen 1T/off policy")
    arguments = parser.parse_args()
    repository = Path(__file__).resolve().parents[1]
    profile = generate(repository, arguments.interface_source.resolve(strict=True))
    arguments.output.write_text(json.dumps(profile, indent=2) + "\n")
    if arguments.request_output_dir is not None:
        arguments.request_output_dir.mkdir(parents=True, exist_ok=True)
        for name in ("weth", "usdc"):
            original = repository / f"examples/no-smt-scale/wire14/{name}-original-request.json"
            request = derive_wire21_request(json.loads(original.read_bytes()))
            output = arguments.request_output_dir / f"{name}-wire21-request.json"
            output.write_text(json.dumps(request, indent=2) + "\n")


if __name__ == "__main__":
    main()
