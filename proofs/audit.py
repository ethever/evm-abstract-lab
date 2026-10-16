#!/usr/bin/env python3
"""Audit checked target dependencies and the precise expected rejection."""

import hashlib
import json
from pathlib import Path
import re
import sys


def lean_code(text):
    """Strip nested comments and strings before scanning declaration keywords."""
    result = []
    index = 0
    depth = 0
    while index < len(text):
        if depth:
            if text.startswith("/-", index):
                depth += 1
                index += 2
            elif text.startswith("-/", index):
                depth -= 1
                index += 2
            else:
                result.append("\n" if text[index] == "\n" else " ")
                index += 1
        elif text.startswith("/-", index):
            depth = 1
            index += 2
        elif text.startswith("--", index):
            end = text.find("\n", index)
            index = len(text) if end == -1 else end
        elif text[index] == '"':
            index += 1
            while index < len(text) and text[index] != '"':
                index += 2 if text[index] == "\\" else 1
            index += 1
            result.append(" ")
        else:
            result.append(text[index])
            index += 1
    assert depth == 0, "unterminated comment"
    return "".join(result)


def audit(proof_root, output, rejected_exit):
    allowed_axioms = {"propext", "Quot.sound", "Classical.choice"}
    targets = {}
    sources = {}
    for source in sorted((proof_root / "NoSmt").glob("*.lean")):
        code = lean_code(source.read_text())
        forbidden = re.search(r"\b(sorry|admit|axiom|native_decide|trustCompiler|skipKernelTC)\b", code)
        assert forbidden is None, f"forbidden declaration/tactic in {source}: {forbidden}"
        namespace = re.search(r"^namespace (\S+)$", code, re.MULTILINE).group(1)
        declarations = set(re.findall(r"^theorem (\w+)\b", code, re.MULTILINE))
        prints = set(re.findall(r"^#print axioms (\w+)$", code, re.MULTILINE))
        assert declarations and declarations == prints, f"incomplete target audit: {source}"
        log = (output / f"{source.stem}.log").read_text()
        assert "sorryAx" not in log, f"unproved target: {source}"
        lines = re.findall(
            r"^'([^']+)' (?:does not depend on any axioms|depends on axioms: \[([^\]]*)\])$",
            log,
            re.MULTILINE,
        )
        checked = {}
        for name, dependencies in lines:
            axioms = sorted(filter(None, (item.strip() for item in dependencies.split(","))))
            assert set(axioms) <= allowed_axioms, f"unexpected target axiom: {name}: {axioms}"
            assert name not in targets, f"duplicate audit output: {name}"
            checked[name] = axioms
        expected = {f"{namespace}.{name}" for name in declarations}
        assert set(checked) == expected, f"checker output missing target dependencies: {source}"
        targets.update(checked)
        sources[str(source.relative_to(proof_root))] = hashlib.sha256(source.read_bytes()).hexdigest()

    rejected = (output / "OldJoinRejected.log").read_text()
    assert rejected_exit == 1, f"unexpected negative-check exit: {rejected_exit}"
    assert rejected.count("error:") == 1 and re.search(
        r"error: Tactic `decide` proved that the proposition\s+0 ∈ \[1\]\s+is false\s*$", rejected
    ), (
        "expected the false old-join proposition, not an import/checker error", rejected
    )
    manifest = {
        "sources_sha256": sources,
        "target_axioms": targets,
        "used_core_axioms": sorted({axiom for axioms in targets.values() for axiom in axioms}),
        "expected_rejection": {
            "file": "counterexamples/OldJoinRejected.lean",
            "exit": rejected_exit,
            "source_sha256": hashlib.sha256(
                (proof_root / "counterexamples/OldJoinRejected.lean").read_bytes()
            ).hexdigest(),
        },
        "harness_sha256": {
            name: hashlib.sha256((proof_root / name).read_bytes()).hexdigest()
            for name in ["check.sh", "audit.py", "lean-toolchain"]
        },
    }
    (output / "audit.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"checked {len(targets)} public targets; old join rejected with exit {rejected_exit}")
    print("core axioms: " + ", ".join(manifest["used_core_axioms"]))


if __name__ == "__main__":
    assert len(sys.argv) == 4, "usage: audit.py PROOF_ROOT OUTPUT_DIRECTORY NEGATIVE_EXIT"
    audit(Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3]))
