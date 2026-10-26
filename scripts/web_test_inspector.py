"""Inspector browser acceptance cases."""

from web_test_graph_helpers import assert_availability, symbol
from web_test_hybrid import assert_root_source, native_entry_context, oracle_graph, process_control_relation, source_program
from pathlib import Path
from playwright.sync_api import expect
import hashlib
import json
import re
from web_test_harness import complete_submission, edit_control, focus_text_control, hide_world_directory, observe_webgpu, open_input, resize_view, select_report_context, select_report_namespace, set_control, settle_gesture, submit_bytecode, ui_control, ui_controls, webgpu_evidence
from web_test_inspector_helpers import inspector_frame_tab, inspector_selected_tab, inspector_title_action, select_inspector_scope, select_inspector_tab


def inspect_shortcut_events(page):
    # Default prevention is recorded in a timer after event propagation. The
    # DOM status can change first, so await that observation before asserting it.
    page.wait_for_function("""() => window.__inspectKeyEvents.length > 0 &&
        window.__inspectKeyEvents.every(event => typeof event.prevented === 'boolean')""",
        timeout=5000)
    return page.evaluate("window.__inspectKeyEvents")

def inspect_key(page, **modifiers):
    keys = {"ctrlKey": "Control", "metaKey": "Meta", "shiftKey": "Shift", "altKey": "Alt"}
    assert modifiers.keys() <= keys.keys() and all(type(value) is bool for value in modifiers.values())
    chord = [key for name, key in keys.items() if modifiers.get(name, False)]
    page.keyboard.press("+".join([*chord, "j"]))
    settle_gesture(page)


def external_input_fixture(page, identifier):
    # Root's explicit fixture exception retains the foreign-input ownership
    # negative. This visible HTML input is not an application control; focus
    # and keyboard events still come from actual browser pointer/key dispatch.
    page.evaluate("""identifier => {
        const outside=document.createElement('input');outside.id=identifier;outside.type='text';
        Object.assign(outside.style,{position:'fixed',left:'8px',top:'8px',width:'120px',
            height:'24px',zIndex:'2147483647'});
        document.body.append(outside);
    }""", identifier)
    fixture = page.locator(f"#{identifier}")
    fixture.click()
    expect(fixture).to_be_focused()
    return fixture


def focus_inspector_value(page, canvas):
    control = ui_control(page, "inspector_ssa_value", role="text")
    left, top, right, bottom = control["rect"]
    bounds = canvas.bounding_box()
    assert bounds is not None, "application canvas is not visible"
    canvas.dblclick(position={"x": (left + right) / 2 - bounds["x"],
                              "y": (top + bottom) / 2 - bounds["y"]})
    settle_gesture(page)
    return focus_text_control(page, canvas, "inspector_ssa_value")


def select_inspector_value(page, canvas, value):
    """Commit an explicit namespace-local value through the real DragValue."""
    assert type(value) is int and value >= 0, value
    focus_inspector_value(page, canvas)
    page.keyboard.press("Control+A")
    page.keyboard.insert_text(str(value))
    page.keyboard.press("Enter")
    settle_gesture(page)
    expect(page.locator("#analysis-status")).to_contain_text(f"Inspector: SSA value %{value}")


def selected_minimum_context(page, canvas, summary, procedure):
    """Observe canonical context selection without activating a replacement."""
    set_control(page, canvas, "Sources", True)
    edit_control(page, canvas, "source_context_filter", f"Minimum {{ summary: {summary}, procedure: {procedure} }}")
    contexts = [control for control in ui_controls(page) if control["role"] == "context" and control["enabled"]]
    assert len(contexts) == 1 and contexts[0]["selected"] is True, contexts
    set_control(page, canvas, "Sources", False)
    return contexts[0]


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
    # Select the actual second source block through its qualified Native
    # context; the scoped Code view no longer uses a global row ordinal.
    root = source_program(report, native["metadata"]["root_program"])
    entry = native_entry_context(report)
    assert entry["program"] == root["id"] and entry["storage_address"] == coordinate
    second = root["blocks"][1]
    target, = [block for block in native["cfg"]
               if block["program"] == root["id"] and block["basic_block"] == second["id"]]
    target_state, = [state for state in native["states"] if state["state"] == target["id"]]
    target_frame, = target_state["entry"]["frames"]
    assert target["code_address"] == target_frame["code_address"] == root["code_address"]
    assert target["storage_address"] == target_frame["storage_address"] == entry["storage_address"]
    assert target_frame["is_static"] == native["metadata"]["environment"]["is_static"]
    assert target["id"] != baseline["cfg"][0]["id"]
    select_report_context(page, report, "Native", state=target["id"])
    expect(status).to_contain_text(f"Selected {symbol('σ', target['id'])}; source {symbol('P', root['id'])}")
    select_inspector_tab(page, canvas, "SSA value")
    target_value = next(value for value in native["ssa"]["values"]
                        if value["origin"].get("Instruction", {}).get("state") == target["id"])
    target_definition = target_value["origin"]["Instruction"]
    select_inspector_value(page, canvas, target_value["id"])
    expect(status).to_contain_text(f"Selected {symbol('σ', target['id'])}; source {symbol('P', root['id'])}")
    expect(status).to_contain_text(f"Instruction {symbol('σ', target['id'])} · pc 0x{target_definition['pc']:x}")
    canvas.screenshot(path=str(output / "namespace-native-selected.png"))
    model = report["minimum"]
    assert model["namespace"] == "Baseline" and model["current"] and model["status"] == "Converged"
    summary_index, relation = next((index, summary["relation"])
                    for index, summary in reversed(list(enumerate(model["summaries"])))
                    if summary["relation"]["source_current"]
                    and summary["relation"]["root_procedure"] is not None
                    and summary["relation"]["procedures"][summary["relation"]["root_procedure"]]["process"]["source"]
                        == model["declared_root_source"])
    root_node = next(node for node in relation["regional_cfg"]["nodes"]
                     if node["procedure"] == relation["root_procedure"])
    point = root_node["frame"]["process"]["location"]
    # The live status does not print CFG receipt IDs on the SSA route. Bind
    # the retained r-ID through its unique admitted point plus the actually
    # selected summary/procedure, instead of expecting an inactive CFG string.
    admitted = []
    for node in relation["regional_cfg"]["nodes"]:
        if node["procedure"] != relation["root_procedure"]:
            continue
        region = ((node["transfer"] or {}).get("process") or {}).get("control")
        if (node["frame"]["process"]["location"] == point
                or region is not None and point in [region["entry"], *region["through"], *region["boundaries"]]):
            admitted.append(node["id"])
    assert admitted == [root_node["id"]], admitted
    point_label = (f"block index {point['Block']}" if type(point) is dict else {
        "Any": "unobserved code · arbitrary actions", "End": "end of source · no instruction",
        "JumpTargets": "unknown-jump hub · all legal JUMPDEST targets"}[point])
    select_report_context(page, report, "Baseline", summary=summary_index, procedure=relation["root_procedure"])
    select_inspector_tab(page, canvas, "Frame", minimum=True)
    select_inspector_scope(page, canvas, "Original observations")
    expect(status).to_contain_text(f"Selected MAY source: {point_label}")
    expect(status).to_contain_text("Inspector: Frame · Original observations")
    assert "Selected σ" not in status.text_content() and "SSA value %" not in status.text_content()
    assert "Additional condition:" not in status.text_content()
    page.keyboard.press("3")
    settle_gesture(page)
    canvas.screenshot(path=str(output / "namespace-global-ssa.png"))
    global_ssa_context = selected_minimum_context(page, canvas, summary_index, relation["root_procedure"])
    assert_availability(page, "MAY control has no Native instruction or SSA receipt")
    expect(status).to_contain_text(f"Selected MAY source: {point_label}")
    page.keyboard.press("2")
    settle_gesture(page)
    canvas.screenshot(path=str(output / "namespace-global-control.png"))
    global_cfg_context = selected_minimum_context(page, canvas, summary_index, relation["root_procedure"])
    expect(status).to_contain_text(f"Selected MAY source: {point_label}")
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
    select_report_context(page, report, "Native", state=entry["id"])
    expect(status).to_contain_text("View: Detailed paths")
    expect(status).to_contain_text(f"Selected σ₀; source {symbol('P', root['id'])}")
    expect(status).to_contain_text(f"Additional condition: root ADDRESS = {coordinate}")
    expect(status).to_contain_text(f"{len(native['cfg'])} CFG states")
    # A context pick has no implicit value selection. Both explicit numeric
    # edits below belong to the retained root context in this Native namespace.
    definition = native["ssa"]["values"][0]["origin"]["Instruction"]
    assert definition["state"] == 0 and definition["pc"] == 0
    assert definition["state"] == entry["id"], (definition, entry)
    select_inspector_tab(page, canvas, "SSA value")
    alternate = next(value for value in native["ssa"]["values"]
                     if value["id"] != 0 and value["origin"].get("Instruction", {}).get("state") == entry["id"])
    select_inspector_value(page, canvas, alternate["id"])
    select_inspector_value(page, canvas, 0)
    expect(status).to_contain_text("Inspector: SSA value %0")
    expect(status).to_contain_text("Instruction σ₀ · pc 0x0")
    select_inspector_tab(page, canvas, "Frame")
    expect(status).to_contain_text("Block entry · Frame f₀")
    page.keyboard.press("0")
    settle_gesture(page)
    canvas.screenshot(path=str(output / "namespace-native-reset.png"))
    return {"native_selected_state": target["id"], "global_regional_node": root_node["id"],
            "native_selected_value": target_value["id"], "native_value_definition": target_definition,
            "global_selected_contexts": {"SSA": global_ssa_context, "CFG": global_cfg_context},
            "global_admitted_regional_nodes": admitted,
            "global_may_location": point, "returned_native_state": entry["id"],
            "root_condition": coordinate, "global_has_no_ssa_selection": True,
            "native_selection_reset": True, "graph_ids_separate": True}


def coarse_process_controls(page, canvas, output):
    """Open the actual global routes retained beside an unfinished CREATE prefix."""
    reply=submit_bytecode(page,canvas,"5a5f5ff0",output/"process-input.png")
    report=reply.report
    (output/"process-original-report.json").write_text(json.dumps(report,indent=2)+"\n")
    assert report["hybrid"]["coverage"]["baseline_current"]
    assert oracle_graph(report, "Native")["status"] == "Incomplete"
    summary=report["minimum"]["summaries"][-1]["relation"]
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
    select_report_namespace(page,report,"Baseline")
    select_inspector_tab(page,canvas,"Coverage",minimum=True)
    select_inspector_scope(page,canvas,"Procedure input")
    inspector_title_action(page,canvas,"Maximize",inspector_selected_tab(page,canvas,"Coverage",minimum=True))
    page.mouse.move(2,2);settle_gesture(page)
    canvas.screenshot(path=str(output/"process-global-routes.png"))
    # The canonical model chooses a procedure instead of expanding a rich
    # private Frame tree. Compare its qualified body below the selector.
    selector=ui_control(page,"minimum_procedure",role="combobox")
    route_region={"x":8,"y":selector["rect"][3]+3,"width":900,"height":140}
    routes=page.screenshot(clip=route_region)
    inspected={}

    def choose_procedure(index):
        page.mouse.move(800,400);page.mouse.wheel(0,-5000);settle_gesture(page)
        assert type(index) is int and 0<=index<len(procedures),index
        chosen=select_report_context(page,report,"Baseline",
                                    summary=len(report["minimum"]["summaries"])-1,procedure=index)
        assert chosen["role"]=="context" and chosen["enabled"],chosen
        select_inspector_tab(page,canvas,"Coverage",minimum=True)
        select_inspector_scope(page,canvas,"Procedure input")
        page.mouse.move(2,2);settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text(f"· procedure {index} · output None")

    for name,index in [("constructor",constructor),("static-callback",callback)]:
        choose_procedure(index)
        painted=canvas.screenshot(path=str(output/f"process-{name}-selected.png"))
        assert page.screenshot(clip=route_region) != routes, f"the actual {name} body did not change"
        inspected[name]=hashlib.sha256(painted).hexdigest()
        choose_procedure(summary["root_procedure"])
        canvas.screenshot(path=str(output/f"process-{name}-returned.png"))
        assert page.screenshot(clip=route_region) == routes, f"returning from {name} lost the global body"

    def open_last_control(name,label):
        page.mouse.move(800,400);page.mouse.wheel(0,5000);settle_gesture(page)
        canvas.screenshot(path=str(output/f"process-{name}-closed.png"))
        control=set_control(page,canvas,label,True,role="section")
        assert control["selected"] is True,control
        page.mouse.move(2,2);settle_gesture(page)

    open_last_control("regional-control","Regional control flow")
    canvas.screenshot(path=str(output/"process-regional-control.png"))
    open_last_control("calls-and-returns","Calls and returns")
    page.mouse.move(1000,800);page.mouse.wheel(0,600);settle_gesture(page)
    canvas.screenshot(path=str(output/"process-creation-deliveries.png"))
    first=calls[0]
    assert first["action"]=="Create" and first["scope"]=="Primitive"
    assert any(case["result"]=="Zero" for delivery in first["deliveries"] for case in delivery["creation"])
    assert any(isinstance(case["result"],dict) and "Address" in case["result"]
               for delivery in first["deliveries"] for case in delivery["creation"])
    inspector_title_action(page,canvas,"Restore",inspector_selected_tab(page,canvas,"Coverage",minimum=True))
    select_report_namespace(page,report,"Native")
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
            "calls":len(calls),"creation_cases":len(cases),"inspected":inspected}

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
        session = context.new_cdp_session(page)
        if platform == "mac-simulated":
            session.send("Emulation.setUserAgentOverride", {
                "userAgent": options["user_agent"], "platform": "MacIntel"})
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
        hide_world_directory(page, canvas)
        namespace_checks = None
        if platform == "linux":
            namespace_checks = model_namespace_controls(page, canvas, initial_reply.report, output)
        page.keyboard.press("3")
        select_inspector_tab(page, canvas, "Frame")
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
        inspect_key(page, **opposite)
        expect(status).to_contain_text("Inspector: Frame")
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        for extra in [{"shiftKey": True}, {"altKey": True}, opposite]:
            inspect_key(page, **accepted, **extra)
            expect(status).to_contain_text("Inspector: Frame")
            assert not inspect_shortcut_events(page)[-1]["prevented"]

        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector hidden")
        assert inspect_shortcut_events(page)[-1]["prevented"]
        page.keyboard.press(f"{modifier}+j")
        expect(status).to_contain_text("Inspector: Frame")

        outside = external_input_fixture(page, "outside-workbench")
        inspect_key(page, **accepted)
        expect(status).to_contain_text("Inspector: Frame")
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        outside.evaluate("element => element.remove()")
        select_inspector_tab(page, canvas, "Frame")
        open_input(page, mode="Bytecode")
        inspect_key(page, **accepted)
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
        navigation_bottom = max(ui_control(page, label, role="action", enabled=None)["rect"][3]
                                for label in ["Back", "Forward", "Entry"])
        assert 0 <= maximized[1] - navigation_bottom <= 16, (navigation_bottom, maximized)
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
        focus_inspector_value(page, canvas)
        assert page.evaluate("document.activeElement.tagName") == "INPUT", "SSA value editor did not own text focus"
        page.keyboard.press(f"{modifier}+a")
        page.keyboard.insert_text("1")
        settle_gesture(page)
        expect(status).to_contain_text("Inspector: SSA value %1")
        canvas.screenshot(path=str(output / f"inspect-{platform}-text-focused.png"))
        # An external HTML input must remain outside shortcut ownership even
        # immediately after a real egui editor held the hidden text agent.
        outside = external_input_fixture(page, "outside-value-editor")
        inspect_key(page, **accepted)
        expect(status).to_contain_text("Inspector: SSA value %1")
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        outside.evaluate("element => element.remove()")
        focus_inspector_value(page, canvas)
        assert page.evaluate("document.activeElement.tagName") == "INPUT", "SSA value editor did not recover text focus"
        expect(status).to_contain_text("Inspector: SSA value %1")
        # Chromium generates the composition lifecycle on the actual focused
        # framework text agent. Test both a composing key and a second chord
        # while preedit remains active, then cancel it without changing ValueId.
        ui_control(page, "Inspect", role="toggle", selected=True, enabled=True)
        session.send("Input.imeSetComposition", {"text": "1", "selectionStart": 0, "selectionEnd": 1})
        settle_gesture(page)
        inspect_key(page, **accepted)
        expect(status).to_contain_text("Inspector: SSA value %1")
        assert inspect_shortcut_events(page)[-1]["composing"]
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        inspect_key(page, **accepted)
        expect(status).to_contain_text("Inspector: SSA value %1")
        assert not inspect_shortcut_events(page)[-1]["prevented"]
        session.send("Input.imeSetComposition", {"text": "", "selectionStart": 0, "selectionEnd": 0})
        settle_gesture(page)
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
        session.detach()
        context.close()
    (output / "inspector-controls-report.json").write_text(json.dumps(results, indent=2) + "\n")
    return results


def run(context):
    return inspector_controls_interactions(context.browser, context.url, context.output)
