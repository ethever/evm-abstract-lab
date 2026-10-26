"""Hierarchy calls browser acceptance cases."""

from web_test_hybrid import oracle_graph, source_instructions, source_program
from pathlib import Path
from tempfile import TemporaryDirectory
import json
from web_test_graph_helpers import graph_mode
from web_test_harness import analysis_server, complete_submission, observe_webgpu, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_layout_helpers import LAYOUT_FIXTURES, layout_counts, layout_detail_regions
from web_test_pixels import node_snapshot
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
    canvas.click(position={"x":1440-180,"y":12});hide_inspector(page,canvas)
    page.keyboard.press("2");settle_gesture(page);graph_mode(page,canvas,"States")
    page.keyboard.press("f");settle_gesture(page)
    scene=layout_counts(page,report,mode="States")
    assert scene["groups"]>=(2 if regions==1 else 3)
    pixels=node_snapshot(page,canvas,args.output,"hierarchy-nested-calls",measure_nodes=True)
    assert len(pixels["nodes"])==len(report["cfg"]),"program/chain frames became phantom selectable states"
    result={"scene":scene,"pixels":pixels,"programs":len(report["programs"]),
            "relative_native":relative,"requested_detail_regions":regions,"renderer":webgpu_evidence(page)}
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
