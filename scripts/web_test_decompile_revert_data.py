"""Occurrence-local REVERT data survives HTTP, Wasm, source links and copying."""

from web_test_decompile_alignment import copy_code
from web_test_decompile_guards import (ANNOTATIONS, DEPENDENCIES, high,
                                       painted_copy, projection)
from web_test_decompile_helpers import (choose_stage, inspector_control,
                                       open_decompiler, submit)
from web_test_decompile_review import source_descendant
from web_test_harness import click_control, resize_view, set_control, webgpu_evidence
from web_test_registry import SOURCE_ROOT


FIXTURE_ROOT = "crates/evm-decompile/tests/fixtures/revert-data/"
FIXTURES = ["panic-41-guard", "custom-symbolic-failed-arm", "custom-symbolic-bound-guard"]


def semantic_lines(text):
    return [line.split(" // memory", 1)[0].split(" // effect", 1)[0]
            for line in text.splitlines()
            if not line.lstrip().startswith("//") and " token on e" not in line]


def source_words(content):
    """Read the typed wire representation; never derive data from code spelling."""
    if isinstance(content, list):
        return [word for item in content for word in source_words(item)]
    if isinstance(content, dict):
        return ([content["source_value"]] if "source_value" in content else []) + [
            word for key, item in content.items() if key != "source_value"
            for word in source_words(item)]
    return []


def original_construction(page, canvas, artifact, current, data, high_text):
    # A compact display is never the owner of the original memory constructor.
    writes = set(data["writes"])
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    assert writes and writes <= operations.keys(), data
    assert operations[data["operation"]]["opcode"] == 0xfd, data
    assert all(operations[operation]["opcode"] in (0x52, 0x53, 0x37, 0x39, 0x3e, 0x5e)
               for operation in writes), data
    click_control(page, canvas, "Structured code", role="tab")
    raw = copy_code(page, canvas)
    assert "evm.revert(" in raw and "evm.mstore(" in raw, raw
    assert "Panic(" not in raw and "Revert(selector =" not in raw, raw
    for operation in [next(iter(data["writes"])), data["operation"]]:
        source_descendant(page, canvas, artifact, current["id"], operation)
    click_control(page, canvas, "Structured code", role="tab")
    assert copy_code(page, canvas) == raw
    choose_stage(page, canvas, artifact, "structured")
    click_control(page, canvas, "Structured code", role="tab")
    assert copy_code(page, canvas) == raw
    choose_stage(page, canvas, artifact, "lifted")
    high(page, canvas)
    assert painted_copy(page, canvas) == high_text
    return raw


def data_navigation(page, canvas, artifact, current, program, data, expected):
    operations = {operation["id"]: operation for block in current["ir"]["blocks"]
                  for operation in block["operations"]}
    terminal = operations[data["operation"]]
    assert terminal["opcode"] == 0xfd and terminal["source"], terminal
    terminal_pc = terminal["source"]["pc"]
    # The helper also checks that this precise REVERT PC belongs to the data's
    # original source spans; a block/memory ancestor's first PC is too broad.
    source_descendant(page, canvas, artifact, current["id"], data["id"], pc=terminal_pc)
    inspector_control(page, canvas, "Recovered REVERT data h" + data["id"],
                      "decompile-entity-detail")
    inspector_control(page, canvas, "Rendered data: " + expected,
                      "decompile-entity-detail")
    for source in [data["operation"], data["memory"], *source_words(data["content"])]:
        source_descendant(page, canvas, artifact, current["id"], data["id"], pc=terminal_pc)
        set_control(page, canvas, "Entity references", True, role="section")
        inspector_control(page, canvas, f"Reference s{current['id']}/e{source} ",
                          "decompile-entity-link")
    guards = [control for control in program["controls"]
              if control.get("guard", {}).get("payload") == {"data": data["id"]}]
    for control in guards:
        source_descendant(page, canvas, artifact, current["id"], control["id"])
        inspector_control(page, canvas, "Lazy failure construction retains original memory expansion",
                          "decompile-entity-detail")
        set_control(page, canvas, "Entity references", True, role="section")
        inspector_control(page, canvas, f"Reference s{current['id']}/e{data['id']} ",
                          "decompile-entity-link")
        inspector_control(page, canvas, "Recovered REVERT data h" + data["id"],
                          "decompile-entity-detail")


def run(context):
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 3000)
    reports = []
    posts, publications = [], []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and "/api/" in request.url else None)
    page.on("response", lambda response: publications.append(response.url)
            if "/api/decompile/" in response.url and response.url.endswith("/result") else None)
    for name in FIXTURES:
        print(f"revert-data fixture start: {name}", flush=True)
        bytecode = (SOURCE_ROOT / (FIXTURE_ROOT + name + ".hex")).read_text().strip()
        artifact, _ = submit(context, page, canvas, bytecode, name)
        current, program = projection(artifact)
        data, = program["revert_data"]
        assert data["block"] in {block["id"] for block in current["ir"]["blocks"]}
        admitted, result_url = len(posts), publications[-1]
        high(page, canvas)
        text = painted_copy(page, canvas)
        if name == "panic-41-guard":
            expected = "Panic(0x41)"
            assert "require(" in text and expected in text and "// 内存分配过大" in text, text
            assert not source_words(data["content"]), data
        else:
            expected = "Revert(selector = 0xdeadbeef, words = ["
            assert expected in text, text
            assert len(source_words(data["content"])) == 3, data
            if name == "custom-symbolic-bound-guard":
                assert "require(" in text and text.count("evm.calldataload(") == 4, text
                assert text.rindex("evm.calldataload(") < text.index("require("), text
            else:
                assert "require(" not in text and "if (" in text, text
                assert text.count("evm.calldataload(") == 4, text
                assert text.count("evm.mstore(") == 4, text
                assert "evm.revert(Revert(" in text, text
                failure_loads = [line for line in text.splitlines()
                                 if "evm.calldataload(" in line and line.startswith("    ")]
                assert len(failure_loads) == 3, text
                assert text.rindex("evm.revert(") < text.index("evm.sstore("), text
        if name != "custom-symbolic-failed-arm":
            assert "evm.mstore(" not in text and "evm.mstore8(" not in text, text
        for dependencies, annotations in [(True, False), (False, True), (True, True), (False, False)]:
            set_control(page, canvas, DEPENDENCIES, dependencies)
            set_control(page, canvas, ANNOTATIONS, annotations)
            assert semantic_lines(painted_copy(page, canvas)) == semantic_lines(text)
        data_navigation(page, canvas, artifact, current, program, data, expected)
        raw = original_construction(page, canvas, artifact, current, data, text)
        retained = page.request.get(result_url)
        assert retained.ok and retained.json()["result"]["Ok"]["artifact"] == artifact
        assert len(posts) == admitted, posts
        reports.append({"case": name, "artifact": artifact["id"], "data": data,
                        "text": text, "raw": raw})
        print(f"revert-data fixture complete: {name}", flush=True)
    result = {"fixtures": reports, "renderer": webgpu_evidence(page)}
    page.close()
    return result
