"""Inspector browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import json
import re
from web_test_harness import observe_webgpu, open_input, resize_view, settle_gesture, webgpu_evidence
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
        page.goto(url, wait_until="networkidle")
        status = page.locator("#analysis-status")
        expect(status).to_contain_text("Ready:", timeout=60000)
        canvas = page.locator("#evm-canvas")
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
        results[platform] = {"initial": initial, "dragged": dragged, "maximized": maximized,
            "restored": restored, "returned": returned, "analysis_submissions": len(posts),
            "key_events": inspect_shortcut_events(page), "renderer": webgpu_evidence(page)}
        context.close()
    (output / "inspector-controls-report.json").write_text(json.dumps(results, indent=2) + "\n")
    return results


def run(context):
    return inspector_controls_interactions(context.browser, context.url, context.output)
