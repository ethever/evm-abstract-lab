#!/usr/bin/env python3
"""Small current-format consumer controls; fake HTTP never starts an engine/RPC."""

from copy import deepcopy
import gzip
import importlib.util
import json
from pathlib import Path
import re
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.parse import urlparse

from benchmark_known_recursion import report, workbench


ROOT = Path(__file__).resolve().parents[1]
JOB = 18_446_744_073_709_551_614
VERSION = "9007199254740993"
ENDPOINT = "https://private.example/secret-rpc-token"
ADDRESS = "0x" + "11" * 20
HASH = "0x" + "22" * 32
SPEC = importlib.util.spec_from_file_location("benchmark", ROOT / "scripts/benchmark-known-recursion.py")
benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark)


def word():
    return {"constants": None, "known_zero": "0x0", "known_one": "0x0", "nonzero": False,
            "unsigned": {"lower": "0x0", "upper": hex(2**256 - 1)}, "congruence": "Any"}


def request():
    return {"input": {"Rpc": {"provider_id": "private", "address": ADDRESS,
                              "block": {"Hash": HASH}, "accounts": []}}, "fork": "Osaka",
            "environment": {"caller": "Unknown", "origin": None, "call_value": "Unknown",
                            "calldata": "Unknown", "gas_price": "Unknown", "blob_count": "Unknown",
                            "is_static": False, "gas_upper_bound": None, "block_hashes": [], "blob_hashes": [],
                            **dict.fromkeys(("coinbase", "timestamp", "number", "prevrandao", "gas_limit",
                                             "chain_id", "base_fee", "blob_base_fee"))},
            "limits": {"max_work": 2_000_000_000, "context_depth": 4, "max_states": 100,
                       "max_transfers": 1000, "max_call_depth": 1025, "max_memory_bytes": 1000,
                       "use_summaries": True, "root_terminal_coverage": True, "numeric_model": "Product",
                       "max_constants": 512, "reduction_rounds": 16, "max_facts": 4096,
                       "relations_enabled": True, "max_constraints": 2048, "max_expression_nodes": 16384,
                       "max_expression_depth": 256, "rpc_max_accounts": 10, "rpc_max_requests": 20,
                       "rpc_max_response_bytes": 1000, "rpc_timeout_ms": 2000}}


def minimum_result():
    # A parser surrogate with sparse source IDs, not a semantic closure witness.
    policy = request()["limits"]
    config = {key: policy[key] for key in ("context_depth", "max_states", "max_transfers", "max_work",
                                         "max_call_depth", "max_memory_bytes", "use_summaries")}
    config["root_terminal_coverage"] = False
    env = {"to": {"Concrete": ADDRESS}, "caller": {"Symbolic": {"kind": "Caller", "index": None}},
           "origin": {"Symbolic": {"kind": "Caller", "index": None}}, "input_scope": "9007199254740997",
           "call_value": word(), "calldata": {"length": word(), "memory": False,
                                               "content": {"Cells": []}, "words": []},
           "is_static": False, "gas_upper_bound": None, "gas_price": word(), "blob_count": word(),
           "block_hashes": [], "blob_hashes": []}
    snapshot = {"Chain": {"chain_id": "0x1", "block_hash": HASH}}
    domain = {"capacity": 512, "reduction_rounds": 16, "fact_limit": 4096,
              "cost_version": 9, "relations": {"enabled": True, "max_constraints": 2048,
                                              "max_nodes": 16384, "max_depth": 256}}
    relation = {"closed": True, "source_current": True, "root_procedure": 0,
                "regional_cfg": {"nodes": [{"id": 0, "transfer": None}]},
                "procedures": [{"process": {"epoch": HASH}, "semantics": "KnownProcess"}]}
    minimum = {"namespace": "Baseline", "scope": "RpcWorld", "fork": "Osaka",
               "source_snapshot": snapshot, "revision": "9007199254740998",
               "root_binding": {"Physical": {"address": ADDRESS}},
               "declared_root_source": {"Runtime": {"source": ADDRESS, "hash": HASH, "fork": "Osaka"}},
               "entry_address": ADDRESS, "fingerprint": HASH, "status": "Converged", "current": True,
               "config": config, "numeric_model": "Product", "domain_spec": domain, "environment": env,
               "accounts": [{"address": ADDRESS, "root_template": False, "code_kind": "Runtime", "program": 91}],
               "summaries": [{"version": {"namespace": 7, "generation": 3}, "relation": relation,
                              "bounds": [{"version": {"namespace": 7, "entry_revision": 1,
                                                      "output_revision": "9007199254740997", "source_epoch": HASH},
                                          "input": {}, "outputs": [{"kind": "Success", "data": {}, "store": {}}]}]}],
               "postprocessing_max_work": 2_000_000_000, "export_limits": {"max_nodes": 100},
               "export_usage": {"nodes": 2, "work": 10}}
    payload = {"schema_version": 21, "selected_view": "Baseline", "minimum": minimum,
               "rich_export": "NotRequested", "status": "Converged", "work": 42, "transfers": 7,
               "postprocessing_work": 10, "timings": {"projecting_ns": 12}, "acquisition": None,
               "process_controls": [],
               "programs": [{"id": 91, "fork": "Osaka", "code_address": ADDRESS, "code_hash": HASH,
                             "kind": "Runtime", "bytecode": "0x00", "blocks": [
                                 {"id": 12, "start_pc": 0, "instructions": [{"pc": 0}]}]}],
               "global": None, "native": None,
               "hybrid": {"coverage": {"snapshot": snapshot, "baseline_revision": minimum["revision"],
                                        "baseline_current": True, "baseline_config": config,
                                        "detail_revision": None, "detail": "Pending"}, "precision": None}}
    publication = {"version": VERSION, "minimum_version": VERSION, "global_version": None,
                   "native_version": None, "model_closed": True}
    return {"id": JOB, "publication": publication, "report": payload}


def rich_result():
    result = minimum_result()
    payload = result["report"]
    minimum = payload["minimum"]
    graph = {"scope": "RpcWorld", "fork": "Osaka", "status": "Converged", "transfers": 7,
             "states": [{"state": 4}], "cfg": [{"id": 4, "program": 91, "basic_block": 12, "start_pc": 0}],
             "abstract_summaries": [], "frontiers": [], "outcomes": [], "diagnostics": [],
             "ssa": {"complete": False}, "symbolic": {"nodes": []}, "stores": [],
             "byte_arrays": [{"length": word(), "cells": []}],
             "metadata": {**report._qualification(minimum), "root_program": 91,
                          "snapshot": minimum["source_snapshot"]["Chain"], "limits": request()["limits"]}}
    graph["metadata"]["environment"] = {**minimum["environment"], "calldata": 0}
    payload["global"] = graph
    payload["native"] = deepcopy(graph)
    payload["native"]["status"] = "Incomplete"
    payload["native"]["metadata"]["revision"] = "9007199254740999"
    payload["native"]["states"][0]["state"] = 4  # Same ID, independent namespace.
    payload["hybrid"]["coverage"].update(detail="Incomplete", detail_revision="9007199254740999")
    payload["rich_export"] = "Complete"
    result["publication"].update(version="9007199254740997", global_version="9007199254740997",
                                 native_version="9007199254740997")
    return result


def ok(value):
    return {"result": {"Ok": value}}


def native_result():
    graph = {"schema_version": 16, "status": "Converged", "work": 100, "transfers": 7,
             "states": [], "frontiers": [], "outcomes": [], "procedure_summaries": [],
             "world": {}, "entry": {}, "config": {"max_work": 2_000_000_000},
             "domain_spec": {"schema_version": 3, "cost_version": 9}}
    return {"schema_version": 16, "selected_view": "Baseline", "status": "Converged",
            "work": 800, "transfers": 18, "global": {"analysis": graph},
            "native": {"analysis": {**graph, "status": "Incomplete", "work": 300},
                       "partial_ssa": {"complete": False}},
            "hybrid": {"coverage": {"baseline_current": True}}, "numeric_model": "product",
            "postprocessing_max_work": 2_000_000_000, "postprocessing_work": 10}


def snapshot(state, publication=None):
    return {"id": JOB, "state": state, "publication": publication,
            "progress": {"phase": "Analyzing", "work": 42}, "timings": {"processing_ns": 123}}


class FakeResponse:
    def __init__(self, value, status=200, encoding="identity"):
        self.status = status
        self.headers = {"Content-Encoding": encoding}
        self.raw = json.dumps(value).encode()
        if encoding == "gzip":
            self.raw = gzip.compress(self.raw)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        pass

    def read(self, limit):
        return self.raw[:limit]


class FakeOpener:
    def __init__(self, steps):
        self.steps, self.requests = list(steps), []

    def open(self, request, timeout):
        self.requests.append(request)
        method, path, value, encoding, status = self.steps.pop(0)
        assert (request.method, urlparse(request.full_url).path) == (method, path)
        assert request.get_header("Accept-encoding") == encoding
        return FakeResponse(value, status, encoding)


class ParserTests(unittest.TestCase):
    def test_current_versions_match_source(self):
        for name, version in (("crates/evm-abstract-protocol/src/lib.rs", report.WIRE_VERSION),
                              ("crates/evm-abstract/src/analysis.rs", report.NATIVE_VERSION)):
            self.assertEqual(int(re.search(r"pub const SCHEMA_VERSION: u16 = (\d+);", (ROOT / name).read_text())[1]), version)

    def parse(self, value):
        return report.wire_report(ok(value), job_id=JOB)

    def test_minimum_and_final_keep_qualification_and_namespace(self):
        early, final = minimum_result(), rich_result()
        self.assertEqual(self.parse(early)["report"]["minimum"], self.parse(final)["report"]["minimum"])
        metrics = report.metrics(early["report"], "rpc")
        self.assertIsNone(metrics["graphs"]["Baseline"])
        self.assertIsNone(metrics["graphs"]["Native"])
        self.assertNotIn("states", metrics)
        self.assertEqual(metrics["minimum"]["revision"], "9007199254740998")
        self.assertTrue(report.request_qualification(final["report"], request())["rich_request_policy_checked"])
        self.assertFalse(report.request_qualification(early["report"], request())["rich_request_policy_checked"])
        # The result can advance while a Running status-triggered GET is in flight.
        self.assertIs(report.wire_report(ok(final), job_id=JOB, advertised=early["publication"]), final)
        with self.assertRaises(AssertionError):
            report.wire_report(ok(early), job_id=JOB, advertised=final["publication"])

    def test_exact_wrapper_ids_publication_and_sources_reject_corruption(self):
        malformed = [ok(minimum_result()["report"]), {"result": {"Ok": minimum_result(), "Err": {}}},
                     {"result": {"Ok": minimum_result()}, "other": None}, {"Ok": minimum_result()}]
        for envelope in malformed:
            with self.subTest(envelope=envelope), self.assertRaises((AssertionError, KeyError)):
                report.wire_report(envelope, job_id=JOB)
        mutations = [("id", "42"), ("id", True), ("id", 2**64), ("id", JOB-1)]
        for key, value in mutations:
            invalid = minimum_result()
            invalid[key] = value
            with self.subTest(value=value), self.assertRaises(AssertionError):
                self.parse(invalid)
        for key, value in (("minimum_version", None), ("version", 1), ("version", "01"),
                           ("version", "0"), ("model_closed", False), ("native_version", "1")):
            invalid = minimum_result()
            invalid["publication"][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                self.parse(invalid)
        for mutate in (
            lambda r: r["publication"].pop("minimum_version"),
            lambda r: r["report"].update(schema_version=20),
            lambda r: r["report"]["minimum"]["accounts"][0].update(program=0),
            lambda r: r["report"]["programs"].append(deepcopy(r["report"]["programs"][0])),
            lambda r: r["report"]["minimum"].update(revision="0"),
            lambda r: r["report"]["minimum"]["summaries"][0].update(bounds=[]),
            lambda r: r["report"]["minimum"]["summaries"][0]["bounds"][0]["version"].update(source_epoch="wrong"),
            lambda r: r["report"]["programs"][0].update(fork="London"),
            lambda r: r["report"].pop("process_controls"),
            lambda r: r["report"].update(process_controls=None),
        ):
            invalid = minimum_result()
            mutate(invalid)
            with self.assertRaises((AssertionError, KeyError)):
                self.parse(invalid)
        for key, value in (("program", 7), ("basic_block", 3), ("start_pc", 1)):
            invalid = rich_result()
            invalid["report"]["native"]["cfg"][0][key] = value
            with self.assertRaises(AssertionError):
                self.parse(invalid)

    def test_no_duplicate_or_nonfinite_json_or_anonymous_legacy_format(self):
        for raw in ('{"result":{},"result":{}}', '{"work": NaN}'):
            with self.assertRaises(ValueError):
                report.strict_json(raw)
        for value in (minimum_result()["report"], {"status": "Converged", "states": []}):
            with self.assertRaises((AssertionError, KeyError)):
                report.metrics(value, "offline")
        with self.assertRaises(ValueError):
            report.metrics({}, "web")
        with self.assertRaises(AssertionError):
            report.metrics({**minimum_result()["report"], "schema_version": 20}, "rpc")

    def test_same_request_policy_and_unknown_inputs_are_checked(self):
        for mutate in (
            lambda r: r["minimum"]["config"].update(max_work=3_000_000_000),
            lambda r: r["minimum"]["domain_spec"].update(capacity=1),
            lambda r: r["minimum"]["source_snapshot"]["Chain"].update(block_hash="0x00"),
            lambda r: r["minimum"]["environment"].update(caller={"Concrete": ADDRESS}),
            lambda r: r["minimum"]["environment"]["call_value"].update(constants=["0x0"]),
            lambda r: r["minimum"]["environment"]["calldata"]["length"].update(constants=["0x0"]),
        ):
            invalid = minimum_result()["report"]
            mutate(invalid)
            with self.assertRaises(AssertionError):
                report.request_qualification(invalid, request())
        invalid = rich_result()["report"]
        invalid["native"]["metadata"]["limits"] = {**request()["limits"], "rpc_max_requests": 1}
        with self.assertRaises(AssertionError):
            report.request_qualification(invalid, request())

    def test_current_native_format_has_fixed_slots_and_independent_totals(self):
        native = native_result()
        graph = native["global"]["analysis"]
        before = deepcopy(native)
        result = report.metrics(native, "offline")
        self.assertEqual(result["work"], 800)
        self.assertEqual(result["graphs"]["Baseline"]["work"], 100)
        self.assertEqual(result["graphs"]["Native"]["work"], 300)
        self.assertEqual(native, before)
        native.update(selected_view="Native")
        self.assertEqual(report.metrics(native, "offline")["status"], "Converged")
        for invalid in (graph, {**native, "schema_version": 15}, {**native, "schema_version": 21},
                        {**native, "native": None}, {**native, "global": graph}):
            with self.assertRaises((AssertionError, KeyError)):
                report.metrics(invalid, "offline")

    def test_native_cli_exit_two_delivers_incomplete_but_external_stop_never_does(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            native = native_result()
            native.update(status="Incomplete")
            native["hybrid"]["coverage"]["baseline_current"] = False
            (directory/"stdout.log").write_text(json.dumps(native))
            (directory/"stderr.log").write_text("")
            for termination, delivered in (("process_error", True), ("external_timeout", False)):
                process = {"termination": termination, "exit_code": 2}
                owner = type("FakeNative", (), {"finish": lambda _: process})()
                with patch.object(benchmark, "ProtectedProcess", return_value=owner):
                    record = benchmark.offline(Path("owned-cli"), directory, ROOT, {"cpus": [1]},
                                               {"offline_arguments": ["--world", "capture.json", "--format", "json"]}, {})
                self.assertEqual(record["report_delivered"], delivered)
                self.assertEqual(record["process"]["exit_code"], 2)
                if delivered:
                    self.assertEqual(record["metrics"]["status"], "Incomplete")


class HttpTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        # Whitespace is deliberate: POST must replay these exact bytes.
        self.request_bytes = (" \n" + json.dumps(request(), indent=3) + "\n").encode()

    def run_case(self, final_state="Completed", final=None, result_override=None, *, first=None):
        first = first or minimum_result()
        final = final or rich_result()
        path = f"/api/tasks/{JOB}"
        end = snapshot(final_state, final["publication"])
        steps = [("POST", "/api/tasks", ok(snapshot("Queued")), "identity", 202),
                 ("GET", path, ok(snapshot("Running", first["publication"])), "identity", 200),
                 ("GET", path+"/result", ok(first), "identity", 200),
                 ("GET", path, ok(end), "identity", 200),
                 ("GET", path+"/result", result_override or ok(final), "identity", 200)]
        if result_override is None:
            steps.extend([("GET", path+"/result", ok(final), "identity", 200),
                          ("GET", path+"/result", ok(final), "gzip", 200)])
        opener = FakeOpener(steps)
        owner = type("FakeOwnedProcess", (), {"done": threading.Event(), "stop_reason": None})()
        owner.record = {"termination": "capture_complete", "wall_seconds": 1, "user_seconds": 0,
                        "system_seconds": 0, "peak_rss_kib": 123, "exit_code": 0}

        def start(command, directory, root, guard, environment):
            owner.command = command
            (directory / "stdout.log").write_text("EVM workbench: http://127.0.0.1:43210\n")
            (directory / "stderr.log").write_text(ENDPOINT)
            return owner

        def finish(reason):
            owner.finish_reason = reason
            return owner.record

        owner.finish = finish
        with patch.object(workbench, "ProtectedProcess", side_effect=start), \
                patch.object(workbench.urllib.request, "build_opener", return_value=opener), \
                patch.object(workbench.time, "sleep"):
            record = workbench.run(Path("owned-server"), self.directory, ROOT, {"cpus": [1]},
                                   self.request_bytes, ENDPOINT, Path("assets"))
        self.assertFalse(opener.steps, record)
        self.assertEqual(opener.requests[0].data, self.request_bytes)
        self.assertEqual((self.directory/"request.json").read_bytes(), self.request_bytes)
        self.assertEqual(owner.command[owner.command.index("--bind")+1], "127.0.0.1:0")
        self.assertEqual(owner.finish_reason, "capture_complete")
        self.assertNotIn(ENDPOINT, json.dumps(record))
        self.assertNotIn(ENDPOINT, (self.directory/"stderr.log").read_text())
        return record

    def test_running_minimum_then_final_same_minimum_with_detail_and_cached_gets(self):
        record = self.run_case()
        self.assertEqual(record["task_id_decimal_string"], str(JOB))
        self.assertEqual(record["task_state"], "Completed")
        first, final = record["first_closed_model"], record["final_result"]
        self.assertEqual(first["task_state_at_observation"], "Running")
        self.assertEqual(first["publication"]["minimum_version"], final["publication"]["minimum_version"])
        self.assertIsNone(first["metrics"]["graphs"]["Baseline"])
        self.assertEqual(record["final_detail"]["graph"]["status"], "Incomplete")
        self.assertEqual(record["final_export"]["outcome"], "Complete")
        self.assertEqual(record["cached-result-gzip"]["http"]["content_encoding"], "gzip")
        self.assertGreaterEqual(final["submit_to_usable_model_seconds"], first["submit_to_usable_model_seconds"])
        self.assertGreaterEqual(record["first_model_to_terminal_seconds"], 0)
        sample = {**record, "case": "fake", "mode": "rpc", "side": "candidate"}
        aggregate = benchmark.aggregates([sample])[0]
        self.assertEqual(aggregate["closed_model_observations"], 1)
        self.assertEqual(aggregate["task_state_counts"], {"Completed": 1})
        self.assertIn("cached-result-gzip_get_median_seconds", aggregate)
        policy = benchmark.policy_comparisons([sample])[0]
        self.assertTrue(policy["observed_policy_consistent"])

    def test_unchanged_minimum_rejects_mutated_shared_control_with_new_detail(self):
        first = minimum_result()
        first["report"]["process_controls"] = [{
            "id": 0, "epoch": HASH, "source": first["report"]["minimum"]["declared_root_source"],
            "program": 91, "blocks": [{"successors": [None, None], "continuation": None, "boundary": True}],
            "jump_targets": []}]
        final = rich_result()
        final["report"]["process_controls"] = deepcopy(first["report"]["process_controls"])
        final["report"]["process_controls"][0]["blocks"][0]["boundary"] = False
        # The unused directory row keeps a well-formed boolean. Each reply
        # passes transport admission; only its immutable content has changed.
        report.wire_report(ok(final), job_id=JOB)
        record = self.run_case(final=final, result_override=ok(final), first=first)
        self.assertTrue(record["report_delivered"])
        self.assertEqual(record["delivered_publication"], first["publication"])
        self.assertIsNone(record["final_result"])
        self.assertEqual(record["runner_error_type"], "AssertionError")

    def test_unchanged_minimum_allows_program_reordering_and_control_appends_with_detail(self):
        first = minimum_result()
        first["report"]["process_controls"] = [{
            "id": 0, "epoch": HASH, "source": first["report"]["minimum"]["declared_root_source"],
            "program": 91, "blocks": [{"successors": [None, None], "continuation": None, "boundary": True}],
            "jump_targets": []}]
        final = rich_result()
        appended = {**deepcopy(first["report"]["programs"][0]), "id": 7,
                    "code_address": "0x" + "33" * 20, "code_hash": "0x" + "44" * 32}
        final["report"]["programs"] = [appended, *deepcopy(first["report"]["programs"])]
        control = {**deepcopy(first["report"]["process_controls"][0]), "id": 1,
                   "program": appended["id"], "source": {"Runtime": {
                       "source": appended["code_address"], "hash": appended["code_hash"], "fork": appended["fork"]}}}
        final["report"]["process_controls"] = [*deepcopy(first["report"]["process_controls"]), control]
        record = self.run_case(final=final, first=first)
        self.assertEqual(record["termination"], "http_report_delivered")
        self.assertEqual(record["final_result"]["publication"], final["publication"])
        self.assertEqual(record["first_closed_model"]["publication"]["minimum_version"],
                         record["final_result"]["publication"]["minimum_version"])
        self.assertIsNotNone(record["final_detail"]["graph"])
        captured = json.loads((self.directory / "cached-result-gzip.json").read_bytes())["result"]["Ok"]["report"]
        for directory in ("programs", "process_controls"):
            self.assertEqual(captured[directory], final["report"][directory])
        self.assertEqual([program["id"] for program in captured["programs"]], [7, 91])
        self.assertEqual(captured["programs"][1:], first["report"]["programs"])
        self.assertEqual([control["id"] for control in captured["process_controls"]], [0, 1])
        self.assertEqual(captured["process_controls"][:-1], first["report"]["process_controls"])

    def test_failed_cancelled_retain_closed_model_and_honest_job_state(self):
        failure = {"Failed": {"code": "Export", "message": ENDPOINT, "details": None}}
        for state in (failure, "Cancelled"):
            for export in ("Cancelled", {"Refused": {"code": "Export", "message": ENDPOINT, "details": None}}):
                with self.subTest(state=state, export=export):
                    final = minimum_result()
                    final["publication"]["version"] = "9007199254740997"
                    final["report"]["rich_export"] = export
                    record = self.run_case(state, final)
                    kind = "Failed" if isinstance(state, dict) else state
                    self.assertEqual(record["task_state"], kind)
                    self.assertEqual(record["termination"], "http_task_"+kind.lower())
                    self.assertTrue(record["report_delivered"])
                    self.assertTrue(record["delivered_publication"]["model_closed"])
                    self.assertIsNone(record["metrics"]["graphs"]["Native"])

    def test_final_result_failure_does_not_erase_the_delivered_minimum(self):
        error = {"result": {"Err": {"code": "Export", "message": ENDPOINT, "details": None}}}
        record = self.run_case("Cancelled", minimum_result(), error)
        self.assertEqual(record["task_state"], "Cancelled")
        self.assertEqual(record["termination"], "http_task_cancelled")
        self.assertTrue(record["report_delivered"])
        self.assertEqual(record["delivered_publication"]["version"], VERSION)
        self.assertIsNone(record["final_result"])

    def test_unchanged_publication_retains_the_exact_model_after_cancel(self):
        record = self.run_case("Cancelled", minimum_result())
        self.assertEqual(record["first_closed_model"]["publication"], record["final_result"]["publication"])

    def test_changed_minimum_or_malformed_final_keeps_prior_delivery_and_final_state(self):
        for mutate in (
            lambda r: r["report"]["minimum"]["environment"].update(input_scope="1"),
            lambda r: r["report"]["programs"][0].update(id=0),
            lambda r: r.update(id=JOB-1),
        ):
            invalid = rich_result()
            mutate(invalid)
            record = self.run_case("Cancelled", rich_result(), ok(invalid))
            self.assertEqual(record["task_state"], "Cancelled")
            self.assertTrue(record["report_delivered"])
            self.assertEqual(record["delivered_publication"]["version"], VERSION)
            self.assertIsNone(record["final_result"])
            self.assertEqual(record["runner_error_type"], "AssertionError")


if __name__ == "__main__":
    unittest.main()
