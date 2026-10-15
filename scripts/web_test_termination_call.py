"""Termination call browser acceptance cases."""

from pathlib import Path
from tempfile import TemporaryDirectory
import json
from web_test_ending_helpers import selected_ending
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import analysis_server, complete_submission, observe_webgpu, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_pixels import node_snapshot
from web_test_rpc_helpers import EntryRpcFixture, automatic_rpc_entry, submit_rpc


def termination_call_interactions(browser,args):
    class TerminalRpc(EntryRpcFixture):
        root_code='0x6003565b5f5f5f5f5f73'+'22'*20+'61fffff100'
        child_code='0x00'
    with TerminalRpc() as rpc,TemporaryDirectory(prefix='termination-rpc-') as directory:
        providers=[{'id':'terminal-world','name':'Termination child frame fixture','endpoint':rpc.endpoint}]
        config=Path(directory)/'providers.json';config.write_text(json.dumps({'providers':providers})+'\n')
        with analysis_server(args,config) as url:
            page=browser.new_page(viewport={'width':1440,'height':1000});observe_webgpu(page)
            automatic_rpc_entry(page,url,providers[0],args.output,'termination-call-startup')
            canvas=page.locator('#evm-canvas')
            admitted=submit_rpc(page,canvas,rpc,args.output,'termination-child-call',0,providers)
            reply=complete_submission(page,admitted)
            assert reply.status==200 and 'Ok' in reply.json()['result'],reply.json()
            report=reply.json()['result']['Ok']
            (args.output/'termination-child-call-analysis.json').write_text(json.dumps(report,indent=2)+'\n')
            depths={node['id']:node['frame_depth'] for node in report['cfg']}
            failure=next(edge for edge in report['edges'] if edge['kind']=='Failure' and depths[edge['from']]==depths[edge['to']]==1)
            caller=failure['from']
            assert not any(outcome['state']==caller for outcome in report['outcomes'])
            returning=next(edge for edge in report['edges'] if edge['kind']=='Return' and depths[edge['from']]==2 and depths[edge['to']]==1)
            canvas.click(position={'x':1440-180,'y':12});hide_inspector(page,canvas)
            page.keyboard.press('2');settle_gesture(page);graph_mode(page,canvas,'States');page.keyboard.press('f');settle_gesture(page)
            submitted=[]
            page.on('request',lambda request:submitted.append(request.url) if request.method=='POST' and request.url.endswith('/api/tasks') else None)
            # The one green child frame and one green resumed root are verified
            # by real clicks and original state IDs, not inferred from opcode.
            scene=node_snapshot(page,canvas,args.output,'termination-call-overview',measure_nodes=True)
            frame=None
            for record in scene['node_records']:
                if record['kind']!='normal':continue
                box=record['bounds'];canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2});page.mouse.move(2,2);settle_gesture(page)
                if f"Selected {symbol('σ',returning['from'])};" not in page.locator('#analysis-status').text_content():continue
                summary=selected_ending(page)
                assert summary['frame_normal']==1 and summary['root_normal']==summary['root_failure']==0,summary
                frame={'summary':summary,'pixels':node_snapshot(page,canvas,args.output,'termination-frame-return-selected')}
                break
            assert frame is not None, 'real child RETURN edge did not paint a normal frame ending'
            neutral=None
            for record in scene['node_records']:
                if record['kind']!='ordinary':continue
                box=record['bounds'];canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2});page.mouse.move(2,2);settle_gesture(page)
                if f"Selected {symbol('σ',caller)};" not in page.locator('#analysis-status').text_content():continue
                summary=selected_ending(page)
                assert summary['kind']=='no termination evidence' and summary['continuing']==1,summary
                assert summary['root_failure']==summary['frame_failure']==0,summary
                neutral={'summary':summary,'pixels':node_snapshot(page,canvas,args.output,'termination-call-failure-continuing')}
                break
            assert neutral is not None, 'same-depth call failure was falsely classified as a terminal ending'
            assert not submitted, 'terminal node selection reran the RPC analysis'
            result={'frame_return':frame,'recoverable_call_failure':neutral,'typed_failure_edge':failure,'typed_return_edge':returning,'renderer':webgpu_evidence(page)}
            page.close()
            return result


def run(context):
    return termination_call_interactions(context.browser, context.args)
