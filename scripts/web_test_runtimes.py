"""Real runtime input, partial execution evidence and browser projection oracles."""

import json
import re
from playwright.sync_api import expect
from web_test_graph_helpers import graph_mode, symbol
from web_test_harness import settle_gesture, webgpu_evidence
from web_test_layout_helpers import LAYOUT_DEFAULT_LIMITS, layout_counts, layout_fixture, layout_page, layout_submission
from web_test_pixels import layout_ink, node_snapshot
from web_test_hybrid import oracle_graph
from web_test_runtime_report import (layout_runtime_programs, layout_runtime_request,
    layout_runtime_coverage, layout_pending_machines, _runtime_boundary_negative_controls)


# Historical runtime detail/precision oracles retain their explicit 1T/OFF
# profile. They are separate from the new 2B/ON default acceptance cases.
RUNTIME_DETAIL_LIMITS = {**LAYOUT_DEFAULT_LIMITS, "max_work": 1_000_000_000_000,
                         "root_terminal_coverage": False}


def _runtime_zoom_percent(status):
    match = re.search(r"; CFG zoom: ([0-9.]+)%", status)
    assert match, status
    return float(match[1])


def _runtime_ssa_detail(zoom_percent, previous_detail):
    # paint.rs::detail_level retains its prior level strictly between 55% and 65%.
    # scene.rs labels Preview as SSA preview while NodeView::Ssa is selected.
    assert previous_detail in {"identities", "SSA preview"}
    if zoom_percent <= 55:
        return "identities"
    if zoom_percent >= 65:
        return "SSA preview"
    return previous_detail


def hierarchy_real_runtime_interactions(browser,url,args,fixture_names=("ethereum-weth9","ethereum-uniswap-v2-router02","ethereum-multicall3")):
    results={}
    for name in fixture_names:
        page,canvas,errors=layout_page(browser,url)
        code,metadata=layout_fixture(name)
        assert metadata["chain_id"]==1 and metadata["requireCanonical"]
        entry=metadata["browser_entry"]
        print(f"real-runtime analysis start: {name}",flush=True)
        report=layout_submission(page,canvas,code,args.output,name,timeout_seconds=1800,entry=entry,
                                 execution_limits=RUNTIME_DETAIL_LIMITS)
        original_report=json.loads((args.output/f"{name}-analysis.json").read_text())
        coverage=layout_runtime_coverage(report,metadata,original_report["timings"])
        (args.output/f"{name}-coverage-comparison.json").write_text(json.dumps(coverage,indent=2)+"\n")
        print(f"real-runtime analysis done: {name} status={report['status']} states={len(report['cfg'])} "
              f"frontiers={[(frontier['kind'],frontier['detail']) for frontier in report['frontiers']]} "
              f"unknown_closed={[summary['closed'] for summary in report['abstract_summaries']]} "
              f"analyzing_ns={original_report['timings']['analyzing_ns']}",flush=True)
        request=json.loads((args.output/f"{name}-request.json").read_text())
        request_evidence=layout_runtime_request(request,oracle_graph(original_report, "Baseline"),code,metadata,RUNTIME_DETAIL_LIMITS)
        layout_runtime_programs(report,code,metadata)
        assert {block["state"] for block in report["ssa"]["blocks"]} == {node["id"] for node in report["cfg"]}
        assert report["status"] == entry["expected_status"]
        pending = layout_pending_machines(report,entry,RUNTIME_DETAIL_LIMITS) if "product_oracle" in entry else None
        boundary_controls = _runtime_boundary_negative_controls(report, entry, RUNTIME_DETAIL_LIMITS) if "product_oracle" in entry else []
        if "minimum_native_states" in entry:
            assert len(report["cfg"]) >= entry["minimum_native_states"] and len(report["frontiers"]) == entry["expected_frontiers"]
        if name == "ethereum-uniswap-v2-router02":
            assert report["abstract_summaries"]
            nodes={node["id"]:node for node in report["cfg"]};states={state["state"]:state for state in report["states"]}
            for summary in report["abstract_summaries"]:
                assert summary["closed"] and summary["kind"] == "UnknownEnvironment" and summary["procedures"]
                source=nodes[summary["source_state"]];active=states[source["id"]]["entry"]["frames"][-1]
                assert source["program"] is None and active["kind"] == "AbstractEnvironment" and active["is_static"]
                for procedure in summary["procedures"]:
                    assert procedure["kind"]=="AbstractEnvironment" and procedure["is_static"]
                    assert not procedure["covered_blocks"]
                    assert set(procedure["outcomes"])=={"Return","Revert","Failure"}
        ending_status=re.search(r"CFG endings: (.+?); Selected CFG ending:",page.locator("#analysis-status").text_content())[1]
        def scene_counts(mode="Blocks"):
            counts=layout_counts(page,report,mode=mode)
            identities={(node["program"],node["basic_block"],node["start_pc"])
                        if mode == "Blocks" and node["program"] is not None and node["start_pc"] is not None
                        else ("state",node["id"]) for node in report["cfg"]}
            assert counts["nodes"] == len(identities) and counts["shown_states"] == counts["total_states"] == len(report["cfg"])
            assert counts["scope"] == "All programs" and counts["hidden_states"] == counts["boundary_edges"] == 0
            status=page.locator("#analysis-status").text_content()
            assert re.search(r"CFG endings: (.+?); Selected CFG ending:",status)[1] == ending_status and report["status"] in status
            return counts
        scene=scene_counts()
        if "minimum_source_blocks" in entry: assert scene["nodes"] >= entry["minimum_source_blocks"]
        if report["status"]=="Incomplete":
            assert report["frontiers"] and not report["ssa"]["complete"]
            expect(page.locator("#analysis-status")).to_contain_text("Incomplete")
        requests=[]
        page.on("request",lambda request:requests.append(request.url)
                if request.method == "POST" and request.url.endswith("/api/tasks") else None)
        canvas.screenshot(path=str(args.output/f"hierarchy-{name}-overview.png"))
        graph_mode(page,canvas,"States")
        page.keyboard.press("f");settle_gesture(page)
        native_scene=scene_counts("States")
        assert native_scene["nodes"] == len(report["cfg"]) and native_scene["edges"] == len(report["edges"])
        canvas.screenshot(path=str(args.output/f"hierarchy-{name}-states-overview.png"))
        chosen=next(block for block in report["ssa"]["blocks"] if block["instructions"] and block["coverage"] == "Current")
        # States mode has no aggregate Instances chooser. Select the original
        # root instruction through Code, then return to the unchanged scene.
        assert chosen["state"] == report["cfg"][0]["id"] and report["cfg"][0]["start_pc"] == 0
        page.keyboard.press("1");settle_gesture(page)
        canvas.click(position={"x":80,"y":156});settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ',chosen['state'])};")
        page.keyboard.press("2");settle_gesture(page)
        canvas.click(position={"x":225,"y":107});settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
        assert _runtime_zoom_percent(page.locator("#analysis-status").text_content()) == 100
        native_focus=node_snapshot(page,canvas,args.output,f"hierarchy-{name}-states-focused")
        selected=re.search(r"Selected (σ[^;]+);",page.locator("#analysis-status").text_content())[1]
        assert selected == symbol("σ",chosen["state"]) and chosen["incoming_complete"] is (report["status"] == "Converged")
        ink=layout_ink(page,canvas.screenshot(),native_focus["bounds"])
        assert ink["preview_pixels"] > 0, "real-runtime focus did not paint the chosen SSA instructions"
        for action in ["fit","zoom","focus"]:
            before_scene=scene_counts("States")
            before_status=page.locator("#analysis-status").text_content()[:4096]
            before_zoom=_runtime_zoom_percent(before_status)
            if action == "fit":page.keyboard.press("f")
            elif action == "zoom":
                page.mouse.move(700,500);page.keyboard.down("Control");page.mouse.wheel(0,-80);page.keyboard.up("Control")
            else:canvas.click(position={"x":225,"y":107})
            settle_gesture(page)
            scene_path=args.output/f"hierarchy-{name}-states-{action}-scene.json"
            screenshot_path=args.output/f"hierarchy-{name}-states-{action}.png"
            canvas.screenshot(path=str(screenshot_path))
            actual_scene=scene_counts("States")
            actual_status=page.locator("#analysis-status").text_content()[:4096]
            actual_zoom=_runtime_zoom_percent(actual_status)
            expected_scene={**native_scene,"detail":_runtime_ssa_detail(actual_zoom,before_scene["detail"])}
            diff={key:{"before":before_scene.get(key),"expected":expected_scene.get(key),"actual":actual_scene.get(key)}
                  for key in sorted(before_scene.keys() | expected_scene.keys() | actual_scene.keys())
                  if before_scene.get(key) != actual_scene.get(key) or expected_scene.get(key) != actual_scene.get(key)}
            # Fixed scene fields and bounded status keep failures inspectable without copying the report.
            scene_path.write_text(json.dumps({"action":action,"baseline":native_scene,"before":before_scene,
                "expected":expected_scene,"actual":actual_scene,"diff":diff,
                "zoom_percent":{"before":before_zoom,"actual":actual_zoom},
                "status":{"before":before_status,"actual":actual_status},
                "filename":str(scene_path),"screenshot":str(screenshot_path)},indent=2)+"\n")
            diagnostic=f"{name} {action}: {diff}; zoom {before_zoom}% -> {actual_zoom}%; see {scene_path} and {screenshot_path}"
            assert actual_scene == expected_scene, diagnostic
            if action == "fit" and name in {"ethereum-uniswap-v2-router02","ethereum-multicall3"}:
                assert actual_zoom <= 55 and actual_scene["detail"] == "identities", diagnostic
            elif action == "zoom":
                assert actual_zoom > before_zoom, diagnostic
            elif action == "focus":
                assert actual_zoom == 100 and actual_scene["detail"] == "SSA preview", diagnostic
            expect(page.locator("#analysis-status")).to_contain_text(f"Selected {selected};")
        native_refocus=node_snapshot(page,canvas,args.output,f"hierarchy-{name}-states-refocused")
        refocus_ink=layout_ink(page,canvas.screenshot(),native_refocus["bounds"])
        assert refocus_ink["preview_pixels"] > 0, "real-runtime refocus did not restore the chosen SSA instructions"
        graph_mode(page,canvas,"Blocks")
        canvas.click(position={"x":225,"y":107});settle_gesture(page)
        assert scene_counts() == {**scene,"detail":"SSA preview"}
        expect(page.locator("#analysis-status")).to_contain_text(f"Selected {selected};")
        focused=node_snapshot(page,canvas,args.output,f"hierarchy-{name}-source-ssa-focused")
        source_ink=layout_ink(page,canvas.screenshot(),focused["bounds"])
        assert source_ink["preview_pixels"] > 0, "source projection lost the chosen SSA preview"
        assert not requests, "runtime focus/fit/zoom/projection triggered another analysis"
        native_scene.update(focused=native_focus,selected_native_state=selected,ssa_instructions=len(chosen["instructions"]),
                            ssa_ink=ink,refocused=native_refocus,refocus_ink=refocus_ink,
                            source_ink=source_ink,unexpected_task_posts=requests)
        assert not errors,errors
        results[name]={"provenance":metadata,"request":request_evidence,"coverage_comparison":coverage,"status":report["status"],
                       "scene":scene,"native_scene":native_scene,"focused":focused,"frontiers":len(report["frontiers"]),
                       "pending":pending,"boundary_controls":boundary_controls,
                       "ending_status":ending_status,"renderer":webgpu_evidence(page)}
        page.close()
    return results


def run(context, fixture):
    return hierarchy_real_runtime_interactions(context.browser, context.url, context.args, fixture_names=(fixture,))
