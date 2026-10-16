"""Hierarchy small browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import re
from web_test_graph_helpers import symbol
from web_test_harness import complete_submission, observe_webgpu, rendered_frame, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_layout_helpers import layout_counts, layout_fixture, layout_page, layout_submission
from web_test_pixels import assert_layout_same_geometry, layout_ink, node_snapshot, require_scene_complete


def hierarchy_small_interactions(browser, url: str, output: Path):
    page, canvas, errors = layout_page(browser, url)
    results = {}
    for name in ["shortcut", "linear-chain"]:
        code, metadata = layout_fixture(name)
        report = layout_submission(page, canvas, code, output, name)
        assert report["status"] == "Converged", report["frontiers"]
        assert len(report["cfg"]) == metadata["expected_native_states"]
        assert len(report["edges"]) == metadata["expected_original_edges"]
        scene = layout_counts(page, report, metadata["expected_spatial_groups"])
        assert scene["nodes"] == metadata["expected_source_blocks"]
        pixels = node_snapshot(page, canvas, output, f"hierarchy-{name}", measure_nodes=True)
        assert len(pixels["nodes"]) == scene["nodes"], "spatial annotations were counted as real CFG states"
        if name == "shortcut":
            require_scene_complete(pixels)
            nodes = sorted(pixels["nodes"], key=lambda rect: rect[1])
            _, a, b = nodes
            assert a[3] < b[1], "shortcut target was placed beside/before its true predecessor"
            interval = (max(a[0],b[0]), a[3]+1, min(a[2],b[2]), b[1]-1)
            ink = layout_ink(page, canvas.screenshot(path=str(output / "hierarchy-shortcut-label.png")), interval)
            assert ink["annotation_pixels"] > 0, "the direct A to B annotation was not painted between its endpoints"
            results[name] = {"scene":scene,"pixels":pixels,"between_endpoints":ink}
        else:
            results[name] = {"scene":scene,"pixels":pixels}
            requests = []
            page.on("request", lambda request: requests.append(request.url)
                    if request.method=="POST" and request.url.endswith("/api/tasks") else None)
            pointer = ((pixels["bounds"][0]+pixels["bounds"][2])/2,(pixels["bounds"][1]+pixels["bounds"][3])/2)
            for _ in range(12):
                if "CFG detail: identities" in page.locator("#analysis-status").text_content(): break
                page.mouse.move(*pointer); page.keyboard.down("Control"); page.mouse.wheel(0,160); page.keyboard.up("Control"); settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text("CFG detail: identities")
            low_zoom=float(re.search(r"; CFG zoom: ([0-9.]+)%",page.locator("#analysis-status").text_content())[1])
            assert low_zoom <= 55
            page.mouse.move(2,2);settle_gesture(page)
            low = node_snapshot(page,canvas,output,"hierarchy-chain-identities",measure_nodes=True)
            assert len(low["nodes"])==4,low
            target = max(low["nodes"],key=lambda rect:rect[1])
            first_state = report["cfg"][0]["id"]
            state = first_state
            while (outgoing := [edge for edge in report["edges"] if edge["from"]==state]):
                assert len(outgoing)==1
                state=outgoing[0]["to"]
            pointer=((target[0]+target[2])/2,(target[1]+target[3])/2)
            canvas.click(position={"x":pointer[0],"y":pointer[1]}); page.mouse.move(2,2); settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ',state)};")
            low_ink=layout_ink(page,canvas.screenshot(),target)
            assert low_ink["preview_pixels"]==0, "low zoom retained the white SSA instruction body"
            for _ in range(12):
                if "CFG detail: SSA preview" in page.locator("#analysis-status").text_content(): break
                page.mouse.move(*pointer);page.keyboard.down("Control");page.mouse.wheel(0,-120);page.keyboard.up("Control");settle_gesture(page)
            page.mouse.move(2,2);settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
            high_zoom=float(re.search(r"; CFG zoom: ([0-9.]+)%",page.locator("#analysis-status").text_content())[1])
            assert high_zoom >= 65
            high=node_snapshot(page,canvas,output,"hierarchy-chain-ssa")
            high_ink=layout_ink(page,canvas.screenshot(),high["bounds"])
            assert high_ink["preview_pixels"]>0, "high zoom did not restore real SSA instruction ink"
            assert layout_counts(page,report,1)["shown_states"]==4
            assert not requests,"semantic zoom or clicking a leaf reran analysis"
            results[name].update(low=low,high=high,low_ink=low_ink,high_ink=high_ink,
                                 low_zoom_percent=low_zoom,high_zoom_percent=high_zoom,selected_native_state=state)
    assert not errors,errors
    results["renderer"]=webgpu_evidence(page)
    page.close()
    # Fresh pages create the same layout for two genuinely different initial
    # viewports. This checks more than resizing an already-cached scene.
    samples=[]
    code,_=layout_fixture("shortcut")
    for width,height in [(1440,1000),(640,800)]:
        page=browser.new_page(viewport={"width":width,"height":height});observe_webgpu(page)
        with page.expect_response(lambda response:response.url.endswith("/api/tasks") and response.request.method=="POST") as initial:
            page.goto(url,wait_until="networkidle")
        complete_submission(page,initial.value)
        canvas=page.locator("#evm-canvas")
        # Inspector is right-aligned and omitted from the graph comparison.
        if width>=800: hide_inspector(page,canvas)
        else:
            canvas.click(position={"x":width-78,"y":12});rendered_frame(page)
            # Use the public Panels menu's State inspector checkbox.
            canvas.click(position={"x":width-80,"y":62});settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text("Inspector hidden")
        page.keyboard.press("2");settle_gesture(page)
        report=layout_submission(page,canvas,code,output,f"hierarchy-initial-{width}")
        layout_counts(page,report,0)
        samples.append(node_snapshot(page,canvas,output,f"hierarchy-initial-{width}-scene",(4,123,width-4,height-30),True))
        page.close()
    results["initial_viewports"]={"wide":samples[0],"narrow":samples[1],"normalization":assert_layout_same_geometry(*samples)}
    return results


def run(context):
    return hierarchy_small_interactions(context.browser, context.url, context.output)
