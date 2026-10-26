"""Termination browser acceptance cases."""

from web_test_hybrid import oracle_graph, source_instructions, source_program
from playwright.sync_api import expect
import json
from web_test_ending_helpers import selected_ending, warning_pixels
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_layout_helpers import layout_page, layout_submission
from web_test_pixels import node_snapshot


def _assert_root_block(report, node, code, instructions):
    root=report['metadata']['root_program']
    address=report['metadata']['entry_address']
    source=source_program(report,root)
    assert source['id']==root and source['code_address']==address and source['kind']=='Runtime'
    assert source['bytecode'].removeprefix('0x')==code
    assert node['frame_depth']==1 and node['program']==root
    assert node['code_address']==node['storage_address']==address
    decoded=source_instructions(report,node)
    assert [(item['pc'],item['opcode']) for item in decoded[:len(instructions)]]==[(pc,op) for pc,op,_ in instructions]
    assert len(decoded)==len(instructions) or instructions[-1][2]=='Faulted'
    assert node['start_pc']==instructions[0][0] and node['executed_pcs']==[pc for pc,_,_ in instructions]
    details=next(item for item in report['states'] if item['state']==node['id'])
    frames=details['entry']['frames']
    assert len(frames)==1 and frames[0]['program']==root
    assert frames[0]['code_address']==frames[0]['storage_address']==address
    assert frames[0]['code_hash']==source['code_hash'] and frames[0]['index']==0
    assert frames[0]['basic_block']==node['basic_block']
    block=next(item for item in report['ssa']['blocks'] if item['state']==node['id'])
    assert block['coverage']=='Current'
    assert [(item['pc'],item['opcode'],item['progress']) for item in block['instructions']]==instructions
    assert all(item['fault']==(item['progress']=='Faulted') for item in block['instructions'])


def _assert_root_outcomes(report, expected):
    """Check every completion; only the named block entries may fail late."""
    def references(value, target):
        if isinstance(value,dict):
            return int(value==target)+sum(references(item,target) for item in value.values())
        if isinstance(value,list):
            return sum(references(item,target) for item in value)
        return 0

    observed={state:set() for state in expected}
    details={item['state']:item for item in report['states']}
    nodes=report['symbolic']['nodes']
    ripemd='0x'+'00'*19+'03'
    for outcome in report['outcomes']:
        state=outcome['state'];kind=outcome['kind']
        assert state in expected and kind in expected[state],outcome
        observed[state].add(kind)
        data=report['byte_arrays'][outcome['data']]
        assert not data['memory'] and [int(word,16) for word in data['length']['constants']]==[0],outcome
        view=nodes[data['symbolic']['node']]['View']
        assert nodes[view['length']['node']]=={'Constant':'0x0'},outcome
        entry=details[state]['entry']
        active=report['stores'][entry['store']]
        assert next((item['touched'] for item in active['accounts'] if item['address']==ripemd),active['touched_default']) is None
        saved=report['stores'][entry['frames'][0]['rollback_store']]
        store=report['stores'][outcome['store']]
        if kind=='Return':
            # UnknownTo calls::initial uses may_touch_all. Finalization prepares
            # an independent unlisted persistent base, preserving listed owners.
            assert report['metadata']['environment']['to']=={'Symbolic':{'kind':'To','index':None}},outcome
            assert active['touched_default'] is None and saved['touched_default'] is False,outcome
            root_ref=store['storage_root']
            assert set(root_ref)=={'node'} and type(root_ref['node']) is int and 0<=root_ref['node']<len(nodes),outcome
            root=nodes[root_ref['node']]
            assert isinstance(root,dict) and set(root)=={'SlotsInput'},outcome
            token=root['SlotsInput']['source']['token']
            assert set(token)=={'node'} and type(token['node']) is int and 0<=token['node']<len(nodes),outcome
            assert root=={'SlotsInput':{'source':{'token':token,'namespace':'0x0'},'bindings':[]}},outcome
            fresh=nodes[token['node']]
            assert isinstance(fresh,dict) and set(fresh)=={'Fresh'},outcome
            assert type(fresh['Fresh']) is int and fresh['Fresh']>=0,outcome
            # Check the whole report jointly: neither the root nor its fresh
            # source may alias saved/known storage, bytes, lengths or guards.
            assert sum(node==fresh for node in nodes)==1,outcome
            assert references(report,root_ref)==references(report,token)==1,outcome
            assert store=={**saved,'storage_root':root_ref},outcome
        else:
            assert {key:value for key,value in store.items() if key!='accounts'}=={key:value for key,value in saved.items() if key!='accounts'},outcome
        before={item['address']:item for item in saved['accounts']}
        after={item['address']:item for item in store['accounts']}
        assert all(after.get(address)==account for address,account in before.items()),outcome
        extra=set(after)-set(before)
        assert extra<=({ripemd} if kind in ('Revert','Failure') else set()),outcome
        if extra:
            # calls::initial may_touch_all + Store::restore's sticky RIPEMD
            # touch permits EIP-161's optional deletion of this account only.
            account=after[ripemd]
            root=nodes[account['storage_root']['node']]['SlotsIte']
            source=nodes[root['then_state']['node']]['SlotsInput']
            assert int(source['source']['namespace'],16)==3<<8 and not source['bindings']
            original=nodes[before[report['metadata']['entry_address']]['storage_root']['node']]['SlotsInput']
            assert source['source']['token']==original['source']['token']
            assert 'Fresh' in nodes[root['condition']['node']] and nodes[root['else_state']['node']]=='SlotsZero'
            assert account=={'address':ripemd,'existence':'Unknown','balance':saved['balance_default'],
                             'nonce':saved['nonce_default'],'code_kind':'Unknown','code_hash':None,
                             'program':None,'delegation_target':None,'storage_default':saved['storage_default'],
                             'storage_root':account['storage_root'],'transient_root':before[report['metadata']['entry_address']]['transient_root'],
                             'storage':[],'transient_default':saved['transient_default'],'transient':[],
                             'created':False,'touched':False,'pending_destruction':False},outcome
    assert observed==expected,observed


def _termination_oracle(report, code, kind):
    entry,=[node for node in report['cfg'] if node['start_pc']==0]
    target,=[node for node in report['cfg'] if node['start_pc']==3]
    assert {node['id'] for node in report['cfg']}=={entry['id'],target['id']}
    _assert_root_block(report,entry,code,[(0,0x60,'Completed'),(2,0x56,'Dispatched')])
    endings={'Return':[(4,0x00,'Dispatched')],
             'Revert':[(4,0x5f,'Completed'),(5,0x5f,'Completed'),(6,0xfd,'Dispatched')],
             'Failure':[(4,0xfe,'Faulted')]}
    _assert_root_block(report,target,code,[(3,0x5b,'Completed')]+endings[kind])
    # Symbolic logical ADDRESS may touch RIPEMD at entry. late_failure retains
    # a rolled-back Failure at this exact source, alongside STOP/REVERT/INVALID.
    details=next(item for item in report['states'] if item['state']==entry['id'])
    if 'Symbolic' in report['metadata']['environment']['to']:
        assert report['stores'][details['entry']['store']]['touched_default'] is None
        _assert_root_outcomes(report,{entry['id']:{'Failure'},target['id']:{kind,'Failure'}})
    else:
        # REVERT retains its opcode failure alternative. STOP's symbolic-owner
        # finalization failure is outside this explicitly conditioned input.
        _conditioned_root_outcomes(report,{entry['id']:{'Failure'},
                                          target['id']:({'Revert','Failure'} if kind=='Revert' else {kind})})
    return entry,target


def _mixed_termination_oracle(report):
    members={node['id'] for node in report['cfg'] if node['start_pc']==9}
    assert len(members)==2
    conditioned='Concrete' in report['metadata']['environment']['to']
    expected={node['id']:(set() if conditioned else {'Failure'}) for node in report['cfg']}
    if conditioned:expected[report['cfg'][0]['id']].add('Failure')
    for node in report['cfg']:
        instructions={0:[(0,0x34,'Completed'),(1,0x60,'Completed'),(3,0x57,'Dispatched')],
                      4:[(4,0x60,'Completed'),(6,0x56,'Dispatched')],
                      7:[(7,0x5b,'Completed'),(8,0x5f,'Completed')]}
        if node['id'] in members:
            assert len(node['entry_stack']) in (0,1)
            tail=[(10,0x50,'Faulted')] if not node['entry_stack'] else [(10,0x50,'Completed'),(11,0x00,'Dispatched')]
            instructions[9]=[(9,0x5b,'Completed')]+tail
            expected[node['id']].add('Return' if node['entry_stack'] else 'Failure')
        _assert_root_block(report,node,'346007576009565b5f5b5000',instructions[node['start_pc']])
    assert {tuple(sorted(expected[member])) for member in members}==({('Return',),('Failure',)} if conditioned else {('Failure','Return'),('Failure',)})
    if conditioned:_conditioned_root_outcomes(report,expected)
    else:_assert_root_outcomes(report,expected)
    return members


def _conditioned_root_outcomes(report, expected):
    """No-op terminal fixtures under the explicit Native root-address subset."""
    assert report['metadata']['environment']['to']=={'Concrete':report['metadata']['entry_address']}
    observed={state:set() for state in expected}
    for outcome in report['outcomes']:
        observed[outcome['state']].add(outcome['kind'])
        entry=next(state['entry'] for state in report['states'] if state['state']==outcome['state'])
        saved=report['stores'][entry['frames'][0]['rollback_store']]
        assert saved['touched_default'] is False
        assert report['stores'][outcome['store']]==saved, outcome
        data=report['byte_arrays'][outcome['data']]
        assert not data['memory'] and data['length']['constants']==['0x0']
        view=report['symbolic']['nodes'][data['symbolic']['node']]['View']
        assert report['symbolic']['nodes'][view['length']['node']]=={'Constant':'0x0'}
    assert observed==expected, observed


def _select_native(page, canvas, scene, state, program, color):
    label=f"Selected {symbol('σ',state)}; source {symbol('P',program)};"
    for record in scene['node_records']:
        if record['kind']!=color:continue
        left,top,right,bottom=record['bounds']
        canvas.click(position={'x':(left+right)/2,'y':(top+bottom)/2})
        page.mouse.move(2,2);settle_gesture(page)
        if label in page.locator('#analysis-status').text_content():return record
    raise AssertionError(f'native selection missing: {label}')


def termination_interactions(browser, url, args):
    page,canvas,errors=layout_page(browser,url)
    output=args.output
    posts=[]
    page.on('request',lambda request:posts.append(request.url)
            if request.method=='POST' and request.url.endswith('/api/tasks') else None)
    results={}
    for name,code,kind,color in [('normal','6003565b00','Return','ordinary'),
                                ('revert','6003565b5f5ffd','Revert','ordinary'),
                                ('failure','6003565bfe','Failure','failure')]:
        report=layout_submission(page,canvas,code,output,f'termination-{name}')
        envelope=json.loads((output/f'termination-{name}-analysis.json').read_text())
        original=oracle_graph(envelope, "Baseline")
        assert original['metadata']['environment']['to']=={'Symbolic':{'kind':'To','index':None}}
        assert original['stores'][original['states'][0]['entry']['store']]['touched_default'] is None
        assert envelope['hybrid']['coverage']['baseline_current']
        expect(page.locator('#analysis-status')).to_contain_text('Additional condition: root ADDRESS =')
        entry,target=_termination_oracle(report,code,kind)
        color={'Return':'normal','Revert':'ordinary','Failure':'failure'}[kind]
        graph_mode(page,canvas,'States');page.keyboard.press('f');settle_gesture(page)
        before=node_snapshot(page,canvas,output,f'termination-{name}-unselected',measure_nodes=True)
        candidate=_select_native(page,canvas,before,target['id'],target['program'],color)
        left,top,right,bottom=candidate['bounds']
        selected=node_snapshot(page,canvas,output,f'termination-{name}-selected',measure_nodes=True)
        ending=selected_ending(page)
        expected_ending={'kind':{'Return':'normal','Revert':'mixed / possible ending','Failure':'exception'}[kind],
                         'members':1,'root_normal':int(kind=='Return'),'root_revert':int(kind=='Revert'),
                         'root_failure':int(kind!='Return'),'frame_normal':0,'frame_revert':0,'frame_failure':0,
                         'continuing':0,'unresolved':0,'non_current':0,'unobserved':0}
        assert ending==expected_ending,ending
        assert selected['selected_kind']==color,selected
        assert candidate['kind']==selected['selected_kind'], 'selection covered the semantic background'
        low=None
        if name=='normal':
            pointer=((left+right)/2,(top+bottom)/2)
            for _ in range(12):
                if 'CFG detail: identities' in page.locator('#analysis-status').text_content():break
                page.mouse.move(*pointer);page.keyboard.down('Control');page.mouse.wheel(0,120);page.keyboard.up('Control');settle_gesture(page)
            page.mouse.move(2,2);settle_gesture(page)
            expect(page.locator('#analysis-status')).to_contain_text('CFG detail: identities')
            low=node_snapshot(page,canvas,output,'termination-normal-low-zoom',min_selected_pixels=1)
            assert low['selected_kind']==color,low
            assert selected_ending(page)==expected_ending
        results[name]={'typed_outcome':kind,'target_state':target['id'],'before':candidate,'selected':selected,'ending':ending,'low_zoom':low}
        # S0 deliberately retains the model's entry-failure alternative plus
        # the real observed JUMP. It must stay neutral and label continuation.
        page.keyboard.press('f');settle_gesture(page)
        scene=node_snapshot(page,canvas,output,f'termination-{name}-refit',measure_nodes=True)
        _select_native(page,canvas,scene,entry['id'],entry['program'],'ordinary')
        mixed=selected_ending(page)
        assert mixed=={**expected_ending,'kind':'mixed / possible ending','root_normal':0,
                       'root_revert':0,'root_failure':1,'continuing':1},mixed
        neutral=node_snapshot(page,canvas,output,f'termination-{name}-entry-mixed')
        assert neutral['selected_kind']=='ordinary',neutral
        results[name]['entry_mixed']=mixed

    mixed_report=layout_submission(page,canvas,'346007576009565b5f5b5000',output,'termination-mixed-block')
    members=_mixed_termination_oracle(mixed_report)
    graph_mode(page,canvas,'Blocks');page.keyboard.press('f');settle_gesture(page)
    scene=node_snapshot(page,canvas,output,'termination-mixed-block-before',measure_nodes=True)
    box=max(scene['nodes'],key=lambda rect:(rect[1]+rect[3])/2)
    canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2});page.mouse.move(2,2);settle_gesture(page)
    root=mixed_report['metadata']['root_program']
    assert any(f"Selected {symbol('σ',member)}; source {symbol('P',root)};" in page.locator('#analysis-status').text_content() for member in members)
    summary=selected_ending(page)
    assert summary=={'kind':'mixed / possible ending','members':2,'root_normal':1,'root_revert':0,
                     'root_failure':1,'frame_normal':0,'frame_revert':0,'frame_failure':0,
                     'continuing':0,'unresolved':0,'non_current':0,'unobserved':0},summary
    mixed=node_snapshot(page,canvas,output,'termination-mixed-block-selected',measure_nodes=True)
    assert mixed['selected_kind']=='ordinary',mixed
    results['mixed_aggregation']={'members':sorted(members),'summary':summary,'pixels':mixed}

    partial=layout_submission(page,canvas,'5a5f5ff0',output,'termination-frontier')
    assert partial['status']=='Incomplete' and partial['frontiers']
    assert any(frontier['kind']=='Creation' and 'UnknownInitCode' in frontier['detail'] for frontier in partial['frontiers'])
    graph_mode(page,canvas,'States');page.keyboard.press('f');settle_gesture(page)
    frontier=node_snapshot(page,canvas,output,'termination-frontier-selected',measure_nodes=True)
    summary=selected_ending(page)
    assert summary['unresolved']>0 and frontier['selected_kind']=='ordinary',summary
    dashes=warning_pixels(page,canvas.screenshot(),frontier['bounds'])
    assert dashes['pixels']>20 and dashes['runs']>=3,dashes
    assert frontier['selected_pixels']>20, 'warning replaced the independent selection outline'
    results['frontier']={'summary':summary,'pixels':frontier,'warning_dashes':dashes}
    assert len(posts)==5, 'selection or semantic zoom reran termination analyses'
    assert not errors,errors
    results['renderer']=webgpu_evidence(page)
    page.close()
    (output/'termination-report.json').write_text(json.dumps(results,indent=2)+'\n')
    return results


def run(context):
    return termination_interactions(context.browser, context.url, context.args)
