"""Hierarchy calls browser acceptance cases."""

from web_test_hybrid import native_entry_context, oracle_graph, source_instructions, source_program
from pathlib import Path
from tempfile import TemporaryDirectory
import json
import re
from playwright.sync_api import expect
from web_test_graph_helpers import graph_content, graph_mode, graph_overview, symbol, workspace_route
from web_test_harness import analysis_server, click_control, complete_submission, edit_control, hide_world_directory, observe_webgpu, rendered_frame, select_report_context, set_control, settle_gesture, ui_control, ui_controls, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_layout_helpers import LAYOUT_FIXTURES, layout_counts, layout_detail_regions
from web_test_pixels import assert_same_scene, assert_translation, node_snapshot
from web_test_rpc_helpers import EntryRpcFixture, automatic_rpc_entry, native_call_summary, submit_rpc


class NestedLayoutRpcFixture(EntryRpcFixture):
    def __init__(self):
        super().__init__()
        self.world=json.loads((LAYOUT_FIXTURES/"nested-call-world.json").read_text())
        self.accounts={account["address"]:account for account in self.world["accounts"]}
        self.root_code=self.accounts[self.root_address]["code"]
        self.child_code=self.accounts[self.child_address]["code"]

    def _result(self,method,params):
        if method in ("eth_getCode","eth_getBalance","eth_getTransactionCount","eth_getStorageAt") and params[0].lower() in self.accounts:
            assert params[-1]=={"blockHash":self.block_hash,"requireCanonical":True}
            account=self.accounts[params[0].lower()]
            return {"eth_getCode":account["code"],"eth_getBalance":account["balance"],"eth_getTransactionCount":account["nonce"],"eth_getStorageAt":"0x"+"00"*32}[method]
        return super()._result(method,params)

def _relative_hierarchy(report, rpc):
    """The B -> C call is a real regional relation at the original cap one."""
    child, = [state for state in report["cfg"] if state["frame_depth"] == 2]
    summary = native_call_summary(report, child)
    procedures = summary["procedures"]
    assert len(procedures) == 2 and summary["root_procedure"] == 0
    third = "0x" + "33" * 20
    assert [procedure["code_address"] for procedure in procedures] == [rpc.child_address, third]
    assert [procedure["storage_address"] for procedure in procedures] == [rpc.child_address, third]
    assert [procedure["covered_blocks"] for procedure in procedures] == [[0, 1, 2], [0, 1]]
    assert [procedure["dependencies"] for procedure in procedures] == [[1], []]
    assert [set(procedure["outcomes"]) for procedure in procedures] == [{"Return", "Failure"}, {"Return"}]
    nodes = {node["id"]: node for node in summary["regional_cfg"]["nodes"]}
    assert len(nodes) == 6
    for node in nodes.values():
        expected_caller = rpc.root_address if node["procedure"] == 0 else rpc.child_address
        assert node["frame"]["caller"] == {"Concrete": expected_caller}
    call, = summary["regional_cfg"]["calls"]
    caller = nodes[call["from"]]
    assert caller["procedure"] == 0 and caller["frame"]["basic_block"] == 1
    assert call["callee"] == 1 and len(call["entries"]) == 1
    callee = nodes[call["entries"][0]]
    assert callee["procedure"] == 1 and callee["frame"]["basic_block"] == 0
    delivery, = call["deliveries"]
    assert delivery["current"] is True and delivery["kind"] == "Return"
    assert delivery["input_revision"] == call["input_revision"]
    assert type(delivery["output_generation"]) is int and delivery["output_generation"] > 0
    resumed, = delivery["successors"]
    assert resumed["kind"] == "Return" and resumed["to"] in caller["successors"]
    assert nodes[resumed["to"]]["procedure"] == 0
    assert nodes[resumed["to"]]["frame"]["basic_block"] == 2
    assert nodes[resumed["to"]]["transfer"]["outcomes"] == ["Return"]
    assert caller["transfer"]["calls"] == [call["id"]]
    assert caller["transfer"]["outcomes"] == ["Failure"]
    assert sum(edge["kind"] == "Call" for edge in report["edges"]) == 1
    assert sum(edge["kind"] == "SummaryReturn" for edge in report["edges"]) == 1
    assert sum(edge["kind"] == "SummaryFailure" for edge in report["edges"]) == 1
    return {"source_state": child["id"], "regional_states": len(nodes),
            "procedures": [procedure["code_address"] for procedure in procedures],
            "call": call, "world_child_has_private_pcs": False}


def _selected_native_context(page, canvas, report, node):
    """Observe a portal's canonical source/context without adding history."""
    set_control(page,canvas,"Sources",True)
    edit_control(page,canvas,"source_context_filter",f"Graph {{ state: {node['id']} }}")
    contexts=[item for item in ui_controls(page) if item["role"]=="context" and item["enabled"]]
    assert len(contexts)==1 and contexts[0]["selected"] is True,contexts
    program=source_program(report,node["program"])
    edit_control(page,canvas,"source_context_filter",program["code_hash"])
    source=program["code_address"]
    caption=source if len(source)<=22 else source[:10]+"…"+source[-8:]
    ui_control(page,caption,role="source",selected=True)
    hide_world_directory(page,canvas)
    return {"context":contexts[0],"program":program["id"],"code_hash":program["code_hash"]}


def _call_navigation(page, canvas, full_report, report, rpc, output):
    """A real recorded CALL portal opens its callee; typed Back restores Compare."""
    caller=native_entry_context(full_report)
    call,=[edge for edge in report["edges"] if edge["kind"]=="Call" and edge["from"]==caller["id"]]
    callee,=[node for node in report["cfg"] if node["id"]==call["to"]]
    snapshots={snapshot["state"]:snapshot for snapshot in report["states"]}
    parent=snapshots[caller["id"]]["entry"]["frames"][-1]
    child=snapshots[callee["id"]]["entry"]["frames"][-1]
    assert caller["code_address"]==caller["storage_address"]==rpc.root_address
    assert callee["code_address"]==callee["storage_address"]==rpc.child_address
    assert callee["frame_depth"]==caller["frame_depth"]+1
    assert (child["program"],child["code_address"],child["storage_address"],child["code_hash"])==(
        callee["program"],rpc.child_address,rpc.child_address,source_program(report,callee["program"])["code_hash"])
    assert child["caller"]=={"Concrete":rpc.root_address}
    assert type(child["is_static"]) is bool and child["is_static"] is parent["is_static"]
    assert child["continuation"] is not None
    instructions=[item for item in source_instructions(report,caller)
                  if item["opcode"]==0xf1 and item["pc"] in caller["executed_pcs"]]
    assert len(instructions)==1,instructions
    block,=[block for block in report["ssa"]["blocks"] if block["state"]==callee["id"]]
    assert block["coverage"]=="Current" and block["instructions"] and callee["executed_pcs"]
    posts=[]
    page.on("request",lambda request:posts.append(request.url)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    select_report_context(page,full_report,"Native",state=caller["id"])
    workspace_route(page,"Compare")
    set_control(page,canvas,"Overview",False,role="checkbox")
    graph_mode(page,canvas,"Blocks")
    graph_content(page,canvas,"Disasm")
    click_control(page,canvas,"Fit graph",role="action")
    page.mouse.move(2,2);rendered_frame(page)
    plot=ui_control(page,"CFG canvas",role="graph-canvas")
    fitted=node_snapshot(page,canvas,output,"call-parent-fit",plot["rect"],True)
    cards=[item for item in ui_controls(page) if item["role"]=="graph-node" and item["enabled"]]
    dx=min(17,(plot["rect"][2]-max(item["rect"][2] for item in cards))/2)
    dy=min(11,(plot["rect"][3]-max(item["rect"][3] for item in cards))/2)
    assert dx>=2 and dy>=2,(plot,cards)
    page.mouse.move(plot["rect"][0]+8,plot["rect"][3]-12)
    page.mouse.wheel(-dx,-dy)
    settle_gesture(page);page.mouse.move(2,2);rendered_frame(page)
    before=node_snapshot(page,canvas,output,"call-parent-before-open",plot["rect"],True)
    assert_translation(fitted,before,dx,dy,tolerance=1.0)
    assert_same_scene(fitted,before)
    status=page.locator("#analysis-status")
    zoom=float(re.search(r"; CFG zoom: ([0-9.]+)%",status.text_content())[1])
    portal=ui_control(page,"call · 1 destination",role="graph-node",within=plot)
    left,top,right,bottom=portal["rect"]
    pl,pt,pr,pb=plot["rect"]
    assert pl<left<right<pr and pt<top<bottom<pb,(portal,plot)
    click_control(page,canvas,portal["label"],role="graph-node",id=portal["id"])
    expect(status).to_contain_text("Route: CFG")
    expect(status).to_contain_text(f"Selected {symbol('σ',callee['id'])}; source {symbol('P',callee['program'])}")
    context=_selected_native_context(page,canvas,full_report,callee)
    canvas.screenshot(path=str(output/"call-callee.png"))
    click_control(page,canvas,"Back",role="action")
    expect(status).to_contain_text("Route: Compare")
    expect(status).to_contain_text(f"Selected {symbol('σ',caller['id'])}; source {symbol('P',caller['program'])}")
    restored_plot=ui_control(page,"CFG canvas",role="graph-canvas")
    assert all(abs(a-b)<=0.5 for a,b in zip(plot["rect"],restored_plot["rect"],strict=True)),(plot,restored_plot)
    restored=node_snapshot(page,canvas,output,"call-parent-back-restored",restored_plot["rect"],True)
    assert_translation(before,restored,0,0,tolerance=1.0)
    assert_same_scene(before,restored)
    assert float(re.search(r"; CFG zoom: ([0-9.]+)%",status.text_content())[1])==zoom
    assert not posts,"call navigation, source qualification or Back reran analysis"
    return {"call":call,"call_pc":instructions[0]["pc"],"caller":caller["id"],"callee":callee["id"],
            "context":context,"child_frame":child,"before":before,"restored":restored,"pan_delta":[dx,dy],"zoom_percent":zoom}


def _hierarchy_page(browser,url,args,rpc,providers,regions):
    page=browser.new_page(viewport={"width":1440,"height":1000});observe_webgpu(page)
    automatic_rpc_entry(page,url,providers[0],args.output,"hierarchy-call-startup")
    canvas=page.locator("#evm-canvas")
    admitted=submit_rpc(page,canvas,rpc,args.output,"hierarchy-calls",0,providers,
                        configure_form=(lambda: layout_detail_regions(page,canvas,args.output,"hierarchy",regions))
                        if regions != 1 else None)
    reply=complete_submission(page,admitted,namespace="Native",expected_detail_regions=regions)
    assert reply.status==200 and "Ok" in reply.json()["result"],reply.json()
    request=reply.request.post_data_json
    (args.output/"hierarchy-request.json").write_text(reply.request.post_data+"\n")
    full_report=reply.report
    (args.output/"hierarchy-nested-calls-full-analysis.json").write_text(json.dumps(full_report,indent=2)+"\n")
    assert full_report["hybrid"]["coverage"]["baseline_current"]
    report=oracle_graph(full_report, "Native")
    assert len(report["programs"])>=3
    for address,account in rpc.accounts.items():
        program,=[program for program in report["programs"] if program["code_address"]==address]
        assert source_program(report,program["id"])["bytecode"]==account["code"]
    if regions==1:
        assert max(state["frame_depth"] for state in report["cfg"])==2
        relative=_relative_hierarchy(report,rpc)
    else:
        assert regions==3
        assert max(state["frame_depth"] for state in report["cfg"])==3
        assert sum(edge["kind"]=="Call" for edge in report["edges"])>=2
        assert sum(edge["kind"]=="Return" for edge in report["edges"])>=2
        blocks={block["state"]:block for block in report["ssa"]["blocks"]}
        executed=set()
        for state in report["cfg"]:
            instructions=blocks[state["id"]]["instructions"]
            assert [item["pc"] for item in instructions]==state["executed_pcs"]
            if instructions:
                assert blocks[state["id"]]["coverage"]=="Current"
                assert state["executed_pcs"]==[item["pc"] for item in source_instructions(report,state)[:len(instructions)]]
                executed.add(source_program(report,state["program"])["code_address"])
        assert executed==set(rpc.accounts)
        relative=None
    (args.output/"hierarchy-nested-calls-analysis.json").write_text(json.dumps(report,indent=2)+"\n")
    hide_world_directory(page,canvas);hide_inspector(page,canvas)
    graph_overview(page,canvas,fit=True)
    graph_content(page,canvas,"SSA")
    graph_mode(page,canvas,"States")
    page.keyboard.press("f");settle_gesture(page)
    scene=layout_counts(page,report,mode="States")
    assert scene["groups"]>=(2 if regions==1 else 3)
    pixels=node_snapshot(page,canvas,args.output,"hierarchy-nested-calls",measure_nodes=True)
    assert len(pixels["nodes"])==len(report["cfg"]),"program/chain frames became phantom selectable states"
    result={"scene":scene,"pixels":pixels,"programs":len(report["programs"]),
            "relative_native":relative,"requested_detail_regions":regions,"renderer":webgpu_evidence(page)}
    if regions==3:
        result["call_navigation"]=_call_navigation(page,canvas,full_report,report,rpc,args.output)
    page.close()
    return result,request


def hierarchy_call_interactions(browser,args):
    with NestedLayoutRpcFixture() as rpc, TemporaryDirectory(prefix="layout-rpc-") as directory:
        providers=[{"id":"layout-world","name":"Synthetic nested layout calls","endpoint":rpc.endpoint}]
        config=Path(directory)/"providers.json";config.write_text(json.dumps({"providers":providers})+"\n")
        with analysis_server(args,config) as url:
            from argparse import Namespace
            default,request=_hierarchy_page(browser,url,args,rpc,providers,1)
            private_output=args.output/"private3";private_output.mkdir()
            private,private_request=_hierarchy_page(browser,url,Namespace(**{**vars(args),"output":private_output}),rpc,providers,3)
            assert private_request=={**request,"limits":{**request["limits"],"max_detail_regions":3}}
            return {"default1":default,"private3":private,"same_original_request_except_detail_regions":True}


def run(context):
    return hierarchy_call_interactions(context.browser, context.args)
