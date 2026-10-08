#!/usr/bin/env python3
"""Exercise the packaged WASM app against its real same-origin analysis server."""

import argparse
import base64
import hashlib
import json
import math
from pathlib import Path
import re
import selectors
import subprocess
import time

from playwright.sync_api import expect, sync_playwright


def server_url(process: subprocess.Popen[str]) -> str:
    """Read the bound :0 address instead of racing another ephemeral-port user."""
    deadline = time.monotonic() + 30
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        while time.monotonic() < deadline:
            if not selector.select(timeout=1):
                if process.poll() is not None:
                    raise RuntimeError(f"server exited before listening: {process.returncode}")
                continue
            line = process.stdout.readline()
            print(line, end="", flush=True)
            match = re.search(r"http://127\.0\.0\.1:\d+", line)
            if match:
                return match.group(0)
    raise RuntimeError("server did not report its bound loopback URL")


def rendered_frame(page) -> None:
    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")


def submit_bytecode(page, canvas, bytecode: str, evidence: Path):
    # At this fixed viewport the compact egui bytecode editor sits below the
    # toolbar. Use real pointer/keyboard input so serialization is tested too.
    canvas.click(position={"x": 300, "y": 60})
    rendered_frame(page)
    page.keyboard.press("Control+A")
    rendered_frame(page)
    focus = page.evaluate("document.activeElement.tagName")
    print(f"Typing {bytecode!r} with browser focus on {focus}", flush=True)
    # Send normal key events: egui can receive them through either its canvas
    # or its hidden IME input. insert_text emits only an input event and is
    # ineffective when Chromium has retained canvas focus.
    page.keyboard.type(bytecode, delay=5)
    rendered_frame(page)
    canvas.screenshot(path=str(evidence))
    with page.expect_response(lambda response: response.url.endswith("/api/analyze")) as response:
        page.keyboard.press("Control+Enter")
    reply = response.value
    posted = reply.request.post_data_json["bytecode"]
    assert posted == bytecode, f"editor input did not reach the typed request: expected {bytecode!r}, got {posted!r}"
    return reply



def settle_gesture(page) -> None:
    # egui deliberately smooths wheel movement across several animation frames.
    page.wait_for_timeout(400)
    rendered_frame(page)


def screenshot_metrics(page, screenshot: bytes, region: tuple[float, float, float, float]):
    """Measure the rendered selected node, independently of application state.

    Chromium decodes its own screenshot into an unattached 2D canvas. This is
    pixel analysis of the artifact, not a test hook in the application DOM.
    Bounds are returned in CSS pixels even for a device_scale_factor of two.
    """
    return page.evaluate(
        """async ({png, region}) => {
            const image = new Image();
            image.src = `data:image/png;base64,${png}`;
            await image.decode();
            const canvas = document.createElement('canvas');
            canvas.width = image.width; canvas.height = image.height;
            const context = canvas.getContext('2d');
            context.drawImage(image, 0, 0);
            const scale = image.width / innerWidth;
            const [left, top, right, bottom] = region.map(v => Math.round(v * scale));
            const width = right - left, height = bottom - top;
            const pixels = context.getImageData(left, top, width, height).data;
            let minX = width, minY = height, maxX = -1, maxY = -1, count = 0;
            for (let y = 0; y < height; y++) {
                for (let x = 0; x < width; x++) {
                    const i = (y * width + x) * 4;
                    // The selected node's solid fill from the semantic palette.
                    if (pixels[i] === 33 && pixels[i + 1] === 65 && pixels[i + 2] === 75) {
                        minX = Math.min(minX, x); minY = Math.min(minY, y);
                        maxX = Math.max(maxX, x); maxY = Math.max(maxY, y); count++;
                    }
                }
            }
            const digest = await crypto.subtle.digest('SHA-256', pixels);
            const hash = Array.from(new Uint8Array(digest), n => n.toString(16).padStart(2, '0')).join('');
            return {
                bounds: count ? [(left + minX) / scale, (top + minY) / scale,
                                 (left + maxX + 1) / scale, (top + maxY + 1) / scale] : null,
                selected_pixels: count, hash, scale
            };
        }""",
        {"png": base64.b64encode(screenshot).decode(), "region": region},
    )


def node_snapshot(page, canvas, output: Path, name: str, region=(4, 126, 1436, 970)):
    screenshot = canvas.screenshot(path=str(output / f"{name}.png"))
    metrics = screenshot_metrics(page, screenshot, region)
    assert metrics["bounds"] and metrics["selected_pixels"] >= 20, f"selected CFG node missing: {name}: {metrics}"
    return metrics


def assert_translation(before, after, dx: float, dy: float) -> None:
    initial, final = before["bounds"], after["bounds"]
    expected = [dx, dy, dx, dy]
    for old, new, delta in zip(initial, final, expected, strict=True):
        assert abs(new - old - delta) <= 3.0, f"wheel must pan without resizing nodes: {initial} -> {final}, expected {expected}"


def assert_anchored_zoom(before, after, pointer) -> float:
    initial, final = before["bounds"], after["bounds"]
    ratio = (final[2] - final[0]) / (initial[2] - initial[0])
    assert ratio > 1.05, f"zoom gesture did not enlarge the selected node: {initial} -> {final}"
    for axis in [0, 1]:
        expected = pointer[axis] + (initial[axis] - pointer[axis]) * ratio
        assert abs(final[axis] - expected) <= 4.0, f"zoom moved the world point under the pointer: {initial} -> {final}, anchor {pointer}, ratio {ratio}"
    return ratio


def pinch_wheel(page, cdp, pointer, delta_y: float) -> None:
    page.mouse.move(*pointer)
    rendered_frame(page)
    # Browser touchpad pinch arrives as Ctrl+wheel without a physical Ctrl
    # keydown. eframe distinguishes this from keyboard Ctrl+wheel.
    cdp.send("Input.dispatchMouseEvent", {
        "type": "mouseWheel", "x": pointer[0], "y": pointer[1],
        "deltaX": 0, "deltaY": delta_y, "modifiers": 2,
    })
    settle_gesture(page)


def launch_browser(browser_type, executable: str, dpr: int = 1):
    return browser_type.launch(
        executable_path=executable, headless=True,
        args=["--no-sandbox", "--use-gl=angle", "--use-angle=swiftshader",
              "--enable-unsafe-swiftshader", f"--force-device-scale-factor={dpr}"],
    )


def graph_interactions(browser_type, executable: str, url: str, output: Path):
    results = {}
    for dpr in [1, 2]:
        # CDP device_scale_factor alone changes devicePixelRatio but Chromium's
        # devicePixelContentBoxSize remains at the physical browser scale. egui
        # trusts that observer, so launch with the matching real scale as well.
        browser = launch_browser(browser_type, executable, dpr)
        context = browser.new_context(
            viewport={"width": 1440, "height": 1000}, device_scale_factor=dpr,
        )
        page = context.new_page()
        failures = []
        page.on("pageerror", lambda error: failures.append(str(error)))
        page.on("console", lambda message: failures.append(message.text)
                if message.type == "error" else None)
        page.goto(url, wait_until="networkidle")
        status = page.locator("#analysis-status")
        expect(status).to_contain_text("Ready:", timeout=60000)
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
        page.keyboard.press("2")
        rendered_frame(page)
        cdp = context.new_cdp_session(page)
        prefix = f"gestures-dpr{dpr}"
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

        # Toolbar representation choices are real canvas controls. The live
        # region describes their user-facing meaning; screenshots retain proof
        # that the corresponding node contents actually repaint.
        canvas.click(position={"x": 78, "y": 110})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: SSA")
        ssa = node_snapshot(page, canvas, output, f"{prefix}-nodes-ssa")
        assert ssa["hash"] != safari_zoom["hash"], "SSA mode retained disassembly pixels"
        ssa_width = ssa["bounds"][2] - ssa["bounds"][0]
        disasm_width = safari_zoom["bounds"][2] - safari_zoom["bounds"][0]
        assert ssa_width > disasm_width + 10, "SSA definitions/effects did not expand the fixture's rendered node"
        canvas.click(position={"x": 143, "y": 110})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: SSA")
        ssa_fit = node_snapshot(page, canvas, output, f"{prefix}-nodes-ssa-fit")
        assert ssa_fit["bounds"][2] - ssa_fit["bounds"][0] < ssa_width, "Fit did not return the enlarged SSA graph to a readable overview"
        canvas.click(position={"x": 28, "y": 110})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: Disasm")
        zoom_screenshots = {}
        for layout_name, height in [("", 1000), ("-horizontal", 390)]:
            page.set_viewport_size({"width": 1440, "height": height})
            page.wait_for_function("() => { const c = document.querySelector('canvas'); return c.width === innerWidth * devicePixelRatio && c.height === innerHeight * devicePixelRatio; }")
            canvas.click(position={"x": 143, "y": 110})
            settle_gesture(page)
            region = (4, 126, 1436, height - 30)
            fitted = node_snapshot(page, canvas, output, f"{prefix}-edges{layout_name}-100", region)
            zoom_screenshots[f"{layout_name or 'vertical'}-100"] = fitted
            previous = 1.0
            for scale in [0.75, 0.40, 0.36, 0.16]:
                pinch_wheel(page, cdp, (720, height * 0.65), -math.log(scale / previous) / 0.01)
                page.mouse.move(30, height - 70)
                rendered_frame(page)
                name = str(round(scale * 100))
                measured = node_snapshot(page, canvas, output, f"{prefix}-edges{layout_name}-{name}", region)
                fitted_width = fitted["bounds"][2] - fitted["bounds"][0]
                actual_width = measured["bounds"][2] - measured["bounds"][0]
                assert abs(actual_width / fitted_width - scale) < 0.05, f"incorrect screenshot zoom at DPR {dpr}: {scale}: {measured}"
                zoom_screenshots[f"{layout_name or 'vertical'}-{name}"] = measured
                previous = scale

        neighboring = {}
        if dpr == 1:
            page.set_viewport_size({"width": 1440, "height": 1000})
            settle_gesture(page)
            # Enough distinct PCs/SSA definitions to make both code panes scroll.
            reply = submit_bytecode(page, canvas, "6001" * 70 + "00", output / "adjacent-scroll-input.png")
            assert reply.status == 200
            expect(status).to_contain_text("Ready:")
            canvas.click(position={"x": 720, "y": 700})
            page.keyboard.press("0")
            settle_gesture(page)
            # This long input wraps into two editor lines. Exclude the graph
            # toolbar's selected Disasm button as well as neighboring panes.
            graph_region = (400, 155, 950, 970)
            for name, pointer, region in [
                ("disassembly", (120, 400), (4, 100, 380, 900)),
                ("ssa", (1100, 400), (975, 100, 1436, 900)),
            ]:
                page.mouse.move(*pointer)
                rendered_frame(page)
                before_png = canvas.screenshot()
                before_node = screenshot_metrics(page, before_png, graph_region)
                before_code = screenshot_metrics(page, before_png, region)
                assert before_node["bounds"], f"missing node before {name} scroll"
                page.mouse.wheel(0, 220)
                settle_gesture(page)
                after_png = canvas.screenshot(path=str(output / f"adjacent-scroll-{name}.png"))
                after_node = screenshot_metrics(page, after_png, graph_region)
                after_code = screenshot_metrics(page, after_png, region)
                assert_translation(before_node, after_node, 0, 0)
                assert before_code["hash"] != after_code["hash"], f"{name} pane did not scroll"
                assert before_code["selected_pixels"] > 100, f"selected {name} block header was not initially visible"
                assert after_code["selected_pixels"] < before_code["selected_pixels"] / 10, f"selected {name} header did not scroll out of view"
                neighboring[name] = {"graph_before": before_node["bounds"], "graph_after": after_node["bounds"], "code_changed": True, "selected_header_pixels": [before_code["selected_pixels"], after_code["selected_pixels"]]}
        assert not failures, f"browser gesture errors at DPR {dpr}: {failures}"
        results[f"dpr{dpr}"] = {
            "display": display, "initial": initial, "pan_background": panned, "pan_over_node": over_node,
            "ctrl_wheel_ratio": ctrl_ratio, "pinch_ratio": pinch_ratio,
            "safari_format_gesture": {"ratio": safari_ratio, "requested_scale": 1.12,
                                      "platform": "synthetic gesturestart/change/end events in Chromium"},
            "ssa_mode": ssa, "ssa_fit": ssa_fit, "edge_zoom_screenshots": zoom_screenshots,
            "neighboring_panes": neighboring,
        }
        context.close()
        browser.close()
    return results

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", required=True)
    parser.add_argument("--assets", required=True)
    parser.add_argument("--browser", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    process = subprocess.Popen(
        [args.server, "--assets", args.assets, "--bind", "127.0.0.1:0"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    try:
        url = server_url(process)
        with sync_playwright() as playwright:
            browser = launch_browser(playwright.chromium, args.browser)
            page = browser.new_page(viewport={"width": 1440, "height": 1000}, color_scheme="light")
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.on(
                "console",
                lambda message: errors.append(message.text)
                if message.type == "error"
                and not (message.location.get("url", "").endswith("/api/analyze") and "400" in message.text)
                else None,
            )
            with page.expect_response(lambda response: response.url.endswith("/api/analyze")) as response:
                page.goto(url, wait_until="networkidle")
            reply = response.value
            assert reply.status == 200, f"initial analysis failed: {reply.status}"
            result = reply.json()
            analysis = result["result"]["Ok"]
            assert analysis["schema_version"] == 1
            assert sum(len(block["instructions"]) for block in analysis["disassembly"]) > 0
            assert len(analysis["cfg"]) > 1, "example did not produce a graph"
            assert {"BranchTrue", "BranchFalse"}.issubset({edge["kind"] for edge in analysis["edges"]})
            assert analysis["ssa"]["blocks"], "example did not produce SSA"
            (args.output / "analysis.json").write_text(json.dumps(result, indent=2) + "\n")
            status = page.locator("#analysis-status")
            expect(status).to_contain_text(re.compile(r"ready", re.IGNORECASE), timeout=60000)
            canvas = page.locator("#evm-canvas")
            expect(canvas).to_be_visible()
            assert canvas.evaluate("canvas => canvas.width > 0 && canvas.height > 0")
            screenshots = {}
            for key, name in [("0", "workspace"), ("1", "disassembly"), ("2", "cfg"), ("3", "ssa")]:
                page.keyboard.press(key)
                # The second animation frame is after egui has consumed the key.
                rendered_frame(page)
                screenshot = canvas.screenshot(path=str(args.output / f"{name}.png"))
                assert len(screenshot) > 10000, f"{name} did not paint a substantial canvas"
                screenshots[name] = hashlib.sha256(screenshot).hexdigest()
            assert len(set(screenshots.values())) == 4, "view shortcuts did not repaint distinct custom views"
            # Resize the same live analysis through desktop, split/tab and
            # portrait/short-window layouts. No reload or new analysis may be
            # needed to keep all views usable after resizing.
            resize_requests = []
            page.on("request", lambda request: resize_requests.append(request.url)
                    if request.url.endswith("/api/analyze") else None)
            viewports = [(1920, 1080), (1440, 900), (1024, 768), (768, 600),
                         (390, 844), (844, 390), (320, 480), (1440, 1000)]
            viewport_screenshots = {}
            for width, height in viewports:
                page.set_viewport_size({"width": width, "height": height})
                page.wait_for_function(
                    "([w,h]) => { const c = document.querySelector('#evm-canvas'); "
                    "return c.width === Math.round(w * devicePixelRatio) && "
                    "c.height === Math.round(h * devicePixelRatio); }", arg=[width, height])
                for key, name in [("0", "workspace"), ("1", "disassembly"),
                                  ("2", "cfg"), ("3", "ssa")]:
                    page.keyboard.press(key)
                    rendered_frame(page)
                    screenshot = canvas.screenshot(path=str(args.output / f"resize-{width}x{height}-{name}.png"))
                    assert len(screenshot) > 4000, f"blank {width}x{height} {name} view"
                    viewport_screenshots[f"{width}x{height}-{name}"] = hashlib.sha256(screenshot).hexdigest()
                expect(status).to_contain_text("Converged")
                assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
                assert page.evaluate("document.documentElement.scrollHeight <= innerHeight")
            assert not resize_requests, "layout resizing unexpectedly reran analysis"
            page.keyboard.press("0")
            rendered_frame(page)
            invalid = submit_bytecode(page, canvas, "this is not bytecode", args.output / "invalid-input.png")
            assert invalid.status == 400
            assert invalid.json()["result"]["Err"]["code"] == "InvalidBytecode"
            expect(status).to_contain_text("Error: InvalidBytecode")
            canvas.screenshot(path=str(args.output / "invalid-bytecode.png"))

            arithmetic = submit_bytecode(page, canvas, "600160020100", args.output / "arithmetic-input.png")
            assert arithmetic.status == 200
            assert arithmetic.json()["result"]["Ok"]["status"] == "Converged"
            expect(status).to_contain_text("Ready:")
            expect(status).to_contain_text("Converged")

            partial = submit_bytecode(page, canvas, "5f5f5f5f5f5f355af100", args.output / "unknown-call-input.png")
            assert partial.status == 200
            partial_reply = partial.json()
            incomplete = partial_reply["result"]["Ok"]
            assert incomplete["status"] == "Incomplete"
            assert any(frontier["kind"] == "UnknownTarget" for frontier in incomplete["frontiers"])
            assert not incomplete["ssa"]["complete"]
            expect(status).to_contain_text("Incomplete")
            expect(status).to_contain_text("SSA partial")
            (args.output / "incomplete.json").write_text(json.dumps(partial_reply, indent=2) + "\n")
            canvas.screenshot(path=str(args.output / "incomplete.png"))
            # Native response rectangles can exist outside a clipped editor.
            # Prove real input still reaches the backend in narrow and short
            # windows, after the resize-only checks above.
            responsive_input = {}
            for width, height in [(390, 844), (844, 390), (320, 480)]:
                page.set_viewport_size({"width": width, "height": height})
                rendered_frame(page)
                reply = submit_bytecode(page, canvas, "600160020100",
                                        args.output / f"editor-{width}x{height}.png")
                assert reply.status == 200
                expect(status).to_contain_text("Converged")
                responsive_input[f"{width}x{height}"] = reply.request.post_data_json["bytecode"]
            gestures = graph_interactions(playwright.chromium, args.browser, url, args.output)
            assert not errors, "browser errors: " + "\n".join(errors)
            report = {
                "browser": browser.version,
                "assets": args.assets,
                "status": status.text_content(),
                "cases": {
                    "branch": analysis["status"],
                    "invalid_bytecode": invalid.json()["result"]["Err"]["code"],
                    "arithmetic": arithmetic.json()["result"]["Ok"]["status"],
                    "unknown_call": incomplete["status"],
                },
                "screenshots": screenshots,
                "viewport_screenshots": viewport_screenshots,
                "responsive_input": responsive_input,
                "graph_interactions": gestures,
                "platform": "Linux headless Chromium; synthesized browser wheel events, not physical macOS hardware",
                "browser_errors": errors,
            }
            (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
            browser.close()
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


if __name__ == "__main__":
    main()
