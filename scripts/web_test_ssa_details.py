"""Ssa details browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import json
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import rendered_frame, resize_view, settle_gesture, webgpu_evidence
from web_test_layout_helpers import layout_page, layout_submission
from web_test_pixels import assert_translation, graph_screenshot_metrics, screenshot_metrics, text_pixels, workspace_content_bottom
from web_test_ssa_helpers import SSA_DETAILS_CODE, ssa_details_toggle


def ssa_text_snapshot(page, canvas, output, name):
    page.mouse.move(2,2);page.wait_for_timeout(800);rendered_frame(page)
    png=canvas.screenshot(path=str(output/f"{name}.png"))
    size=page.viewport_size
    bottom=workspace_content_bottom(page,png,above_inspector=True)
    selected=screenshot_metrics(page,png,(4,75,size["width"]-4,bottom))
    assert selected["bounds"], f"missing selected native SSA header: {name}"
    # The selected state header supplies the actual start of the virtual table,
    # independently of the global metadata bar's height or responsive layout.
    region=(4,selected["bounds"][1],size["width"]-4,bottom)
    return {**text_pixels(page,png,region),"selected":selected,"region":region}

def details_graph_snapshot(page, canvas, output, name):
    page.mouse.move(2,2);settle_gesture(page)
    png=canvas.screenshot(path=str(output/f"{name}.png"))
    size=page.viewport_size
    node=graph_screenshot_metrics(page,png,(4,123,size["width"]-4,size["height"]-30),True)
    assert node["bounds"] and len(node["nodes"])==1,node
    left,top,right,bottom=node["bounds"]
    # Exclude the selected card's border, title and source-location header.
    body=text_pixels(page,png,(left+6,top+34,right-6,bottom-6))
    return {"node":node,"body":body}

def ssa_details_interactions(browser, url: str, output: Path):
    page,canvas,errors=layout_page(browser,url)
    status=page.locator("#analysis-status")
    expect(status).to_contain_text("SSA details: Compact")
    requests=[]
    page.on("request",lambda request:requests.append(request.post_data_json)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    report=layout_submission(page,canvas,SSA_DETAILS_CODE,output,"ssa-details-core")
    assert report["status"]=="Converged" and report["ssa"]["complete"]
    assert len(report["cfg"])==len(report["ssa"]["blocks"])==1
    block=report["ssa"]["blocks"][0]
    instructions=block["instructions"]
    assert [(item["pc"],item["name"]) for item in instructions]==[
        (0,"PUSH1"),(2,"PUSH1"),(4,"DUP2"),(5,"SWAP1"),(6,"ADD"),
        (7,"SWAP1"),(8,"POP"),(9,"PUSH1"),(11,"MSTORE"),(12,"STOP")]
    assert not block["phis"]
    by_pc={item["pc"]:item for item in instructions}
    hidden_pcs=[4,5,7,8]
    assert all(by_pc[pc]["progress"]=="Completed" and not by_pc[pc]["fault"] for pc in hidden_pcs)
    # Bookkeeping retains the actual original SSA identities. MSTORE still
    # consumes the offset definition and the ADD result after the hidden ops.
    assert by_pc[4]["operands"]==by_pc[0]["results"] and not by_pc[4]["results"]
    assert by_pc[8]["operands"]==by_pc[0]["results"]
    assert by_pc[11]["operands"]==by_pc[9]["results"]+by_pc[6]["results"]
    graph_mode(page,canvas,"States");page.keyboard.press("f");settle_gesture(page)
    compact_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-compact")
    expect(status).to_contain_text("CFG detail: SSA preview")
    page.keyboard.press("3");settle_gesture(page)
    compact=ssa_text_snapshot(page,canvas,output,"ssa-details-pane-compact")
    assert len(compact["rows"])==9,compact
    ssa_details_toggle(page,canvas,"Full")
    full=ssa_text_snapshot(page,canvas,output,"ssa-details-pane-full")
    assert len(full["rows"])==13,full
    assert full["muted_pixels"]>compact["muted_pixels"]*1.4, "ordinary μ/completion details did not return"
    page.keyboard.press("2");settle_gesture(page)
    full_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-full")
    assert_translation(compact_graph["node"],full_graph["node"],0,0,tolerance=1.0)
    assert full_graph["body"]["hash"]!=compact_graph["body"]["hash"]
    assert full_graph["body"]["white_pixels"]>compact_graph["body"]["white_pixels"]*1.2
    ssa_details_toggle(page,canvas,"Compact")
    restored_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-restored")
    assert restored_graph["body"]["hash"]==compact_graph["body"]["hash"]
    assert_translation(compact_graph["node"],restored_graph["node"],0,0,tolerance=1.0)

    # Click the real DUP2 PC in the independent disassembly table. Its selected
    # block row and native 22pt row stride locate the control from PNG evidence.
    page.keyboard.press("1");settle_gesture(page);page.mouse.move(2,2)
    source_png=canvas.screenshot(path=str(output/"ssa-details-disassembly-before.png"))
    source=screenshot_metrics(page,source_png,(4,75,1436,970))
    assert source["bounds"],source
    source_order=report["programs"][0]["blocks"][0]["instructions"]
    index=next(index for index,item in enumerate(source_order) if item["pc"]==4)
    clicked_y=source["bounds"][3]+22*(index+0.5)
    canvas.click(position={"x":80,"y":clicked_y});page.mouse.move(2,2);settle_gesture(page)
    selected_source=screenshot_metrics(page,canvas.screenshot(path=str(output/"ssa-details-disassembly-pc4.png")),(4,75,1436,970))
    assert selected_source["selected_pixels"]>source["selected_pixels"]*1.8
    assert abs(selected_source["bounds"][3]-(clicked_y+11))<=2, (source,selected_source,clicked_y)
    expect(status).to_contain_text(f"Selected {symbol('σ',block['state'])};")
    page.keyboard.press("3");settle_gesture(page)
    revealed=ssa_text_snapshot(page,canvas,output,"ssa-details-pane-pc4-revealed")
    expect(status).to_contain_text("SSA details: Compact")
    assert len(revealed["rows"])==10,revealed
    assert revealed["selected"]["selected_pixels"]>compact["selected"]["selected_pixels"]*1.8
    page.keyboard.press("2");settle_gesture(page)
    revealed_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-pc4-revealed")
    assert revealed_graph["body"]["hash"]!=compact_graph["body"]["hash"]
    assert_translation(compact_graph["node"],revealed_graph["node"],0,0,tolerance=1.0)
    page.keyboard.press("3");settle_gesture(page)
    # The SSA header clears only the selected PC. The temporary DUP2 row must
    # disappear from both views while the global mode remains Compact.
    canvas.click(position={"x":30,"y":revealed["selected"]["bounds"][1]+11})
    page.mouse.move(2,2);settle_gesture(page)
    cleared=ssa_text_snapshot(page,canvas,output,"ssa-details-pane-pc-cleared")
    assert len(cleared["rows"])==9 and cleared["hash"]==compact["hash"],cleared
    page.keyboard.press("2");settle_gesture(page)
    cleared_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-pc-cleared")
    assert cleared_graph["body"]["hash"]==compact_graph["body"]["hash"]
    assert len(requests)==1, "reading mode or linked PC selection reran analysis"

    ssa_details_toggle(page,canvas,"Full")
    for key in ["1","0","2","3"]:
        page.keyboard.press(key);settle_gesture(page)
        expect(status).to_contain_text("SSA details: Full")
    resize_view(page,640,800);settle_gesture(page)
    narrow_full=ssa_text_snapshot(page,canvas,output,"ssa-details-narrow-full")
    assert len(narrow_full["rows"])==13,narrow_full
    ssa_details_toggle(page,canvas,"Compact")
    narrow_compact=ssa_text_snapshot(page,canvas,output,"ssa-details-narrow-compact")
    assert len(narrow_compact["rows"])==9,narrow_compact
    ssa_details_toggle(page,canvas,"Full")
    resize_view(page,1440,1000);settle_gesture(page)
    assert len(requests)==1, "responsive detail controls submitted another analysis"
    repeated=layout_submission(page,canvas,SSA_DETAILS_CODE,output,"ssa-details-reanalysis")
    assert repeated["ssa"]["blocks"][0]["instructions"]==instructions
    expect(status).to_contain_text("SSA details: Full")
    after_reanalysis=ssa_text_snapshot(page,canvas,output,"ssa-details-full-after-reanalysis")
    assert len(after_reanalysis["rows"])==13,after_reanalysis

    # Two cheap real safety examples complement native filtering checks:
    # JUMPDEST remains visible, and an underflowing DUP is not bookkeeping.
    ssa_details_toggle(page,canvas,"Compact")
    jumpdest=layout_submission(page,canvas,"5b600160020100",output,"ssa-details-jumpdest")
    jump_ops=jumpdest["ssa"]["blocks"][0]["instructions"]
    assert jump_ops[0]["name"]=="JUMPDEST" and jump_ops[0]["progress"]=="Completed"
    jump_pixels=ssa_text_snapshot(page,canvas,output,"ssa-details-jumpdest-compact")
    assert len(jump_pixels["rows"])==len(jump_ops)+3,jump_pixels
    fault=layout_submission(page,canvas,"80",output,"ssa-details-faulted-dup")
    fault_ops=fault["ssa"]["blocks"][0]["instructions"]
    assert len(fault_ops)==1 and fault_ops[0]["name"]=="DUP1" and fault_ops[0]["fault"]
    assert fault_ops[0]["progress"]=="Faulted"
    fault_compact=ssa_text_snapshot(page,canvas,output,"ssa-details-fault-pane-compact")
    assert len(fault_compact["rows"])==4 and fault_compact["error_pixels"]>20,fault_compact
    page.keyboard.press("2");settle_gesture(page);page.keyboard.press("f");settle_gesture(page)
    fault_graph=details_graph_snapshot(page,canvas,output,"ssa-details-fault-graph-compact")
    assert fault_graph["body"]["error_pixels"]>20,fault_graph
    ssa_details_toggle(page,canvas,"Full")
    fault_graph_full=details_graph_snapshot(page,canvas,output,"ssa-details-fault-graph-full")
    assert fault_graph_full["body"]["hash"]==fault_graph["body"]["hash"]
    page.keyboard.press("3");settle_gesture(page)
    fault_full=ssa_text_snapshot(page,canvas,output,"ssa-details-fault-pane-full")
    assert fault_full["hash"]==fault_compact["hash"]
    assert len(requests)==4, "SSA detail display created an extra backend task"
    assert not errors,errors
    result={"fixture":SSA_DETAILS_CODE,"hidden_completed_pcs":hidden_pcs,"instructions":instructions,
            "compact":compact,"full":full,"compact_graph":compact_graph,"full_graph":full_graph,
            "revealed":revealed,"revealed_graph":revealed_graph,"cleared":cleared,
            "source_selection":{"pc":4,"before":source,"after":selected_source},
            "narrow_compact":narrow_compact,"narrow_full":narrow_full,
            "full_after_reanalysis":after_reanalysis,"jumpdest":jump_pixels,
            "fault_compact":fault_compact,"fault_full":fault_full,"fault_graph":fault_graph,
            "analysis_submissions":len(requests),"renderer":webgpu_evidence(page)}
    (output/"ssa-details-report.json").write_text(json.dumps(result,indent=2)+"\n")
    page.close()
    return result


def run(context):
    return ssa_details_interactions(context.browser, context.url, context.output)
