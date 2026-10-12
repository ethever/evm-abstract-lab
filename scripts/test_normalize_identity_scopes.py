#!/usr/bin/env python3
"""Regressions for whole-report namespace normalization and ordered comparison."""

import argparse
import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest

from normalize_identity_scopes import canonical_json


def graph(scope):
    return {
        "identity_scope": scope,
        "root": 4,
        "nodes": [
            {"Fresh": 18446744073709551615},
            {"Input": {"scope": 9007199254740993, "symbol": "CallValue"}},
            {"Constant": "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"},
            {"BytesInput": {"source": {"token": 0, "namespace": "0xbeef"},
                            "length": 2, "bindings": []}},
            {"View": {"state": 3, "offset": 2, "length": 2}},
        ],
    }


def report(scopes):
    return {"graphs": [graph(scope) for scope in scopes],
            "path": {"condition": 9007199254740993, "nonzero": True},
            "numeric": 2**256 - 1, "product": [1, 2],
            "provenance": {"source": 18446744073709551615},
            "status": "Incomplete", "frontiers": [{"reason": {"MissingCode": "0x01"}}]}


def canonical(value):
    return canonical_json(json.dumps(value))


class NamespaceTests(unittest.TestCase):
    def test_scope_reuse_across_separate_graphs_preserves_all_bindings(self):
        original = report(["a", "a"])
        normalized = json.loads(canonical(original))
        self.assertEqual(normalized["graphs"][0]["identity_scope"], "producer-0")
        self.assertEqual(normalized["graphs"][1]["identity_scope"], "producer-0")
        for before, after in zip(original["graphs"], normalized["graphs"]):
            self.assertEqual(before["nodes"], after["nodes"])
            self.assertEqual(before["root"], after["root"])
        self.assertEqual(canonical(original), canonical(report(["x", "x"])))

    def test_bijection_and_first_occurrence_span_the_complete_report(self):
        original = report(["a", "b", "a", "b"])
        self.assertEqual(canonical(original), canonical(report(["b", "a", "b", "a"])))
        normalized = json.loads(canonical(original))
        self.assertEqual([g["identity_scope"] for g in normalized["graphs"]],
                         ["producer-0", "producer-1", "producer-0", "producer-1"])
        self.assertEqual(canonical(report(["b"])), canonical(report(["a"])))
        self.assertEqual(canonical(report(["a", "producer-0"])),
                         canonical(report(["x", "y"])))
        nested = {"world": {"storage_source": graph("a")},
                  "states": [graph("b")], "summaries": [graph("a"), graph("b")]}
        normalized = json.loads(canonical(nested))
        self.assertEqual(normalized["world"]["storage_source"]["identity_scope"],
                         normalized["summaries"][0]["identity_scope"])
        self.assertEqual(normalized["states"][0]["identity_scope"],
                         normalized["summaries"][1]["identity_scope"])
        self.assertNotEqual(normalized["states"][0]["identity_scope"],
                            normalized["summaries"][0]["identity_scope"])

    def test_changed_alias_partition_and_partial_swap_fail(self):
        original = canonical(report(["a", "b", "a", "b"]))
        for scopes in (["x", "x", "x", "x"], ["x", "y", "z", "y"],
                       ["a", "b", "b", "a"]):
            with self.subTest(scopes=scopes):
                self.assertNotEqual(original, canonical(report(scopes)))
        self.assertNotEqual(canonical(report(["a", "a"])),
                            canonical(report(["x", "y"])))

    def test_changes_to_preserved_semantics_fail(self):
        original = report(["a", "a"])
        changes = [
            (["graphs", 1, "nodes", 0, "Fresh"], 18446744073709551614),
            (["graphs", 1, "nodes", 1, "Input", "scope"], 9007199254740992),
            (["graphs", 1, "nodes", 1, "Input", "symbol"], "Caller"),
            (["graphs", 1, "nodes", 2, "Constant"], "0xff"),
            (["graphs", 1, "nodes", 3, "BytesInput", "source", "namespace"], "0xbeee"),
            (["graphs", 1, "nodes", 3, "BytesInput", "source", "token"], 1),
            (["graphs", 1, "root"], 3),
            (["path", "condition"], 9007199254740992),
            (["numeric"], 2**256 - 2),
            (["product", 0], 0),
            (["provenance", "source"], 18446744073709551614),
            (["status"], "Converged"),
            (["frontiers", 0, "reason", "MissingCode"], "0x02"),
        ]
        for path, replacement in changes:
            with self.subTest(path=path):
                changed = copy.deepcopy(original)
                target = changed
                for key in path[:-1]:
                    target = target[key]
                target[path[-1]] = replacement
                self.assertNotEqual(canonical(original), canonical(changed))

    def test_unrelated_identity_scope_fields_are_preserved(self):
        original = {"identity_scope": "metadata", "nested": [
            {"identity_scope": "input", "nodes": []},
            dict(graph("decorated"), role="metadata"),
            graph("a"),
        ]}
        normalized = json.loads(canonical(original))
        self.assertEqual(normalized["identity_scope"], "metadata")
        self.assertEqual(normalized["nested"][:2], original["nested"][:2])
        self.assertEqual(normalized["nested"][2]["identity_scope"], "producer-0")
        changed = copy.deepcopy(original)
        changed["identity_scope"] = "changed metadata"
        self.assertNotEqual(canonical(original), canonical(changed))

    def test_object_and_array_order_remain_significant(self):
        original = report(["a"])
        self.assertNotEqual(canonical(original), canonical(dict(reversed(list(original.items())))))
        changed = copy.deepcopy(original)
        changed["graphs"][0] = dict(reversed(list(changed["graphs"][0].items())))
        self.assertNotEqual(canonical(original), canonical(changed))
        changed = copy.deepcopy(original)
        changed["product"].reverse()
        self.assertNotEqual(canonical(original), canonical(changed))
        changed = copy.deepcopy(original)
        changed["graphs"][0]["nodes"].reverse()
        self.assertNotEqual(canonical(original), canonical(changed))
        self.assertEqual(canonical(original), canonical_json(json.dumps(original, indent=4)))

    def test_number_lexemes_never_pass_through_float(self):
        numbers = ("9007199254740993,18446744073709551615," + str(2**256 - 1)
                   + ",1.00000000000000001,1e400,-0")
        self.assertEqual(canonical_json("[ " + numbers + " ]"), "[" + numbers + "]")
        self.assertEqual(json.loads(canonical(report(["a"])))["numeric"], 2**256 - 1)

    def test_malformed_native_graphs_are_rejected(self):
        for key, invalid in [("identity_scope", None), ("identity_scope", 123),
                             ("identity_scope", ""), ("root", True), ("root", -1),
                             ("root", 5), ("root", 0.0), ("nodes", []),
                             ("nodes", [{"Unknown": 1}]), ("nodes", "metadata")]:
            with self.subTest(key=key, invalid=invalid):
                malformed = graph("a")
                malformed[key] = invalid
                if key == "nodes":
                    malformed["root"] = 0
                with self.assertRaises(ValueError):
                    canonical(malformed)

    def test_duplicate_keys_and_non_json_numbers_are_rejected(self):
        for text in ('{"Fresh":1,"Fresh":2}', '[NaN]', '[Infinity]', '[-Infinity]'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                canonical_json(text)

    def test_cli_uses_the_same_complete_report_contract(self):
        helper = Path(__file__).with_name("normalize_identity_scopes.py")
        text = json.dumps(report(["a", "b", "a"]))
        result = subprocess.run([sys.executable, str(helper)], input=text,
                                text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, canonical_json(text) + "\n")
        result = subprocess.run([sys.executable, str(helper)], input='{"a":NaN}',
                                text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertIn("invalid JSON numeric constant", result.stderr)


def actual_reports(first, second):
    original = [path.read_text() for path in (first, second)]
    if original[0] == original[1]:
        raise AssertionError("two-process regression must first differ without normalization")
    if canonical_json(original[0]) != canonical_json(original[1]):
        raise AssertionError("actual two-process reports differ after scope normalization")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reports", nargs=2, type=Path,
                        help="also compare two actual independent-process reports")
    arguments = parser.parse_args()
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(NamespaceTests)
    if arguments.reports:
        suite.addTest(unittest.FunctionTestCase(lambda: actual_reports(*arguments.reports),
                                               description="actual two-process reports"))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    sys.exit(not result.wasSuccessful())
