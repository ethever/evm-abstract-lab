#!/usr/bin/env python3
"""Adapter/identity regressions with synthetic JSON only; never execute an ELF."""

import contextlib
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "examples" / "no-smt-scale"
SPEC = importlib.util.spec_from_file_location("scale", ROOT / "scripts" / "benchmark-no-smt-scale.py")
scale = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(scale)


def report():
    # This is a schema surrogate, not an analysis or coverage witness.
    return {"status": "Incomplete", "states": [], "transfers": 7, "work": 42,
            "world": {}, "entry": {}, "config": {"synthetic": True},
            "frontiers": [{"pc": 3, "reason": "Work", "payload": {"retained": True}}],
            "outcomes": [{"kind": "Failure", "data": {}, "store": {"rollback": True}}]}


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="no-smt-scale-adapter-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.process = {"exit_code": 0, "termination": "normal_exit", "peak_rss_kib": 123}
        self.commands = {"report_exit_codes": [0], "expected_native_config": report()["config"]}
        self.write_stdout(report())

    def write_stdout(self, value):
        (self.directory / "stdout.log").write_text(json.dumps(value))

    def collect(self, limit=1024 * 1024):
        return scale.collect_report(self.directory, self.process, self.commands, limit)

    def test_external_stops_cannot_promote_even_complete_stdout_to_final(self):
        for reason in ("external_timeout", "external_rss_guard", "external_host_memory_guard",
                       "external_keyboard_interrupt", "resource_monitor_error"):
            with self.subTest(reason=reason):
                self.process.update(termination=reason, exit_code=-15)
                sample = self.collect()
                self.assertFalse(sample["report_delivered"])
                self.assertIsNone(sample["nativefinal"])
                self.assertIsNone(sample["native_phase"])
                self.assertEqual(sample["exit_code"], -15)
                self.assertIn("stdout.log", sample["rawhash"])
        self.process.update(termination="external_timeout", exit_code=0)
        self.assertIsNone(self.collect()["nativefinal"])

    def test_progress_status_and_stderr_final_do_not_deliver_a_report(self):
        self.write_stdout({"status": "Incomplete", "work": 42, "phase": "executing"})
        (self.directory / "stderr.log").write_text(json.dumps(report()))
        sample = self.collect()
        self.assertFalse(sample["report_delivered"])
        self.assertIsNone(sample["nativefinal"])
        self.assertIsNotNone(sample["report_error"])

    def test_unaccepted_actual_exit_stays_no_report(self):
        self.process.update(termination="process_error", exit_code=2)
        sample = self.collect()
        self.assertEqual(sample["exit_code"], 2)
        self.assertFalse(sample["report_delivered"])
        self.assertIsNone(sample["nativefinal"])
        # A reviewed profile may explicitly allow a nonzero final-report exit.
        self.commands["report_exit_codes"] = [0, 2]
        sample = self.collect()
        self.assertTrue(sample["report_delivered"])
        self.assertEqual(sample["termination"], "process_error")
        self.assertEqual(sample["nativefinal"]["status"], "Incomplete")
        self.assertEqual(sample["nativefinal"]["frontiers"], report()["frontiers"])
        self.assertIsNone(sample["nativefinal"]["closure"]["all_closed"])
        # Accepting exit 2 must still enforce the parsing cap before reading JSON.
        sample = self.collect(limit=1)
        self.assertFalse(sample["report_delivered"])
        self.assertIsNone(sample["nativefinal"])
        self.assertEqual(sample["phase"], "output_unvalidated_parse_guard")
        self.assertIsNotNone(sample["report_error"])

    def test_incomplete_keeps_failure_frontier_and_unknown_closure(self):
        sample = self.collect()
        self.assertTrue(sample["report_delivered"])
        self.assertTrue(sample["policy_matches"])
        final = sample["nativefinal"]
        self.assertEqual(final["status"], "Incomplete")
        self.assertEqual(final["frontiers"], report()["frontiers"])
        self.assertEqual(final["outcome_kind_counts"], {"Failure": 1})
        self.assertIsNone(final["closure"]["all_closed"])
        self.assertIn("pending", final["semantic_acceptance"])

    def test_delivered_config_mismatch_is_exposed_without_rewriting_report(self):
        self.commands["expected_native_config"] = {"synthetic": False}
        sample = self.collect()
        self.assertTrue(sample["report_delivered"])
        self.assertFalse(sample["policy_matches"])
        self.assertEqual(sample["nativefinal"]["config"], {"synthetic": True})

    def test_parse_guard_retains_raw_bytes_and_does_not_infer_final(self):
        raw = (self.directory / "stdout.log").read_bytes()
        sample = self.collect(limit=len(raw) - 1)
        self.assertFalse(sample["report_delivered"])
        self.assertIsNone(sample["nativefinal"])
        self.assertEqual(sample["phase"], "output_unvalidated_parse_guard")
        self.assertEqual((self.directory / "stdout.log").read_bytes(), raw)
        self.assertEqual(sample["rawhash"]["stdout.log"]["sha256"], hashlib.sha256(raw).hexdigest())

    def test_partial_or_malformed_json_stays_no_report(self):
        for value in ('{"status":"Converged",', '{"status":"Converged","status":"Incomplete"}'):
            with self.subTest(value=value):
                (self.directory / "stdout.log").write_text(value)
                self.assertFalse(self.collect()["report_delivered"])
        bad = report()
        bad["frontiers"] = ["Work"]
        self.write_stdout(bad)
        self.assertFalse(self.collect()["report_delivered"])


class InputTests(unittest.TestCase):
    def test_committed_plan_preserves_unknown_and_separates_acquisition(self):
        manifest, cases, _ = scale.load_inputs(FIXTURES / "manifest.json")
        scale.load_commands(FIXTURES / "commands-e325-native.json", cases)
        self.assertEqual(len(cases), 7)
        for case in cases.values():
            self.assertEqual(case["spec"]["environment"]["calldata"], "Unknown")
            self.assertEqual(case["spec"]["environment"]["caller"], "Unknown")
            self.assertEqual(case["spec"]["environment"]["call_value"], "Unknown")
            self.assertTrue(all(fact["storage_unknown"] for fact in case["spec"]["initial_facts"]))
        self.assertEqual({profile["profile"] for profile in manifest["acquisition_profiles"].values()},
                         {"original-rpc-acquisition"})
        facts = cases["unknown-input"]["spec"]["initial_facts"][0]
        self.assertIsNone(facts["balance"])
        self.assertIsNone(facts["nonce"])
        self.assertEqual(facts["existence"], "unknown")
        self.assertEqual(facts["storage"], {})

    def test_wire14_derivation_removes_only_reviewed_smt_fields(self):
        manifest = scale.read_json(FIXTURES / "manifest.json")
        for spec in manifest["request_derivations"].values():
            original = scale.read_json(scale.checked_file(FIXTURES, spec["original"]))
            derived = scale.read_json(scale.checked_file(FIXTURES, spec["derived"]))
            expected = scale.derive_wire14_request(original)
            self.assertEqual(scale.digest(derived), scale.digest(expected))
            self.assertEqual(original["input"], derived["input"])
            self.assertEqual(original["environment"], derived["environment"])
            self.assertEqual(derived["environment"]["calldata"], "Unknown")
            self.assertEqual(derived["input"]["Rpc"]["provider_id"], "default")
            self.assertEqual(set(original["limits"]) - set(derived["limits"]),
                             {"smt_provider", "smt_rlimit"})
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            original = {"limits": {"smt_provider": "Z3", "smt_rlimit": 10},
                        "environment": {"calldata": "Unknown"}}
            changed = scale.derive_wire14_request(original)
            changed["environment"]["calldata"] = {"Known": "0x"}
            (directory / "old.json").write_text(json.dumps(original))
            (directory / "new.json").write_text(json.dumps(changed))
            record = copy.deepcopy(next(iter(manifest["request_derivations"].values())))
            record["original"] = {"path": "old.json", "sha256": scale.sha256(directory / "old.json")}
            record["derived"] = {"path": "new.json", "sha256": scale.sha256(directory / "new.json")}
            with self.assertRaisesRegex(ValueError, "changed more"):
                scale.validate_request_derivations({"request_derivations": {"bad": record}}, directory, {})

    def test_unknown_and_empty_calldata_have_different_input_identities(self):
        _, cases, _ = scale.load_inputs(FIXTURES / "manifest.json", ["unknown-input"])
        changed = copy.deepcopy(cases["unknown-input"]["identity"])
        changed["environment"]["calldata"] = {"Known": "0x"}
        self.assertNotEqual(scale.digest(changed), cases["unknown-input"]["input_identity_sha256"])
        cases["unknown-input"]["input_identity_sha256"] = scale.digest(changed)
        with self.assertRaisesRegex(ValueError, "different input boundary"):
            scale.load_commands(FIXTURES / "commands-e325-native.json", cases)

    def test_code_only_cannot_smuggle_known_zero_or_account_absence(self):
        manifest = scale.read_json(FIXTURES / "manifest.json")
        original = scale.read_json(FIXTURES / "code-only" / "unknown-input.json")
        with tempfile.TemporaryDirectory(prefix="no-smt-scale-input-") as directory:
            directory = Path(directory)
            world_path, manifest_path = directory / "world.json", directory / "manifest.json"
            for field, value in (("balance", "0x0"), ("nonce", "0x0"), ("existence", "absent"),
                                 ("storage", {"0x1": "0x0"})):
                with self.subTest(field=field):
                    world = copy.deepcopy(original)
                    world["accounts"][0][field] = value
                    scale.write_json(world_path, world)
                    case = manifest["cases"]["unknown-input"]
                    case["world"] = {"path": str(world_path), "sha256": scale.sha256(world_path)}
                    case["initial_facts"] = scale.initial_facts(world)
                    scale.write_json(manifest_path, manifest)
                    with self.assertRaisesRegex(ValueError, "state observations"):
                        scale.load_inputs(manifest_path, ["unknown-input"])

    def test_changed_file_cannot_keep_the_old_world_hash(self):
        with tempfile.TemporaryDirectory(prefix="no-smt-scale-hash-") as directory:
            directory = Path(directory)
            path = directory / "input.json"
            path.write_text('{"storage":{}}')
            record = {"path": path.name, "sha256": scale.sha256(path)}
            path.write_text('{"storage":{"0x0":"0x0"}}')
            with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                scale.checked_file(directory, record)

    def test_default_mode_never_calls_run_or_starts_an_engine(self):
        arguments = ["benchmark-no-smt-scale.py", "--manifest", str(FIXTURES / "manifest.json"),
                     "--commands", str(FIXTURES / "commands-e325-native.json")]
        with patch.object(scale, "run", side_effect=AssertionError("unexpected run")), \
                patch.object(scale, "ProtectedProcess", side_effect=AssertionError("unexpected ELF")), \
                patch("sys.argv", arguments), contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(scale.main(), 0)
        self.assertEqual(json.loads(output.getvalue())["phase"], "plan_checked_no_process_started")


if __name__ == "__main__":
    unittest.main()
