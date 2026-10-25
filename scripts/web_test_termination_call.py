"""Termination call browser acceptance cases."""

from web_test_hybrid import oracle_graph, source_instructions, source_program
from pathlib import Path
from tempfile import TemporaryDirectory
import json
from web_test_ending_helpers import selected_ending
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import analysis_server, complete_submission, observe_webgpu, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_pixels import node_snapshot
from web_test_rpc_helpers import EntryRpcFixture, automatic_rpc_entry, submit_rpc


def _termination_call_oracle(report, root_code, root_address, child_address):
    root=report['metadata']['root_program']
    assert report['metadata']['entry_address'].lower()==root_address.lower()
    assert source_program(report,root)['bytecode'].removeprefix('0x')==root_code.removeprefix('0x')
    entry,=[node for node in report['cfg'] if node['frame_depth']==1 and node['start_pc']==0]
    caller,=[node for node in report['cfg'] if node['frame_depth']==1 and node['start_pc']==3]
    child,=[node for node in report['cfg'] if node['frame_depth']==2]
    failure,=[edge for edge in report['edges'] if edge['kind']=='Failure']
    returning,=[edge for edge in report['edges'] if edge['kind']=='Return']
    by_id={node['id']:node for node in report['cfg']}
    rejected=by_id[failure['to']];resumed=by_id[returning['to']]
    assert len({entry['id'],caller['id'],child['id'],rejected['id'],resumed['id']})==len(by_id)==5
    assert {(edge['from'],edge['to'],edge['kind']) for edge in report['edges']}=={
        (entry['id'],caller['id'],'Jump'),(caller['id'],rejected['id'],'Failure'),
        (caller['id'],child['id'],'Call'),(child['id'],resumed['id'],'Return')}
    details={item['state']:item for item in report['states']}
    ssa={item['state']:item for item in report['ssa']['blocks']}
    instructions={entry['id']:[(0,0x60,'Completed'),(2,0x56,'Dispatched')],
                  caller['id']:[(3,0x5b,'Completed')]+[(pc,0x5f,'Completed') for pc in range(4,9)]+
                               [(9,0x73,'Completed'),(30,0x61,'Completed'),(33,0xf1,'Dispatched')],
                  child['id']:[(0,0x00,'Dispatched')],
                  rejected['id']:[(34,0x00,'Dispatched')],resumed['id']:[(34,0x00,'Dispatched')]}
    for node in report['cfg']:
        is_child=node['id']==child['id']
        address=child_address if is_child else root_address
        assert node['frame_depth']==(2 if is_child else 1)
        assert node['code_address'].lower()==node['storage_address'].lower()==address.lower()
        assert node['program']==(child['program'] if is_child else root)
        source=source_program(report,node['program'])
        assert source['id']==node['program'] and source['code_address'].lower()==address.lower() and source['kind']=='Runtime'
        assert source['bytecode'].removeprefix('0x')==('00' if is_child else root_code.removeprefix('0x'))
        expected=instructions[node['id']]
        assert node['start_pc']==expected[0][0] and node['executed_pcs']==[pc for pc,_,_ in expected]
        assert [(item['pc'],item['opcode']) for item in source_instructions(report,node)]==[(pc,op) for pc,op,_ in expected]
        block=ssa[node['id']]
        assert block['coverage']=='Current'
        assert [(item['pc'],item['opcode'],item['progress']) for item in block['instructions']]==expected
        assert not any(item['fault'] for item in block['instructions'])
        frames=details[node['id']]['entry']['frames']
        assert len(frames)==node['frame_depth'] and frames[0]['program']==root
        assert [frame['index'] for frame in frames]==list(range(node['frame_depth']))
        assert frames[0]['code_address'].lower()==frames[0]['storage_address'].lower()==root_address.lower()
        assert frames[0]['code_hash']==source_program(report,root)['code_hash'] and frames[-1]['code_hash']==source['code_hash']
        assert frames[-1]['program']==node['program'] and frames[-1]['basic_block']==node['basic_block']
        assert frames[-1]['code_address']==node['code_address'] and frames[-1]['storage_address']==node['storage_address']
    child_frames=details[child['id']]['entry']['frames']
    assert child_frames[0]['basic_block']==caller['basic_block']
    assert child_frames[1]['continuation']['return_block']==rejected['basic_block']==resumed['basic_block']

    def empty_data(index):
        data=report['byte_arrays'][index]
        assert not data['memory'] and [int(word,16) for word in data['length']['constants']]==[0],data
        nodes=report['symbolic']['nodes'];view=nodes[data['symbolic']['node']]['View']
        assert nodes[view['length']['node']]=={'Constant':'0x0'},data

    empty_data(child_frames[1]['calldata'])
    for node,value in [(rejected,0),(resumed,1)]:
        frame=details[node['id']]['entry']['frames'][0]
        assert [int(word,16) for word in frame['stack'][-1]['constants']]==[value]
        empty_data(frame['returndata'])
    expected={entry['id']:{'Failure'},caller['id']:{'Failure'},
              rejected['id']:{'Return'},resumed['id']:{'Return'}}
    observed={state:set() for state in expected}
    for outcome in report['outcomes']:
        state=outcome['state'];kind=outcome['kind']
        assert state in expected and kind in expected[state],outcome
        observed[state].add(kind)
        empty_data(outcome['data'])
        machine=details[state]['entry']
        saved=machine['frames'][0]['rollback_store']
        # No writes and zero CALL value: every completion retains the complete
        # pre-entry store, with transient/touch flags finalized. In particular,
        # CALL's two pre-memory gas failures are root outcomes of the caller;
        # its separate same-depth rejection edge still resumes at PC 34.
        assert report['stores'][outcome['store']]==report['stores'][saved],outcome
        assert report['stores'][machine['store']]['touched_default'] is False
    assert observed==expected,observed
    return failure,returning


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
            reply=complete_submission(page,admitted, namespace="Native")
            assert reply.status==200 and 'Ok' in reply.json()['result'],reply.json()
            report=reply.report
            (args.output/'termination-child-call-analysis.json').write_text(json.dumps(report,indent=2)+'\n')
            assert report["hybrid"]["coverage"]["baseline_current"]
            report=oracle_graph(report, "Native")
            failure,returning=_termination_call_oracle(report,rpc.root_code,rpc.root_address,rpc.child_address)
            caller=failure['from']
            sources={node['id']:node['program'] for node in report['cfg']}
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
                assert f"Selected {symbol('σ',returning['from'])}; source {symbol('P',sources[returning['from']])};" in page.locator('#analysis-status').text_content()
                summary=selected_ending(page)
                assert summary=={'kind':'normal','members':1,'root_normal':0,'root_revert':0,'root_failure':0,
                                 'frame_normal':1,'frame_revert':0,'frame_failure':0,'continuing':0,
                                 'unresolved':0,'non_current':0,'unobserved':0},summary
                frame={'summary':summary,'pixels':node_snapshot(page,canvas,args.output,'termination-frame-return-selected')}
                assert frame['pixels']['selected_kind']=='normal',frame
                break
            assert frame is not None, 'real child RETURN edge did not paint a normal frame ending'
            neutral=None
            for record in scene['node_records']:
                if record['kind']!='ordinary':continue
                box=record['bounds'];canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2});page.mouse.move(2,2);settle_gesture(page)
                if f"Selected {symbol('σ',caller)};" not in page.locator('#analysis-status').text_content():continue
                assert f"Selected {symbol('σ',caller)}; source {symbol('P',sources[caller])};" in page.locator('#analysis-status').text_content()
                summary=selected_ending(page)
                assert summary=={'kind':'mixed / possible ending','members':1,'root_normal':0,'root_revert':0,
                                 'root_failure':1,'frame_normal':0,'frame_revert':0,'frame_failure':0,
                                 'continuing':1,'unresolved':0,'non_current':0,'unobserved':0},summary
                neutral={'summary':summary,'pixels':node_snapshot(page,canvas,args.output,'termination-call-failure-continuing')}
                assert neutral['pixels']['selected_kind']=='ordinary',neutral
                break
            assert neutral is not None, 'CALL gas outcomes must coexist with same-depth continuation, without a frame failure'
            assert not submitted, 'terminal node selection reran the RPC analysis'
            result={'frame_return':frame,'recoverable_call_failure':neutral,'typed_failure_edge':failure,'typed_return_edge':returning,'renderer':webgpu_evidence(page)}
            page.close()
            return result


def run(context):
    return termination_call_interactions(context.browser, context.args)
