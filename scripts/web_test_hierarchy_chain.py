"""Hierarchy chain browser acceptance cases."""

from playwright.sync_api import expect
import hashlib
import json
import re
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_layout_helpers import LAYOUT_DEFAULT_LIMITS, layout_counts, layout_fixture, layout_page, layout_submission
from web_test_pixels import layout_camera_intervals, layout_card_extent, layout_ink, node_snapshot
from web_test_hybrid import source_instructions


def hierarchy_long_chain_interactions(browser, url, output, fixture_name="long-linear-chain", native_projection=False):
    page, canvas, errors = layout_page(browser, url)
    code, metadata = layout_fixture(fixture_name)
    report = layout_submission(page, canvas, code, output, fixture_name)
    assert report["status"] == "Converged", report["frontiers"]
    assert len(report["cfg"]) == metadata["expected_native_states"]
    assert len(report["edges"]) == metadata["expected_original_edges"]
    scene = layout_counts(page, report, metadata["expected_spatial_groups"])
    assert scene["nodes"] == metadata["expected_source_blocks"]
    assert scene["detail"] == "identities"
    if native_projection:
        assert hashlib.sha256(bytes.fromhex(code)).hexdigest() == metadata["code_sha256"]
        request = json.loads((output/f"{fixture_name}-request.json").read_text())
        assert request["limits"] == report["metadata"]["limits"] == LAYOUT_DEFAULT_LIMITS
        assert not report["frontiers"] and report["ssa"]["complete"]
        assert len(report["programs"]) == metadata["expected_programs"]
        assert len(report["ssa"]["blocks"]) == metadata["expected_ssa_blocks"]
        assert {block["state"] for block in report["ssa"]["blocks"]} == {node["id"] for node in report["cfg"]}
        assert {node["frame_depth"] for node in report["cfg"]} == {metadata["expected_frame_depth"]}
        pcs = {node["id"]:node["start_pc"] for node in report["cfg"]}
        stride = metadata["block_stride"]
        assert sorted(pcs.values()) == list(range(0,metadata["last_block_start_pc"]+1,stride))
        assert all(edge["kind"] == metadata["expected_edge_kind"] for edge in report["edges"])
        assert {(pcs[edge["from"]],pcs[edge["to"]]) for edge in report["edges"]} == {
            (pc,pc+stride) for pc in range(0,metadata["last_block_start_pc"],stride)}
        assert any(instruction["pc"] == metadata["stop_pc"] and instruction["opcode"] == 0
                   for node in report["cfg"] for instruction in source_instructions(report, node))
        assert scene["nodes"] >= 600 and scene["hidden_states"] == scene["boundary_edges"] == 0
    status = page.locator("#analysis-status").text_content()
    fit_zoom = float(re.search(r"; CFG zoom: ([0-9.]+)%", status)[1])
    assert 0 < fit_zoom < 8, status
    prefix = "hierarchy-long-chain" if fixture_name == "long-linear-chain" else f"hierarchy-{fixture_name}"
    before = layout_card_extent(page, canvas.screenshot(path=str(output/f"{prefix}-fit.png")))
    assert before["bounds"] and before["pixels"] > 50, before
    requests = []
    page.on("request", lambda request: requests.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    pointer = ((before["bounds"][0]+before["bounds"][2])/2,
               (before["bounds"][1]+before["bounds"][3])/2)
    page.mouse.move(*pointer)
    page.keyboard.down("Control"); page.mouse.wheel(0, 160); page.keyboard.up("Control")
    page.mouse.move(2,2); settle_gesture(page)
    after = layout_card_extent(page, canvas.screenshot(path=str(output/f"{prefix}-zoom-out.png")))
    assert after["bounds"] and after["pixels"] > 50, after
    before_height = before["bounds"][3]-before["bounds"][1]
    after_height = after["bounds"][3]-after["bounds"][1]
    assert after_height < before_height-10, "zoom-out enlarged or retained the fitted long chain"
    zoom_out = float(re.search(r"; CFG zoom: ([0-9.]+)%", page.locator("#analysis-status").text_content())[1])
    assert 0 < zoom_out < fit_zoom
    camera_ratio = zoom_out/fit_zoom
    if native_projection:
        camera_precision = layout_camera_intervals(before,after,fit_zoom,zoom_out,pointer)
    else:
        assert abs(after_height-before_height*camera_ratio) <= 4.0, (before,after,fit_zoom,zoom_out)
        for axis in [0,1]:
            for corner in [axis,axis+2]:
                expected = pointer[axis]+(before["bounds"][corner]-pointer[axis])*camera_ratio
                assert abs(after["bounds"][corner]-expected) <= 4.0, (before,after,pointer,camera_ratio)
    assert layout_counts(page, report, 1) == scene
    assert not requests and not errors, (requests, errors)
    result = {"scene":scene,"fit":before,"zoom_out":after,"height_ratio":after_height/before_height,
              "fit_zoom_percent":fit_zoom,"zoom_out_percent":zoom_out,
              "renderer":webgpu_evidence(page)}
    if native_projection:
        graph_mode(page,canvas,"States")
        page.keyboard.press("f");settle_gesture(page)
        native_scene = layout_counts(page,report,metadata["expected_spatial_groups"],mode="States")
        assert native_scene["nodes"] >= 600 and native_scene["nodes"] == len(report["cfg"])
        assert native_scene["edges"] == native_scene["native_edges"] == metadata["expected_original_edges"]
        assert native_scene["shown_states"] == native_scene["total_states"] == metadata["expected_native_states"]
        assert native_scene["hidden_states"] == native_scene["boundary_edges"] == 0
        canvas.screenshot(path=str(output/f"{prefix}-states-overview.png"))
        canvas.click(position={"x":225,"y":107});settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
        native_focus = node_snapshot(page,canvas,output,f"{prefix}-states-focused")
        selected = re.search(r"Selected (σ[^;]+);",page.locator("#analysis-status").text_content())[1]
        selected_node = next(node for node in report["cfg"] if symbol("σ",node["id"]) == selected)
        selected_ssa = next(block for block in report["ssa"]["blocks"] if block["state"] == selected_node["id"])
        assert selected_ssa["instructions"] and selected_ssa["incoming_complete"]
        native_ink = layout_ink(page,canvas.screenshot(),native_focus["bounds"])
        assert native_ink["preview_pixels"] > 0, "native focus did not paint real SSA instructions"
        assert layout_counts(page,report,1,mode="States") == {**native_scene,"detail":"SSA preview"}
        graph_mode(page,canvas,"Blocks")
        canvas.click(position={"x":225,"y":107});settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
        source_scene = layout_counts(page,report,1)
        assert source_scene == {**scene,"detail":"SSA preview"}
        expect(page.locator("#analysis-status")).to_contain_text(f"Selected {selected};")
        source_focus = node_snapshot(page,canvas,output,f"{prefix}-source-ssa-focused")
        source_ink = layout_ink(page,canvas.screenshot(),source_focus["bounds"])
        assert source_ink["preview_pixels"] > 0, "source projection lost the selected native SSA preview"
        assert not requests and not errors, (requests, errors)
        result.update(camera_precision=camera_precision,native_scene=native_scene,native_focus=native_focus,native_ink=native_ink,
                      source_scene=source_scene,source_focus=source_focus,source_ink=source_ink,
                      selected_native_state=selected_node["id"],selected_source_pc=selected_node["start_pc"],
                      selected_ssa_instructions=len(selected_ssa["instructions"]),unexpected_task_posts=requests,
                      renderer=webgpu_evidence(page))
    page.close()
    return result

def hierarchy_native_projection_interactions(browser, url, output):
    return hierarchy_long_chain_interactions(browser,url,output,"long-linear-chain-600",native_projection=True)


def run(context, fixture_name="long-linear-chain", native_projection=False):
    return hierarchy_long_chain_interactions(context.browser, context.url, context.output, fixture_name, native_projection)
