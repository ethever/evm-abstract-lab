"""Explicit transaction premises through HTTP, copied code and Pair source proof."""

import json
import re

from playwright.sync_api import expect
from web_test_decompile_guards import ANNOTATIONS, DEPENDENCIES, high, painted_copy, projection
from web_test_decompile_helpers import inspector_control, open_decompiler, prepare, stage, submit
from web_test_decompile_review import selected_row, source_descendant
from web_test_harness import click_control, resize_view, set_control, ui_control, webgpu_evidence
from web_test_registry import SOURCE_ROOT


FIXTURE = "crates/evm-decompile/tests/fixtures/error-payloads/pair-returndata-allocation-guard.hex"
PAIR = "crates/evm-decompile/tests/fixtures/structure/ethereum-uniswap-v2-usdc-weth-pair.hex"
MESSAGE = "UniswapV2: TRANSFER_FAILED"
DECLARATION = "// Execution scope: valid included Ethereum transactions and their contract calls."


def semantic_rows(text, current):
    blocks = {block["id"]: block for block in current["ir"]["blocks"]}
    tokens = set()
    for edge in current["ir"]["edges"]:
        target = blocks[edge["to"]]
        tokens.add(f"m{target['memory_parameter']} = m{edge['memory']}; // memory token on e{edge['id']}")
        tokens.add(f"q{target['effect_parameter']} = q{edge['effect']}; // effect token on e{edge['id']}")
    return [re.split(r" // (?:memory|effect)", line, maxsplit=1)[0]
            for line in text.splitlines() if line.lstrip() not in tokens
            and not re.match(r"^\s*// (?:sequence ast|b\d+\b|block b\d+\b|on e\d+\b)", line)]


def reject_mismatched_result(context, page, canvas, artifact, text, code, mutation):
    received = []

    def corrupt(route):
        response = route.fetch()
        envelope = response.json()
        changed = envelope["result"]["Ok"]["artifact"]
        if mutation == "proof_scope":
            changed = json.loads(json.dumps(artifact))
            envelope["result"]["Ok"]["artifact"] = changed
        current = stage(changed, "lifted")
        if mutation == "stage_scope":
            current["ir"].pop("execution_domain", None)
            current["ir"].pop("execution_fork", None)
        elif mutation == "stage_fork":
            current["ir"]["execution_fork"] = "cancun"
        elif mutation == "request_fork":
            changed["input"]["fork"] = "cancun"
            changed.pop("readability", None)
            for item in changed["stages"]:
                item["ir"]["execution_fork"] = "cancun"
                for proof in item["ir"].get("lifted", {}).get("reverts", []):
                    proof["execution_fork"] = "cancun"
        else:
            assert mutation in {"request_scope", "proof_scope"}
            changed["config"].pop("execution_domain", None)
            for item in changed["stages"]:
                item["ir"].pop("execution_domain", None)
                item["ir"].pop("execution_fork", None)
        received.append(envelope)
        (context.output / ("execution-scope-forged-" + mutation + ".json")).write_text(json.dumps(envelope, indent=2) + "\n")
        headers = {key: value for key, value in response.headers.items()
                   if key.lower() not in {"content-encoding", "content-length"}}
        route.fulfill(status=response.status, headers=headers, body=json.dumps(envelope))

    pattern = "**/api/decompile/tasks/*/result"
    page.route(pattern, corrupt)
    try:
        prepare(page, canvas, code)
        if mutation == "proof_scope":
            click_control(page, canvas, "Execution scope", role="combobox")
            click_control(page, canvas, "Generic EVM", role="tab")
        with page.expect_response(lambda response: response.request.method == "GET"
                                  and "/api/decompile/tasks/" in response.url
                                  and response.url.endswith("/result")):
            click_control(page, canvas, "Decompile", role="action")
        error = {"request_scope": "Execution scope mismatch", "request_fork": "Result fork mismatch"}.get(
            mutation, "Invalid decompiler artifact")
        expect(page.locator("#analysis-status")).to_contain_text(error)
        expect(page.locator("#analysis-status")).to_contain_text("Artifact: " + artifact["id"])
        expect(page.locator("#analysis-status")).to_contain_text("Execution scope: Ethereum valid transactions")
        assert len(received) == 1, "rejected scope metadata must not retry or replace the publication"
        high(page, canvas)
        assert painted_copy(page, canvas) == text
    finally:
        page.unroute(pattern, corrupt)


def run(context):
    code = (SOURCE_ROOT / FIXTURE).read_text().strip()
    pair = bytes.fromhex((SOURCE_ROOT / PAIR).read_text().strip())
    assert bytes.fromhex(code)[88:] == pair[8571:8673], "test the original Pair constructor bytes"
    page, canvas, _, _ = open_decompiler(context)
    resize_view(page, 1920, 2800)
    generic, request = submit(context, page, canvas, code, "pair-scope-generic", fork="Osaka")
    current, program = projection(generic)
    assert request.get("execution_domain", "generic") == "generic", "fork selection cannot silently narrow execution semantics"
    assert not any(payload["message"] == MESSAGE for payload in program.get("payloads", []))
    high(page, canvas)
    generic_text = painted_copy(page, canvas)
    assert DECLARATION not in generic_text
    assert "evm.revert(offset =" in generic_text and "evm.mstore(offset =" in generic_text

    bounded, request = submit(context, page, canvas, code, "pair-scope-ethereum", fork="Osaka",
                              execution_scope="Ethereum valid transactions")
    assert request["execution_domain"] == "ethereum_transactions"
    assert bounded["id"] != generic["id"]
    current, program = projection(bounded)
    payload, = [payload for payload in program["payloads"] if payload["message"] == MESSAGE]
    proof, = [proof for proof in program["reverts"] if proof["payload"] == payload["id"]]
    construction, = [plan for plan in program["constructions"] if plan["revert"] == proof["id"]]
    assert proof["execution_domain"] == "ethereum_transactions" and proof["execution_fork"] == "osaka"
    assert len(payload["bytes"]) == 100 and bytes(payload["bytes"])[68:94] == MESSAGE.encode()
    operations = {operation["id"]: operation for block in current["ir"]["blocks"] for operation in block["operations"]}
    assert operations[proof["operation"]]["source"]["pc"] == 189
    assert [operations[identity]["source"]["pc"] for identity in proof["writes"]] == [126, 133, 140, 178]
    high(page, canvas)
    reading = painted_copy(page, canvas)
    assert reading.startswith(DECLARATION + "\n")
    assert 'evm.revert("' not in reading, "standalone construction keeps original memory expansion and faults"
    assert "evm.call(gas =" in reading and "evm.returndatacopy(memory_offset =" in reading
    for dependencies, annotations in [(True, False), (True, True), (False, True), (False, False)]:
        set_control(page, canvas, DEPENDENCIES, dependencies)
        set_control(page, canvas, ANNOTATIONS, annotations)
        text = painted_copy(page, canvas)
        assert text.startswith(DECLARATION + "\n")
        assert semantic_rows(text, current) == reading.splitlines()
        assert text.count("evm.mstore(offset =") == reading.count("evm.mstore(offset =")
        assert text.count("evm.revert(offset =") == reading.count("evm.revert(offset =") == 1
    for entity, prefix in [(payload["id"], "Certified Error(string) payload h"),
                           (proof["id"], "Certified Error(string) REVERT h"),
                           (construction["id"], "Certified lazy failure construction h")]:
        source_descendant(page, canvas, bounded, current["id"], entity)
        ui_control(page, "IR", role="tab", selected=True)
        inspector_control(page, canvas, prefix + entity, "decompile-entity-detail")
    source_descendant(page, canvas, bounded, current["id"], payload["id"])
    inspector_control(page, canvas, 'Exact UTF-8 message: "' + MESSAGE + '"', "decompile-entity-detail")
    statement = next(statement for block in program["blocks"] for statement in block["statements"]
                     if statement["operation"] == proof["operation"])
    source_descendant(page, canvas, bounded, current["id"], statement["id"], pc=189)
    ui_control(page, "Simplified structured code", role="tab", selected=True)
    selected_row(page, "decompile-code", "evm.revert(offset =")
    assert painted_copy(page, canvas) == reading
    for mutation in ["stage_scope", "stage_fork", "request_scope", "request_fork", "proof_scope"]:
        reject_mismatched_result(context, page, canvas, bounded, reading, code, mutation)
    restored, request = submit(context, page, canvas, code, "pair-scope-generic-restored",
                               execution_scope="Generic EVM")
    assert request.get("execution_domain", "generic") == "generic"
    assert restored["id"] == generic["id"]
    high(page, canvas)
    assert painted_copy(page, canvas) == generic_text
    gpu = webgpu_evidence(page)
    page.close()
    return {"generic": generic["id"], "ethereum_transactions": bounded["id"],
            "payload": payload["id"], "construction": construction["id"],
            "terminal_source_pc": 189, "original_pair_terminal_pc": 8672, "webgpu": gpu}
