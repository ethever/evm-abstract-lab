"""Runtimes browser acceptance cases."""

from playwright.sync_api import expect
import re
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_layout_helpers import layout_counts, layout_fixture, layout_page, layout_submission
from web_test_pixels import layout_ink, node_snapshot


def layout_pending_machines(report, entry):
    assert report["schema_version"] == 13
    pending = report["pending_machines"]
    assert len(pending) == entry["expected_pending_machines"]
    assert len(report["frontiers"]) == entry["expected_frontiers"]
    pending_frontiers = [frontier for frontier in report["frontiers"] if frontier["payload"] is not None]
    assert len(pending_frontiers) == len(pending)
    assert {frontier["payload"] for frontier in pending_frontiers} == set(range(len(pending)))
    nodes = {node["id"]:node for node in report["cfg"]}
    states = {state["state"]:state for state in report["states"]}
    ssa_blocks = {block["state"]:block for block in report["ssa"]["blocks"]}
    programs = {program["id"] for program in report["programs"]}
    graph = report["symbolic"]["nodes"]
    word_kinds = {"Constant","Input","Fresh","Operation","Ite","ByteRead","WordRead","SlotRead","Keccak"}
    byte_kinds = {"BytesZero","BytesInput","BytesExact","WriteByte","WriteWord","Copy","Slice","BytesIte"}
    slot_kinds = {"SlotsZero","SlotsInput","Store","SlotsIte"}
    path_kinds = {"PathTrue","PathFalse","PathAtoms","PathAnd","PathIte"}
    def reference(value, kinds):
        index = value["node"]
        assert type(index) is int and 0 <= index < len(graph)
        node = graph[index]
        assert (node if isinstance(node,str) else next(iter(node))) in kinds
    def facts(value):
        assert set(value) == {"summary","constants","known_zero","known_one","nonzero","unsigned","signed",
                              "congruence","origins","code_address_role","identity","expression","symbolic_limit"}
        words = [value["known_zero"],value["known_one"],*value["unsigned"].values(),*value["signed"].values()]
        words.extend(value["constants"] or [])
        assert all(isinstance(word,str) and word.startswith("0x") and 0 <= int(word,16) < 2**256 for word in words)
        if value["expression"] is not None:reference(value["expression"],word_kinds)
    def pool(index, values):
        assert type(index) is int and 0 <= index < len(values)
        return values[index]
    buffers = set(); stores = set()
    def machine_snapshot(machine):
        assert set(machine) == {"frames","store","path"} and machine["frames"]
        reference(machine["path"],path_kinds)
        pool(machine["store"],report["stores"]);stores.add(machine["store"])
        for index,frame in enumerate(machine["frames"]):
            assert set(frame) == {"index","program","code_address","code_hash","storage_address","address_value",
                                  "caller","address_operand","caller_operand","call_value","is_static","kind","basic_block","context","stack",
                                  "memory","calldata","returndata","rollback_store","continuation"}
            assert frame["index"] == index and frame["program"] in programs
            facts(frame["call_value"])
            for name in ["address_operand","caller_operand"]:
                if frame[name] is not None: facts(frame[name])
            for value in frame["stack"]:facts(value)
            for name in ["memory","calldata","returndata"]:
                pool(frame[name],report["byte_arrays"]);buffers.add(frame[name])
            pool(frame["rollback_store"],report["stores"]);stores.add(frame["rollback_store"])
            if frame["continuation"] is not None:
                facts(frame["continuation"]["output_offset"]);facts(frame["continuation"]["output_size"])
        active = machine["frames"][-1]
        assert any(node["program"] == active["program"] and node["basic_block"] == active["basic_block"]
                   and node["frame_depth"] == len(machine["frames"]) for node in nodes.values())
        return active
    pending_locations = []; instruction_frontiers = []
    for frontier in report["frontiers"]:
        assert frontier["from"] in nodes and frontier["range"] is None
        assert frontier["kind"] == entry["expected_frontier_reason"]
        assert frontier["reason"] == entry["expected_frontier_details"]
        assert frontier["detail"] == f"{frontier['kind']}({frontier['reason'][frontier['kind']]})"
        source = nodes[frontier["from"]]
        if frontier["payload"] is not None:
            assert frontier["pc"] is None
            machine = pool(frontier["payload"],pending)
            active = machine_snapshot(machine)
            target = next(node for node in nodes.values() if node["program"] == active["program"]
                          and node["basic_block"] == active["basic_block"]
                          and node["frame_depth"] == len(machine["frames"]))
            pending_locations.append({"source_start_pc":source["start_pc"],"source_frame_depth":source["frame_depth"],
                                      "target_start_pc":target["start_pc"],"target_frame_depth":target["frame_depth"]})
            continue
        assert frontier["pc"] is not None
        state = states[frontier["from"]]
        block = ssa_blocks[frontier["from"]]
        for name in ["entry","exit"]:
            active = machine_snapshot(state[name])
            assert (active["program"],active["basic_block"],len(state[name]["frames"])) == (
                source["program"],source["basic_block"],source["frame_depth"])
        instruction = block["instructions"][-1]
        assert instruction["pc"] == frontier["pc"] == source["executed_pcs"][-1]
        assert not instruction["fault"] and not instruction["results"] and len(instruction["operands"]) == 2
        # The exit snapshot is after operand consumption, not a resumable pre-MSTORE checkpoint.
        assert instruction["effect_result"] is not None and instruction["effect_result"] == block["exit_effect"]
        assert [len(frame) for frame in block["exit_frames"]] == [len(frame["stack"]) for frame in state["exit"]["frames"]]
        assert not any(outcome["state"] == frontier["from"] for outcome in report["outcomes"])
        outgoing = [edge for edge in report["edges"] if edge["from"] == frontier["from"]]
        assert outgoing
        edge_ids = {edge["id"] for edge in outgoing}
        assert len(edge_ids) == len(outgoing)
        assert not edge_ids.intersection(transition["edge"] for transition in report["ssa"]["transitions"])
        deferred = {edge["edge"]:edge["reason"] for edge in report["ssa"]["deferred_edges"]}
        assert all(deferred.get(edge) == "NotInExecutionEvidence" for edge in edge_ids)
        instruction_frontiers.append({"source_start_pc":source["start_pc"],"source_frame_depth":source["frame_depth"],
                                      "pc":instruction["pc"],"opcode":instruction["opcode"],"name":instruction["name"],
                                      "progress":instruction["progress"],"coverage":block["coverage"],
                                      "deferred_edge_kinds":sorted(edge["kind"] for edge in outgoing)})
    assert sorted(pending_locations,key=lambda location:location["source_start_pc"]) == entry["expected_pending_frontiers"]
    assert sorted(instruction_frontiers,key=lambda location:location["pc"]) == entry["expected_instruction_frontiers"]
    for index in stores:
        store = report["stores"][index]
        assert set(store) == {"storage_default","transient_default","balance_default","accounts","logs",
                              "logs_unknown","storage_facts","transient_facts"}
        for name in ["storage_default","transient_default","balance_default"]:facts(store[name])
        for account in store["accounts"]:
            assert set(account) == {"address","existence","balance","nonce","code_kind","code_hash","program",
                                    "delegation_target","storage_default","storage_root","transient_root","storage",
                                    "transient_default","transient","created","pending_destruction"}
            assert account["program"] is None or account["program"] in programs
            for name in ["balance","nonce","storage_default","transient_default"]:facts(account[name])
            for name in ["storage_root","transient_root"]:reference(account[name],slot_kinds)
            for slot in account["storage"]+account["transient"]:
                assert slot["slot"].startswith("0x") and 0 <= int(slot["slot"],16) < 2**256
                facts(slot["value"])
        for binding in store["storage_facts"]+store["transient_facts"]:
            reference(binding["expression"],word_kinds);facts(binding["value"])
        for log in store["logs"]:
            pool(log["data"],report["byte_arrays"]);buffers.add(log["data"])
            for value in log["topics"]:facts(value)
    for index in buffers:
        buffer = report["byte_arrays"][index]
        reference(buffer["symbolic"],{"View"})
        view = graph[buffer["symbolic"]["node"]]["View"]
        reference(view["state"],byte_kinds)
        reference(view["offset"],word_kinds);reference(view["length"],word_kinds)
        for value in [buffer["length"],buffer["default"],*buffer["values"]]:facts(value)
        for cell in buffer["cells"]:pool(cell["value"],buffer["values"])
        for binding in buffer["symbolic_facts"]:
            reference(binding["expression"],word_kinds);facts(binding["value"])
    assert not report["ssa"]["complete"]
    assert {block["state"] for block in report["ssa"]["blocks"]} == set(nodes)
    assert all(not block["incoming_complete"] for block in report["ssa"]["blocks"])
    return {"pending_machines":len(pending),"instruction_frontiers":instruction_frontiers,
            "byte_arrays":sorted(buffers),"stores":sorted(stores),
            "typed_reasons":[frontier["reason"] for frontier in report["frontiers"]]}

def hierarchy_real_runtime_interactions(browser,url,args,fixture_names=("ethereum-weth9","ethereum-uniswap-v2-router02","ethereum-multicall3")):
    results={}
    for name in fixture_names:
        page,canvas,errors=layout_page(browser,url)
        code,metadata=layout_fixture(name)
        assert metadata["chain_id"]==1 and metadata["requireCanonical"]
        entry=metadata["browser_entry"]
        print(f"real-runtime analysis start: {name}",flush=True)
        report=layout_submission(page,canvas,code,args.output,name,timeout_seconds=1800,entry=entry)
        print(f"real-runtime analysis done: {name} status={report['status']} states={len(report['cfg'])} "
              f"frontiers={[(frontier['kind'],frontier['detail']) for frontier in report['frontiers']]} "
              f"unknown_closed={[summary['closed'] for summary in report['abstract_summaries']]} "
              f"analyzing_ns={report['metadata']['timings']['analyzing_ns']}",flush=True)
        assert report["programs"][0]["code_hash"]==metadata["code_hash"]
        assert report["status"] == entry["expected_status"]
        assert len(report["cfg"]) >= entry["minimum_native_states"]
        assert len(report["frontiers"]) == entry["expected_frontiers"]
        if "expected_frontier_reason" in entry:
            assert all(frontier["kind"] == entry["expected_frontier_reason"] for frontier in report["frontiers"])
        if "expected_frontiers_by_kind" in entry:
            expected=entry["expected_frontiers_by_kind"]
            counts={kind:sum(frontier["kind"]==kind for frontier in report["frontiers"])
                    for kind in {frontier["kind"] for frontier in report["frontiers"]}}
            assert counts=={kind:details["count"] for kind,details in expected.items()},counts
            for frontier in report["frontiers"]:
                assert frontier["reason"]==expected[frontier["kind"]]["reason"]
                assert frontier["detail"]==expected[frontier["kind"]]["detail"]
        if "expected_closed_procedure_summaries" in entry:
            summaries=report["abstract_summaries"]
            assert len(summaries)==entry["expected_closed_procedure_summaries"]
            assert all(summary["closed"] for summary in summaries)
        if name == "ethereum-uniswap-v2-router02":
            for summary in report["abstract_summaries"]:
                assert summary["procedures"]
                for procedure in summary["procedures"]:
                    assert procedure["kind"]=="AbstractEnvironment" and procedure["is_static"]
                    assert not procedure["covered_blocks"]
                    assert set(procedure["outcomes"])=={"Return","Revert","Failure"}
        pending = layout_pending_machines(report,entry) if "expected_pending_machines" in entry else None
        scene=layout_counts(page,report)
        assert scene["nodes"] >= entry["minimum_source_blocks"]
        if report["status"]=="Incomplete":
            assert report["frontiers"] and not report["ssa"]["complete"]
            expect(page.locator("#analysis-status")).to_contain_text("Incomplete")
        # A full overview can contain many tiny cards. Focus is a camera action,
        # leaving the entire captured graph and its coverage untouched.
        canvas.screenshot(path=str(args.output/f"hierarchy-{name}-overview.png"))
        canvas.click(position={"x":225,"y":83});settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
        focused=node_snapshot(page,canvas,args.output,f"hierarchy-{name}-focused")
        assert layout_counts(page,report)=={**scene,"detail":"SSA preview"}
        native_scene = None
        if name == "ethereum-multicall3":
            requests = []
            page.on("request", lambda request: requests.append(request.url)
                    if request.method == "POST" and request.url.endswith("/api/tasks") else None)
            graph_mode(page,canvas,"States")
            page.keyboard.press("f");settle_gesture(page)
            native_scene = layout_counts(page,report,mode="States")
            assert native_scene["nodes"] >= entry["minimum_native_states"] and native_scene["nodes"] == len(report["cfg"])
            assert native_scene["native_edges"] == len(report["edges"])
            canvas.screenshot(path=str(args.output/f"hierarchy-{name}-states-overview.png"))
            canvas.click(position={"x":225,"y":83});settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
            native_focus = node_snapshot(page,canvas,args.output,f"hierarchy-{name}-states-focused")
            selected = re.search(r"Selected (σ[^;]+);",page.locator("#analysis-status").text_content())[1]
            assert selected in [symbol("σ",node["id"]) for node in report["cfg"]]
            selected_node = next(node for node in report["cfg"] if symbol("σ",node["id"]) == selected)
            selected_ssa = next(block for block in report["ssa"]["blocks"] if block["state"] == selected_node["id"])
            assert selected_ssa["instructions"] and not selected_ssa["incoming_complete"]
            ink = layout_ink(page,canvas.screenshot(),native_focus["bounds"])
            assert ink["preview_pixels"] > 0, "real-runtime native focus did not paint SSA instructions"
            assert not requests, "native projection/focus triggered another real-runtime analysis"
            native_scene.update(focused=native_focus,selected_native_state=selected,ssa_instructions=len(selected_ssa["instructions"]),
                                ssa_ink=ink,unexpected_task_posts=requests)
        assert not errors,errors
        results[name]={"provenance":metadata,"status":report["status"],"scene":scene,"native_scene":native_scene,"focused":focused,"frontiers":len(report["frontiers"]),"pending":pending,"renderer":webgpu_evidence(page)}
        page.close()
    return results


def run(context, fixture):
    return hierarchy_real_runtime_interactions(context.browser, context.url, context.args, fixture_names=(fixture,))
