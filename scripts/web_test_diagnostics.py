"""Diagnostics browser acceptance cases."""

from pathlib import Path
from tempfile import TemporaryDirectory
from playwright.sync_api import expect
import json
from web_test_graph_helpers import symbol
from web_test_harness import analysis_server, complete_submission, observe_webgpu, open_input, posted_bytecode, rendered_frame, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import inspector_diagnostics_region, select_inspector_tab
from web_test_layout_helpers import layout_modal
from web_test_pixels import text_pixels
from web_test_rpc_helpers import EntryRpcFixture, automatic_rpc_entry, submit_rpc


def diagnostics_interactions(browser, url: str, args):
    """Real native receipts survive HTTP/WASM and retain independent source jumps."""
    output = args.output
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors, posts = [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    complete_submission(page, initial.value)
    canvas, status = page.locator("#evm-canvas"), page.locator("#analysis-status")
    select_inspector_tab(page, canvas, "Diagnostics")

    def submit(code, fork, name):
        open_input(page)
        page.keyboard.press("Control+A")
        page.keyboard.insert_text(code)
        rendered_frame(page)
        modal = layout_modal(page, canvas)
        # The actual fork selector sits beside Bytecode and RPC account.
        canvas.click(position={"x": modal[0] + 220, "y": modal[1] + 44})
        settle_gesture(page)
        canvas.click(position={"x": modal[0] + 220,
                               "y": modal[1] + 70 + {"Cancun": 0, "Prague": 1, "Osaka": 2}[fork] * 23})
        settle_gesture(page)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                  and response.request.method == "POST") as submitted:
            page.keyboard.press("Control+Enter")
        reply = complete_submission(page, submitted.value)
        request = reply.request.post_data_json
        assert request["fork"] == fork and posted_bytecode(reply) == code, request
        assert reply.status == 200 and "Ok" in reply.json()["result"], reply.json()
        report = reply.json()["result"]["Ok"]
        assert report["schema_version"] == 13 and report["status"] == "Converged", report
        (output / f"diagnostics-{name}-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
        return report

    def snapshot(name, minimum_rows):
        page.mouse.move(2, 2)
        settle_gesture(page)
        png = canvas.screenshot(path=str(output / f"diagnostics-{name}.png"))
        # Measure the painted summary, state/opcode receipt and explanatory cause.
        pixels = text_pixels(page, png, inspector_diagnostics_region(page, canvas))
        assert len(pixels["rows"]) >= minimum_rows and pixels["white_pixels"] > 50, pixels
        return pixels

    result = {}
    for name, code, fork, opcode, mnemonic, reason, cause in [
        ("explicit", "fe", "Osaka", 0xfe, "INVALID", "ExplicitInvalid", "Explicit exceptional halt instruction"),
        ("undefined", "ef", "Osaka", 0xef, None, "UndefinedOpcode", "Undefined ordinary bytecode opcode"),
        ("clz-cancun", "5f1e00", "Cancun", 0x1e, "CLZ", {"UnavailableInFork": "Cancun"}, "Instruction is not enabled in fork Cancun"),
    ]:
        report = submit(code, fork, name)
        diagnostics = [item for item in report["diagnostics"] if item["kind"] == "InvalidOpcode"]
        assert len(diagnostics) == 1, diagnostics
        diagnostic = diagnostics[0]
        assert diagnostic["invalid_opcode"] == {"Instruction": {"opcode": opcode, "mnemonic": mnemonic, "reason": reason}}, diagnostic
        label = f"{symbol('σ', diagnostic['state'])} · PC 0x{diagnostic['pc']:04x} · InvalidOpcode · 0x{opcode:02x}"
        if mnemonic is not None:
            label += f" ({mnemonic})"
        expect(status).to_contain_text(label)
        expect(status).to_contain_text(cause)
        expect(status).to_contain_text("counts are abstract-state entries, not instructions or transactions")
        if mnemonic is None:
            assert "0xef (INVALID)" not in status.text_content()
        pixels = snapshot(name, 3)
        # Receipt rows remain real links to the native state/source location.
        row = pixels["rows"][1]
        submissions = len(posts)
        canvas.click(position={"x": 120, "y": sum(row) / 2})
        expect(status).to_contain_text(f"Selected {symbol('σ', diagnostic['state'])};")
        assert len(posts) == submissions, "diagnostic selection submitted another analysis"
        result[name] = {"diagnostic": diagnostic, "painted": pixels}
    assert len({case["painted"]["hash"] for case in result.values()}) == 3

    for name, code in [("clz-osaka", "5f1e00"), ("unreachable", "00fe")]:
        report = submit(code, "Osaka", name)
        assert not any(item["kind"] == "InvalidOpcode" for item in report["diagnostics"]), report["diagnostics"]
        expect(status).to_contain_text("0 abstract-state diagnostic entries")
        if name == "unreachable":
            assert any(instruction["opcode"] == 0xfe for block in report["disassembly"] for instruction in block["instructions"])
        result[name] = {"status": report["status"], "diagnostics": report["diagnostics"]}

    repeated = submit("5f35600a5760016010565b60026010565bfe", "Osaka", "repeated-pc")
    receipts = [item for item in repeated["diagnostics"] if item["kind"] == "InvalidOpcode"]
    assert len(receipts) == 2 and len({item["state"] for item in receipts}) == 2, receipts
    assert {item["pc"] for item in receipts} == {17}, receipts
    expect(status).to_contain_text("2 abstract-state diagnostic entries")
    pixels = snapshot("repeated-pc", 5)
    for index, receipt in enumerate(receipts):
        expect(status).to_contain_text(f"{symbol('σ', receipt['state'])} · PC 0x0011")
        row = pixels["rows"][1 + index * 2]
        submissions = len(posts)
        canvas.click(position={"x": 120, "y": sum(row) / 2})
        expect(status).to_contain_text(f"Selected {symbol('σ', receipt['state'])};")
        assert len(posts) == submissions
    result["repeated-pc"] = {"diagnostics": receipts, "painted": pixels, "all_state_links_work": True}
    assert not errors, errors
    result["renderer"] = webgpu_evidence(page)
    page.close()

    class InvalidChildRpc(EntryRpcFixture):
        root_code = "0x5f5f5f5f5f73" + "22" * 20 + "61fffff100"
        child_code = "0xfe"

    with InvalidChildRpc() as rpc, TemporaryDirectory(prefix="diagnostics-rpc-") as directory:
        provider = {"id": "invalid-child", "name": "Invalid child fixture", "endpoint": rpc.endpoint}
        config = Path(directory) / "providers.json"
        config.write_text(json.dumps({"providers": [provider]}) + "\n")
        with analysis_server(args, config) as rpc_url:
            child_page = browser.new_page(viewport={"width": 1440, "height": 1000})
            observe_webgpu(child_page)
            automatic_rpc_entry(child_page, rpc_url, provider, output, "diagnostics-child-startup")
            child_canvas = child_page.locator("#evm-canvas")
            admitted = submit_rpc(child_page, child_canvas, rpc, output, "diagnostics-child", 0, [provider])
            reply = complete_submission(child_page, admitted)
            report = reply.json()["result"]["Ok"]
            diagnostic = next(item for item in report["diagnostics"] if item["kind"] == "InvalidOpcode")
            node = next(item for item in report["cfg"] if item["id"] == diagnostic["state"])
            source = next(item for item in report["programs"] if item["id"] == node["program"])
            root = next(item for item in report["programs"] if item["id"] == report["metadata"]["root_program"])
            assert diagnostic["pc"] == 0 and source["code_address"] == rpc.child_address
            assert bytes.fromhex(root["bytecode"].removeprefix("0x"))[0] == 0x5f
            assert diagnostic["invalid_opcode"]["Instruction"]["opcode"] == 0xfe
            select_inspector_tab(child_page, child_canvas, "Diagnostics")
            child_status = child_page.locator("#analysis-status")
            expect(child_status).to_contain_text("0xfe (INVALID)")
            child_page.mouse.move(2, 2)
            settle_gesture(child_page)
            png = child_canvas.screenshot(path=str(output / "diagnostics-child.png"))
            pixels = text_pixels(child_page, png, inspector_diagnostics_region(child_page, child_canvas))
            assert len(pixels["rows"]) >= 3, pixels
            row = pixels["rows"][1]
            child_canvas.click(position={"x": 120, "y": sum(row) / 2})
            expect(child_status).to_contain_text(f"Selected {symbol('σ', diagnostic['state'])}; source {symbol('P', source['id'])};")
            result["child-source"] = {"diagnostic": diagnostic, "program": source["id"], "root_pc0_opcode": 0x5f, "correct_source_jump": True}
            (output / "diagnostics-child-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
            child_page.close()
    (output / "diagnostics-report.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def run(context):
    return diagnostics_interactions(context.browser, context.url, context.args)
