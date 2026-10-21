"""Ssa values browser acceptance cases."""

import base64
from playwright.sync_api import expect
import json
from web_test_graph_helpers import symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_layout_helpers import layout_page, layout_submission
from web_test_pixels import screenshot_metrics
from web_test_ssa_helpers import ssa_details_toggle


def ssa_definition_tokens(page, png, top):
    """Locate real purple definition tokens; ordinary operand links are blue."""
    return page.evaluate("""async ({png,top}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        const scale=image.width/innerWidth,left=Math.round(4*scale),right=Math.round(350*scale);
        const first=Math.round(top*scale),last=Math.round((innerHeight-30)*scale);
        const width=right-left,height=last-first;
        const pixels=context.getImageData(left,first,width,height).data,rows=[];
        for(let y=0;y<height;y++) {
            let x0=width,x1=-1,count=0;
            for(let x=0;x<width;x++) {
                const i=(y*width+x)*4,r=pixels[i],g=pixels[i+1],b=pixels[i+2];
                if(r>=150 && g>=100 && g<=205 && b>=210 && b-r>=25 && r-g>=20) {
                    x0=Math.min(x0,x);x1=Math.max(x1,x);count++;
                }
            }
            if(!count) continue;
            const current=[(left+x0)/scale,(first+y)/scale,(left+x1+1)/scale,(first+y+1)/scale];
            const previous=rows.at(-1);
            if(previous && current[1]-previous[3]<=4) {
                previous[0]=Math.min(previous[0],current[0]);previous[2]=Math.max(previous[2],current[2]);previous[3]=current[3];
            } else rows.push(current);
        }
        return rows;
    }""",{'png':base64.b64encode(png).decode(),'top':top})

def ssa_values_interactions(browser, url, output):
    page,canvas,errors=layout_page(browser,url)
    status=page.locator('#analysis-status')
    posts=[]
    page.on('request',lambda request:posts.append(request.url)
            if request.method=='POST' and request.url.endswith('/api/tasks') else None)
    results={}
    cases=[('fold','600260030100','Converged'),
           ('copy','345f0100','Converged'),
           ('partial','60026003015a5f5ff0','Incomplete')]
    for name,code,completion in cases:
        if 'Inspector hidden' not in status.text_content():hide_inspector(page,canvas)
        report=layout_submission(page,canvas,code,output,f'ssa-values-{name}')
        assert report['schema_version']==17 and report['status']==completion
        ssa=report['ssa']
        assert len(ssa['values'])==ssa['value_count']
        assert [value['id'] for value in ssa['values']]==list(range(ssa['value_count']))
        block=next(block for block in ssa['blocks'] if any(item['opcode']==1 for item in block['instructions']))
        operation=next(item for item in block['instructions'] if item['opcode']==1)
        value=ssa['values'][operation['results'][0]]
        assert operation['progress']=='Completed' and operation['effect_result'] is not None
        assert value['dependencies']==operation['operands']
        if name=='copy':
            caller=next(item['results'][0] for item in block['instructions'] if item['opcode']==0x34)
            assert operation['operation']=={'Copy':{'Value':caller}}
            assert value['equivalent']==caller
        else:
            assert operation['operation']=={'Constant':'0x5'}
            assert value['equivalent'] is None
            assert any(fact['scope']=='Definition' and fact['constant']=='0x5' for fact in value['facts'])
        if name=='partial':
            assert any(fact['scope']=={'State':block['state']} for fact in value['facts'])
            assert any(frontier['kind']=='Creation' and 'UnknownInitCode' in frontier['detail'] for frontier in report['frontiers'])
        page.keyboard.press('3');settle_gesture(page);page.mouse.move(2,2)
        png=canvas.screenshot(path=str(output/f'ssa-values-{name}-table.png'))
        selected=screenshot_metrics(page,png,(4,75,1436,970))
        assert selected['bounds'],selected
        tokens=ssa_definition_tokens(page,png,selected['bounds'][3])
        ids=[phi['result'] for phi in block['phis']]
        ids.extend(result for item in block['instructions'] for result in item['results'])
        index=ids.index(value['id'])
        assert len(tokens)>index,(tokens,ids)
        box=tokens[index]
        canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2})
        page.mouse.move(2,2);settle_gesture(page)
        expect(status).to_contain_text(f"Inspector: SSA value %{value['id']}")
        expect(status).to_contain_text('COPY' if name=='copy' else 'CONST 0x5')
        if name=='partial':expect(status).to_contain_text(f"State {symbol('σ',block['state'])}")
        canvas.screenshot(path=str(output/f'ssa-values-{name}-inspector.png'))
        before=status.text_content()
        ssa_details_toggle(page,canvas,'Full')
        expect(status).to_contain_text(f"Inspector: SSA value %{value['id']}")
        ssa_details_toggle(page,canvas,'Compact')
        navigation=None
        if name=='fold':
            # The selected inspector tab anchors its public ValueId editor and
            # the first-row source button without exposing egui rectangles.
            png=canvas.screenshot()
            inspector=screenshot_metrics(page,png,(4,650,1436,970))
            assert inspector['bounds'],inspector
            tab_bottom=inspector['bounds'][3]
            native_region=(4,selected['bounds'][3]+1,1436,450)
            native_before=screenshot_metrics(page,png,native_region)
            assert native_before['bounds'],native_before
            canvas.dblclick(position={'x':85,'y':tab_bottom+12})
            page.keyboard.press('Control+A');page.keyboard.insert_text('0');page.keyboard.press('Enter')
            page.mouse.move(2,2);settle_gesture(page)
            expect(status).to_contain_text('Inspector: SSA value %0')
            expect(status).to_contain_text('CONST 0x2')
            inspected=screenshot_metrics(page,canvas.screenshot(),native_region)
            assert inspected['bounds']==native_before['bounds'], 'inspecting another value moved the selected native instruction'
            canvas.click(position={'x':100,'y':tab_bottom+44})
            page.mouse.move(2,2);settle_gesture(page)
            source=screenshot_metrics(page,canvas.screenshot(path=str(output/'ssa-values-source-navigation.png')),native_region)
            assert source['bounds'],source
            first_y=(tokens[0][1]+tokens[0][3])/2
            assert source['bounds'][1]<=first_y<=source['bounds'][3], 'Go to source did not select the original PUSH instruction'
            navigation={'inspector_tab':inspector['bounds'],'before':native_before['bounds'],
                        'inspected':inspected['bounds'],'source':source['bounds']}
        results[name]={'value':value,'original_instruction':operation,'definition_token':box,
                       'inspector':before,'navigation':navigation,
                       'cfg_states':len(report['cfg']),'frontiers':len(report['frontiers'])}
    assert len(posts)==len(cases),'value inspection or reading-mode changes reran analysis'
    assert not errors,errors
    results['renderer']=webgpu_evidence(page)
    results['analysis_submissions']=len(posts)
    (output/'ssa-values-report.json').write_text(json.dumps(results,indent=2)+'\n')
    page.close()
    return results


def run(context):
    return ssa_values_interactions(context.browser, context.url, context.output)
