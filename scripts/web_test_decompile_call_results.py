"""Named call results, prepared forwarding and original witnesses in a browser."""

import re

from web_test_decompile_alignment import copy_code
from web_test_decompile_guards import (ANNOTATIONS, DEPENDENCIES, high,
                                       painted_copy, projection)
from web_test_decompile_helpers import (choose_stage, inspector_control,
                                       open_decompiler, submit)
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import click_control, resize_view, set_control, webgpu_evidence
from web_test_registry import SOURCE_ROOT


FIXTURES = "crates/evm-decompile/tests/fixtures/call-results/"
CASES = [("delegate-precopy-return", 1, 2),
         ("delegate-repeated-sizes", 1, 3),
         ("delegate-nested-buffers", 2, 4)]


def semantic_lines(text):
    return [line.split(" // memory", 1)[0].split(" // effect", 1)[0]
            for line in text.splitlines()
            if not line.lstrip().startswith("//") and " token on e" not in line]


def witness(page, canvas, artifact, current, operation, entity=None):
    assert operation["source"], operation
    source_descendant(page, canvas, artifact, current["id"],
                      operation["id"] if entity is None else entity,
                      pc=operation["source"]["pc"])


def reader_calls(current, program, operations):
    """Check the producer's typed provenance, without recreating an epoch analysis."""
    values = {value["id"]: value for value in current["ir"]["values"]}
    owners = {}
    for data in program["revert_data"]:
        segment, = data["content"]["segments"]
        call = segment["slice"]["source"]["return_data"]
        for value in [data["size"], *(operations[writer]["inputs"][2] for writer in data["writes"]
                                       if operations[writer]["opcode"] == 0x3e)]:
            definition = values[value]["kind"]["operation"]
            assert definition["index"] == 0 and operations[definition["operation"]]["opcode"] == 0x3d
            previous = owners.setdefault(definition["operation"], call)
            assert previous == call
    return owners


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 3000)
    reports = []
    posts, publications = [], []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and "/api/" in request.url else None)
    page.on("response", lambda response: publications.append(response.url)
            if "/api/decompile/" in response.url and response.url.endswith("/result") else None)
    for name, call_count, size_count in CASES:
        print(f"call-results fixture start: {name}", flush=True)
        bytecode = (SOURCE_ROOT / (FIXTURES + name + ".hex")).read_text().strip()
        artifact, _ = submit(context, page, canvas, bytecode, name, procedures="Inline")
        current, program = projection(artifact)
        result_url, admitted = publications[-1], len(posts)
        operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                      for operation in block["operations"]}
        statements = {statement["operation"]: statement for block in program["blocks"]
                      for statement in block["statements"]}
        calls = sorted((op for op in operations.values() if op["opcode"] == 0xf4),
                       key=lambda op: op["source"]["pc"])
        readers = sorted((op for op in operations.values() if op["opcode"] == 0x3d),
                         key=lambda op: op["source"]["pc"])
        copies = sorted((op for op in operations.values() if op["opcode"] == 0x3e),
                        key=lambda op: op["source"]["pc"])
        terminals = [op for op in operations.values() if op["opcode"] in (0xf3, 0xfd)]
        assert len(calls) == call_count and len(readers) == size_count and len(copies) == call_count
        assert len(program["revert_data"]) == call_count
        plans = [plan for plan in program.get("constructions", [])
                 if plan.get("kind", "constructor") == "prepared_consumer"]
        assert len(plans) == call_count, program
        for plan in plans:
            body = next(block for block in program["blocks"] if block["block"] == plan["block"])
            consumed = body["statements"][plan["start"]:plan["end"]]
            opcodes = [operations[statement["operation"]]["opcode"] for statement in consumed]
            assert 0x3d in opcodes and opcodes[-1] == 0xfd and 0x3e not in opcodes

        high(page, canvas)
        text = painted_copy(page, canvas)
        require_rows = [line for line in text.splitlines() if line.lstrip().startswith("require(")]
        assert len(require_rows) == call_count and "evm.revert(" not in text, text
        assert text.count("evm.returndatacopy(") == call_count and "evm.return(" in text, text
        assert "evm.returndatasize(" not in text and "returndata(op" not in text, text
        words = set(re.findall(r"\bv\d+\b", "\n".join(semantic_lines(text))))
        for reader in readers:
            assert all("v" + output not in words for output in reader["outputs"]), text
        previous = -1
        for ordinal, (call, copied, required) in enumerate(zip(calls, copies, require_rows), 1):
            declaration = f"(success{ordinal}, returndata{ordinal}) = evm.delegatecall("
            assert text.count(declaration) == 1 and f"returndata{ordinal}.size" in text, text
            assert f"success{ordinal}" in required and f"returndata{ordinal}" in required, required
            at_call = text.index(declaration)
            at_copy = text.index("evm.returndatacopy(", at_call)
            at_guard = text.index(required)
            assert previous < at_call < at_copy < at_guard, text
            previous = at_guard
            assert not required.startswith(" "), required
            row = next(line for line in text.splitlines() if "evm.returndatacopy(" in line
                       and f"returndata{ordinal}.size" in line)
            assert not row.startswith(" "), row
            witness(page, canvas, artifact, current, call, statements[call["id"]]["id"])
            selected_row(page, "decompile-code", declaration)
            inspector_control(page, canvas, "Call result #" + str(ordinal), "decompile-entity-detail")
            witness(page, canvas, artifact, current, copied, statements[copied["id"]]["id"])
            selected_row(page, "decompile-code", "evm.returndatacopy(")
        assert previous < text.index("evm.return("), text
        for dependencies, annotations in [(True, False), (False, True), (True, True), (False, False)]:
            set_control(page, canvas, DEPENDENCIES, dependencies)
            set_control(page, canvas, ANNOTATIONS, annotations)
            assert semantic_lines(painted_copy(page, canvas)) == semantic_lines(text)

        owners = reader_calls(current, program, operations)
        for reader in readers:
            call = owners.get(reader["id"])
            if call is None:
                assert call_count == 1, reader
                call = calls[0]["id"]
            witness(page, canvas, artifact, current, reader, statements[reader["id"]]["id"])
            inspector_control(page, canvas, "Returndata source: o" + call, "decompile-entity-detail")
            set_control(page, canvas, "Entity references", True, role="section")
            inspector_control(page, canvas, f"Reference s{current['id']}/e{call} ",
                              "decompile-entity-link")
        for terminal in terminals:
            witness(page, canvas, artifact, current, terminal)
        click_control(page, canvas, "Structured code", role="tab")
        raw = copy_code(page, canvas)
        assert raw.count("evm.delegatecall(") == call_count
        assert raw.count("evm.returndatasize(") == size_count
        assert raw.count("evm.returndatacopy(") == call_count
        assert raw.count("evm.revert(") == call_count and raw.count("evm.return(") == 1
        assert "returndata1.size" not in raw and "success1" not in raw and "require(" not in raw
        choose_stage(page, canvas, artifact, "structured")
        click_control(page, canvas, "Structured code", role="tab")
        assert copy_code(page, canvas) == raw
        choose_stage(page, canvas, artifact, "lifted")
        high(page, canvas)
        assert painted_copy(page, canvas) == text
        retained = page.request.get(result_url)
        assert retained.ok and retained.json()["result"]["Ok"]["artifact"] == artifact
        assert len(posts) == admitted
        reports.append({"case": name, "artifact": artifact["id"], "text": text, "raw": raw})
        print(f"call-results fixture complete: {name}", flush=True)
    result = {"fixtures": reports, "renderer": webgpu_evidence(page)}
    page.close()
    return result
