"""Inspector browser acceptance cases."""

from web_test_graph_helpers import symbol
from web_test_hybrid import assert_root_source, oracle_graph, process_control_relation, source_program
from pathlib import Path
from playwright.sync_api import expect
import hashlib
import json
import re
from web_test_harness import complete_submission, observe_webgpu, open_input, resize_view, settle_gesture, submit_bytecode, webgpu_evidence
from web_test_inspector_helpers import inspector_frame_tab, inspector_title_action, select_inspector_tab


def inspect_shortcut_events(page):
    # Default prevention is recorded in a timer after event propagation. The
    # DOM status can change first, so await that observation before asserting it.
    page.wait_for_function("""() => window.__inspectKeyEvents.length > 0 &&
        window.__inspectKeyEvents.every(event => typeof event.prevented === 'boolean')""",
        timeout=5000)
    return page.evaluate("window.__inspectKeyEvents")

def synthetic_inspect_key(page, **changes):
    page.evaluate("""changes => {
        const target=document.activeElement;
        target.dispatchEvent(new KeyboardEvent('keydown', {
            key:'j',code:'KeyJ',bubbles:true,cancelable:true,...changes
        }));
        target.dispatchEvent(new KeyboardEvent('keyup', {
            key:'j',code:'KeyJ',bubbles:true,cancelable:true,...changes,repeat:false
        }));
    }""", changes)
    settle_gesture(page)


def model_namespace_controls(page, canvas, report, output):
    """Use the real selectors; equal numeric IDs never retain another graph's selection."""
    status = page.locator("#analysis-status")
    baseline, native = oracle_graph(report, "Baseline"), oracle_graph(report, "Native")
    (output / "namespace-original-report.json").write_text(json.dumps(report, indent=2) + "\n")
    assert report["hybrid"]["coverage"]["baseline_current"]
    assert native is not None and native["ssa"]["values"] and not baseline["ssa"]["values"]
    coordinate = baseline["metadata"]["root_binding"]["Namespace"]["coordinate"]
    assert_root_source(report, baseline["metadata"])
    assert native["metadata"]["environment"]["to"] == {"Concrete": coordinate}
    expect(status).to_contain_text("View: Detailed paths")
    expect(status).to_contain_text(f"Additional condition: root ADDRESS = {coordinate}")
    page.keyboard.press("1")
    settle_gesture(page)
    # Existing default program: the first instruction of its second source
    # block follows the first block's four actual instructions and two headers.
    root = source_program(report, native["metadata"]["root_program"])
    second = root["blocks"][1]
    target = next(block for block in native["cfg"] if block["program"] == root["id"] and block["basic_block"] == second["id"])
    assert target["id"] != baseline["cfg"][0]["id"]
    canvas.click(position={"x": 80, "y": 134 + 22 * (len(root["blocks"][0]["instructions"]) + 2)})
    expect(status).to_contain_text(f"Selected {symbol('σ', target['id'])}; source {symbol('P', root['id'])}")
    select_inspector_tab(page, canvas, "SSA value")
    expect(status).to_contain_text("Inspector: SSA value %0")
    canvas.screenshot(path=str(output / "namespace-native-selected.png"))
    canvas.click(position={"x": 50, "y": 38})
    expect(status).to_contain_text("View: Global model")
    expect(status).to_contain_text(f"Selected σ₀; source {symbol('P', baseline['metadata']['root_program'])}")
    expect(status).to_contain_text("0 SSA values")
    expect(status).to_contain_text("Inspector: SSA value · no value selected")
    assert "Additional condition:" not in status.text_content()
    page.keyboard.press("3")
    settle_gesture(page)
    canvas.screenshot(path=str(output / "namespace-global-ssa.png"))
    select_inspector_tab(page, canvas, "Frame")
    expect(status).to_contain_text("Block entry · Frame f₀")
    page.keyboard.press("2")
    settle_gesture(page)
    canvas.screenshot(path=str(output / "namespace-global-control.png"))
    expect(status).to_contain_text("Original input entry")
    # Decoded original-input blocks are legal source provenance. Actual process
    # bodies live in a separate regional namespace and have no instruction cursor.
    assert all(not block["executed_pcs"] for block in baseline["cfg"])
    assert all(not block["instructions"] for block in baseline["ssa"]["blocks"])
    for summary in baseline["abstract_summaries"]:
        for node in summary["regional_cfg"]["nodes"]:
            frame=node["frame"]
            assert frame["process"] is not None
            assert all(frame[key] is None for key in ["code_address","code_hash","storage_address",
                                                     "address_value","caller","basic_block","stack_height"])
            assert frame["context"] == []
            transfer = node["transfer"]
            if transfer is not None:
                assert not transfer["visited_block"]
                process = transfer["process"]
                if process is not None and process["control"] is not None:
                    process_control_relation(baseline, frame["process"], process["control"])
    canvas.click(position={"x": 148, "y": 38})
    expect(status).to_contain_text("View: Detailed paths")
    expect(status).to_contain_text(f"Selected σ₀; source {symbol('P', root['id'])}")
    expect(status).to_contain_text(f"Additional condition: root ADDRESS = {coordinate}")
    expect(status).to_contain_text(f"{len(native['cfg'])} CFG states")
    expect(status).to_contain_text("Block entry · Frame f₀")
    page.keyboard.press("0")
    settle_gesture(page)
    canvas.screenshot(path=str(output / "namespace-native-reset.png"))
    return {"native_selected_state": target["id"], "global_state": 0, "returned_native_state": 0,
            "root_condition": coordinate, "ssa_value_cleared": True, "graph_ids_separate": True}


def coarse_process_controls(page, canvas, output):
    """Open the actual global routes retained beside an unfinished CREATE prefix."""
    reply=submit_bytecode(page,canvas,"5a5f5ff0",output/"process-input.png")
    report=reply.report
    (output/"process-original-report.json").write_text(json.dumps(report,indent=2)+"\n")
    assert report["hybrid"]["coverage"]["baseline_current"]
    assert oracle_graph(report, "Native")["status"] == "Incomplete"
    summary=oracle_graph(report, "Baseline")["abstract_summaries"][-1]
    assert summary["source_current"] and summary["closed"]
    procedures=summary["procedures"]
    constructor=next(i for i,p in enumerate(procedures)
                     if p["semantics"]=="ConstructorProcess" and p["process"]["owner"]=="Residual")
    callback=next(i for i,p in enumerate(procedures) if p["semantics"]=="KnownProcess" and p["is_static"])
    calls=summary["regional_cfg"]["calls"]
    assert {call["action"] for call in calls} == {"Call","StaticCall","DelegateCall","CallCode","Create","Create2"}
    assert all(call["scope"]=="Primitive" for call in calls)
    cases=[case for call in calls for delivery in call["deliveries"] for case in delivery["creation"]]
    assert any(case["result"]=="Zero" for case in cases)
    assert any(isinstance(case["result"],dict) and "Address" in case["result"] for case in cases)
    canvas.click(position={"x":50,"y":38})
    expect(page.locator("#analysis-status")).to_contain_text("View: Global model")
    select_inspector_tab(page,canvas,"Frame")
    expect(page.locator("#analysis-status")).to_contain_text("Original input entry")
    inspector_title_action(page,canvas,"Maximize",inspector_frame_tab(page,canvas))
    page.mouse.move(2,2);settle_gesture(page)
    canvas.screenshot(path=str(output/"process-global-routes.png"))
    # Compare the route list below the expanded header, excluding task timings
    # and the clicked header's legitimate keyboard-focus decoration.
    route_region={"x":0,"y":510,"width":500,"height":100}
    routes=page.screenshot(clip=route_region)
    expanded={}
    # Actual maximized Frame layout: the original snapshot stays collapsed;
    # Procedure 0 begins at y234 and each collapsed procedure uses 23 points.
    for name,index in [("constructor",constructor),("static-callback",callback)]:
        y=234+23*index
        canvas.click(position={"x":110,"y":y})
        page.mouse.move(2,2);settle_gesture(page)
        painted=canvas.screenshot(path=str(output/f"process-{name}-expanded.png"))
        assert page.screenshot(clip=route_region) != routes, f"the actual {name} procedure did not expand"
        expanded[name]=hashlib.sha256(painted).hexdigest()
        canvas.click(position={"x":110,"y":y})
        page.mouse.move(2,2);settle_gesture(page)
        canvas.screenshot(path=str(output/f"process-{name}-closed.png"))
        assert page.screenshot(clip=route_region) == routes, f"closing {name} lost the global routes"
    canvas.click(position={"x":110,"y":234+23*len(procedures)})
    page.mouse.move(2,2);settle_gesture(page)
    canvas.screenshot(path=str(output/"process-regional-control.png"))
    page.mouse.move(1000,800);page.mouse.wheel(0,600);settle_gesture(page)
    canvas.screenshot(path=str(output/"process-regional-control-end.png"))
    canvas.click(position={"x":110,"y":page.viewport_size["height"]-103})
    page.mouse.move(1000,800);page.mouse.wheel(0,500);settle_gesture(page)
    canvas.screenshot(path=str(output/"process-creation-deliveries.png"))
    first=calls[0]
    assert first["action"]=="Create" and first["scope"]=="Primitive"
    assert any(case["result"]=="Zero" for delivery in first["deliveries"] for case in delivery["creation"])
    assert any(isinstance(case["result"],dict) and "Address" in case["result"]
               for delivery in first["deliveries"] for case in delivery["creation"])
    inspector_title_action(page,canvas,"Restore",inspector_frame_tab(page,canvas))
    canvas.click(position={"x":148,"y":38})
    expect(page.locator("#analysis-status")).to_contain_text("View: Detailed paths")
    expect(page.locator("#analysis-status")).to_contain_text("SSA partial")
    select_inspector_tab(page,canvas,"Diagnostics")
    native=oracle_graph(report, "Native")
    assert any(frontier["kind"]=="Creation" and "UnknownInitCode" in frontier["detail"]
               for frontier in native["frontiers"])
    expect(page.locator("#analysis-status")).to_contain_text(f"{len(native['frontiers'])} unexpanded frontiers")
    page.keyboard.press("3");settle_gesture(page)
    canvas.screenshot(path=str(output/"process-native-frontier.png"))
    page.keyboard.press("Control+j");settle_gesture(page)
    expect(page.locator("#analysis-status")).to_contain_text("Inspector hidden")
    canvas.screenshot(path=str(output/"process-native-prefix.png"))
    assert any(step["phase"]=="Dispatched" for prefix in report["hybrid"]["local_prefixes"]
               for step in prefix["instructions"])
    return {"constructor_procedure":constructor,"static_callback_procedure":callback,
            "calls":len(calls),"creation_cases":len(cases),"expanded":expanded}

def inspector_controls_interactions(browser, url: str, output: Path):
    """Exercise panel state and the DOM shortcut boundary on the packaged app."""
    results = {}
    for platform in ["linux", "mac-simulated"]:
        options = {"viewport": {"width": 1440, "height": 1000}}
        if platform == "mac-simulated":
            options["user_agent"] = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36"
        context = browser.new_context(**options)
        page = context.new_page()
        observe_webgpu(page)
        if platform == "mac-simulated":
            page.add_init_script("Object.defineProperty(navigator,'platform',{get:()=> 'MacIntel'});")
        page.add_init_script("""window.__inspectKeyEvents=[];
            window.addEventListener('keydown',event=>{
                if(event.key.toLowerCase()!=='j')return;
                const record={ctrl:event.ctrlKey,meta:event.metaKey,shift:event.shiftKey,
                    alt:event.altKey,repeat:event.repeat,composing:event.isComposing};
                window.__inspectKeyEvents.push(record);
                setTimeout(()=>record.prevented=event.defaultPrevented,0);
            },true);""")
        errors, posts = [], []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.on("request", lambda request: posts.append(request.url)
                if request.method == "POST" and request.url.endswith("/api/tasks") else None)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                  and response.request.method == "POST") as initial_post:
            page.goto(url, wait_until="networkidle")
        initial_reply = complete_submission(page, initial_post.value, namespace="Native")
        status = page.locator("#analysis-status")
        expect(status).to_contain_text("Ready:", timeout=60000)
        canvas = page.locator("#evm-canvas")
        namespace_checks = None
        if platform == "linux":
            namespace_checks = model_namespace_controls(page, canvas, initial_reply.report, output)
        canvas.click(position={"x": 720, "y": 12})
        page.keyboard.press("3")
        settle_gesture(page)
        expect(status).to_contain_text("Inspector: Frame")
        original_selection = re.search(r"Selected ([^;]+); source ([^;]+);", status.text_content()).group(0)
        initial = inspector_frame_tab(page, canvas)
        modifier = "Meta" if platform == "mac-simulated" else "Control"
        accepted = {"metaKey": True} if platform == "mac-simulated" else {"ctrlKey": True}

        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector hidden")
        assert inspect_shortcut_events(page)[-1]["prevented"], "browser default action was not suppressed"
        assert len(context.pages) == 1 and page.url.startswith(url), "shortcut opened a browser page"
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector: Frame")
        assert inspector_frame_tab(page, canvas) == initial

        page.keyboard.down(modifier)
        page.keyboard.down("j")
        expect(status).to_contain_text("Inspector hidden")
        page.keyboard.down("j")
        settle_gesture(page)
        expect(status).to_contain_text("Inspector hidden")
        assert inspect_shortcut_events(page)[-1]["repeat"]
        assert inspect_shortcut_events(page)[-1]["prevented"]
        page.keyboard.up("j")
        page.keyboard.up(modifier)
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector: Frame")

        opposite = {"ctrlKey": True} if platform == "mac-simulated" else {"metaKey": True}
        synthetic_inspect_key(page, **opposite)
        expect(status).to_contain_text("Inspector: Frame")
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        for extra in [{"shiftKey": True}, {"altKey": True}, opposite, {"isComposing": True}]:
            synthetic_inspect_key(page, **accepted, **extra)
            expect(status).to_contain_text("Inspector: Frame")
            assert not inspect_shortcut_events(page)[-1]["prevented"]

        page.evaluate("document.activeElement.dispatchEvent(new CompositionEvent('compositionstart',{bubbles:true}))")
        synthetic_inspect_key(page, **accepted)
        expect(status).to_contain_text("Inspector: Frame")
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        page.evaluate("document.activeElement.dispatchEvent(new CompositionEvent('compositionend',{bubbles:true}))")
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector hidden")
        assert inspect_shortcut_events(page)[-1]["prevented"]
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector: Frame")

        page.evaluate("""() => {
            const outside=document.createElement('input');outside.id='outside-workbench';outside.type='text';
            outside.style.position='fixed';outside.style.left='-1000px';
            document.body.append(outside);outside.focus();
        }""")
        synthetic_inspect_key(page, **accepted)
        expect(status).to_contain_text("Inspector: Frame")
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        page.evaluate("document.getElementById('outside-workbench').remove()")
        canvas.click(position={"x": 720, "y": 12})
        open_input(page)
        synthetic_inspect_key(page, **accepted)
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        page.keyboard.press("Escape")
        settle_gesture(page)
        expect(status).to_contain_text("Inspector: Frame")

        before_drag = inspector_frame_tab(page, canvas)
        page.mouse.move(720, before_drag[1] - 3)
        page.mouse.down()
        page.mouse.move(720, 150, steps=16)
        page.mouse.up()
        dragged = inspector_frame_tab(page, canvas)
        assert dragged[1] < 350, f"inspector is still capped near 38%: {dragged}"
        canvas.screenshot(path=str(output / f"inspect-{platform}-dragged.png"))

        inspector_title_action(page, canvas, "Maximize", dragged)
        expect(status).to_contain_text(re.compile("maximized", re.IGNORECASE))
        maximized = inspector_frame_tab(page, canvas)
        assert maximized[1] < 100, maximized
        canvas.screenshot(path=str(output / f"inspect-{platform}-maximized.png"))
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector hidden")
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text(re.compile("maximized", re.IGNORECASE))
        assert inspector_frame_tab(page, canvas) == maximized
        inspector_title_action(page, canvas, "Restore", maximized)
        expect(status).not_to_contain_text(re.compile("maximized", re.IGNORECASE))
        expect(status).to_contain_text("Inspector: Frame")
        restored = inspector_frame_tab(page, canvas)
        assert abs(restored[1] - dragged[1]) <= 2, (dragged, restored)
        resize_view(page, 1440, 450)
        canvas.screenshot(path=str(output / f"inspect-{platform}-short.png"))
        resize_view(page, 1440, 1000)
        returned = inspector_frame_tab(page, canvas)
        assert abs(returned[1] - dragged[1]) <= 2, (dragged, returned)
        inspector_title_action(page, canvas, "Hide", returned)
        expect(status).to_contain_text("Inspector hidden")
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector: Frame")
        assert inspector_frame_tab(page, canvas) == returned
        expect(status).to_contain_text(original_selection)

        select_inspector_tab(page, canvas, "SSA value", (returned[1] + returned[3]) / 2)
        canvas.dblclick(position={"x": 85, "y": returned[3] + 16})
        settle_gesture(page)
        assert page.evaluate("document.activeElement.tagName") == "INPUT", "SSA value editor did not own text focus"
        page.keyboard.press(f"{modifier}+a")
        page.keyboard.insert_text("1")
        settle_gesture(page)
        expect(status).to_contain_text("Inspector: SSA value %1")
        canvas.screenshot(path=str(output / f"inspect-{platform}-text-focused.png"))
        # An external HTML input must remain outside shortcut ownership even
        # immediately after a real egui editor held the hidden text agent.
        page.evaluate("""() => {
            const outside=document.createElement('input');outside.id='outside-value-editor';outside.type='text';
            outside.style.position='fixed';outside.style.left='-1000px';
            document.body.append(outside);outside.focus();
        }""")
        synthetic_inspect_key(page, **accepted)
        expect(status).to_contain_text("Inspector: SSA value %1")
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        page.evaluate("document.getElementById('outside-value-editor').remove()")
        canvas.dblclick(position={"x": 85, "y": returned[3] + 16})
        settle_gesture(page)
        assert page.evaluate("document.activeElement.tagName") == "INPUT", "SSA value editor did not recover text focus"
        expect(status).to_contain_text("Inspector: SSA value %1")
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector hidden")
        assert inspect_shortcut_events(page)[-1]["prevented"], "text-agent shortcut leaked to the browser"
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector: SSA value %1")
        expect(status).to_contain_text(original_selection)
        assert len(context.pages) == 1 and page.url.startswith(url)
        canvas.screenshot(path=str(output / f"inspect-{platform}-text-focus-restored.png"))
        assert len(posts) == 1, "panel operations reran the analysis"
        assert not errors, errors
        process_checks = coarse_process_controls(page,canvas,output) if platform == "linux" else None
        assert len(posts) == (2 if process_checks else 1)
        results[platform] = {"initial": initial, "dragged": dragged, "maximized": maximized,
            "restored": restored, "returned": returned, "analysis_submissions": len(posts),
            "namespace_controls": namespace_checks,
            "process_controls": process_checks,
            "key_events": inspect_shortcut_events(page), "renderer": webgpu_evidence(page)}
        context.close()
    (output / "inspector-controls-report.json").write_text(json.dumps(results, indent=2) + "\n")
    return results


def run(context):
    return inspector_controls_interactions(context.browser, context.url, context.output)
