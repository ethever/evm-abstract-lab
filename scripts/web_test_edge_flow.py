"""Edge flow browser acceptance cases."""

from pathlib import Path
import base64
from playwright.sync_api import expect
import json
import math
import time
from web_test_graph_helpers import graph_mode, graph_scope, pick_graph_instance, symbol
from web_test_harness import rendered_frame, resize_view, settle_gesture, webgpu_evidence
from web_test_layout_helpers import layout_fixture, layout_page, layout_submission
from web_test_pixels import assert_layout_same_geometry, assert_translation, node_snapshot


def flow_pixels(page, screenshot: bytes, regions, screenshot_box):
    """Locate bright moving arrowheads in real PNG pixels, outside node cards."""
    return page.evaluate("""async ({png, regions, box}) => {
        const image = new Image(); image.src = `data:image/png;base64,${png}`; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width=image.width; canvas.height=image.height;
        const context=canvas.getContext('2d'); context.drawImage(image,0,0);
        const scale=image.width/(box[2]-box[0]);
        const result={};
        for(const [name,region] of Object.entries(regions)) {
            const [left,top,right,bottom]=region.map((v,axis)=>Math.round((v-box[axis%2])*scale));
            const width=right-left,height=bottom-top;
            const pixels=context.getImageData(left,top,width,height).data;
            const mask=new Uint8Array(width*height), queue=new Int32Array(width*height);
            for(let at=0;at<mask.length;at++) {
                const i=at*4;
                // Blue/green moving heads are brighter than their kind-colored
                // base line, labels, spatial annotations, and the dark grid.
                if(pixels[i]>=150 && pixels[i+1]>=185 && pixels[i+2]>=205) mask[at]=1;
            }
            const markers=[];
            for(let start=0;start<mask.length;start++) {
                if(!mask[start]) continue;
                let read=0,write=1,sumX=0,sumY=0;
                queue[0]=start;mask[start]=0;
                while(read<write) {
                    const at=queue[read++],x=at%width,y=Math.floor(at/width);
                    sumX+=x;sumY+=y;
                    for(let dy=-1;dy<=1;dy++) for(let dx=-1;dx<=1;dx++) {
                        const nx=x+dx,ny=y+dy;
                        if(nx<0 || nx>=width || ny<0 || ny>=height) continue;
                        const next=ny*width+nx;
                        if(mask[next]) { mask[next]=0;queue[write++]=next; }
                    }
                }
                if(write>=3*scale*scale) markers.push({
                    center:[box[0]+(left+sumX/write)/scale,box[1]+(top+sumY/write)/scale],pixels:write});
            }
            const digest=await crypto.subtle.digest('SHA-256',pixels);
            result[name]={markers,bounds:region,hash:Array.from(new Uint8Array(digest),n=>n.toString(16).padStart(2,'0')).join('')};
        }
        return result;
    }""", {"png":base64.b64encode(screenshot).decode(),"regions":regions,"box":screenshot_box})

def flow_frames(page, canvas, output: Path, name: str, regions):
    captures=[]
    # Crop the actual browser capture, rather than downsampling a full PNG.
    # Full SwiftShader screenshots can take half a second: that would alias a
    # periodic moving head and make forward movement look like wraparound.
    box=[math.floor(min(region[0] for region in regions.values())),
         math.floor(min(region[1] for region in regions.values())),
         math.ceil(max(region[2] for region in regions.values())),
         math.ceil(max(region[3] for region in regions.values()))]
    canvas.screenshot(path=str(output/f"{name}-scene.png"))
    start=time.monotonic()
    max_interval=min(56,min(region[3]-region[1] for region in regions.values()))/(2*60)
    valid_pairs=0
    # egui reads performance.now for its animation phase. Hold that application
    # clock at each sample while the real compositor presents the frame: slow
    # SwiftShader PNG capture must not advance a periodic head past half a cycle.
    # Browser rAF and DOM timers still run normally, and each clock step exceeds
    # the selected edge's 33ms repaint deadline. Direction is checked below from
    # actual PNG pixels; unchanged or reversed frames still fail acceptance.
    step_seconds=min(0.1,max_interval/3)
    assert step_seconds>0.033, "flow sampling region must allow a full repaint step"
    cdp=page.context.new_cdp_session(page)
    clock_started=False
    try:
        clock_start=page.evaluate("""() => {
            const own=Object.getOwnPropertyDescriptor(performance,'now');
            const nativeNow=performance.now.bind(performance);
            const now=nativeNow();
            window.__evmFlowSamplingClock={own,nativeNow,now};
            Object.defineProperty(performance,'now',{
                configurable:true,value:() => window.__evmFlowSamplingClock.now});
            return now;
        }""")
        clock_started=True
        for index in range(24):
            animation_seconds=page.evaluate("""({start,step,index}) => {
                const clock=window.__evmFlowSamplingClock;
                clock.now=start+step*(index+1)*1000;
                return (performance.now()-start)/1000;
            }""", {"start":clock_start,"step":step_seconds,"index":index})
            rendered_frame(page)
            capture_begin=time.monotonic()-start
            captured=cdp.send("Page.captureScreenshot",{
                "format":"png","fromSurface":True,"captureBeyondViewport":False,
                "clip":{"x":box[0],"y":box[1],"width":box[2]-box[0],"height":box[3]-box[1],"scale":1}})
            capture_end=time.monotonic()-start
            png=base64.b64decode(captured["data"])
            (output/f"{name}-{index:02}.png").write_bytes(png)
            if captures and animation_seconds-captures[-1][0]["animation_seconds"]<max_interval:
                valid_pairs+=1
            captures.append(({"capture_begin_seconds":capture_begin,"capture_end_seconds":capture_end,
                              "animation_seconds":animation_seconds,
                              "animation_clock":"controlled performance.now", "screenshot_box":box},png))
            # Six timing-valid pairs provide several opportunities to see a head
            # fully inside the gap. Direction still requires its own two different
            # advancing frame pairs below; timing or presence alone cannot pass it.
            if len(captures)>=10 and valid_pairs>=6: break
    finally:
        try:
            # Preserve monotonicity when handing the application back to its
            # native clock, including on a failed capture or pixel assertion.
            if clock_started:
                try:
                    page.wait_for_function("""() => {
                        const clock=window.__evmFlowSamplingClock;
                        return clock.nativeNow()>=clock.now;
                    }""", timeout=5000)
                finally:
                    page.evaluate("""() => {
                        const clock=window.__evmFlowSamplingClock;
                        if(clock.own) Object.defineProperty(performance,'now',clock.own);
                        else delete performance.now;
                        delete window.__evmFlowSamplingClock;
                    }""")
        finally:
            cdp.detach()
    frames=[]
    for frame,png in captures:
        frame["regions"]=flow_pixels(page,png,regions,box)
        frames.append(frame)
    return frames

def assert_flow_direction(frames, region: str, direction):
    """Track nearby heads between actual frames; wraparound starts a new head."""
    advances=[]
    backwards=[]
    advanced_pairs=[]
    skipped_pairs=[]
    height=frames[0]["regions"][region]["bounds"][3]-frames[0]["regions"][region]["bounds"][1]
    # Normal routes move at 60 CSS pixels/s. A visible gap is no longer than
    # its route, so this conservative half-cycle limit prevents a reversed
    # periodic head from wrapping forward under a heavily loaded compositor.
    max_interval=min(56,height)/(2*60)
    for index,(before,after) in enumerate(zip(frames,frames[1:])):
        interval=after["animation_seconds"]-before["animation_seconds"]
        if interval>=max_interval:
            skipped_pairs.append({"index":index,"animation_interval":interval})
            continue
        old=before["regions"][region]["markers"]
        new=after["regions"][region]["markers"]
        advanced=False
        for marker in old:
            candidates=[next_marker for next_marker in new
                        if 1.5<math.dist(marker["center"],next_marker["center"])<25]
            if not candidates: continue
            nearest=min(candidates,key=lambda candidate:math.dist(marker["center"],candidate["center"]))
            delta=[b-a for a,b in zip(marker["center"],nearest["center"],strict=True)]
            along=sum(a*b for a,b in zip(delta,direction,strict=True))
            across=abs(delta[0]*direction[1]-delta[1]*direction[0])
            if across>2.0: continue
            if along>1.5:
                advances.append(along);advanced=True
            elif along<-1.5: backwards.append(along)
        if advanced: advanced_pairs.append(index)
    assert len(advanced_pairs)>=2 and len(backwards)<=len(advances)//3, (
        f"rendered heads did not move source to target in {region}: "
        f"forward={advances}, backward={backwards}, frames={frames}")
    return {"screen_direction":direction,"forward_steps":advances,"backward_steps":backwards,
            "advanced_pairs":advanced_pairs,"animation_clock":"controlled performance.now",
            "max_animation_interval":max_interval,"skipped_pairs":skipped_pairs}

def vertical_edge_region(source, target):
    assert source[3]+8<target[1], (source,target)
    return (max(source[0],target[0]),source[3]+4,
            min(source[2],target[2]),target[1]-4)

def loop_return_region(page, png, cards):
    """Find the upward return using its static target arrow at the upper card."""
    return page.evaluate("""async ({png,cards}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        const scale=image.width/innerWidth,left=Math.floor(Math.min(...cards.map(card=>card[0]))*scale);
        const top=Math.floor(cards[0][3]*scale),bottom=Math.ceil(cards[1][1]*scale);
        const width=Math.ceil(Math.max(...cards.map(card=>card[2]))*scale)-left,height=bottom-top;
        const data=context.getImageData(left,top,width,height).data;
        const columns=Array.from({length:width},()=>({count:0,first:height,last:0}));
        const mask=new Uint8Array(width*height);
        for(let y=0;y<height;y++) for(let x=0;x<width;x++) {
            const i=(y*width+x)*4,r=data[i],g=data[i+1],b=data[i+2];
            // The destination arrow retains the green kind color; moving
            // heads are brighter and must not affect destination detection.
            if(r<150 && g>=150 && g-r>=50 && b>=130) {
                mask[y*width+x]=1;
                const column=columns[x];column.count++;column.first=Math.min(column.first,y);column.last=Math.max(column.last,y);
            }
        }
        const targetScore=x=>{
            let score=0;
            for(let y=0;y<Math.min(height,10*scale);y++) for(let dx=-5*scale;dx<=5*scale;dx++) {
                if(Math.abs(dx)<=1.5*scale || x+dx<0 || x+dx>=width) continue;
                score+=mask[y*width+x+dx];
            }
            return score;
        };
        const candidates=columns.map((column,x)=>({x,score:targetScore(x),count:column.count}))
            .filter(column=>column.count>=20*scale);
        candidates.sort((a,b)=>b.score-a.score || b.count-a.count);
        const x=candidates[0]?.x ?? 0;
        const leg=columns[x];
        return {pixels:leg.count,target_arrow_pixels:targetScore(x),
                region:[(left+x)/scale-9,(top+leg.first)/scale+9,
                        (left+x)/scale+9,(top+leg.last)/scale-4]};
    }""", {"png":base64.b64encode(png).decode(),"cards":cards})

def edge_flow_interactions(browser, url: str, output: Path):
    page,canvas,errors=layout_page(browser,url)
    code,metadata=layout_fixture("linear-chain")
    report=layout_submission(page,canvas,code,output,"flow-chain")
    assert report["status"]=="Converged" and len(report["cfg"])==metadata["expected_native_states"]
    graph_mode(page,canvas,"States")
    page.keyboard.press("f");settle_gesture(page);page.mouse.move(2,2)
    scene=node_snapshot(page,canvas,output,"flow-chain-before",measure_nodes=True)
    cards=sorted(scene["nodes"],key=lambda card:card[1])
    assert len(cards)==4, cards
    # Match the rendered downward chain to the actual typed graph, rather than
    # assuming state IDs or substituting hand-authored routes for ELK's output.
    current=next(node["id"] for node in report["cfg"]
                 if not any(edge["to"]==node["id"] for edge in report["edges"]))
    states=[current]
    while outgoing:=[edge for edge in report["edges"] if edge["from"]==current]:
        assert len(outgoing)==1
        current=outgoing[0]["to"];states.append(current)
    assert len(states)==len(cards)
    requests=[]
    page.on("request",lambda request:requests.append(request.url)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    def select(index):
        card=cards[index]
        canvas.click(position={"x":(card[0]+card[2])/2,"y":(card[1]+card[3])/2})
        page.mouse.move(2,2);settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ',states[index])};")
    select(1)
    regions={"incoming":vertical_edge_region(cards[0],cards[1]),
             "outgoing":vertical_edge_region(cards[1],cards[2])}
    incident=flow_frames(page,canvas,output,"flow-chain-selected-middle",regions)
    incoming=assert_flow_direction(incident,"incoming",[0,1])
    outgoing=assert_flow_direction(incident,"outgoing",[0,1])
    select(3)
    changed=flow_frames(page,canvas,output,"flow-chain-selected-last",
                        {**regions,"new_incoming":vertical_edge_region(cards[2],cards[3])})
    for name in regions:
        assert len({frame["regions"][name]["hash"] for frame in changed})==1, f"old {name} edge kept animating"
        assert all(not frame["regions"][name]["markers"] for frame in changed), f"old {name} edge remained highlighted"
    new_incoming=assert_flow_direction(changed,"new_incoming",[0,1])
    before=node_snapshot(page,canvas,output,"flow-chain-before-zoom",measure_nodes=True)
    pointer=(720,500)
    page.mouse.move(*pointer);page.keyboard.down("Control");page.mouse.wheel(0,35);page.keyboard.up("Control")
    page.mouse.move(2,2);settle_gesture(page)
    zoomed=node_snapshot(page,canvas,output,"flow-chain-zoomed",measure_nodes=True)
    camera=assert_layout_same_geometry(before,zoomed)
    assert camera["camera_scale"]<0.95, camera
    resize_view(page,1600,1100)
    resized=node_snapshot(page,canvas,output,"flow-chain-resized",(4,123,1596,1070),True)
    assert_translation(zoomed,resized,0,0,tolerance=1.0)
    assert abs(assert_layout_same_geometry(zoomed,resized)["camera_scale"]-1)<0.01
    zoom_cards=sorted(resized["nodes"],key=lambda card:card[1])
    zoom_frames=flow_frames(page,canvas,output,"flow-chain-zoom-resize",
                            {"new_incoming":vertical_edge_region(zoom_cards[2],zoom_cards[3])})
    zoom_flow=assert_flow_direction(zoom_frames,"new_incoming",[0,1])
    assert not requests, "selection, animation, zoom or resize reran analysis"
    resize_view(page,1440,1000)
    # Sixty-four bounded trips insert one zero below the countdown per trip.
    # Different stack heights retain parallel native edges without relying on
    # repeated jump sources surviving context canonicalization.
    loop=layout_submission(page,canvas,"60405b600190035f90600c565b8060025700",output,"flow-loop")
    assert loop["status"]=="Converged" and loop["ssa"]["complete"]
    assert len(loop["cfg"])==len(loop["ssa"]["blocks"])==130
    assert {node["id"] for node in loop["cfg"]}==set(range(130))
    assert {node["start_pc"] for node in loop["cfg"]}=={0,2,12,17}
    pcs={node["id"]:node["start_pc"] for node in loop["cfg"]}
    expected_edges={(0,1,"Fallthrough"),(128,129,"BranchFalse")}
    expected_edges.update((state,state+1,"Jump" if state%2 else "BranchTrue") for state in range(1,128))
    assert len(loop["edges"])==129
    assert {(edge["from"],edge["to"],edge["kind"]) for edge in loop["edges"]}==expected_edges
    parallel={(source,target):sum(pcs[edge["from"]]==source and pcs[edge["to"]]==target for edge in loop["edges"])
              for source,target in [(2,12),(12,2)]}
    assert parallel=={(2,12):64,(12,2):63}
    for pc in [2,12]:
        members=[node for node in loop["cfg"] if node["start_pc"]==pc]
        expected_heights=range(1,65) if pc==2 else range(2,66)
        assert len(members)==64 and {len(node["entry_stack"]) for node in members}==set(expected_heights)
    graph_mode(page,canvas,"Blocks");page.keyboard.press("f");settle_gesture(page);page.mouse.move(2,2)
    grouped=graph_scope(page,"Blocks",4,130,130)
    assert grouped["edges"]==4 and grouped["native_edges"]==129
    card=node_snapshot(page,canvas,output,"flow-loop-before",measure_nodes=True)
    assert len(card["nodes"])==4,card
    rendered={}
    status=page.locator("#analysis-status")
    # Identify the two real cycle cards through native selection. The entry
    # and STOP cards must not be mistaken for the return edge's endpoints.
    for bounds in card["nodes"]:
        canvas.click(position={"x":(bounds[0]+bounds[2])/2,"y":(bounds[1]+bounds[3])/2})
        settle_gesture(page)
        selected=next(node for node in loop["cfg"] if f"Selected {symbol('σ',node['id'])};" in status.text_content())
        rendered[selected["start_pc"]]=bounds
        if selected["start_pc"] in [2,12]:
            pick_graph_instance(page,canvas,selected["id"])
    assert set(rendered)=={0,2,12,17}
    cycle=sorted(((pc,rendered[pc]) for pc in [2,12]),key=lambda item:item[1][1])
    (upper_pc,_),(lower_pc,_)=cycle
    loop_cards=[bounds for _,bounds in cycle]
    bounds=rendered[2]
    canvas.click(position={"x":(bounds[0]+bounds[2])/2,"y":(bounds[1]+bounds[3])/2})
    settle_gesture(page)
    pick_graph_instance(page,canvas,127,"sigma_127")
    assert next(node for node in loop["cfg"] if node["id"]==127)["start_pc"]==2
    page.mouse.move(2,2);settle_gesture(page)
    assert any(pcs[edge["from"]]==lower_pc and pcs[edge["to"]]==upper_pc for edge in loop["edges"])
    leg=loop_return_region(page,canvas.screenshot(),loop_cards)
    assert leg["pixels"]>30 and leg["target_arrow_pixels"]>=2 and leg["region"][3]-leg["region"][1]>25,leg
    returning=flow_frames(page,canvas,output,"flow-loop-return",{"loop_return":leg["region"]})
    loop_flow=assert_flow_direction(returning,"loop_return",[0,-1])
    loop_flow.update(source_pc=lower_pc,target_pc=upper_pc,target_arrow_pixels=leg["target_arrow_pixels"])
    assert len(requests)==1, "loop animation or graph scope reran analysis"
    assert not errors,errors
    result={"chain_native_states":states,"selected_middle":states[1],"incoming":incoming,"outgoing":outgoing,
            "selected_last":states[3],"old_edges_stop":True,"new_incoming":new_incoming,
            "camera":camera,"zoom_resize_flow":zoom_flow,"loop_return":loop_flow,
            "loop_original_edges":len(loop["edges"]),"loop_grouped":grouped,
            "loop_parallel_edges":{"forward":parallel[(2,12)],"return":parallel[(12,2)]},
            "loop_selected_native_state":127,"incident_frames":incident,
            "selection_frames":changed,"zoom_resize_frames":zoom_frames,"loop_frames":returning,
            "renderer":webgpu_evidence(page)}
    (output/"edge-flow-report.json").write_text(json.dumps(result,indent=2)+"\n")
    page.close()
    return result


def run(context):
    return edge_flow_interactions(context.browser, context.url, context.output)
