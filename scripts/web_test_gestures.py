"""Gestures browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
import math
import re
from web_test_graph_helpers import frozen_camera, graph_content, graph_mode, graph_overview, pinch_wheel, workspace_panes
from web_test_harness import click_control, complete_submission, hide_world_directory, launch_browser, observe_webgpu, open_input, posted_bytecode, rendered_frame, settle_gesture, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_pixels import assert_anchored_zoom, assert_same_scene, assert_translation, canvas_screenshot, graph_screenshot_metrics, node_snapshot, require_selected_complete, screenshot_metrics


def graph_interactions(browser_type, executable: str, url: str, output: Path, dprs=(1, 2)):
    results = {}
    for dpr in dprs:
        # CDP device_scale_factor alone changes devicePixelRatio but Chromium's
        # devicePixelContentBoxSize remains at the physical browser scale. egui
        # trusts that observer, so launch with the matching real scale as well.
        browser = launch_browser(browser_type, executable, dpr)
        try:
            context = browser.new_context(
                viewport={"width": 1440, "height": 1000}, device_scale_factor=dpr,
            )
            page = context.new_page()
            observe_webgpu(page)
            failures = []
            page.on("pageerror", lambda error: failures.append(str(error)))
            page.on("console", lambda message: failures.append(message.text)
                    if message.type == "error" else None)
            with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                      and response.request.method == "POST") as initial:
                page.goto(url, wait_until="networkidle")
            complete_submission(page, initial.value, namespace="Native")
            status = page.locator("#analysis-status")
            expect(status).to_contain_text("Ready:", timeout=60000)
            expect(status).to_contain_text("Route: CFG")
            expect(status).to_contain_text("CFG nodes: Disasm")
            canvas = page.locator("#evm-canvas")
            page.wait_for_function("() => { const c = document.querySelector('canvas'); return c.width === innerWidth * devicePixelRatio && c.height === innerHeight * devicePixelRatio; }")
            display = page.evaluate("""() => new Promise(resolve => {
                const canvas = document.querySelector('canvas');
                const observer = new ResizeObserver(([entry]) => {
                    observer.disconnect();
                    resolve({dpr: devicePixelRatio, visual_scale: visualViewport.scale,
                             backing: [canvas.width, canvas.height],
                             client: canvas.getBoundingClientRect().toJSON(),
                             content: entry.contentRect.toJSON(),
                             physical: Array.from(entry.devicePixelContentBoxSize,
                                                  box => [box.inlineSize, box.blockSize])});
                });
                observer.observe(canvas, {box: 'device-pixel-content-box'});
            })""")
            hide_inspector(page, canvas)
            hide_world_directory(page, canvas)
            graph_overview(page, canvas, fit=True)
            graph_content(page, canvas, "SSA")
            graph_mode(page, canvas, "States")
            cdp = context.new_cdp_session(page)
            prefix = f"gestures-dpr{dpr}"
            initial_frozen = frozen_camera(page, canvas, output, f"{prefix}-initial-frozen", True)
            page.keyboard.press("f")
            settle_gesture(page)
            initial = node_snapshot(page, canvas, output, f"{prefix}-initial")
            page.mouse.move(30, 700)
            page.mouse.wheel(-48, -32)
            settle_gesture(page)
            panned = node_snapshot(page, canvas, output, f"{prefix}-pan-background")
            assert_translation(initial, panned, 48, 32)

            bounds = panned["bounds"]
            node_center = ((bounds[0] + bounds[2]) / 2, (bounds[1] + bounds[3]) / 2)
            page.mouse.move(*node_center)
            page.mouse.wheel(-35, -26)
            settle_gesture(page)
            # Move off the node so its hover tooltip cannot contaminate pixels.
            page.mouse.move(30, 700)
            rendered_frame(page)
            over_node = node_snapshot(page, canvas, output, f"{prefix}-pan-over-node")
            assert_translation(panned, over_node, 35, 26)

            bounds = over_node["bounds"]
            anchor = ((bounds[0] + bounds[2]) / 2, (bounds[1] + bounds[3]) / 2)
            page.mouse.move(*anchor)
            page.keyboard.down("Control")
            page.mouse.wheel(0, -80)
            page.keyboard.up("Control")
            settle_gesture(page)
            page.mouse.move(30, 700)
            rendered_frame(page)
            ctrl_zoom = node_snapshot(page, canvas, output, f"{prefix}-ctrl-wheel")
            ctrl_ratio = assert_anchored_zoom(over_node, ctrl_zoom, anchor)

            bounds = ctrl_zoom["bounds"]
            anchor = (bounds[0] * 0.75 + bounds[2] * 0.25,
                      bounds[1] * 0.65 + bounds[3] * 0.35)
            pinch_wheel(page, cdp, anchor, -18)
            page.mouse.move(30, 700)
            rendered_frame(page)
            pinched = node_snapshot(page, canvas, output, f"{prefix}-pinch-wheel")
            pinch_ratio = assert_anchored_zoom(ctrl_zoom, pinched, anchor)
            assert abs(pinch_ratio - math.exp(0.18)) < 0.06, f"eframe pinch conversion changed: {pinch_ratio}"
            bounds = pinched["bounds"]
            safari_anchor = ((bounds[0] + bounds[2]) / 2, (bounds[1] + bounds[3]) / 2)
            page.mouse.move(*safari_anchor)
            rendered_frame(page)
            # Safari sends non-standard gesture events instead of relying solely on
            # Ctrl+wheel. Exercise eframe's generic Event/Reflect adapter in Chromium;
            # this is not a claim of testing Safari or a physical Mac trackpad.
            page.evaluate("""() => {
                const canvas = document.querySelector('#evm-canvas');
                for (const [kind, scale] of [['gesturestart', 1], ['gesturechange', 1.12], ['gestureend', 1.12]]) {
                    const event = new Event(kind, {bubbles: true, cancelable: true});
                    Object.assign(event, {scale, rotation: 0});
                    canvas.dispatchEvent(event);
                }
            }""")
            settle_gesture(page)
            page.mouse.move(30, 700)
            rendered_frame(page)
            safari_zoom = node_snapshot(page, canvas, output, f"{prefix}-safari-format-gesture")
            safari_ratio = assert_anchored_zoom(pinched, safari_zoom, safari_anchor)
            assert abs(safari_ratio - 1.12) < 0.04, f"Safari-format gesture scale was not applied: {safari_ratio}"
            assert page.evaluate("devicePixelRatio") == dpr, "graph gesture changed browser page zoom"
            assert page.evaluate("visualViewport.scale") == 1, "graph gesture changed visual viewport zoom"

            manual_frozen = frozen_camera(page, canvas, output, f"{prefix}-manual-frozen", False)

            # Explicit Native SSA keeps the original all-state gesture oracle.
            # Real toolbar controls and measured node widths verify the
            # representation change independently of screenshot hashes.
            expect(status).to_contain_text("CFG nodes: SSA")
            graph_content(page, canvas, "Disasm")
            expect(status).to_contain_text("CFG nodes: Disasm")
            disasm = node_snapshot(page, canvas, output, f"{prefix}-nodes-disasm")
            require_selected_complete(disasm);require_selected_complete(safari_zoom)
            disasm_width = disasm["bounds"][2] - disasm["bounds"][0]
            assert disasm["hash"] != safari_zoom["hash"], "Disasm mode retained SSA pixels"
            assert safari_zoom["bounds"][2] - safari_zoom["bounds"][0] > disasm_width + 10, "Disasm mode did not replace the fixture's SSA definitions/effects"
            graph_content(page, canvas, "SSA")
            expect(status).to_contain_text("CFG nodes: SSA")
            ssa = node_snapshot(page, canvas, output, f"{prefix}-nodes-ssa")
            require_selected_complete(ssa)
            assert ssa["hash"] != disasm["hash"], "SSA mode retained disassembly pixels"
            ssa_width = ssa["bounds"][2] - ssa["bounds"][0]
            assert ssa_width > disasm_width + 10, "SSA definitions/effects did not expand the fixture's rendered node"
            click_control(page, canvas, "Fit graph", role="action")
            expect(status).to_contain_text("CFG nodes: SSA")
            ssa_fit = node_snapshot(page, canvas, output, f"{prefix}-nodes-ssa-fit")
            require_selected_complete(ssa_fit)
            assert ssa_fit["bounds"][2] - ssa_fit["bounds"][0] < ssa_width, "Fit did not return the enlarged SSA graph to a readable overview"
            graph_content(page, canvas, "Disasm")
            expect(status).to_contain_text("CFG nodes: Disasm")
            zoom_screenshots = {}
            scene_reference = None
            for layout_name, height in [("tall", 1000), ("short", 390)]:
                page.set_viewport_size({"width": 1440, "height": height})
                page.wait_for_function("() => { const c = document.querySelector('canvas'); return c.width === innerWidth * devicePixelRatio && c.height === innerHeight * devicePixelRatio; }")
                click_control(page, canvas, "Fit graph", role="action")
                # The restored compact toolbar ends at y=115. Include the short
                # viewport root's real outline at y=122 after the fresh upward pan.
                region = (4, 119, 1436, height - 30)
                fitted = node_snapshot(page, canvas, output, f"{prefix}-edges-{layout_name}-fit", region, True)
                require_selected_complete(fitted)
                if scene_reference is None:
                    scene_reference = fitted
                else:
                    assert_same_scene(scene_reference, fitted)
                zoom_screenshots[f"{layout_name}-fit"] = fitted
                fitted_zoom = float(re.search(r"; CFG zoom: ([0-9.]+)%", status.text_content())[1])
                previous = 1.0
                for scale in [0.75, 0.40, 0.36, 0.16]:
                    pinch_wheel(page, cdp, (720, height * 0.65), -math.log(scale / previous) / 0.01)
                    page.mouse.move(30, height - 70)
                    rendered_frame(page)
                    name = str(round(scale * 100))
                    measured = node_snapshot(page, canvas, output, f"{prefix}-edges-{layout_name}-{name}", region,
                                             min_selected_pixels=1 if scale <= 0.16 else 20)
                    require_selected_complete(measured)
                    fitted_width = fitted["bounds"][2] - fitted["bounds"][0]
                    fitted_outline = fitted["selected_record"]["outline_bounds"]
                    actual_outline = measured["selected_record"]["outline_bounds"]
                    fitted_outline_width = fitted_outline[2] - fitted_outline[0]
                    actual_outline_width = actual_outline[2] - actual_outline[0]
                    observed_zoom = float(re.search(r"; CFG zoom: ([0-9.]+)%", status.text_content())[1])
                    assert abs(observed_zoom / fitted_zoom - scale) < 0.005, f"incorrect camera zoom at DPR {dpr}: {scale}: {observed_zoom}/{fitted_zoom}"
                    # paint.rs uses a fixed 1.5px inside selection stroke. Measure
                    # the complete selected outline for physical card size; its
                    # inset fill loses the stroke width even at tiny zoom levels.
                    assert abs(actual_outline_width - fitted_outline_width * scale) <= 4.0, f"incorrect screenshot zoom at DPR {dpr}: {scale}: {measured}"
                    zoom_screenshots[f"{layout_name}-{name}"] = measured
                    previous = scale

            # Compare shortcut behavior with the actual Fit button at the same
            # viewport, without assuming where any individual node should end up.
            canvas.click(position={"x": 143, "y": 110})
            settle_gesture(page)
            fit_keys = {"button": fitted["bounds"]}
            fresh_wheel_after_fit = None
            for key, name in [("f", "f"), ("Shift+f", "shift-f")]:
                page.mouse.move(30, height - 70)
                # Leave room above the selected root before enlarging it in the
                # short viewport, so the width comparison measures a whole card.
                page.mouse.wheel(-24, -48)
                settle_gesture(page)
                pinch_wheel(page, cdp, (720, height * 0.65), -18)
                page.mouse.move(30, height - 70)
                rendered_frame(page)
                manual = node_snapshot(page, canvas, output, f"{prefix}-{name}-manual", region)
                require_selected_complete(manual)
                assert manual["bounds"][2] - manual["bounds"][0] > fitted_width + 8, "shortcut fixture did not enter a zoomed manual camera"
                if key == "f":
                    # Do not settle here: Fit must override both the current wheel
                    # event and its remaining smooth-scroll animation on later frames.
                    page.mouse.wheel(-240, -180)
                page.keyboard.press(key)
                if key == "f":
                    # Confirm Fit ran, without waiting for the old 400 ms scroll
                    # animation to settle. A fresh opposite-axis gesture must start
                    # from zero instead of inheriting the previous wheel's tail.
                    rendered_frame(page)
                    page.mouse.wheel(-4, 3)
                    settle_gesture(page)
                    fresh = node_snapshot(page, canvas, output, f"{prefix}-f-fresh-wheel", region)
                    assert_translation(fitted, fresh, 4, -3, tolerance=0.75)
                    fresh_wheel_after_fit = fresh["bounds"]
                    page.keyboard.press("f")
                settle_gesture(page)
                restored = node_snapshot(page, canvas, output, f"{prefix}-{name}-fit", region)
                assert_translation(fitted, restored, 0, 0)
                fit_keys[key] = restored["bounds"]

            page.set_viewport_size({"width": 1440, "height": 1000})
            settle_gesture(page)
            page.keyboard.press("f")
            settle_gesture(page)
            page.mouse.move(30, 700)
            page.mouse.wheel(-60, -30)
            settle_gesture(page)
            # The modal blocks background graph shortcuts. Compare the actual
            # camera before opening and after dismissing it, then submit the exact
            # draft containing f/F through the same form.
            bytecode = "\n".join(f"60{index:02x}" for index in range(70)) + "\n00"
            editor_region = (4, 123, 1436, 970)
            before_typing = node_snapshot(page, canvas, output, f"{prefix}-editor-before-f", editor_region)
            open_input(page, mode="Bytecode")
            page.keyboard.press("Control+A")
            page.keyboard.type(bytecode, delay=5)
            settle_gesture(page)
            editor_before_scroll = canvas_screenshot(page, path=str(output / f"{prefix}-modal-before-scroll.png"))
            page.mouse.move(600, 420)
            page.mouse.wheel(0, -300)
            settle_gesture(page)
            editor_after_scroll = canvas_screenshot(page, path=str(output / f"{prefix}-modal-after-scroll.png"))
            assert screenshot_metrics(page, editor_before_scroll, (350, 365, 1090, 480))["hash"] != screenshot_metrics(page, editor_after_scroll, (350, 365, 1090, 480))["hash"], "wheel did not scroll the actual modal editor"
            page.keyboard.type("fF", delay=40)
            rendered_frame(page)
            canvas_screenshot(page, path=str(output / f"{prefix}-editor-draft-f.png"))
            page.keyboard.press("Escape")
            settle_gesture(page)
            after_typing = node_snapshot(page, canvas, output, f"{prefix}-editor-after-f", editor_region)
            assert_translation(before_typing, after_typing, 0, 0)
            open_input(page, mode="Bytecode")
            with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
                page.keyboard.press("Control+Enter")
            reply = complete_submission(page, response.value, namespace="Native")
            hide_world_directory(page, canvas)
            assert reply.status == 200
            assert posted_bytecode(reply) == bytecode + "fF", "focused f/F keys did not reach the bytecode request"
            expect(status).to_contain_text("Ready:")
            focused_fit_keys = {"before": before_typing["bounds"], "after": after_typing["bounds"],
                                "submitted_bytecode": posted_bytecode(reply)}

            neighboring = {}
            if dpr == 1:
                # The editor fixture has enough PCs/definitions for both code panes
                # to scroll; trailing fF is an unreachable byte after STOP.
                canvas.click(position={"x": 720, "y": 700})
                page.keyboard.press("0")
                settle_gesture(page)
                # Moving into the workspace may clip the frozen full-view camera.
                # Explicitly fit once, then verify neighbor scrolling cannot move it.
                page.keyboard.press("f")
                settle_gesture(page)
                panes, _ = workspace_panes(page, canvas, output, "adjacent-pane-boundaries")
                left, right = panes["left"], panes["right"]
                graph_region = (left + 4, 155, right - 4, 970)
                for name, pointer, region in [
                    ("disassembly", (left / 2, 400), (4, 74, left - 4, 900)),
                    ("ssa", ((right + 1440) / 2, 400), (right + 4, 74, 1436, 900)),
                ]:
                    # Read geometry with tooltips closed. In content-sized panes a
                    # source tooltip can overlap the neighboring selected node.
                    page.mouse.move(2, 2)
                    settle_gesture(page)
                    before_png = canvas_screenshot(page)
                    before_node = graph_screenshot_metrics(page, before_png, graph_region)
                    before_code = screenshot_metrics(page, before_png, region)
                    assert before_node["bounds"], f"missing node before {name} scroll"
                    page.mouse.move(*pointer)
                    page.mouse.wheel(0, 220)
                    settle_gesture(page)
                    page.mouse.move(2, 2)
                    settle_gesture(page)
                    after_png = canvas_screenshot(page, path=str(output / f"adjacent-scroll-{name}.png"))
                    after_node = graph_screenshot_metrics(page, after_png, graph_region)
                    after_code = screenshot_metrics(page, after_png, region)
                    assert_translation(before_node, after_node, 0, 0)
                    assert before_code["hash"] != after_code["hash"], f"{name} pane did not scroll"
                    assert before_code["selected_pixels"] > 100, f"selected {name} block header was not initially visible"
                    assert after_code["selected_pixels"] < before_code["selected_pixels"] / 10, f"selected {name} header did not scroll out of view"
                    neighboring[name] = {"graph_before": before_node["bounds"], "graph_after": after_node["bounds"], "code_changed": True, "selected_header_pixels": [before_code["selected_pixels"], after_code["selected_pixels"]]}
            assert not failures, f"browser gesture errors at DPR {dpr}: {failures}"
            results[f"dpr{dpr}"] = {
                "renderer": webgpu_evidence(page), "display": display, "initial": initial, "pan_background": panned, "pan_over_node": over_node,
                "ctrl_wheel_ratio": ctrl_ratio, "pinch_ratio": pinch_ratio,
                "safari_format_gesture": {"ratio": safari_ratio, "requested_scale": 1.12,
                                          "platform": "synthetic gesturestart/change/end events in Chromium"},
                "initial_frozen": initial_frozen, "manual_frozen": manual_frozen,
                "initial_node_view": "SSA", "disasm_mode": disasm,
                "ssa_mode": ssa, "ssa_fit": ssa_fit, "edge_zoom_screenshots": zoom_screenshots,
                "fit_while_wheel_pending": True, "fit_keys": fit_keys,
                "fresh_wheel_after_fit": fresh_wheel_after_fit, "focused_fit_keys": focused_fit_keys,
                "neighboring_panes": neighboring,
            }
            context.close()
        finally:
            browser.close()
    return results


def run(context, dpr):
    return graph_interactions(context.playwright.chromium, context.args.browser, context.url, context.output, dprs=(dpr,))
