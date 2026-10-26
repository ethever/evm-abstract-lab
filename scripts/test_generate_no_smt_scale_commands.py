#!/usr/bin/env python3
"""Command-profile contracts over synthetic source; never build or execute EVM."""

from copy import deepcopy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "generate_scale_commands", ROOT / "scripts/generate-no-smt-scale-commands.py")
generator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(generator)

WITNESSES = {
    "crates/evm-abstract-cli/src/main.rs": "// Synthetic selected CLI source.\n",
    "crates/evm-abstract-cli/src/symbolic.rs": "// Bounded local facts only.\n",
    "crates/evm-abstract-cli/src/numeric.rs": "// Application-owned numeric model and limits.\n",
    "crates/evm-abstract/src/analysis.rs": "pub const SCHEMA_VERSION: u16 = 16;\n",
    "crates/evm-abstract-protocol/src/lib.rs": "pub const SCHEMA_VERSION: u16 = 21;\n",
    "crates/evm-abstract/src/domain/spec.rs": "schema_version: 3, cost_version: 9,\n",
}
COMMIT = "0123456789abcdef0123456789abcdef01234567"


class GeneratorTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="scale-command-contract-")
        self.addCleanup(self.temporary.cleanup)
        self.source = Path(self.temporary.name)
        for name, text in WITNESSES.items():
            path = self.source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.baseline_path = ROOT / "examples/no-smt-scale/commands-e325-native.json"
        self.baseline_bytes = self.baseline_path.read_bytes()
        self.baseline = json.loads(self.baseline_bytes)

    def generate(self):
        with patch.object(generator.subprocess, "check_output", return_value=COMMIT + "\n") as git:
            profile = generator.generate(ROOT, self.source)
        git.assert_called_once_with(["git", "rev-parse", "HEAD"], cwd=self.source, text=True)
        return profile

    def test_derivation_preserves_frozen_policy_and_binds_selected_source(self):
        profile = self.generate()
        for field in ["schema", "boundary", "report_adapter"]:
            self.assertEqual(profile[field], self.baseline[field])
        self.assertEqual(profile["report_exit_codes"], [0, 2])
        self.assertEqual(self.baseline["report_exit_codes"], [0])
        self.assertEqual(profile["policy"], {key: value for key, value in
                         self.baseline["policy"].items() if key not in {"smt_provider", "smt_rlimit", "domain_profile"}} |
                         {"numeric_model": "Product", "postprocessing_max_work": 2_000_000_000})
        mapping = profile["policy_mapping"]
        self.assertEqual(set(mapping), set(self.baseline["policy_mapping"]) -
                         {"smt_provider", "smt_rlimit", "domain_profile"} |
                         {"numeric_model", "postprocessing_max_work"})
        for key in mapping:
            if key not in {"relations_enabled", "numeric_model", "postprocessing_max_work"}:
                self.assertEqual(mapping[key], self.baseline["policy_mapping"][key])
        expected_config = deepcopy(self.baseline["expected_native_config"])
        expected_config["root_terminal_coverage"] = False
        expected_config["analysis"]["relations"] = {
            key: value for key, value in expected_config["analysis"]["relations"].items()
            if key not in {"provider", "rlimit"}}
        analysis = expected_config["analysis"]
        expected_spec = {
            "schema_version": 3, "word_bits": 256,
            "capacity": analysis.pop("max_constants"),
            "reduction_rounds": analysis.pop("reduction_rounds"),
            "fact_limit": analysis.pop("max_facts"),
            "cost_version": 9, "widening_after_updates": 2,
            "provenance_policy": "scoped-expressions-and-value-identities-v3",
            "relations": analysis.pop("relations"),
        }
        self.assertEqual(analysis.pop("domain_profile"), "product")
        self.assertEqual(profile["expected_native_config"], expected_config)
        self.assertEqual(profile["expected_native_domain_spec"], expected_spec)
        self.assertEqual(profile["expected_native_numeric_model"], "product")
        self.assertEqual(profile["expected_native_postprocessing_max_work"], 2_000_000_000)
        self.assertEqual(set(profile["cases"]), set(self.baseline["cases"]))
        self.assertEqual(len(profile["cases"]), 7)
        for name, case in profile["cases"].items():
            old = deepcopy(self.baseline["cases"][name])
            argv = old.pop("argv")
            new = deepcopy(case)
            actual_argv = new.pop("argv")
            self.assertEqual(new, old)
            # Preserve input/budgets; migrate the selector and retain coverage-off.
            argv[argv.index("--domain")] = "--numeric-model"
            omitted = set()
            for flag in ["--smt.provider", "--smt.rlimit"]:
                position = argv.index(flag)
                omitted.update([position, position + 1])
            self.assertEqual(actual_argv, [arg for index, arg in enumerate(argv)
                                         if index not in omitted] + ["--no-root-terminal-coverage"])
            self.assertEqual(actual_argv.count("--no-root-terminal-coverage"), 1)
            self.assertNotIn("--root-terminal-coverage", actual_argv)
            self.assertEqual(actual_argv[actual_argv.index("--max-work") + 1], "1000000000000")
        interface = profile["interface"]
        self.assertEqual(interface["source_commit"], COMMIT)
        self.assertEqual(interface["versions"], {"native": 16, "wire": 21, "domain": 3, "cost": 9})
        self.assertEqual(interface["source_witnesses"], {
            name: hashlib.sha256((self.source / name).read_bytes()).hexdigest()
            for name in WITNESSES})
        self.assertEqual(self.baseline_path.read_bytes(), self.baseline_bytes)

    def test_actual_selected_source_matches_the_current_interface(self):
        # Nix projects these actual witnesses. Its immutable source has no .git,
        # so only the commit identity is supplied; source bytes remain real.
        with patch.object(generator.subprocess, "check_output", return_value=COMMIT + "\n"):
            profile = generator.generate(ROOT, ROOT)
        self.assertEqual(profile["interface"]["versions"],
                         {"native": 16, "wire": 21, "domain": 3, "cost": 9})
        self.assertEqual(profile["interface"]["source_witnesses"], {
            name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
            for name in WITNESSES})

    def test_obsolete_or_wrong_source_contract_is_rejected_before_git(self):
        bad_sources = [
            ("crates/evm-abstract/src/analysis.rs", "pub const SCHEMA_VERSION: u16 = 15;", "native schema 16"),
            ("crates/evm-abstract-protocol/src/lib.rs", "pub const SCHEMA_VERSION: u16 = 20;", "wire schema 21"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3, cost_version: 8,", "cost version 9"),
            ("crates/evm-abstract/src/analysis.rs", "pub const SCHEMA_VERSION: u16 = 13;", "native schema 16"),
            ("crates/evm-abstract-protocol/src/lib.rs", "pub const SCHEMA_VERSION: u16 = 18;", "wire schema 21"),
            ("crates/evm-abstract/src/analysis.rs", "pub const SCHEMA_VERSION: u16 = 10;", "native schema 16"),
            ("crates/evm-abstract-protocol/src/lib.rs", "pub const SCHEMA_VERSION: u16 = 15;", "wire schema 21"),
            ("crates/evm-abstract/src/analysis.rs", "pub const SCHEMA_VERSION: u16 = 11;", "native schema 16"),
            ("crates/evm-abstract/src/analysis.rs", "pub const SCHEMA_VERSION: u16 = 12;", "native schema 16"),
            ("crates/evm-abstract-protocol/src/lib.rs", "pub const SCHEMA_VERSION: u16 = 17;", "wire schema 21"),
            ("crates/evm-abstract-protocol/src/lib.rs", "pub const SCHEMA_VERSION: u16 = 16;", "wire schema 21"),
            ("crates/evm-abstract/src/analysis.rs", "pub const SCHEMA_VERSION: u16 = 9;", "native"),
            ("crates/evm-abstract-protocol/src/lib.rs", "pub const SCHEMA_VERSION: u16 = 14;", "wire"),
            ("crates/evm-abstract/src/analysis.rs", "pub const SCHEMA_VERSION: u16 = 8;", "native"),
            ("crates/evm-abstract-protocol/src/lib.rs", "pub const SCHEMA_VERSION: u16 = 13;", "wire"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3, cost_version: 2,", "cost"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3, cost_version: 3,", "cost"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3, cost_version: 4,", "cost"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3, cost_version: 5,", "cost version 9"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3, cost_version: 6,", "cost version 9"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3, cost_version: 7,", "cost version 9"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3, cost_version: 50,", "cost"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 3,", "cost"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 2, cost_version: 8,", "numeric policy schema 3"),
            ("crates/evm-abstract/src/domain/spec.rs", "schema_version: 4, cost_version: 8,", "numeric policy schema 3"),
        ] + [("crates/evm-abstract-cli/src/symbolic.rs", field, "solver policy")
             for field in ["smt_provider", "smt_rlimit", "embedded_smt"]]
        for name, text, error in bad_sources:
            with self.subTest(name=name, text=text):
                path = self.source / name
                path.write_text(text)
                try:
                    with patch.object(generator.subprocess, "check_output") as git:
                        with self.assertRaisesRegex(ValueError, error):
                            generator.generate(ROOT, self.source)
                    git.assert_not_called()
                finally:
                    path.write_text(WITNESSES[name])

    def test_changed_selected_source_changes_its_witness(self):
        before = self.generate()
        name = "crates/evm-abstract-cli/src/main.rs"
        (self.source / name).write_text("// A distinct synthetic selected CLI source.\n")
        after = self.generate()
        self.assertNotEqual(before["interface"]["source_witnesses"][name],
                            after["interface"]["source_witnesses"][name])
        self.assertEqual(before["cases"], after["cases"])
        self.assertEqual(before["policy"], after["policy"])

    def test_cli_writes_profile_only_to_explicit_output(self):
        output = self.source / "selected-command-profile.json"
        with patch("sys.argv", ["generate-no-smt-scale-commands.py", "--interface-source",
                               str(self.source), "--output", str(output)]), \
                patch.object(generator.subprocess, "check_output", return_value=COMMIT + "\n"):
            generator.main()
        actual = json.loads(output.read_bytes())
        self.assertEqual(actual["interface"]["source_commit"], COMMIT)
        self.assertEqual(set(actual["cases"]), set(self.baseline["cases"]))
        self.assertEqual(self.baseline_path.read_bytes(), self.baseline_bytes)

    def test_generated_wire21_requests_preserve_frozen_inputs_and_1t_off_policy(self):
        originals = {}
        for name in ("weth", "usdc"):
            path = ROOT / f"examples/no-smt-scale/wire14/{name}-original-request.json"
            originals[name] = (path, path.read_bytes())
        output = self.source / "selected-command-profile.json"
        requests = self.source / "wire21"
        with patch("sys.argv", ["generate-no-smt-scale-commands.py", "--interface-source",
                               str(self.source), "--output", str(output),
                               "--request-output-dir", str(requests)]), \
                patch.object(generator.subprocess, "check_output", return_value=COMMIT + "\n"):
            generator.main()
        for name, (path, frozen) in originals.items():
            original = json.loads(frozen)
            actual = json.loads((requests / f"{name}-wire21-request.json").read_bytes())
            self.assertIs(actual["limits"].pop("root_terminal_coverage"), False)
            self.assertEqual(actual["limits"].pop("numeric_model"), "Product")
            self.assertEqual(original["limits"].pop("domain_profile"), "Product")
            self.assertEqual(actual, original)
            self.assertEqual(actual["limits"]["max_work"], 1_000_000_000_000)
            self.assertEqual(path.read_bytes(), frozen)
        self.assertEqual(self.baseline_path.read_bytes(), self.baseline_bytes)


if __name__ == "__main__":
    unittest.main()
