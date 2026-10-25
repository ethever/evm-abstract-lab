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

    def report(self):
        # Equal local IDs deliberately name different Native/Global data. Source
        # IDs are sparse and shared, so array-position lookups cannot pass.
        return {
            "schema_version": 21, "selected_view": "Baseline", "status": "Converged",
            "hybrid": {"coverage": {"baseline_current": True}},
            "programs": [{"id": 91, "blocks": [{"id": 12, "instructions": [{"pc": 40}]}]},
                         {"id": 7, "blocks": [{"id": 3, "instructions": [{"pc": 5}]}]}],
            "global": {"status": "Converged", "metadata": {"root_program": 91},
                       "states": [{"state": 4, "entry": "global"}]},
            "native": {"status": "Incomplete", "metadata": {"root_program": 7},
                       "states": [{"state": 4, "entry": "native"}]},
        }

    def publication(self):
        return {"version": "9007199254740997", "global_version": "9007199254740993",
                "native_version": "9007199254740997", "model_closed": True}

    def test_graph_selection_borrows_sources_without_combining_namespaces(self):
        from web_test_hybrid import displayed_graph, graph, oracle_graph, source_block
        report = self.report()
        self.assertIs(graph(report, "Baseline"), report["global"])
        self.assertIs(graph(report, "Native"), report["native"])
        native = oracle_graph(report, "Native")
        self.assertIs(native["states"], report["native"]["states"])
        self.assertIs(native["programs"], report["programs"])
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


if __name__ == "__main__":
    unittest.main()
