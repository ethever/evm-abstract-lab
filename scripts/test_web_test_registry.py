#!/usr/bin/env python3
"""Check browser scheduling and source-closure invariants without a browser."""

import ast
from contextlib import contextmanager, redirect_stderr, redirect_stdout
import importlib.util
import io
from collections import defaultdict
from copy import deepcopy
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, patch
from types import ModuleType, SimpleNamespace

sys.dont_write_bytecode = True
from web_test_registry import SOURCE_ROOT, load_manifest, select_cases, validate_manifest


class BrowserRegistryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = load_manifest()
        cls.cases = {case["id"]: case for case in cls.manifest["cases"]}

    def test_default_and_legacy_selections_are_exact_ordered_unions(self):
        expected = self.manifest["cases"]
        self.assertEqual(select_cases(self.manifest), expected)
        selected = select_cases(self.manifest, explicit=["termination"],
                                legacy=["flow-only", "termination-only", "ssa-details-only"])
        ids = [case["id"] for case in selected]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertEqual(set(ids), {"termination", "termination-call", "edge-flow", "compact-ssa",
                                    "ssa-details", "gestures-dpr1", "gestures-dpr2"})
        self.assertEqual(selected, [case for case in expected if case["id"] in ids])
        self.assertEqual([case["id"] for case in select_cases(self.manifest, legacy=["compression-only"])],
                         ["compression"])

    def test_bad_or_repeated_selection_cannot_silently_skip(self):
        with self.assertRaisesRegex(ValueError, "unknown browser cases"):
            select_cases(self.manifest, ["not-a-case"])
        with self.assertRaisesRegex(ValueError, "only be specified once"):
            select_cases(self.manifest, ["compression", "compression"])
        altered = deepcopy(self.manifest)
        altered["cases"].append(deepcopy(altered["cases"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate browser case"):
            validate_manifest(altered)

    def test_every_case_has_a_distinct_invocation(self):
        invocations = [(case["module"], json.dumps(case.get("parameters", {}), sort_keys=True))
                       for case in self.manifest["cases"]]
        self.assertEqual(len(invocations), len(set(invocations)))
        self.assertEqual({case["parameters"]["dpr"] for case in self.manifest["cases"]
                          if case["module"] == "web_test_gestures"}, {1, 2})
        runtimes = [case for case in self.manifest["cases"] if case["group"] == "captured-runtime"]
        self.assertEqual(len(runtimes), 3)
        self.assertEqual(len({case["parameters"]["fixture"] for case in runtimes}), 3)

    def test_every_case_module_is_registered(self):
        modules = {path.stem for path in (SOURCE_ROOT / "scripts").glob("web_test_*.py")
                   if any(isinstance(node, ast.FunctionDef) and node.name == "run"
                          for node in ast.parse(path.read_text()).body)}
        self.assertEqual(modules, {case["module"] for case in self.manifest["cases"]})

    def test_each_nix_projection_works_with_only_its_declared_files(self):
        for case in self.manifest["cases"]:
            with self.subTest(case=case["id"]), TemporaryDirectory() as directory:
                root = Path(directory)
                for relative in case["dependencies"]:
                    target = root / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.symlink_to(SOURCE_ROOT / relative)
                projection = {"schema_version": 1, "cases": [case], "legacy_flags": {}}
                self.assertEqual(validate_manifest(projection, root), projection)
                self.assertFalse((root / "scripts/web_test_cases.json").exists())

    def test_missing_and_overbroad_module_closures_fail(self):
        altered = deepcopy(self.manifest)
        altered["cases"][0]["dependencies"].remove("scripts/web_test_harness.py")
        with self.assertRaisesRegex(ValueError, "inexact Python closure"):
            validate_manifest(altered)
        altered = deepcopy(self.manifest)
        altered["cases"][0]["dependencies"].append("scripts/web_test_rpc.py")
        altered["cases"][0]["dependencies"].sort()
        with self.assertRaisesRegex(ValueError, "inexact Python closure"):
            validate_manifest(altered)

    def test_parameters_match_the_case_entrypoint_before_launching_services(self):
        altered = deepcopy(self.manifest)
        gestures = next(case for case in altered["cases"] if case["id"] == "gestures-dpr1")
        gestures["parameters"] = {}
        with self.assertRaisesRegex(ValueError, "inexact parameters.*missing=.*dpr"):
            validate_manifest(altered)
        gestures["parameters"] = {"dpr": 1, "unregistered_option": True}
        with self.assertRaisesRegex(ValueError, "inexact parameters.*extra=.*unregistered_option"):
            validate_manifest(altered)

    def test_fixture_dependencies_match_only_the_selected_scenario(self):
        # These reads use computed fixture paths, so the import closure alone
        # cannot prove their presence or catch an unrelated fixture in a case.
        layout = "crates/evm-abstract-web/tests/fixtures/layout/"
        fixtures = {"edge-flow": ["linear-chain"], "hierarchy-small": ["linear-chain", "shortcut"]}
        for case in self.manifest["cases"]:
            case_id = case["id"]
            names = fixtures.get(case_id, [])
            if case["module"] == "web_test_hierarchy_chain":
                names = [case["parameters"].get("fixture_name", "long-linear-chain")]
            elif case["module"] == "web_test_runtimes":
                names = [case["parameters"]["fixture"]]
            expected = {layout + name + extension for name in names for extension in [".hex", ".json"]}
            if case_id == "hierarchy-calls":
                expected.add(layout + "nested-call-world.json")
            if case_id in {"storage-table", "storage-scroll"}:
                expected.add("scripts/web_test_storage_literals.json")
            actual = {dependency for dependency in case["dependencies"] if not dependency.endswith(".py")}
            with self.subTest(case=case_id):
                self.assertEqual(actual, expected)

    def test_rpc_changes_do_not_invalidate_independent_graph_cases(self):
        for case_id in ["edge-flow", "gestures-dpr1", "gestures-dpr2", "hierarchy-small",
                        "hierarchy-long-chain", "hierarchy-native-projection", "runtime-weth9",
                        "runtime-router02", "runtime-multicall3", "root-terminal-coverage"]:
            with self.subTest(case=case_id):
                self.assertFalse(any("web_test_rpc" in dependency for dependency in self.cases[case_id]["dependencies"]))

    def test_terminal_coverage_keeps_the_wire_oracle_consumers_exact(self):
        consumers = {case["id"] for case in self.manifest["cases"]
                     if "scripts/web_test_wire_bytes.py" in case["dependencies"]}
        self.assertEqual(consumers, {"rpc-world", "known-recursion", "unknown-world"})
        self.assertNotIn("scripts/web_test_runtimes.py",
                         self.cases["root-terminal-coverage"]["dependencies"])

    def test_direct_execution_runs_every_selected_case_once_and_keeps_failure_evidence(self):
        # Exercise the actual CLI orchestration with inert acceptance functions.
        # This checks scheduling/reporting, without a browser or native server.
        specification = importlib.util.spec_from_file_location("browser_case_runner", SOURCE_ROOT / "scripts/test-web-browser.py")
        runner = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(runner)

        @contextmanager
        def errors():
            yield []

        @contextmanager
        def playwright():
            yield object()

        fake_harness = ModuleType("web_test_harness")
        fake_harness.capture_browser_errors = errors
        fake_playwright = ModuleType("playwright.sync_api")
        fake_playwright.sync_playwright = playwright
        for failing in [None, "submissions"]:
            with self.subTest(failing=failing), TemporaryDirectory() as directory:
                calls = []
                def acceptance(context, **parameters):
                    calls.append(context.output.name)
                    if context.output.name == failing:
                        raise AssertionError("deliberate acceptance failure")
                    return {"verified": context.output.name}
                def module(_name):
                    return SimpleNamespace(run=acceptance)
                argv = ["test-web-browser.py", "--server", "unused-server", "--assets", "unused-assets",
                        "--browser", "unused-browser", "--output", directory]
                with patch.object(sys, "argv", argv), patch.object(runner, "import_module", side_effect=module), \
                     patch.dict(sys.modules, {"playwright.sync_api": fake_playwright, "web_test_harness": fake_harness}), \
                     redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    if failing:
                        with self.assertRaises(SystemExit) as failure:
                            runner.main()
                        self.assertEqual(failure.exception.code, 1)
                    else:
                        runner.main()
                self.assertEqual(calls, [case["id"] for case in self.manifest["cases"]])
                aggregate = json.loads((Path(directory) / "report.json").read_text())
                timing = json.loads((Path(directory) / "timings.json").read_text())
                self.assertEqual(aggregate["status"], "failed" if failing else "passed")
                self.assertEqual(set(timing["cases"]), set(calls))
                for case_id in calls:
                    report = json.loads((Path(directory) / case_id / "report.json").read_text())
                    self.assertEqual(report["status"], "failed" if case_id == failing else "passed")
                    self.assertGreater(report["duration_ns"], 0)
                    self.assertTrue((Path(directory) / case_id / "timings.json").is_file())

    def test_failed_case_disposes_explicit_contexts_before_the_next_case(self):
        specification = importlib.util.spec_from_file_location("browser_cleanup_runner", SOURCE_ROOT / "scripts/test-web-browser.py")
        runner = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(runner)

        @contextmanager
        def errors():
            yield []

        @contextmanager
        def playwright():
            yield object()

        fake_harness = ModuleType("web_test_harness")
        fake_harness.capture_browser_errors = errors
        fake_playwright = ModuleType("playwright.sync_api")
        fake_playwright.sync_playwright = playwright
        browser = SimpleNamespace(contexts=[])
        disposed = []

        class OwnedContext:
            pages = []

        def acceptance(context, **_parameters):
            self.assertEqual(browser.contexts, [], "a prior failed case left a browser context")
            context.prerequisites._browser = browser
            owned = OwnedContext()

            def close():
                browser.contexts.remove(owned)
                disposed.append(context.output.name)

            owned.close = close
            browser.contexts.append(owned)
            raise AssertionError("deliberate failure with an explicit context")

        with TemporaryDirectory() as directory:
            argv = ["test-web-browser.py", "--case", "startup", "--case", "submissions",
                    "--server", "unused", "--assets", "unused", "--browser", "unused", "--output", directory]
            with patch.object(sys, "argv", argv), \
                 patch.object(runner, "import_module", return_value=SimpleNamespace(run=acceptance)), \
                 patch.dict(sys.modules, {"playwright.sync_api": fake_playwright, "web_test_harness": fake_harness}), \
                 redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as failure:
                    runner.main()
            self.assertEqual(failure.exception.code, 1)
            self.assertEqual(disposed, ["startup", "submissions"])
            self.assertEqual(browser.contexts, [])

    def test_gesture_browser_closes_when_page_initialization_fails(self):
        # Gestures own a separate browser for the physical DPR. The common
        # runner cannot discover that browser through its shared prerequisites.
        fake_playwright = ModuleType("playwright.sync_api")
        fake_playwright.expect = Mock()
        specification = importlib.util.spec_from_file_location("gesture_cleanup_case", SOURCE_ROOT / "scripts/web_test_gestures.py")
        gestures = importlib.util.module_from_spec(specification)
        with patch.dict(sys.modules, {"playwright.sync_api": fake_playwright}):
            specification.loader.exec_module(gestures)
        browser = SimpleNamespace(new_context=Mock(side_effect=RuntimeError("page initialization failed")), close=Mock())
        with patch.object(gestures, "launch_browser", return_value=browser):
            with self.assertRaisesRegex(RuntimeError, "page initialization failed"):
                gestures.graph_interactions(object(), "unused", "unused", Path("unused"), dprs=(1,))
        browser.close.assert_called_once_with()

    def test_all_asserting_acceptance_paths_are_reachable_from_registered_cases(self):
        """Moving a family into a file must not disconnect it from the full gate."""
        trees = {path.stem: ast.parse(path.read_text())
                 for path in (SOURCE_ROOT / "scripts").glob("web_test_*.py")
                 if path.stem not in {"web_test_registry", "web_test_rpc"}}
        definitions, imports = {}, defaultdict(dict)
        for module, tree in trees.items():
            for node in tree.body:
                if isinstance(node, ast.ImportFrom) and node.module in trees:
                    imports[module].update({name.asname or name.name: (node.module, name.name)
                                            for name in node.names})
                if isinstance(node, (ast.FunctionDef, ast.ClassDef)):
                    definitions[module, node.name] = node
        pending = [(case["module"], "run") for case in self.manifest["cases"]]
        visited = set()
        while pending:
            key = pending.pop()
            if key in visited or key not in definitions:
                continue
            visited.add(key)
            module, _ = key
            for node in ast.walk(definitions[key]):
                if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
                    target = imports[module].get(node.id, (module, node.id))
                    if target in definitions:
                        pending.append(target)
        acceptance = {key for key, node in definitions.items()
                      if key[1].endswith("_interactions")
                      and any(isinstance(child, ast.Assert) for child in ast.walk(node))}
        self.assertTrue(acceptance)
        self.assertEqual(acceptance - visited, set())


class ReportWireTests(unittest.TestCase):
    """Namespace/source adapters cannot reinterpret lifecycle or remap wire IDs."""

    def test_empty_return_is_a_zero_byte_view_not_backing_state_identity(self):
        from web_test_wire_bytes import assert_empty_return_data
        zero = {"constants": ["0x0"], "known_zero": hex((1 << 256) - 1), "known_one": "0x0",
                "unsigned": {"lower": "0x0", "upper": "0x0"}, "signed": {"lower": "0x0", "upper": "0x0"},
                "nonzero": False, "congruence": {"Exact": "0x0"}}
        report = {"symbolic": {"nodes": [{"Constant": "0x0"}, {"Constant": "0x1"},
            {"BytesExact": "0x"}, "BytesZero", {"Fresh": 0},
            {"BytesIte": {"condition": {"node": 4}, "then_state": {"node": 2}, "else_state": {"node": 3}}},
            {"View": {"state": {"node": 5}, "offset": {"node": 0}, "length": {"node": 0}}}]},
            "byte_arrays": [{"memory": False, "length": zero, "symbolic": {"node": 6}, "cells": []}]}
        assert_empty_return_data(report, 0)
        report["symbolic"]["nodes"][6]["View"]["state"] = {"node": 2}
        assert_empty_return_data(report, 0)
        for corruption in ["length", "view-length", "numeric", "memory", "cells", "word-state", "forward-reference"]:
            damaged = deepcopy(report)
            data, view = damaged["byte_arrays"][0], damaged["symbolic"]["nodes"][6]["View"]
            if corruption == "length": data["length"]["constants"] = ["0x1"]
            elif corruption == "view-length": view["length"] = {"node": 1}
            elif corruption == "numeric": data["length"]["known_one"] = "0x1"
            elif corruption == "memory": data["memory"] = True
            elif corruption == "cells": data["cells"] = [{"offset": 0, "value": 0}]
            elif corruption == "word-state": view["state"] = {"node": 0}
            else: view["state"] = {"node": 6}
            with self.subTest(corruption=corruption), self.assertRaises(AssertionError):
                assert_empty_return_data(damaged, 0)

    def test_retained_caller_zero_window_rejects_nonzero_literal_background(self):
        from web_test_wire_bytes import _Graph
        for payload in ["0x", "0xff", "0xff" + "00" * 63, "0x" + "00" * 64 + "ff"]:
            with self.subTest(payload=payload):
                _Graph({"symbolic": {"nodes": [{"BytesExact": payload}]}}).zero_window({"node": 0}, 1, 1, 64)
        for position in [1, 31, 32, 63]:
            payload = bytearray(64);payload[position] = 1
            graph = _Graph({"symbolic": {"nodes": [{"BytesExact": "0x" + payload.hex()}]}})
            with self.subTest(position=position), self.assertRaises(AssertionError):
                graph.zero_window({"node": 0}, 1, 1, 64)

    def report(self):
        # Equal local IDs deliberately name different Native/Global data. Source
        # IDs are sparse and shared, so array-position lookups cannot pass.
        return {
            "schema_version": 21, "selected_view": "Baseline", "status": "Converged",
            "minimum": None, "rich_export": "Complete",
            "hybrid": {"coverage": {"baseline_current": True}},
            "programs": [{"id": 91, "blocks": [{"id": 12, "instructions": [{"pc": 40}]}]},
                         {"id": 7, "blocks": [{"id": 3, "instructions": [{"pc": 5}]}]}],
            "process_controls": [],
            "global": {"status": "Converged", "metadata": {"root_program": 91},
                       "states": [{"state": 4, "entry": "global"}], "abstract_summaries": []},
            "native": {"status": "Incomplete", "metadata": {"root_program": 7},
                       "states": [{"state": 4, "entry": "native"}], "abstract_summaries": []},
        }

    def publication(self):
        return {"version": "9007199254740997", "minimum_version": None, "global_version": "9007199254740993",
                "native_version": "9007199254740997", "model_closed": True}

    def policy_report(self, requested=1):
        report = self.report()
        coverage = report["hybrid"]["coverage"]
        coverage.update(requested_detail_regions=requested, detail_regions=[],
                        detail="Disabled" if requested == 0 else "Current")
        report["hybrid"].update(precision=None, local_prefixes=[])
        for namespace in ["global", "native"]:
            report[namespace]["metadata"]["limits"] = {"max_detail_regions": requested}
        if requested == 0:
            report["native"] = None
        return report

    def test_detail_policy_uses_actual_native_selection_with_fixed_requested_bound(self):
        from web_test_hybrid import assert_detail_policy
        for requested in [1, 2, 3]:
            report = self.policy_report(requested)
            precision = deepcopy(report["hybrid"]["coverage"])
            precision["detail_regions"] = [{"source": {"Runtime": {
                "source": "0x" + f"{index + 1:040x}", "hash": "0x" + "ab" * 32, "fork": "Osaka"}},
                "code_address": "0x" + f"{index + 1:040x}", "code_hash": "0x" + "ab" * 32}
                for index in range(requested)]
            report["hybrid"]["precision"] = precision
            with self.subTest(requested=requested):
                self.assertIs(assert_detail_policy(report, requested), precision)
                self.assertEqual(len(precision["detail_regions"]), requested)
                self.assertEqual(report["hybrid"]["coverage"]["detail_regions"], [])
        report = self.policy_report()
        # The requested policy exists before optional rich graphs arrive.
        report["global"] = report["native"] = None
        report["minimum"] = {"namespace": "Baseline"}
        self.assertIs(assert_detail_policy(report, 1), report["hybrid"]["coverage"])
        disabled = self.policy_report(0)
        self.assertIs(assert_detail_policy(disabled, 0), disabled["hybrid"]["coverage"])

    def test_detail_policy_rejects_missing_stale_and_mismatched_applied_metadata(self):
        from web_test_hybrid import assert_detail_policy
        for field in ["missing", "echo", "precision-echo", "global-limit", "native-limit", "bool-echo", "bool-limit"]:
            report = self.policy_report()
            coverage = report["hybrid"]["coverage"]
            if field == "missing":
                del coverage["requested_detail_regions"]
            elif field == "echo":
                coverage["requested_detail_regions"] = 2
            elif field == "precision-echo":
                report["hybrid"]["precision"] = {**coverage, "requested_detail_regions": 2}
            elif field == "bool-echo":
                coverage["requested_detail_regions"] = True
            else:
                namespace = "native" if field == "native-limit" else "global"
                report[namespace]["metadata"]["limits"]["max_detail_regions"] = True if field == "bool-limit" else 2
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_detail_policy(report, 1)
        for requested in [False, -1, 1.0, "1", 2**64]:
            with self.subTest(requested=requested), self.assertRaises(AssertionError):
                assert_detail_policy(self.policy_report(), requested)

    def test_zero_detail_keeps_pending_global_distinct_from_disabled_detail(self):
        from web_test_hybrid import assert_detail_policy
        report = self.policy_report(0)
        coverage = report["hybrid"]["coverage"]
        coverage.update(baseline_current=False, detail="BaselinePending")
        precision = {**coverage}
        report["hybrid"]["precision"] = precision
        self.assertIs(assert_detail_policy(report, 0), precision)
        for stage_name in ["coverage", "precision"]:
            stage = report["hybrid"][stage_name]
            stage["baseline_current"] = True
            with self.subTest(stage=stage_name), self.assertRaises(AssertionError):
                assert_detail_policy(report, 0)
            stage["baseline_current"] = False
        for detail in ["Current", "Incomplete", "Refused"]:
            precision["detail"] = detail
            with self.subTest(detail=detail), self.assertRaises(AssertionError):
                assert_detail_policy(report, 0)

    def test_detail_policy_rejects_bad_selected_containers_and_fake_disabled_native(self):
        from web_test_hybrid import assert_detail_policy
        for regions in [{}, "", [None, None]]:
            report = self.policy_report()
            report["hybrid"]["coverage"]["detail_regions"] = regions
            with self.subTest(regions=regions), self.assertRaises(AssertionError):
                assert_detail_policy(report, 1)
        for corruption in ["native", "prefix", "detail"]:
            report = self.policy_report(0)
            if corruption == "native":
                report["native"] = self.policy_report(0)["global"]
            elif corruption == "prefix":
                report["hybrid"]["local_prefixes"] = [{}]
            else:
                report["hybrid"]["coverage"]["detail"] = "Incomplete"
            with self.subTest(corruption=corruption), self.assertRaises(AssertionError):
                assert_detail_policy(report, 0)

    def test_graph_selection_borrows_sources_without_combining_namespaces(self):
        from web_test_hybrid import displayed_graph, graph, oracle_graph, source_block
        report = self.report()
        self.assertIs(graph(report, "Baseline"), report["global"])
        self.assertIs(graph(report, "Native"), report["native"])
        native = oracle_graph(report, "Native")
        self.assertIs(native["states"], report["native"]["states"])
        self.assertIs(native["programs"], report["programs"])
        self.assertIs(native["process_controls"], report["process_controls"])
        self.assertIs(native["disassembly"], report["programs"][1]["blocks"])
        self.assertEqual(displayed_graph(report)["states"][0]["entry"], "global")
        report["selected_view"] = "Native"
        self.assertEqual(displayed_graph(report)["states"][0]["entry"], "native")
        self.assertIs(source_block(report, {"program": 7, "basic_block": 3}),
                      report["programs"][1]["blocks"][0])
        self.assertIsNone(source_block(report, {"program": None, "basic_block": None}))
        with self.assertRaises(AssertionError):
            source_block(report, {"program": 0, "basic_block": 3})
        report["programs"].append(deepcopy(report["programs"][1]))
        with self.assertRaises(AssertionError):
            source_block(report, {"program": 7, "basic_block": 3})

    def test_shared_catalog_keeps_original_template_and_conditional_native_sources_distinct(self):
        from web_test_hybrid import assert_root_source
        report = self.report()
        coordinate = "0x" + "11" * 20
        digest = "0x" + "ab" * 32
        source = {"InputProgram": {"hash": digest, "fork": "Osaka"}}
        metadata = {"root_program": 91, "declared_root_source": source}
        for program, address in zip(report["programs"], [None, coordinate], strict=True):
            program.update(code_address=address, code_hash=digest, fork="Osaka", kind="Runtime")
        assert_root_source(report, metadata)
        for field, value in [("code_address", coordinate), ("code_hash", "0x" + "cd" * 32),
                             ("fork", "Prague"), ("kind", "Initcode")]:
            invalid = deepcopy(report)
            invalid["programs"][0][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_root_source(invalid, metadata)
        with self.assertRaises(AssertionError):
            assert_root_source(report, {**metadata, "root_program": 7})
        runtime = deepcopy(report)
        runtime["programs"][0]["code_address"] = coordinate
        captured = {"Runtime": {"source": coordinate, "hash": digest, "fork": "Osaka"}}
        assert_root_source(runtime, {**metadata, "declared_root_source": captured})
        constructor = deepcopy(report)
        constructor["programs"][0]["kind"] = "Initcode"
        initcode = {"InitCode": {"hash": digest, "fork": "Osaka"}}
        assert_root_source(constructor, {**metadata, "declared_root_source": initcode})

    def test_process_control_receipt_resolves_shared_source_epoch_without_native_fields(self):
        from web_test_hybrid import oracle_graph, process_control_relation
        report = self.report()
        digest, epoch = "0x" + "ab" * 32, "0x" + "cd" * 32
        source = {"InputProgram": {"hash": digest, "fork": "Osaka"}}
        report["programs"][0].update(code_address=None, code_hash=digest, fork="Osaka", kind="Runtime")
        relation = {"id": 0, "epoch": epoch, "source": source, "program": 91,
                    "blocks": [{"successors": [None, None], "continuation": None, "boundary": True}],
                    "jump_targets": []}
        report["process_controls"] = [relation]
        key = {"epoch": epoch, "owner": "RootNamespace", "source": source,
               "is_static": False, "location": {"Block": 0}}
        region = {"relation": 0, "entry": {"Block": 0}, "through": [], "boundaries": [{"Block": 0}]}
        for namespace in ["Baseline", "Native"]:
            selected = oracle_graph(report, namespace)
            self.assertIs(selected["process_controls"], report["process_controls"])
            self.assertIs(process_control_relation(selected, key, region), relation)
        for changed in [{**key, "epoch": "0x" + "ef" * 32}, {**key, "location": "JumpTargets"},
                        {**key, "source": {"Runtime": {"hash": digest, "fork": "Osaka", "source": "0x1"}}}]:
            with self.subTest(key=changed), self.assertRaises(AssertionError):
                process_control_relation(report, changed, region)
        for index in [True, -1, 1]:
            with self.subTest(index=index), self.assertRaises(AssertionError):
                process_control_relation(report, key, {**region, "relation": index})
        invalid = deepcopy(report)
        invalid["process_controls"][0]["program"] = 7
        invalid["programs"][1].update(code_address="0x1", code_hash=digest, fork="Osaka", kind="Runtime")
        with self.assertRaises(AssertionError):
            process_control_relation(invalid, key, region)
        self.assertNotIn("executed_pcs", relation)
        self.assertNotIn("basic_block", key)
        self.assertEqual(report["global"]["states"], [{"state": 4, "entry": "global"}])
        self.assertEqual(report["native"]["states"], [{"state": 4, "entry": "native"}])

        # A real hub coordinate stays virtual; its targets use dense source
        # block indices, independently of the sparse ProgramInfo/Block IDs.
        report["programs"][0]["blocks"].append({"id": 83, "instructions": [{"pc": 80, "opcode": 0x5b}]})
        relation["blocks"].append({"successors": [None, None], "continuation": None, "boundary": True})
        relation["jump_targets"] = [1]
        hub_key = {**key, "location": "JumpTargets"}
        hub = {"relation": 0, "entry": "JumpTargets", "through": ["JumpTargets"], "boundaries": [{"Block": 1}]}
        self.assertIs(process_control_relation(report, hub_key, hub), relation)
        self.assertEqual(relation["jump_targets"], [1])
        for point in ["Any", {"Block": True}, {"Block": -1}, {"Block": 2}, {"Block": 83}, {"PC": 80}]:
            with self.subTest(point=point), self.assertRaises(AssertionError):
                process_control_relation(report, hub_key, {**hub, "boundaries": [point]})

    def test_task_reply_keeps_exact_id_publication_and_global_closure(self):
        from web_test_hybrid import job_result_report
        report, publication = self.report(), self.publication()
        job_id = 18_446_744_073_709_551_614
        envelope = {"result": {"Ok": {"id": job_id, "publication": publication, "report": report}}}
        self.assertIs(job_result_report(envelope, job_id=job_id, publication=publication), report)
        parsed = json.loads(json.dumps(envelope))
        self.assertEqual(job_result_report(parsed, job_id=job_id, publication=publication), report)
        self.assertEqual(report["native"]["status"], "Incomplete")
        # First Global publication has its own version and no Native graph.
        early = deepcopy(envelope)
        early["result"]["Ok"]["report"]["native"] = None
        early["result"]["Ok"]["publication"].update(version="9007199254740993", native_version=None)
        self.assertIsNone(job_result_report(early)["native"])
        self.assertEqual(early["result"]["Ok"]["publication"]["global_version"], publication["global_version"])

    def test_final_report_requires_the_shared_process_control_array(self):
        from web_test_hybrid import job_result_report
        report = self.report()
        envelope = {"result": {"Ok": {"id": 42, "publication": self.publication(), "report": report}}}
        del report["process_controls"]
        with self.assertRaises(AssertionError):
            job_result_report(envelope)
        for value in [None, {}, "controls", True]:
            report["process_controls"] = value
            with self.subTest(value=value), self.assertRaises(AssertionError):
                job_result_report(envelope)
        report["process_controls"] = []
        for value in [20, 22, True, "21"]:
            report["schema_version"] = value
            with self.subTest(schema=value), self.assertRaises(AssertionError):
                job_result_report(envelope)

    def test_wrong_endpoint_job_and_graph_publications_are_rejected(self):
        from web_test_hybrid import job_result_report
        envelope = {"result": {"Ok": {"id": 42, "publication": self.publication(), "report": self.report()}}}
        with self.assertRaises(AssertionError):
            job_result_report({"result": {"Ok": self.report()}})  # AnalyzeReply
        with self.assertRaises(AssertionError):
            job_result_report(envelope, job_id=43)
        with self.assertRaises(AssertionError):
            job_result_report(envelope, publication={**self.publication(), "version": "1"})
        for job_id in [True, "42", -1, 1 << 64]:
            invalid = deepcopy(envelope)
            invalid["result"]["Ok"]["id"] = job_id
            with self.subTest(job_id=job_id), self.assertRaises(AssertionError):
                job_result_report(invalid)
        for field, value in [("native_version", None), ("global_version", None),
                             ("model_closed", False), ("version", 9007199254740997),
                             ("version", "09007199254740997"), ("native_version", "9007199254740998")]:
            invalid = deepcopy(envelope)
            invalid["result"]["Ok"]["publication"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                job_result_report(invalid)

    def test_task_report_requires_exact_success_wrapper_and_result_grammar(self):
        from web_test_hybrid import job_result_report
        payload = {"id": 42, "publication": self.publication(), "report": self.report()}
        valid = {"result": {"Ok": payload}}
        self.assertIs(job_result_report(valid), payload["report"])
        malformed = [
            ("unknown-wrapper-field", {**valid, "unknown": None}),
            ("dual-result-tags", {"result": {"Ok": payload, "Err": {}}}),
            ("unknown-result-tag", {"result": {"Ok": payload, "Pending": None}}),
            ("missing-wrapper-field", {}),
            ("missing-result-tag", {"result": {}}),
            ("wrong-wrapper-tag", {"Ok": payload}),
        ]
        for value in [None, True, 42, "result", []]:
            malformed.append((f"wrapper-type-{type(value).__name__}", value))
            malformed.append((f"result-type-{type(value).__name__}", {"result": value}))
            malformed.append((f"payload-type-{type(value).__name__}", {"result": {"Ok": value}}))
        for name, envelope in malformed:
            with self.subTest(name=name), self.assertRaises(AssertionError):
                job_result_report(envelope)

    def minimum_publication(self):
        # Only the transport-visible minimum qualification is under test here;
        # complete numerical/control admission belongs to Rust report validation.
        report = self.report()
        report.update({"global": None, "native": None, "rich_export": "NotRequested",
                       "minimum": {"namespace": "Baseline", "status": "Converged", "current": True,
                                   "revision": "0", "summaries": []}})
        publication = {**self.publication(), "version": "9007199254740993",
                       "minimum_version": "9007199254740993", "global_version": None,
                       "native_version": None}
        return {"result": {"Ok": {"id": 42, "publication": publication, "report": report}}}

    def control_publication(self, profile):
        # Small source-derived wire fragments, not generated report captures.
        # Only the fields consumed by this Python reference boundary are needed;
        # numerical closure and topology remain Rust validator responsibilities.
        envelope = (self.minimum_publication() if profile == "minimum" else
                    {"result": {"Ok": {"id": 42, "publication": self.publication(), "report": self.report()}}})
        report = envelope["result"]["Ok"]["report"]
        digest, epoch = "0x" + "ab" * 32, "0x" + "cd" * 32
        source = {"InputProgram": {"hash": digest, "fork": "Osaka"}}
        report["programs"][0].update(code_address=None, code_hash=digest, fork="Osaka", kind="Runtime")
        report["process_controls"] = [{
            "id": 0, "epoch": epoch, "source": deepcopy(source), "program": 91,
            "blocks": [{"successors": [None, None], "continuation": None, "boundary": True}],
            "jump_targets": [],
        }]
        key = {"epoch": epoch, "owner": "RootNamespace", "source": source,
               "is_static": False, "location": {"Block": 0}}
        region = {"relation": 0, "entry": {"Block": 0}, "through": [], "boundaries": [{"Block": 0}]}
        node = {"frame": {"process": key}, "transfer": {"process": {"control": region}}}
        summaries = [{"regional_cfg": {"nodes": []}}, {"regional_cfg": {"nodes": [
            {"frame": {"process": deepcopy(key)}, "transfer": None},
            {"frame": {"process": None}, "transfer": {"process": None}}, node,
        ]}}]
        if profile == "minimum":
            report["minimum"]["summaries"] = [{"relation": summary} for summary in summaries]
        else:
            # Native receipts must be checked even while Global is selected.
            report[profile]["abstract_summaries"] = summaries
        return envelope, node

    def test_task_report_resolves_present_controls_in_every_summary_profile(self):
        from web_test_hybrid import job_result_report
        for profile in ["minimum", "global", "native"]:
            envelope, node = self.control_publication(profile)
            report = envelope["result"]["Ok"]["report"]
            with self.subTest(profile=profile):
                self.assertIs(job_result_report(envelope, job_id=42,
                    publication=envelope["result"]["Ok"]["publication"]), report)
                self.assertEqual(node["frame"]["process"]["location"], {"Block": 0})
                self.assertNotIn("executed_pcs", node)

    def test_task_report_allows_primitive_no_control_and_pending_transfers(self):
        from web_test_hybrid import job_result_report
        for profile in ["minimum", "global", "native"]:
            envelope, node = self.control_publication(profile)
            report = envelope["result"]["Ok"]["report"]
            report["process_controls"] = []
            node["frame"]["process"].update(source={"Empty": {"fork": "Osaka"}}, location="Any")
            node["transfer"]["process"]["control"] = None
            with self.subTest(profile=profile):
                self.assertIs(job_result_report(envelope), report)

    def test_task_report_rejects_dangling_and_misqualified_present_controls(self):
        from web_test_hybrid import job_result_report
        for profile in ["minimum", "global", "native"]:
            for corruption in ["missing-row", "row-id", "row-source", "row-epoch", "key-source",
                               "key-epoch", "key-entry", "missing-key", "program-source"]:
                envelope, node = self.control_publication(profile)
                report = envelope["result"]["Ok"]["report"]
                row, key = report["process_controls"][0], node["frame"]["process"]
                if corruption == "missing-row":
                    report["process_controls"] = []
                elif corruption == "row-id":
                    row["id"] = True
                elif corruption == "row-source":
                    row["source"] = {"InputProgram": {"hash": "0x" + "ef" * 32, "fork": "Osaka"}}
                elif corruption == "row-epoch":
                    row["epoch"] = "0x" + "ef" * 32
                elif corruption == "key-source":
                    key["source"] = {"Runtime": {"source": "0x1", "hash": "0x" + "ab" * 32, "fork": "Osaka"}}
                elif corruption == "key-epoch":
                    key["epoch"] = "0x" + "ef" * 32
                elif corruption == "key-entry":
                    key["location"] = "End"
                elif corruption == "missing-key":
                    node["frame"]["process"] = None
                else:
                    row["program"] = 7
                    report["programs"][1].update(code_address="0x1", code_hash="0x" + "ab" * 32,
                                                  fork="Osaka", kind="Runtime")
                with self.subTest(profile=profile, corruption=corruption), self.assertRaises(AssertionError):
                    job_result_report(envelope)

    def test_task_report_checks_both_location_types_before_python_equality(self):
        from web_test_hybrid import job_result_report
        for profile in ["minimum", "global", "native"]:
            for field in ["key", "entry", "through", "boundaries"]:
                for point in [{"Block": False}, {"Block": 0.0}, {"Block": -1}, {"Block": 1},
                              {"Block": 12}, {"PC": 40}, [], False]:
                    envelope, node = self.control_publication(profile)
                    region = node["transfer"]["process"]["control"]
                    if field == "key":
                        node["frame"]["process"]["location"] = point
                    else:
                        region[field] = [point] if field in {"through", "boundaries"} else point
                    with self.subTest(profile=profile, field=field, point=point), self.assertRaises(AssertionError):
                        job_result_report(envelope)

    def test_task_report_requires_lists_before_consuming_control_containers(self):
        from web_test_hybrid import job_result_report
        for profile in ["minimum", "global", "native"]:
            for field in ["through", "boundaries", "programs", "blocks", "program-blocks", "summaries", "nodes"]:
                for value in [{}, ""]:
                    envelope, node = self.control_publication(profile)
                    report = envelope["result"]["Ok"]["report"]
                    if field in {"through", "boundaries"}:
                        node["transfer"]["process"]["control"][field] = value
                    elif field == "programs":
                        report["programs"] = value
                    elif field == "blocks":
                        report["process_controls"][0]["blocks"] = value
                    elif field == "program-blocks":
                        report["programs"][0]["blocks"] = value
                    elif profile == "minimum":
                        if field == "summaries":
                            report["minimum"]["summaries"] = value
                        else:
                            report["minimum"]["summaries"][-1]["relation"]["regional_cfg"]["nodes"] = value
                    elif field == "summaries":
                        report[profile]["abstract_summaries"] = value
                    else:
                        report[profile]["abstract_summaries"][-1]["regional_cfg"]["nodes"] = value
                    with self.subTest(profile=profile, field=field, value=value), self.assertRaises(AssertionError):
                        job_result_report(envelope)

    def test_minimum_publication_keeps_identity_separate_from_optional_richness(self):
        from web_test_hybrid import graph, job_result_report, namespace_available, oracle_graph
        early = self.minimum_publication()
        minimum = early["result"]["Ok"]["report"]["minimum"]
        report = job_result_report(early)
        self.assertIs(report["minimum"], minimum)
        self.assertEqual(minimum["revision"], "0")
        self.assertTrue(namespace_available(report, "Baseline"))
        self.assertFalse(namespace_available(report, "Native"))
        self.assertIsNone(graph(report, "Baseline"))
        self.assertIsNone(oracle_graph(report, "Native"))
        refusal = {"Refused": {"code": "Export", "message": "typed projection depth control",
            "details": {"Export": {"kind": "Depth", "required": 3,
                "limits": {"max_nodes": 100, "max_depth": 2, "max_exact_bytes": 100},
                "usage": {"nodes": 2, "exact_bytes": 0, "work": 10}, "written": None}}}}
        for outcome in [refusal, "Cancelled"]:
            final = deepcopy(early)
            final["result"]["Ok"]["publication"]["version"] = "9007199254740997"
            final["result"]["Ok"]["report"]["rich_export"] = outcome
            with self.subTest(outcome=outcome):
                retained = job_result_report(final)
                self.assertEqual(retained["minimum"], minimum)
                self.assertTrue(final["result"]["Ok"]["publication"]["model_closed"])
                self.assertEqual(final["result"]["Ok"]["publication"]["minimum_version"],
                                 early["result"]["Ok"]["publication"]["minimum_version"])
        rich = self.report()
        rich["minimum"] = minimum
        final = {"result": {"Ok": {"id": 42, "publication": {
            **self.publication(), "minimum_version": "9007199254740993",
            "global_version": "9007199254740997"}, "report": rich}}}
        self.assertEqual(job_result_report(final)["minimum"], minimum)
        self.assertEqual(final["result"]["Ok"]["publication"]["global_version"], "9007199254740997")

    def test_minimum_profile_rejects_missing_mixed_and_noncanonical_identity(self):
        from web_test_hybrid import job_result_report
        valid = self.minimum_publication()
        for field, value in [("minimum_version", None), ("minimum_version", 9007199254740993),
                             ("minimum_version", "09007199254740993"), ("minimum_version", "0"),
                             ("minimum_version", "9007199254740994"), ("global_version", "1"),
                             ("native_version", "1")]:
            invalid = deepcopy(valid)
            invalid["result"]["Ok"]["publication"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                job_result_report(invalid)
        for field, value in [("minimum", None), ("selected_view", "Native"), ("selected_view", "Global"),
                             ("rich_export", "Unknown"), ("rich_export", {"Complete": None}),
                             ("rich_export", {"Refused": {}, "Cancelled": None}),
                             ("rich_export", {"Refused": None}), ("rich_export", {"Refused": {}}),
                             ("rich_export", None), ("rich_export", True), ("rich_export", [])]:
            invalid = deepcopy(valid)
            invalid["result"]["Ok"]["report"][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                job_result_report(invalid)
        invalid = deepcopy(valid)
        invalid["result"]["Ok"]["report"]["minimum"]["namespace"] = "Native"
        with self.assertRaises(AssertionError):
            job_result_report(invalid)
        old_grammar = deepcopy(valid)
        del old_grammar["result"]["Ok"]["publication"]["minimum_version"]
        with self.assertRaises(AssertionError):
            job_result_report(old_grammar)

    def test_pure_native_publication_has_no_minimum_or_global_version(self):
        from web_test_hybrid import job_result_report
        report = self.report()
        report.update({"global": None, "hybrid": None, "selected_view": "Native"})
        report["native"]["status"] = "Converged"
        publication = {**self.publication(), "global_version": None}
        envelope = {"result": {"Ok": {"id": 42, "publication": publication, "report": report}}}
        self.assertIs(job_result_report(envelope), report)

    def test_model_closed_includes_minimum_currentness_independently_of_rich_export(self):
        from web_test_hybrid import job_result_report
        report = self.minimum_publication()
        result = report["result"]["Ok"]
        result["report"]["minimum"]["current"] = False
        with self.assertRaises(AssertionError):
            job_result_report(report)
        result["publication"]["model_closed"] = False
        self.assertIs(job_result_report(report), result["report"])

    def test_minimum_transport_timings_allow_no_ssa_without_relaxing_native_or_phase_totals(self):
        fake_playwright = ModuleType("playwright.sync_api")
        fake_playwright.expect = Mock()
        specification = importlib.util.spec_from_file_location(
            "minimum_timing_harness", SOURCE_ROOT / "scripts/web_test_harness.py")
        harness = importlib.util.module_from_spec(specification)
        with patch.dict(sys.modules, {"playwright.sync_api": fake_playwright}):
            specification.loader.exec_module(harness)
        report = self.minimum_publication()["result"]["Ok"]["report"]
        report["timings"] = {"queue_ns": 7, "processing_ns": 11, "validating_ns": 1,
                             "pinning_ns": 0, "acquiring_ns": 2, "analyzing_ns": 5,
                             "building_ssa_ns": 0, "projecting_ns": 3}
        self.assertIs(harness.measured_backend_timings(report, require_ssa=False), report["timings"])
        with self.assertRaises(AssertionError):
            harness.measured_backend_timings(report)
        with self.assertRaises(AssertionError):
            harness.measured_backend_timings(report, require_ssa=True)
        report["timings"]["processing_ns"] = 12
        with self.assertRaises(AssertionError):
            harness.measured_backend_timings(report, require_ssa=False)


class NativeContextAdmissionTests(unittest.TestCase):
    @staticmethod
    def report():
        # A source-derived wire fragment: the real entry ID is deliberately
        # nonzero, and state0 is a different suspended invocation.
        address, digest = "0x" + "11" * 20, "0x" + "ab" * 32
        frame = dict(process=None, index=0, program=7, continuation=None, context=[], stack=[],
                     basic_block=3, storage_address=address, code_address=address, code_hash=digest,
                     is_static=False, kind="Runtime", caller={"Concrete": "0x" + "22" * 20},
                     address_value={"Concrete": address})
        node = dict(process=None, id=47, program=7, frame_depth=1, context=[], entry_stack=[],
                    basic_block=3, start_pc=0, storage_address=address, code_address=address)
        return dict(programs=[dict(id=7, code_address=address, code_hash=digest, kind="Runtime",
                                   blocks=[dict(id=3, start_pc=0)])], native=dict(
            metadata=dict(root_program=7, entry_address=address, environment=dict(
                is_static=False, caller=frame["caller"], to=frame["address_value"])),
            cfg=[dict(node, id=0, frame_depth=2), node],
            states=[dict(state=0, entry=dict(frames=[frame, frame])),
                    dict(state=47, entry=dict(frames=[frame]))]))

    def test_native_entry_is_an_actual_qualified_invocation_not_state_zero(self):
        from web_test_hybrid import native_entry_context
        report = self.report()
        self.assertIs(native_entry_context(report), report["native"]["cfg"][1])
        self.assertEqual(native_entry_context(report)["id"], 47)

    def test_missing_ambiguous_or_wrong_root_evidence_cannot_supply_a_context(self):
        from web_test_hybrid import native_entry_context
        for field in ["missing-frame", "duplicate-frame", "duplicate-root", "caller", "owner", "static",
                      "code-hash", "source", "address-condition", "bool-state", "no-native"]:
            report = self.report()
            native = report["native"]
            frame = native["states"][1]["entry"]["frames"][0]
            if field == "missing-frame":
                native["states"].pop()
            elif field == "duplicate-frame":
                native["states"].append(deepcopy(native["states"][1]))
            elif field == "duplicate-root":
                native["cfg"].append(dict(native["cfg"][1], id=48))
                native["states"].append(dict(state=48, entry=dict(frames=[deepcopy(frame)])))
            elif field == "source":
                frame["program"] = 91
            elif field == "bool-state":
                native["cfg"][1]["id"] = True
            elif field == "no-native":
                report["native"] = None
            else:
                key, value = {
                    "caller": ("caller", {"Concrete": "0x" + "33" * 20}),
                    "owner": ("storage_address", "0x" + "33" * 20),
                    "static": ("is_static", True),
                    "code-hash": ("code_hash", "0x" + "cd" * 32),
                    "address-condition": ("address_value", {"Symbolic": {"kind": "Address"}}),
                }[field]
                frame[key] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                native_entry_context(report)


if __name__ == "__main__":
    unittest.main()
