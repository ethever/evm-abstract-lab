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
from web_test_wire_bytes import _Graph, _buffer, _numeric_contains, assert_child_bytes


def _world_call_contract(report, rpc, child, child_state):
    """Match this fixed CALL fixture, including both ways to resume with zero."""
    states = report["cfg"]
    edges = report["edges"]
    programs = {program["id"]: program for program in report["programs"]}
    root = programs[report["metadata"]["root_program"]]
    assert len(programs) == len(report["programs"]) == 2 and root != child
    assert root["code_address"] == report["metadata"]["entry_address"] == rpc.root_address
    assert child == programs[child["id"]] and child["code_address"] == rpc.child_address
    assert root["bytecode"] == rpc.root_code and child["bytecode"] == rpc.child_code
    child_ids = {state["id"] for state in states if state["program"] == child["id"]}
    assert len(states) == 4 and child_ids == {child_state["id"]}
    call, = [edge for edge in edges if edge["kind"] == "Call"]
    returned, = [edge for edge in edges if edge["kind"] == "Return"]
    failed = [edge for edge in edges if edge["kind"] == "Failure"]
    assert len(edges) == 4 and len(failed) == 2
    assert call["to"] == returned["from"] == child_state["id"]
    failure_id, = {edge["to"] for edge in failed}
    # calls::immediate_call_failure retains a same-depth rejection. The gas
    # envelope at child MSTORE/RETURN uses calls::finish to pop the child and
    # restore its checkpoint. Both edges survive native edge deduplication.
    assert {(edge["from"], edge["to"]) for edge in failed} == {
        (call["from"], failure_id), (child_state["id"], failure_id)}
    assert len({edge["id"] for edge in edges}) == 4
    parent_ids = {call["from"], failure_id, returned["to"]}
    assert len(parent_ids) == 3 and parent_ids.isdisjoint(child_ids)
    by_id = {state["id"]: state for state in states}
    assert len(by_id) == 4 and child_state == by_id[child_state["id"]]
    parent = by_id[call["from"]]
    assert child_state["frame_depth"] == 2
    assert all(by_id[state]["program"] == root["id"]
               and by_id[state]["frame_depth"] == 1 for state in parent_ids)
    failure_resume, success_resume = by_id[failure_id], by_id[returned["to"]]
    assert parent["basic_block"] == child_state["basic_block"] == 0
    assert parent["start_pc"] == child_state["start_pc"] == 0
    assert failure_resume["basic_block"] == success_resume["basic_block"] == 1
    assert failure_resume["start_pc"] == success_resume["start_pc"] == rpc.expected["root_call_pc"] + 1
    assert parent["instructions"][-1]["name"] == "CALL"
    assert parent["executed_pcs"][-1] == rpc.expected["root_call_pc"]
    assert child_state["instructions"][-1]["name"] == "RETURN"
    assert child_state["executed_pcs"][-1] == len(bytes.fromhex(rpc.child_code[2:])) - 1
    snapshots = {snapshot["state"]: snapshot for snapshot in report["states"]}
    assert len(snapshots) == len(report["states"]) == 4 and snapshots.keys() == by_id.keys()
    for state in states:
        program = programs[state["program"]]
        block = program["blocks"][state["basic_block"]]
        assert state["instructions"] == block["instructions"] and state["start_pc"] == block["start_pc"]
        assert state["code_address"] == state["storage_address"] == program["code_address"]
        for phase in ("entry", "exit"):
            frames = snapshots[state["id"]][phase]["frames"]
            assert len(frames) == state["frame_depth"]
            assert frames[-1]["program"] == state["program"]
            assert frames[-1]["basic_block"] == state["basic_block"]
            assert frames[-1]["context"] == state["context"] == []
            for index, frame in enumerate(frames):
                source = programs[frame["program"]]
                assert frame["index"] == index and frame["kind"] == source["kind"] == "Runtime"
                assert frame["code_address"] == frame["storage_address"] == source["code_address"]
                assert frame["code_hash"] == source["code_hash"]
                assert frame["address_value"] == {"Concrete": source["code_address"]}

    def constant(value, number):
        assert value["constants"] == [hex(number)] and _numeric_contains(value, number)

    for state, status in [(failure_resume, 0), (success_resume, 1)]:
        stack = snapshots[state["id"]]["entry"]["frames"][-1]["stack"]
        assert len(stack) == 1
        constant(stack[0], status)
    parent_exit = snapshots[parent["id"]]["exit"]
    child_entry, child_exit = (snapshots[child_state["id"]][phase] for phase in ("entry", "exit"))
    failure_entry, success_entry = (snapshots[state["id"]]["entry"] for state in (failure_resume, success_resume))
    entry_frame, exit_frame = child_entry["frames"][-1], child_exit["frames"][-1]
    assert child_entry["frames"][0] == child_exit["frames"][0] == parent_exit["frames"][0]
    assert {**exit_frame, "memory": entry_frame["memory"]} == entry_frame
    assert entry_frame["caller"] == {"Concrete": rpc.root_address}
    constant(entry_frame["call_value"], 0)
    continuation = entry_frame["continuation"]
    assert set(continuation) == {"return_block", "output_offset", "output_size", "creation"}
    assert continuation["return_block"] == failure_resume["basic_block"] and continuation["creation"] is None
    constant(continuation["output_offset"], 32)
    constant(continuation["output_size"], 32)
    for state, resumed in ((failure_resume, failure_entry), (success_resume, success_entry)):
        frame = resumed["frames"][0]
        preserved = {**frame, "basic_block": parent["basic_block"], "stack": [],
                     "memory": parent_exit["frames"][0]["memory"],
                     "returndata": parent_exit["frames"][0]["returndata"]}
        assert preserved == parent_exit["frames"][0]
        observed_exit = snapshots[state["id"]]["exit"]
        assert observed_exit["store"] == resumed["store"]
        assert observed_exit["frames"] == [{**frame, "stack": []}]
    assert failure_entry["frames"][0]["memory"] == parent_exit["frames"][0]["memory"]
    assert failure_entry["store"] == entry_frame["rollback_store"] == parent_exit["store"]
    assert success_entry["store"] == child_exit["store"] != failure_entry["store"]

    graph = _Graph(report)
    empty = failure_entry["frames"][0]["returndata"]
    data = _buffer(report, empty, False, 0)
    empty_id, view = graph.view(data["symbolic"], len(graph.nodes), 0)
    assert graph.node(view["state"], empty_id, "BytesExact")[1] == "0x"
    stores = report["stores"]
    root_entry = snapshots[parent["id"]]["entry"]
    root_checkpoint = parent_exit["frames"][0]["rollback_store"]
    initial = stores[root_checkpoint]
    assert len(initial["accounts"]) == 2
    assert {account["address"] for account in initial["accounts"]} == {rpc.root_address, rpc.child_address}
    for account in initial["accounts"]:
        source = programs[account["program"]]
        assert account["address"] == source["code_address"] and account["code_hash"] == source["code_hash"]
        assert account["touched"] is False
    assert stores[root_entry["store"]] == {
        **initial, "accounts": [{**account, "touched": True} if account["address"] == rpc.root_address
                                else account for account in initial["accounts"]]}
    before_call = stores[parent_exit["store"]]
    assert stores[child_entry["store"]] == {
        **before_call, "accounts": [{**account, "touched": True} if account["address"] == rpc.child_address
                                    else account for account in before_call["accounts"]]}
    # Compare whole Stores, allowing only these literal writes. Numeric fields
    # constrain the same canonical Store roots; separate accounts or snapshots
    # cannot supply independent storage/transient witnesses.
    for before, after, owner, writes in [
        (snapshots[parent["id"]]["entry"]["store"], parent_exit["store"], rpc.root_address,
         (("storage", 1, 42), ("transient", 5, 8))),
        (child_entry["store"], child_exit["store"], rpc.child_address,
         (("storage", 3, 99), ("transient", 2, 7))),
    ]:
        old, new = stores[before], stores[after]
        previous, = [account for account in old["accounts"] if account["address"] == owner]
        current, = [account for account in new["accounts"] if account["address"] == owner]
        changes = {}
        for plane, slot, value in writes:
            root_key = plane + "_root"
            write_id, write = graph.node(current[root_key], len(graph.nodes), "Store", ("previous", "key", "value"))
            assert write["previous"] == previous[root_key]
            assert graph.node(write["key"], write_id, "Constant")[1] == hex(slot)
            assert graph.node(write["value"], write_id, "Constant")[1] == hex(value)
            cells = {int(entry["slot"], 16): entry["value"] for entry in current[plane]}
            assert len(cells) == len(current[plane])
            assert cells.keys() == {int(entry["slot"], 16) for entry in previous[plane]} | {slot}
            constant(cells[slot], value)
            assert all(cells[int(entry["slot"], 16)] == entry["value"] for entry in previous[plane])
            changes.update({root_key: current[root_key], plane: current[plane]})
        assert new == {**old, "accounts": [{**account, **changes} if account["address"] == owner
                                           else account for account in old["accounts"]]}
    checkpoint_child, = [account for account in before_call["accounts"] if account["address"] == rpc.child_address]
    assert checkpoint_child["storage"] == checkpoint_child["transient"] == []
    assert checkpoint_child["touched"] is False

    # Root gas failures are terminal outcomes, not extra CFG links. Successful
    # root completion retains persistent writes but clears transaction transient
    # storage and touches; the successful resume above still contains both.
    terminal_failures = [outcome for outcome in report["outcomes"] if outcome["kind"] == "Failure"]
    terminal_returns = [outcome for outcome in report["outcomes"] if outcome["kind"] == "Return"]
    assert terminal_failures and len(terminal_returns) == 2
    assert len(terminal_failures) + len(terminal_returns) == len(report["outcomes"])
    assert {outcome["state"] for outcome in terminal_failures} == parent_ids
    assert all(outcome["store"] == root_checkpoint and outcome["data"] == empty for outcome in terminal_failures)
    assert {outcome["state"] for outcome in terminal_returns} == {failure_id, success_resume["id"]}
    zero_slots = checkpoint_child["transient_root"]
    assert graph.node(zero_slots, len(graph.nodes))[1] == "SlotsZero"
    for outcome in terminal_returns:
        source = snapshots[outcome["state"]]["exit"]
        assert stores[outcome["store"]] == {
            **stores[source["store"]], "accounts": [
                {**account, "transient": [], "transient_root": zero_slots, "touched": False}
                for account in stores[source["store"]]["accounts"]]}
    boundary = [edge for edge in edges if (edge["from"] in child_ids) != (edge["to"] in child_ids)]
    assert len(boundary) == 3 and {edge["kind"] for edge in boundary} == {"Call", "Return", "Failure"}
    return {"call": call, "return": returned, "failures": failed, "boundary_edges": boundary,
            "rollback_store": failure_entry["store"], "success_store": success_entry["store"],
            "root_failure_outcomes": len(terminal_failures)}


def world_graph_scopes(page, canvas, report, child, child_state, output: Path, rpc):
    contract = _world_call_contract(report, rpc, child, child_state)
    edges = report["edges"]
    # Keep the native child selected while narrowing visibility. Switching to
    # Local must cross all three boundary links even with P1 still selected.
    submitted = []
    page.on("request", lambda request: submitted.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    canvas.click(position={"x": page.viewport_size["width"] - 180, "y": 12})  # World directory
    hide_inspector(page, canvas)
    page.keyboard.press("2")
    settle_gesture(page)
    graph_mode(page, canvas, "States")
    full = graph_scope(page, "States", 4, 4, 4)
    assert full["scope"] == "All programs" and full["edges"] == full["native_edges"] == len(edges)
    canvas.click(position={"x": 205, "y": 109})  # native program ComboBox
    rendered_frame(page)
    canvas.screenshot(path=str(output / "world-graph-program-menu.png"))
    canvas.click(position={"x": 205, "y": 181})  # P1 · Runtime (native menu geometry)
    settle_gesture(page)
    filtered = graph_scope(page, "States", 1, 1, 4)
    assert filtered["scope"] == symbol("P", child["id"])
    assert filtered["edges"] == filtered["native_edges"] == 0
    assert filtered["hidden_states"] == 3 and filtered["boundary_edges"] == 3
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
            "local_pixels": local_pixels, "native_call_contract": contract}

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
    child_snapshot = next(state for state in report["states"] if state["state"] == child_state["id"])
    assert child_snapshot["exit"] and len(child_snapshot["exit"]["frames"]) == 2
    child_frame = child_snapshot["exit"]["frames"][-1]
    assert child_frame["kind"] == "Runtime" and child_frame["program"] == child["id"]
    assert_child_bytes(report, child_frame)
    def contains(value, number):
        return value["constants"] is not None and number in [int(word, 16) for word in value["constants"]]
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
    graph_scopes = world_graph_scopes(page, canvas, report, child, child_state, output, rpc)
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
