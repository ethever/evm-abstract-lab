"""Hierarchy calls browser acceptance cases."""

from pathlib import Path
from tempfile import TemporaryDirectory
import json
from web_test_graph_helpers import graph_mode
from web_test_harness import analysis_server, complete_submission, observe_webgpu, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_layout_helpers import LAYOUT_FIXTURES, layout_counts
from web_test_pixels import node_snapshot
from web_test_rpc_helpers import EntryRpcFixture, automatic_rpc_entry, submit_rpc


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

def hierarchy_call_interactions(browser,args):
    with NestedLayoutRpcFixture() as rpc, TemporaryDirectory(prefix="layout-rpc-") as directory:
        providers=[{"id":"layout-world","name":"Synthetic nested layout calls","endpoint":rpc.endpoint}]
        config=Path(directory)/"providers.json";config.write_text(json.dumps({"providers":providers})+"\n")
        with analysis_server(args,config) as url:
            page=browser.new_page(viewport={"width":1440,"height":1000});observe_webgpu(page)
            automatic_rpc_entry(page,url,providers[0],args.output,"hierarchy-call-startup")
            canvas=page.locator("#evm-canvas")
            admitted=submit_rpc(page,canvas,rpc,args.output,"hierarchy-calls",0,providers)
            reply=complete_submission(page,admitted)
            assert reply.status==200 and "Ok" in reply.json()["result"],reply.json()
            report=reply.json()["result"]["Ok"]
            assert len(report["programs"])>=3
            assert max(state["frame_depth"] for state in report["cfg"])==3
            assert sum(edge["kind"]=="Call" for edge in report["edges"])>=2
            assert sum(edge["kind"]=="Return" for edge in report["edges"])>=2
            (args.output/"hierarchy-nested-calls-analysis.json").write_text(json.dumps(report,indent=2)+"\n")
            canvas.click(position={"x":1440-180,"y":12});hide_inspector(page,canvas)
            page.keyboard.press("2");settle_gesture(page);graph_mode(page,canvas,"States")
            scene=layout_counts(page,report,mode="States")
            assert scene["groups"]>=3
            pixels=node_snapshot(page,canvas,args.output,"hierarchy-nested-calls",measure_nodes=True)
            assert len(pixels["nodes"])==len(report["cfg"]),"program/chain frames became phantom selectable states"
            return {"scene":scene,"pixels":pixels,"programs":len(report["programs"]),"renderer":webgpu_evidence(page)}


def run(context):
    return hierarchy_call_interactions(context.browser, context.args)
