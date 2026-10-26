"""Independent small controls for the report checker, with no browser imports."""
from copy import deepcopy
import unittest

from web_test_hybrid import oracle_graph
from web_test_runtime_report import _runtime_account_code, _runtime_numeric_contains, layout_runtime_programs


def instruction(pc, opcode, name, immediate, size=1, valid=True):
    return dict(pc=pc, opcode=opcode, name=name, immediate=immediate, size=size, valid=valid)


def source_fixture():
    # Includes two unreachable blocks, an invalid opcode and a PUSH immediate.
    code = "610009575b600000fe5b00"
    blocks = [
        dict(id=0, start_pc=0, instructions=[instruction(0, 0x61, "PUSH2", "0x9", 3), instruction(3, 0x57, "JUMPI", None)]),
        dict(id=1, start_pc=4, instructions=[instruction(4, 0x5b, "JUMPDEST", None), instruction(5, 0x60, "PUSH1", "0x0", 2), instruction(7, 0, "STOP", None)]),
        dict(id=2, start_pc=8, instructions=[instruction(8, 0xfe, "INVALID", None, valid=False)]),
        dict(id=3, start_pc=9, instructions=[instruction(9, 0x5b, "JUMPDEST", None), instruction(10, 0, "STOP", None)]),
    ]
    metadata = dict(browser_entry=dict(fork="Osaka"), code_hash="captured-source", byte_length=11)
    report = dict(fork="Osaka", scope="SingleProgram", byte_len=11,
                  metadata=dict(snapshot=None, root_program=7),
                  programs=[dict(id=7, fork="Osaka", kind="Runtime", code_hash="captured-source", bytecode="0x" + code, blocks=blocks)])
    programs = report.pop("programs")
    envelope = {"schema_version": 21, "selected_view": "Native", "native": report, "global": None,
                "programs": programs, "process_controls": []}
    return envelope, code, metadata


class RuntimeReportTests(unittest.TestCase):
    def test_visited_store_account_code_uses_exact_wire_variants(self):
        for code in [None, "Empty", "Unknown", {"Runtime": "0x00"}, {"Runtime": "0x"},
                     {"Runtime": "0xA0ff"}, {"Delegation": "0x" + "12" * 20}]:
            with self.subTest(code=code):
                self.assertTrue(_runtime_account_code(code))
        for code in [False, "Runtime", [], {}, {"Runtime": "0x0"}, {"Runtime": "0xgg"},
                     {"Runtime": False}, {"Delegation": "0x" + "12" * 19},
                     {"Delegation": "0x" + "12" * 21}, {"Code": "0x00"},
                     {"Runtime": "0x00", "Delegation": "0x" + "12" * 20}]:
            with self.subTest(code=code):
                self.assertFalse(_runtime_account_code(code))

    def test_complete_catalog_and_independent_corruption_controls(self):
        report, code, metadata = source_fixture()
        layout_runtime_programs(oracle_graph(report, "Native"), code, metadata)
        for field, value in [("pc", 1), ("opcode", 0), ("immediate", "0x8"), ("size", 2), ("valid", False)]:
            damaged = deepcopy(report)
            damaged["programs"][0]["blocks"][0]["instructions"][0][field] = value
            # The source view borrows the one catalog; independent byte decoding
            # must reject corruption even though every reference sees that row.
            with self.subTest(field=field), self.assertRaises(AssertionError):
                layout_runtime_programs(oracle_graph(damaged, "Native"), code, metadata)
        damaged = deepcopy(report)
        damaged["programs"][0]["blocks"].pop()
        with self.assertRaises(AssertionError):
            layout_runtime_programs(oracle_graph(damaged, "Native"), code, metadata)

    def test_source_fork_is_qualified_by_the_same_graph(self):
        report, code, metadata = source_fixture()
        report["programs"][0]["fork"] = "Prague"
        with self.assertRaises(AssertionError):
            layout_runtime_programs(oracle_graph(report, "Native"), code, metadata)

    def test_numeric_membership_checks_every_reported_constraint(self):
        value = dict(constants=["0x2"], known_zero="0x1", known_one="0x2",
                     unsigned=dict(lower="0x0", upper="0x4"),
                     signed=dict(lower=hex(1 << 255), upper=hex((1 << 255) - 1)),
                     nonzero=True, congruence={"Modulo": dict(modulus="0x2", residue="0x0")})
        self.assertTrue(_runtime_numeric_contains(value, 2))
        for actual in [0, 1, 3, 4, -1, 1 << 256]:
            self.assertFalse(_runtime_numeric_contains(value, actual))
        for field, replacement in [
            ("constants", ["0x3"]), ("known_zero", "0x2"), ("known_one", "0x4"),
            ("unsigned", dict(lower="0x3", upper="0x4")),
            ("signed", dict(lower="0x3", upper="0x4")),
            ("congruence", {"Exact": "0x3"}),
        ]:
            with self.subTest(field=field):
                self.assertFalse(_runtime_numeric_contains({**value, field: replacement}, 2))


if __name__ == "__main__":
    unittest.main()
