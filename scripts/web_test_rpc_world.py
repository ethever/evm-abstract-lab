"""Rpc world browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import hashlib
import json
from web_test_graph_helpers import graph_mode, graph_scope, symbol
from web_test_harness import complete_submission, observe_webgpu, rendered_frame, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector, inspector_block_entry_control, select_inspector_tab
from web_test_pixels import node_snapshot
from web_test_rpc_helpers import RPC_PROVIDERS, RPC_TEST_TOKEN, automatic_rpc_entry, configured_rpc_server, submit_rpc


def world_graph_scopes(page, canvas, report, child, child_state, output: Path):
    # Keep the native child selected while narrowing visibility. Switching to
    # Local must cross the real CALL/RETURN links even with P1 still selected.
    states = report["cfg"]
    edges = report["edges"]
    child_ids = {state["id"] for state in states if state["program"] == child["id"]}
    assert len(states) == 4 and child_ids == {child_state["id"]}
    call, = [edge for edge in edges if edge["kind"] == "Call"]
    returned, = [edge for edge in edges if edge["kind"] == "Return"]
    failed, = [edge for edge in edges if edge["kind"] == "Failure"]
    assert len(edges) == 3 and call["to"] == returned["from"] == child_state["id"]
    assert failed["from"] == call["from"]
    parent_ids = {call["from"], failed["to"], returned["to"]}
    assert len(parent_ids) == 3 and parent_ids.isdisjoint(child_ids)
    by_id = {state["id"]: state for state in states}
    parent = by_id[call["from"]]
    assert child_state["frame_depth"] == 2
    assert all(by_id[state]["program"] == parent["program"] != child["id"]
               and by_id[state]["frame_depth"] == 1 for state in parent_ids)
    # A refused CALL and a successful child return resume the same bytecode
    # with distinct 0/1 status inputs; neither native state may disappear.
    failure_resume, success_resume = by_id[failed["to"]], by_id[returned["to"]]
    assert failure_resume["basic_block"] == success_resume["basic_block"] != parent["basic_block"]
    assert failure_resume["start_pc"] == success_resume["start_pc"]
    snapshots = {snapshot["state"]: snapshot for snapshot in report["states"]}
    for state, status in [(failure_resume, 0), (success_resume, 1)]:
        stack = snapshots[state["id"]]["entry"]["frames"][-1]["stack"]
        assert len(stack) == 1 and stack[0]["constants"] is not None
        assert {int(value, 16) for value in stack[0]["constants"]} == {status}
    boundary = [edge for edge in edges if (edge["from"] in child_ids) != (edge["to"] in child_ids)]
    assert len(boundary) == 2 and {edge["kind"] for edge in boundary} == {"Call", "Return"}
    submitted = []
    page.on("request", lambda request: submitted.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    canvas.click(position={"x": page.viewport_size["width"] - 180, "y": 12})  # World directory
    hide_inspector(page, canvas)
    page.keyboard.press("2")
    settle_gesture(page)
    graph_mode(page, canvas, "States")
    full = graph_scope(page, "States", 4, 4, 4)
    assert full["scope"] == "All programs" and full["native_edges"] == len(edges)
    canvas.click(position={"x": 205, "y": 109})  # native program ComboBox
    rendered_frame(page)
    canvas.screenshot(path=str(output / "world-graph-program-menu.png"))
    canvas.click(position={"x": 205, "y": 181})  # P1 · Runtime (native menu geometry)
    settle_gesture(page)
    filtered = graph_scope(page, "States", 1, 1, 4)
    assert filtered["scope"] == symbol("P", child["id"])
    assert filtered["edges"] == filtered["native_edges"] == 0
    assert filtered["hidden_states"] == 3 and filtered["boundary_edges"] == 2
    status = page.locator("#analysis-status")
    expect(status).to_contain_text(f"Selected {symbol('σ', child_state['id'])}; source {symbol('P', child['id'])}")
    child_pixels = node_snapshot(page, canvas, output, "world-graph-child-program", measure_nodes=True)
    assert len(child_pixels["nodes"]) == 1, child_pixels
    graph_mode(page, canvas, "Local")
    local = graph_scope(page, "Local", 4, 4, 4)
    assert local["scope"] == "Cross-program neighborhood"
    assert local["hidden_states"] == local["boundary_edges"] == 0
    assert local["edges"] == local["native_edges"] == len(edges)
    expect(status).to_contain_text(f"Selected {symbol('σ', child_state['id'])}; source {symbol('P', child['id'])}")
    expect(status).to_contain_text("Converged")
    local_pixels = node_snapshot(page, canvas, output, "world-graph-local-cross-program", measure_nodes=True)
    assert len(local_pixels["nodes"]) == 4, local_pixels
    assert not submitted, "program filtering or local exploration reran analysis"
    return {"all_programs": full, "child_program": filtered, "cross_program_local": local,
            "selected_native_state": child_state["id"], "child_pixels": child_pixels,
            "local_pixels": local_pixels}

def world_interactions(browser, url: str, output: Path, rpc, pending_rpc, providers):
    results = {}
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    failures = []
    page.on("pageerror", lambda error: failures.append(str(error)))
    startup = automatic_rpc_entry(page, url, providers[0], output, "world-startup")
    pending_rpc.requests.clear()
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    admitted = submit_rpc(page, canvas, rpc, output, "world", provider_index=1, providers=providers)
    reply = complete_submission(page, admitted)
    envelope = reply.json()
    (output / "world-task-reply.json").write_text(json.dumps({"admission_http_status": reply.admission_status, "observed_http_status": reply.status, "reply": envelope}, indent=2) + "\n")
    assert reply.status == 200 and "Ok" in envelope["result"], f"RPC world task failed: {json.dumps(envelope, sort_keys=True)}"
    report = envelope["result"]["Ok"]
    assert len(report["programs"]) >= 2, "RPC call failed to capture the child program"
    assert {program["code_address"] for program in report["programs"]} == {rpc.root_address, rpc.child_address}
    assert max(block["frame_depth"] for block in report["cfg"]) == 2
    snapshot = report["metadata"]["snapshot"]
    assert int(snapshot["chain_id"], 16) == int(rpc.chain_id, 16) and snapshot["block_hash"] == rpc.block_hash
    assert report["acquisition"]["requests"] > 0
    assert report["outcomes"] and report["stores"] and report["byte_arrays"]
    assert any(state["exit"] and len(state["exit"]["frames"]) == 2 for state in report["states"])
    child = next(program for program in report["programs"] if program["code_address"] == rpc.child_address)
    child_state = next(block for block in report["cfg"] if block["program"] == child["id"])
    child_frames = [frame for state in report["states"] for point in (state["entry"], state["exit"]) if point for frame in point["frames"] if frame["program"] == child["id"]]
    def contains(value, number):
        return value["constants"] is not None and number in [int(word, 16) for word in value["constants"]]
    def byte_at(array, offset, number):
        return any(cell["offset"] == offset and contains(array["values"][cell["value"]], number) for cell in array["cells"])
    assert any(contains(report["byte_arrays"][frame["calldata"]]["length"], 1) and byte_at(report["byte_arrays"][frame["calldata"]], 0, 0xab) for frame in child_frames)
    assert any(byte_at(report["byte_arrays"][frame["memory"]], 0, 0xab) for frame in child_frames)
    child_accounts = [account for store in report["stores"] for account in store["accounts"] if account["address"] == rpc.child_address]
    assert any(any(int(entry["slot"], 16) == 3 and contains(entry["value"], 99) for entry in account["storage"]) for account in child_accounts)
    assert any(any(int(entry["slot"], 16) == 2 and contains(entry["value"], 7) for entry in account["transient"]) for account in child_accounts)
    # Select the actual child program from the native directory. It must
    # update code, SSA, graph selection and the active frame together.
    settle_gesture(page)
    canvas.screenshot(path=str(output / "world-overview.png"))
    canvas.click(position={"x": 80, "y": 135})
    expect(status).to_contain_text(f"source {symbol('P', child['id'])}")
    expect(status).to_contain_text(f"Selected {symbol('σ', child_state['id'])}")
    expect(status).to_contain_text("Frame f₁")
    canvas.screenshot(path=str(output / "world-child-frame.png"))
    screenshots = {}
    tabs = ["Stack", "Memory", "Storage", "Transient", "Calldata", "Returndata",
            "Outcomes", "Acquisition", "Environment", "Diagnostics", "Frame"]
    for tab in tabs:
        select_inspector_tab(page, canvas, tab)
        expect(status).to_contain_text(tab)
        rendered_frame(page)
        png = canvas.screenshot(path=str(output / f"world-{tab.lower()}.png"))
        screenshots[tab] = hashlib.sha256(png).hexdigest()
    assert len(set(screenshots.values())) == len(tabs), "inspector controls did not expose distinct views"
    entry = inspector_block_entry_control(page, canvas)
    # inspector.rs places Observed exit directly after Block entry in this row.
    canvas.click(position={"x": entry[2] + 50, "y": (entry[1] + entry[3]) / 2})
    expect(status).to_contain_text("Observed exit")
    select_inspector_tab(page, canvas, "Memory")
    expect(status).to_contain_text("Memory")
    canvas.screenshot(path=str(output / "world-child-exit-memory.png"))
    graph_scopes = world_graph_scopes(page, canvas, report, child, child_state, output)
    # All acquisition reads remain on one canonical snapshot.
    reads = [request for request in rpc.requests if request["method"] in ("eth_getCode", "eth_getStorageAt", "eth_getBalance", "eth_getTransactionCount")]
    assert reads and all(request["params"][-1] == {"blockHash": rpc.block_hash, "requireCanonical": True} for request in reads)
    assert sum(request["method"] == "eth_getBlockByNumber" and request["params"][0] == "latest" for request in rpc.requests) == 1
    assert not pending_rpc.requests, "selecting the second provider contacted the default provider"
    assert RPC_TEST_TOKEN not in json.dumps(report), "provider URL escaped into the analysis report"
    (output / "world-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    results["analysis"] = {"programs": len(report["programs"]), "states": len(report["states"]), "outcomes": len(report["outcomes"]), "rpc_requests": len(rpc.requests), "inspector_views": screenshots, "graph_scopes": graph_scopes, "renderer": webgpu_evidence(page)}
    assert not failures, failures
    page.close()
    results["automatic_entry"] = startup["input"]
    return results


def run(context):
    with configured_rpc_server(context.args) as (url, pending, rpc, providers):
        result = world_interactions(context.browser, url, context.output, rpc, pending, providers)
        result.update(providers=providers, selected_provider_id=RPC_PROVIDERS[1]["id"],
                      endpoint_displayed_in_selector=True, submission_uses_provider_id=True)
        return result
