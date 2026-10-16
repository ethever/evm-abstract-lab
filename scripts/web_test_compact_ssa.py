"""Compact ssa browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import json
from web_test_harness import observe_webgpu, rendered_frame, resize_view, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_pixels import screenshot_metrics


def compact_ssa_interactions(browser, url: str, output: Path):
    page=browser.new_page(viewport={"width":1440,"height":1000});observe_webgpu(page)
    page.goto(url,wait_until="networkidle")
    expect(page.locator("#analysis-status")).to_contain_text("Ready:",timeout=60000)
    canvas=page.locator("#evm-canvas");hide_inspector(page,canvas)
    requests=[]
    page.on("request",lambda request:requests.append(request.url)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    results={}
    for width,name,ssa_x in [(1024,"medium",118),(640,"narrow",198)]:
        resize_view(page,width,800);page.keyboard.press("0");page.mouse.move(2,2);settle_gesture(page)
        region=(8,100,172,740)
        def source_hash(label):
            page.mouse.move(2,2);page.wait_for_timeout(800);rendered_frame(page)
            png=canvas.screenshot(path=str(output/f"workspace-{name}-{label}.png"))
            metrics=screenshot_metrics(page,png,region)
            assert metrics["selected_pixels"]>100, f"missing selected source block: {name}/{label}"
            return metrics["hash"]
        initial=source_hash("initial-ssa")
        canvas.click(position={"x":ssa_x,"y":65});settle_gesture(page)
        explicit_ssa=source_hash("explicit-ssa")
        assert initial==explicit_ssa, f"{name} workspace did not initially open SSA"
        canvas.click(position={"x":52,"y":65});settle_gesture(page)
        disasm=source_hash("disassembly")
        assert disasm!=explicit_ssa, f"{name} Disassembly tab did not show different source content"
        canvas.click(position={"x":ssa_x,"y":65});settle_gesture(page)
        assert source_hash("ssa-return")==initial, f"{name} SSA tab did not restore its initial source content"
        canvas.click(position={"x":52,"y":65});settle_gesture(page)
        assert source_hash("disassembly-return")==disasm, f"{name} Disassembly tab did not restore source content"
        page.keyboard.press("2");settle_gesture(page);page.keyboard.press("0");settle_gesture(page)
        assert source_hash("view-return-disassembly")==disasm, f"{name} tab choice was lost across views"
        resize_view(page,1440,1000);settle_gesture(page);resize_view(page,width,800);settle_gesture(page)
        assert source_hash("resize-return-disassembly")==disasm, f"{name} tab choice was lost across width classes"
        results[name]={"initial_is_ssa":True,"ssa_hash":explicit_ssa,"disassembly_hash":disasm,
                       "preserves_manual_tab_across_views_and_widths":True}
    resize_view(page,1024,800);settle_gesture(page)
    returned=screenshot_metrics(page,canvas.screenshot(path=str(output/"workspace-medium-after-narrow.png")),(8,100,172,740))
    assert returned["hash"]==results["medium"]["disassembly_hash"], "narrow workspace overwrote the medium tab preference"
    assert not requests,"compact source tabs or workspace resizing reran analysis"
    results["renderer"]=webgpu_evidence(page)
    (output/"compact-ssa-report.json").write_text(json.dumps(results,indent=2)+"\n")
    page.close()
    return results


def run(context):
    return compact_ssa_interactions(context.browser, context.url, context.output)
