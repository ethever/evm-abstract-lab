"""Termination browser acceptance cases."""

from playwright.sync_api import expect
import json
from web_test_ending_helpers import selected_ending, warning_pixels
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_layout_helpers import layout_page, layout_submission
from web_test_pixels import node_snapshot


def termination_interactions(browser, url, args):
    page,canvas,errors=layout_page(browser,url)
    output=args.output
    posts=[]
    page.on('request',lambda request:posts.append(request.url)
            if request.method=='POST' and request.url.endswith('/api/tasks') else None)
    results={}
    for name,code,kind,color in [('normal','6003565b00','Return','normal'),
                                ('revert','6003565b5f5ffd','Revert','revert'),
                                ('failure','6003565bfe','Failure','failure')]:
        report=layout_submission(page,canvas,code,output,f'termination-{name}')
        target=next(node for node in report['cfg'] if node['start_pc']==3)
        assert {item['kind'] for item in report['outcomes'] if item['state']==target['id']}=={kind}
        graph_mode(page,canvas,'States');page.keyboard.press('f');settle_gesture(page)
        before=node_snapshot(page,canvas,output,f'termination-{name}-unselected',measure_nodes=True)
        candidate=next(record for record in before['node_records'] if record['kind']==color)
        left,top,right,bottom=candidate['bounds']
        canvas.click(position={'x':(left+right)/2,'y':(top+bottom)/2});page.mouse.move(2,2);settle_gesture(page)
        expect(page.locator('#analysis-status')).to_contain_text(f"Selected {symbol('σ',target['id'])};")
        selected=node_snapshot(page,canvas,output,f'termination-{name}-selected',measure_nodes=True)
        ending=selected_ending(page)
        assert ending['members']==1 and ending['continuing']==ending['unresolved']==0,ending
        assert ending[{'normal':'root_normal','revert':'root_revert','failure':'root_failure'}[color]]==1,ending
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
            assert low['selected_kind']=='normal',low
            assert selected_ending(page)['root_normal']==1
        results[name]={'typed_outcome':kind,'target_state':target['id'],'before':candidate,'selected':selected,'ending':ending,'low_zoom':low}
        # S0 deliberately retains the model's entry-failure alternative plus
        # the real observed JUMP. It must stay neutral and label continuation.
        page.keyboard.press('f');settle_gesture(page)
        scene=node_snapshot(page,canvas,output,f'termination-{name}-refit',measure_nodes=True)
        entry=next(record for record in scene['node_records'] if record['kind']=='ordinary')
        box=entry['bounds'];canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2})
        page.mouse.move(2,2);settle_gesture(page)
        expect(page.locator('#analysis-status')).to_contain_text('Selected σ₀;')
        mixed=selected_ending(page)
        assert mixed['kind']=='mixed / possible ending' and mixed['root_failure']==mixed['continuing']==1,mixed
        neutral=node_snapshot(page,canvas,output,f'termination-{name}-entry-mixed')
        assert neutral['selected_kind']=='ordinary',neutral
        results[name]['entry_mixed']=mixed

    mixed_report=layout_submission(page,canvas,'346007576009565b5f5b5000',output,'termination-mixed-block')
    members={node['id'] for node in mixed_report['cfg'] if node['start_pc']==9}
    assert len(members)==2
    observed={member:tuple(sorted({item['kind'] for item in mixed_report['outcomes'] if item['state']==member})) for member in members}
    assert set(observed.values())=={('Return',),('Failure',)},observed
    graph_mode(page,canvas,'Blocks');page.keyboard.press('f');settle_gesture(page)
    scene=node_snapshot(page,canvas,output,'termination-mixed-block-before',measure_nodes=True)
    box=max(scene['nodes'],key=lambda rect:(rect[1]+rect[3])/2)
    canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2});page.mouse.move(2,2);settle_gesture(page)
    assert any(f"Selected {symbol('σ',member)};" in page.locator('#analysis-status').text_content() for member in members)
    summary=selected_ending(page)
    assert summary['members']==2 and summary['root_normal']==summary['root_failure']==1 and summary['continuing']==0,summary
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
