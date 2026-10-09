#!/usr/bin/env python3
"""Exercise the packaged WASM app against its real same-origin analysis server."""

import argparse
import base64
from contextlib import contextmanager
import hashlib
import json
import math
from pathlib import Path
import re
import selectors
from statistics import median
import subprocess
import sys
from tempfile import TemporaryDirectory
import time

from playwright.sync_api import expect, sync_playwright
sys.dont_write_bytecode = True
from web_test_rpc import RpcFixture


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


@contextmanager
def analysis_server(args, rpc_config: Path | str | None = None):
    command = [args.server, "--assets", args.assets, "--bind", "127.0.0.1:0",
               "--workers", "2", "--queue-capacity", "4"]
    if rpc_config is not None:
        command.extend(["--rpc-config", str(rpc_config)])
    process = subprocess.Popen(command, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, bufsize=1)
    try:
        yield server_url(process)
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def rendered_frame(page) -> None:
    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")


class AnalysisResponse:
    """Final typed envelope plus the actual originating form submission."""
    def __init__(self, response, envelope, status):
        self.request = response.request
        self.status = status
        self.admission_status = response.status
        self.envelope = envelope

    def json(self):
        return self.envelope


def complete_submission(page, admitted):
    reply = admitted.json()["result"]
    if "Err" in reply:
        return AnalysisResponse(admitted, {"result": reply}, admitted.status)
    job = reply["Ok"]
    observed_status = admitted.status
    task_url = admitted.url + "/" + str(job["id"])
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        state = job["state"]
        if state == "Completed":
            result = page.request.get(task_url + "/result")
            expect(page.locator("#analysis-status")).to_contain_text("Ready:", timeout=60000)
            return AnalysisResponse(admitted, result.json(), result.status)
        if isinstance(state, dict) and "Failed" in state:
            error = state["Failed"]
            expect(page.locator("#analysis-status")).to_contain_text("Error:", timeout=60000)
            return AnalysisResponse(admitted, {"result": {"Err": error}}, observed_status)
        assert state not in ["Cancelling", "Cancelled"], f"unexpected cancellation: {job}"
        page.wait_for_timeout(100)
        result = page.request.get(task_url)
        assert result.ok, result.text()
        observed_status = result.status
        job = result.json()["result"]["Ok"]
    raise AssertionError(f"task did not reach a terminal state: {job}")


def open_input(page):
    # The product shortcut opens the modal and focuses its initial source field.
    page.keyboard.press("Control+Enter")
    settle_gesture(page)


def posted_bytecode(reply):
    return reply.request.post_data_json["input"]["Bytecode"]["bytecode"]


def submit_bytecode(page, canvas, bytecode: str, evidence: Path):
    open_input(page)
    page.keyboard.press("Control+A")
    rendered_frame(page)
    page.keyboard.type(bytecode, delay=5)
    rendered_frame(page)
    canvas.screenshot(path=str(evidence))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
        page.keyboard.press("Control+Enter")
    admitted = response.value
    posted = posted_bytecode(admitted)
    assert posted == bytecode, f"editor input did not reach the typed request: expected {bytecode!r}, got {posted!r}"
    return complete_submission(page, admitted)


def budget_interactions(browser, url: str, output: Path):
    """Real decimal editing preserves u64 values through WASM, HTTP and metadata."""
    defaults = {
        "max_states": 100_000,
        "max_transfers": 10_000_000,
        "max_work": 1_000_000_000_000,
        "max_call_depth": 1025,
        "max_memory_bytes": 64 * 1024 * 1024,
        "context_depth": 128,
        "max_constants": 512,
        "reduction_rounds": 16,
        "max_facts": 4096,
        "max_constraints": 2048,
        "max_expression_nodes": 16_384,
        "max_expression_depth": 256,
        "smt_rlimit": 10_000_000,
        "rpc_max_accounts": 4096,
        "rpc_max_requests": 1_000_000,
        "rpc_max_response_bytes": 64 * 1024 * 1024,
        "rpc_timeout_ms": 120_000,
    }
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    initial_reply = complete_submission(page, initial.value)
    assert initial_reply.status == 200, initial_reply.json()
    initial_request = json.loads(initial_reply.request.post_data)
    initial_report = initial_reply.json()["result"]["Ok"]
    assert initial_report["schema_version"] == 5
    for field, value in defaults.items():
        assert initial_request["limits"][field] == value, (field, initial_request["limits"])
        assert initial_report["metadata"]["limits"][field] == value

    canvas = page.locator("#evm-canvas")
    open_input(page)
    page.keyboard.press("Control+A")
    page.keyboard.type("600160020100", delay=5)
    rendered_frame(page)
    canvas.click(position={"x": 419, "y": 559})  # Execution budget
    settle_gesture(page)
    # Edit actual single-line controls. Values above 2^53 would be rounded
    # by DragValue/f64 or JavaScript Number, so compare raw HTTP with Python ints.
    edited = {"max_work": 9_007_199_254_740_993,
              "max_transfers": 9_007_199_254_740_995,
              "max_states": 500_000}
    for field, y in [("max_work", 559), ("max_transfers", 536), ("max_states", 513)]:
        canvas.click(position={"x": 510, "y": y})
        page.keyboard.press("Control+A")
        rendered_frame(page)
        page.keyboard.type(str(edited[field]), delay=5)
        rendered_frame(page)
    canvas.screenshot(path=str(output / "large-budget-input.png"))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as submitted:
        page.keyboard.press("Control+Enter")
    reply = complete_submission(page, submitted.value)
    assert reply.admission_status == 202, reply.json()
    assert reply.status == 200, reply.json()
    request = json.loads(reply.request.post_data)
    report = reply.json()["result"]["Ok"]
    assert request["input"]["Bytecode"]["bytecode"] == "600160020100"
    assert report["schema_version"] == 5
    assert report["status"] == "Converged", report["frontiers"]
    expected = {**defaults, **edited}
    for field, value in expected.items():
        assert request["limits"][field] == value, (field, request["limits"])
        assert report["metadata"]["limits"][field] == value, (field, report["metadata"])
    assert not errors, errors
    canvas.screenshot(path=str(output / "large-budget-result.png"))
    (output / "large-budget-request.json").write_text(reply.request.post_data + "\n")
    (output / "large-budget-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    result = {"defaults": defaults, "edited": edited, "status": report["status"],
              "metadata_preserves_exact_integers": True,
              "renderer": webgpu_evidence(page)}
    page.close()
    return result



def settle_gesture(page) -> None:
    # egui deliberately smooths wheel movement across several animation frames.
    page.wait_for_timeout(400)
    rendered_frame(page)


def screenshot_metrics(page, screenshot: bytes, region: tuple[float, float, float, float], measure_nodes: bool = False):
    """Measure the rendered selected node, independently of application state.

    Chromium decodes its own screenshot into an unattached 2D canvas. This is
    pixel analysis of the artifact, not a test hook in the application DOM.
    Bounds are returned in CSS pixels even for a device_scale_factor of two.
    """
    return page.evaluate(
        """async ({png, region, measureNodes}) => {
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
            const dividerCounts = new Uint32Array(width);
            const mask = measureNodes ? new Uint8Array(width * height) : null;
            for (let y = 0; y < height; y++) {
                for (let x = 0; x < width; x++) {
                    const i = (y * width + x) * 4;
                    if (Math.abs(pixels[i] - 145) <= 50 && Math.abs(pixels[i + 1] - 164) <= 50 && Math.abs(pixels[i + 2] - 186) <= 50) dividerCounts[x]++;
                    if (mask && ((pixels[i] === 21 && pixels[i + 1] === 29 && pixels[i + 2] === 42) ||
                                 (pixels[i] === 33 && pixels[i + 1] === 65 && pixels[i + 2] === 75) ||
                                 (pixels[i] === 47 && pixels[i + 1] === 62 && pixels[i + 2] === 80))) mask[y * width + x] = 1;
                    // The selected node's solid fill from the semantic palette.
                    if (pixels[i] === 33 && pixels[i + 1] === 65 && pixels[i + 2] === 75) {
                        minX = Math.min(minX, x); minY = Math.min(minY, y);
                        maxX = Math.max(maxX, x); maxY = Math.max(maxY, y); count++;
                    }
                }
            }
            // Full-height idle dividers are distinguishable from text, node
            // borders and scrollbar thumbs. Collapse HiDPI columns to one line.
            const dividers = [];
            for (let x = 0; x < width;) {
                if (dividerCounts[x] < height * 0.85) { x++; continue; }
                const start = x;
                while (x < width && dividerCounts[x] >= height * 0.85) x++;
                dividers.push((left + (start + x) / 2) / scale);
            }
            const nodes = [];
            if (mask) {
                const queue = new Int32Array(width * height);
                for (let start = 0; start < mask.length; start++) {
                    if (!mask[start]) continue;
                    let read = 0, write = 1, x0 = width, y0 = height, x1 = 0, y1 = 0;
                    queue[0] = start; mask[start] = 0;
                    while (read < write) {
                        const at = queue[read++], x = at % width, y = Math.floor(at / width);
                        x0 = Math.min(x0, x); y0 = Math.min(y0, y);
                        x1 = Math.max(x1, x); y1 = Math.max(y1, y);
                        for (const next of [x > 0 ? at - 1 : -1, x + 1 < width ? at + 1 : -1,
                                            y > 0 ? at - width : -1, y + 1 < height ? at + width : -1]) {
                            if (next >= 0 && mask[next]) { mask[next] = 0; queue[write++] = next; }
                        }
                    }
                    if (write >= 30 * scale * scale && x1 - x0 >= 10 * scale && y1 - y0 >= 8 * scale) {
                        nodes.push([(left + x0) / scale, (top + y0) / scale,
                                    (left + x1 + 1) / scale, (top + y1 + 1) / scale]);
                    }
                }
                nodes.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
            }
            const digest = await crypto.subtle.digest('SHA-256', pixels);
            const hash = Array.from(new Uint8Array(digest), n => n.toString(16).padStart(2, '0')).join('');
            return {
                bounds: count ? [(left + minX) / scale, (top + minY) / scale,
                                 (left + maxX + 1) / scale, (top + maxY + 1) / scale] : null,
                selected_pixels: count, hash, scale, dividers, nodes
            };
        }""",
        {"png": base64.b64encode(screenshot).decode(), "region": region, "measureNodes": measure_nodes},
    )


def node_snapshot(page, canvas, output: Path, name: str, region=(4, 104, 1436, 970), measure_nodes=False):
    screenshot = canvas.screenshot(path=str(output / f"{name}.png"))
    metrics = screenshot_metrics(page, screenshot, region, measure_nodes)
    assert metrics["bounds"] and metrics["selected_pixels"] >= 20, f"selected CFG node missing: {name}: {metrics}"
    return metrics


def assert_translation(before, after, dx: float, dy: float, tolerance: float = 3.0) -> None:
    initial, final = before["bounds"], after["bounds"]
    expected = [dx, dy, dx, dy]
    for old, new, delta in zip(initial, final, expected, strict=True):
        assert abs(new - old - delta) <= tolerance, f"wheel must pan without resizing nodes: {initial} -> {final}, expected {expected}"


def assert_anchored_zoom(before, after, pointer) -> float:
    initial, final = before["bounds"], after["bounds"]
    ratio = (final[2] - final[0]) / (initial[2] - initial[0])
    assert ratio > 1.05, f"zoom gesture did not enlarge the selected node: {initial} -> {final}"
    for axis in [0, 1]:
        expected = pointer[axis] + (initial[axis] - pointer[axis]) * ratio
        assert abs(final[axis] - expected) <= 4.0, f"zoom moved the world point under the pointer: {initial} -> {final}, anchor {pointer}, ratio {ratio}"
    return ratio


def resize_view(page, width: int, height: int) -> None:
    page.set_viewport_size({"width": width, "height": height})
    page.wait_for_function(
        "() => { const c = document.querySelector('#evm-canvas'); return c.width === innerWidth * devicePixelRatio && c.height === innerHeight * devicePixelRatio; }")
    rendered_frame(page)


def assert_same_scene(before, after) -> None:
    """Fit may uniformly scale/translate nodes, but must not rearrange them."""
    # The default jump history separates the fixture's two merge states.
    assert len(before["nodes"]) == len(after["nodes"]) == 5, (before["nodes"], after["nodes"])
    pairs = list(zip(before["nodes"], after["nodes"], strict=True))
    # Node outlines retain screen-space width, so shrinking fill rectangles
    # underestimates zoom. Relative distances between all five centers avoid
    # that border bias and still reject any rearrangement of the scene.
    centers = [([(old[0] + old[2]) / 2, (old[1] + old[3]) / 2],
                [(new[0] + new[2]) / 2, (new[1] + new[3]) / 2])
               for old, new in pairs]
    scale = median(math.dist(centers[a][1], centers[b][1]) /
                   math.dist(centers[a][0], centers[b][0])
                   for a in range(len(centers)) for b in range(a + 1, len(centers)))
    assert scale > 0
    translation = [median((new[axis] + new[axis + 2]) / 2
                          - (old[axis] + old[axis + 2]) / 2 * scale
                          for old, new in pairs) for axis in [0, 1]]
    for original, fitted in pairs:
        for axis in [0, 1]:
            original_center = (original[axis] + original[axis + 2]) / 2
            fitted_center = (fitted[axis] + fitted[axis + 2]) / 2
            expected = translation[axis] + original_center * scale
            assert abs(fitted_center - expected) <= 4.0, f"Fit rearranged the scene: {before['nodes']} -> {after['nodes']}"
            original_size = original[axis + 2] - original[axis]
            fitted_size = fitted[axis + 2] - fitted[axis]
            assert abs(fitted_size - original_size * scale) <= 3.0, "Fit resized a node independently of the scene"


def frozen_camera(page, canvas, output: Path, prefix: str, inspect_scene: bool):
    page.mouse.move(2, 2)
    rendered_frame(page)
    baseline = node_snapshot(page, canvas, output, f"{prefix}-before", measure_nodes=inspect_scene)
    if inspect_scene:
        assert len(baseline["nodes"]) == 5, baseline["nodes"]
    enlarged = {}
    for width, height in [(1920, 1100), (1440, 1200)]:
        resize_view(page, width, height)
        current = node_snapshot(page, canvas, output, f"{prefix}-{width}x{height}",
                                (4, 126, width - 4, height - 30), inspect_scene)
        # Same canvas origin: even the immediate enlarged frame must preserve
        # the camera. Merely returning to the old size could hide auto-recentering.
        assert_translation(baseline, current, 0, 0, tolerance=1.0)
        if inspect_scene:
            assert_same_scene(baseline, current)
        enlarged[f"{width}x{height}"] = current["bounds"]
    resize_view(page, 500, 380)
    canvas.screenshot(path=str(output / f"{prefix}-clipped.png"))
    resize_view(page, 1440, 1000)
    for key in ["1", "3", "0", "2"]:
        page.keyboard.press(key)
        rendered_frame(page)
    returned = node_snapshot(page, canvas, output, f"{prefix}-returned", measure_nodes=inspect_scene)
    assert_translation(baseline, returned, 0, 0, tolerance=1.0)
    if inspect_scene:
        assert_same_scene(baseline, returned)
    return {"before": baseline["bounds"], "enlarged": enlarged, "returned": returned["bounds"]}


def workspace_panes(page, canvas, output: Path, name: str):
    page.mouse.move(2, 2)
    rendered_frame(page)
    viewport = page.viewport_size
    width, height = viewport["width"], viewport["height"]
    png = canvas.screenshot(path=str(output / f"{name}.png"))
    metrics = screenshot_metrics(page, png, (4, 160, width - 4, height - 30))
    assert len(metrics["dividers"]) == 2, f"expected two visible workspace dividers: {metrics['dividers']}"
    left, right = metrics["dividers"]
    return {"left": left, "right": right, "disassembly_width": left - 6.5,
            "ssa_width": width - right - 6.5, "graph_width": right - left - 5}, png


def hide_inspector(page, canvas):
    # Native geometry tests cover this right-aligned header control; the action
    # is exercised with an actual pointer and leaves more canvas for gestures.
    canvas.click(position={"x": page.viewport_size["width"] - 124, "y": 12})
    settle_gesture(page)


def pane_interactions(browser, url: str, output: Path):
    page = browser.new_page(viewport={"width": 1920, "height": 1000})
    observe_webgpu(page)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("console", lambda message: errors.append(message.text) if message.type == "error" else None)
    page.goto(url, wait_until="networkidle")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    hide_inspector(page, canvas)
    for bytecode, name in [("600160020100", "short"), ("7f" + "ff" * 32 + "00", "long")]:
        reply = submit_bytecode(page, canvas, bytecode, output / f"panes-{name}-input.png")
        assert reply.status == 200
        expect(status).to_contain_text("Converged")
        canvas.click(position={"x": 20, "y": 700})
        page.keyboard.press("0")
        settle_gesture(page)
        resize_view(page, 2560, 1000)
        current, _ = workspace_panes(page, canvas, output, f"panes-{name}-2560")
        if name == "short":
            short = current
            resize_view(page, 1920, 1000)
            smaller, _ = workspace_panes(page, canvas, output, "panes-short-1920")
            for field in ["disassembly_width", "ssa_width"]:
                assert abs(smaller[field] - short[field]) <= 1, f"side pane grew proportionally: {smaller} -> {short}"
            assert abs(short["graph_width"] - smaller["graph_width"] - 640) <= 2
        else:
            long = current
            assert long["disassembly_width"] > short["disassembly_width"] + 150, (short, long)
            assert long["ssa_width"] > short["ssa_width"] + 150, (short, long)
    # Fit once so the selected node is visible before exercising divider changes.
    # No later resize or divider action is followed by Fit.
    page.keyboard.press("f")
    settle_gesture(page)
    before_panes, before_png = workspace_panes(page, canvas, output, "panes-before-divider")
    graph_region = (before_panes["left"] + 4, 160, before_panes["right"] - 4, 970)
    before = screenshot_metrics(page, before_png, graph_region)
    assert before["bounds"], before
    page.mouse.move(before_panes["left"], 600)
    page.mouse.down()
    page.mouse.move(before_panes["left"] + 48, 600, steps=6)
    page.mouse.up()
    rendered_frame(page)
    after_panes, after_png = workspace_panes(page, canvas, output, "panes-after-divider")
    shift = after_panes["left"] - before_panes["left"]
    assert shift > 30, (before_panes, after_panes)
    after = screenshot_metrics(page, after_png, (after_panes["left"] + 4, 160, after_panes["right"] - 4, 970))
    assert_translation(before, after, shift, 0, tolerance=1.0)
    resize_view(page, 1128, 700)
    canvas.screenshot(path=str(output / "panes-manual-clipped.png"))
    resize_view(page, 2560, 1000)
    restored_panes, restored_png = workspace_panes(page, canvas, output, "panes-manual-restored")
    for field in ["disassembly_width", "ssa_width"]:
        assert abs(after_panes[field] - restored_panes[field]) <= 1, (after_panes, restored_panes)
    restored = screenshot_metrics(page, restored_png, (restored_panes["left"] + 4, 160, restored_panes["right"] - 4, 970))
    assert_translation(after, restored, 0, 0, tolerance=1.0)
    assert not errors, f"browser pane errors: {errors}"
    result = {"short": short, "long": long, "manual": after_panes, "restored": restored_panes,
              "camera_before_divider": before["bounds"], "camera_after_divider": after["bounds"],
              "renderer": webgpu_evidence(page)}
    page.close()
    return result


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


def observe_webgpu(page, unavailable: str | None = None) -> None:
    """Observe the application's real browser API calls before WASM starts.

    Wrappers call the native implementations unchanged. The test never supplies
    a renderer or creates its own GPU device on the successful application path.
    """
    page.add_init_script("""(() => {
        const unavailable = UNAVAILABLE;
        const evidence = {contexts: [], adapter_requests: 0, adapters: [],
                          devices: 0, configurations: [], textures: 0,
                          submissions: 0, indexed_draws: 0, errors: []};
        window.__webgpuEvidence = evidence;
        const getContext = HTMLCanvasElement.prototype.getContext;
        HTMLCanvasElement.prototype.getContext = function(kind, ...args) {
            const result = getContext.call(this, kind, ...args);
            evidence.contexts.push({canvas: this.id, kind, succeeded: result !== null});
            return result;
        };
        if (typeof GPU !== 'undefined') {
            const requestAdapter = GPU.prototype.requestAdapter;
            GPU.prototype.requestAdapter = async function(...args) {
                evidence.adapter_requests++;
                if (unavailable === 'adapter-null') return null;
                const adapter = await requestAdapter.apply(this, args);
                if (adapter) {
                    const info = adapter.info;
                    evidence.adapters.push({vendor: info.vendor, architecture: info.architecture,
                                            device: info.device, description: info.description,
                                            fallback: info.isFallbackAdapter});
                }
                return adapter;
            };
            const requestDevice = GPUAdapter.prototype.requestDevice;
            GPUAdapter.prototype.requestDevice = async function(...args) {
                const device = await requestDevice.apply(this, args);
                evidence.devices++;
                device.addEventListener('uncapturederror', event => evidence.errors.push(event.error.message));
                return device;
            };
            const configure = GPUCanvasContext.prototype.configure;
            GPUCanvasContext.prototype.configure = function(config) {
                const result = configure.call(this, config);
                evidence.configurations.push({canvas: this.canvas.id, format: config.format,
                                              alpha_mode: config.alphaMode, usage: config.usage});
                return result;
            };
            const getCurrentTexture = GPUCanvasContext.prototype.getCurrentTexture;
            GPUCanvasContext.prototype.getCurrentTexture = function() {
                const texture = getCurrentTexture.call(this);
                if (this.canvas.id === 'evm-canvas') evidence.textures++;
                return texture;
            };
            const submit = GPUQueue.prototype.submit;
            GPUQueue.prototype.submit = function(...args) {
                const result = submit.apply(this, args);
                evidence.submissions++;
                return result;
            };
            const drawIndexed = GPURenderPassEncoder.prototype.drawIndexed;
            GPURenderPassEncoder.prototype.drawIndexed = function(...args) {
                const result = drawIndexed.apply(this, args);
                evidence.indexed_draws++;
                return result;
            };
        }
        if (unavailable === 'missing-api') {
            Object.defineProperty(navigator, 'gpu', {get: () => undefined});
        }
    })();""".replace("UNAVAILABLE", json.dumps(unavailable)))


def webgpu_evidence(page):
    evidence = page.evaluate("window.__webgpuEvidence")
    contexts = evidence["contexts"]
    assert any(item["canvas"] == "evm-canvas" and item["kind"] == "webgpu"
               and item["succeeded"] for item in contexts), f"workspace never acquired WebGPU: {evidence}"
    assert not any(item["kind"] in ["webgl", "webgl2", "experimental-webgl"]
                   for item in contexts), f"unexpected WebGL fallback: {evidence}"
    assert evidence["adapter_requests"] > 0 and evidence["adapters"], f"no real GPU adapter: {evidence}"
    assert evidence["devices"] > 0, f"no real GPU device: {evidence}"
    assert any(item["canvas"] == "evm-canvas" for item in evidence["configurations"]), f"WebGPU canvas was not configured: {evidence}"
    assert evidence["textures"] > 0 and evidence["submissions"] > 0 and evidence["indexed_draws"] > 0, f"WebGPU did not render a frame: {evidence}"
    assert not evidence["errors"], f"WebGPU validation errors: {evidence['errors']}"
    return evidence


def webgpu_unavailable(browser, url: str, output: Path):
    results = {}
    for failure in ["missing-api", "adapter-null"]:
        page = browser.new_page(viewport={"width": 840, "height": 540})
        observe_webgpu(page, failure)
        requests = []
        page.on("request", lambda request: requests.append(request.url)
                if request.url.endswith("/api/tasks") else None)
        page.goto(url, wait_until="networkidle")
        boot = page.locator("#boot")
        status = page.locator("#analysis-status")
        expect(boot).to_be_visible()
        expect(boot).to_have_attribute("role", "alert", timeout=60000)
        expect(boot).to_contain_text("Unable to start WebGPU")
        expect(status).to_contain_text("Error: Unable to start WebGPU")
        if failure == "adapter-null":
            expect(boot).to_contain_text("could not create a usable WebGPU graphics device")
            expect(boot).not_to_contain_text("not requested")
        assert not requests, "analysis started despite WebGPU initialization failure"
        evidence = page.evaluate("window.__webgpuEvidence")
        assert evidence["devices"] == 0
        assert not any(item["kind"] in ["webgl", "webgl2", "experimental-webgl"]
                       for item in evidence["contexts"]), f"failure attempted a WebGL fallback: {evidence}"
        page.screenshot(path=str(output / f"webgpu-{failure}.png"))
        results[failure] = {"status": status.text_content(), "renderer": evidence}
        page.close()
    return results


def launch_browser(browser_type, executable: str, dpr: int = 1):
    return browser_type.launch(
        executable_path=executable, headless=True,
        # WebGPU and Chromium's compositor share SwiftShader Vulkan. A virtual
        # X display is required even with headless=True for canvas presentation;
        # --disable-vulkan-surface would silently produce blank screenshots.
        args=["--no-sandbox", "--use-gl=angle", "--use-angle=swiftshader",
              "--enable-unsafe-swiftshader", "--enable-unsafe-webgpu",
              "--ignore-gpu-blocklist", "--enable-gpu", "--enable-features=Vulkan",
              "--use-vulkan=swiftshader", f"--force-device-scale-factor={dpr}"],
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
        observe_webgpu(page)
        failures = []
        page.on("pageerror", lambda error: failures.append(str(error)))
        page.on("console", lambda message: failures.append(message.text)
                if message.type == "error" else None)
        page.goto(url, wait_until="networkidle")
        status = page.locator("#analysis-status")
        expect(status).to_contain_text("Ready:", timeout=60000)
        expect(status).to_contain_text("CFG nodes: SSA")
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
        page.keyboard.press("2")
        rendered_frame(page)
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

        # Start in the product default SSA, then exercise both directions using
        # real toolbar controls. Status and measured node widths verify the
        # representation change independently of screenshot hashes.
        expect(status).to_contain_text("CFG nodes: SSA")
        canvas.click(position={"x": 28, "y": 86})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: Disasm")
        disasm = node_snapshot(page, canvas, output, f"{prefix}-nodes-disasm")
        disasm_width = disasm["bounds"][2] - disasm["bounds"][0]
        assert disasm["hash"] != safari_zoom["hash"], "Disasm mode retained SSA pixels"
        assert safari_zoom["bounds"][2] - safari_zoom["bounds"][0] > disasm_width + 10, "Disasm mode did not replace the fixture's SSA definitions/effects"
        canvas.click(position={"x": 78, "y": 86})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: SSA")
        ssa = node_snapshot(page, canvas, output, f"{prefix}-nodes-ssa")
        assert ssa["hash"] != disasm["hash"], "SSA mode retained disassembly pixels"
        ssa_width = ssa["bounds"][2] - ssa["bounds"][0]
        assert ssa_width > disasm_width + 10, "SSA definitions/effects did not expand the fixture's rendered node"
        canvas.click(position={"x": 143, "y": 86})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: SSA")
        ssa_fit = node_snapshot(page, canvas, output, f"{prefix}-nodes-ssa-fit")
        assert ssa_fit["bounds"][2] - ssa_fit["bounds"][0] < ssa_width, "Fit did not return the enlarged SSA graph to a readable overview"
        canvas.click(position={"x": 28, "y": 86})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: Disasm")
        zoom_screenshots = {}
        scene_reference = None
        for layout_name, height in [("tall", 1000), ("short", 390)]:
            page.set_viewport_size({"width": 1440, "height": height})
            page.wait_for_function("() => { const c = document.querySelector('canvas'); return c.width === innerWidth * devicePixelRatio && c.height === innerHeight * devicePixelRatio; }")
            canvas.click(position={"x": 143, "y": 86})
            settle_gesture(page)
            region = (4, 104, 1436, height - 30)
            fitted = node_snapshot(page, canvas, output, f"{prefix}-edges-{layout_name}-fit", region, True)
            if scene_reference is None:
                scene_reference = fitted
            else:
                assert_same_scene(scene_reference, fitted)
            zoom_screenshots[f"{layout_name}-fit"] = fitted
            previous = 1.0
            for scale in [0.75, 0.40, 0.36, 0.16]:
                pinch_wheel(page, cdp, (720, height * 0.65), -math.log(scale / previous) / 0.01)
                page.mouse.move(30, height - 70)
                rendered_frame(page)
                name = str(round(scale * 100))
                measured = node_snapshot(page, canvas, output, f"{prefix}-edges-{layout_name}-{name}", region)
                fitted_width = fitted["bounds"][2] - fitted["bounds"][0]
                actual_width = measured["bounds"][2] - measured["bounds"][0]
                assert abs(actual_width / fitted_width - scale) < 0.05, f"incorrect screenshot zoom at DPR {dpr}: {scale}: {measured}"
                zoom_screenshots[f"{layout_name}-{name}"] = measured
                previous = scale

        # Compare shortcut behavior with the actual Fit button at the same
        # viewport, without assuming where any individual node should end up.
        canvas.click(position={"x": 143, "y": 86})
        settle_gesture(page)
        fit_keys = {"button": fitted["bounds"]}
        fresh_wheel_after_fit = None
        for key, name in [("f", "f"), ("Shift+f", "shift-f")]:
            page.mouse.move(30, height - 70)
            page.mouse.wheel(-24, -12)
            settle_gesture(page)
            pinch_wheel(page, cdp, (720, height * 0.65), -18)
            page.mouse.move(30, height - 70)
            rendered_frame(page)
            manual = node_snapshot(page, canvas, output, f"{prefix}-{name}-manual", region)
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
        editor_region = (4, 104, 1436, 970)
        before_typing = node_snapshot(page, canvas, output, f"{prefix}-editor-before-f", editor_region)
        open_input(page)
        page.keyboard.press("Control+A")
        page.keyboard.type(bytecode, delay=5)
        settle_gesture(page)
        editor_before_scroll = canvas.screenshot(path=str(output / f"{prefix}-modal-before-scroll.png"))
        page.mouse.move(600, 420)
        page.mouse.wheel(0, -300)
        settle_gesture(page)
        editor_after_scroll = canvas.screenshot(path=str(output / f"{prefix}-modal-after-scroll.png"))
        assert screenshot_metrics(page, editor_before_scroll, (350, 365, 1090, 480))["hash"] != screenshot_metrics(page, editor_after_scroll, (350, 365, 1090, 480))["hash"], "wheel did not scroll the actual modal editor"
        page.keyboard.type("fF", delay=40)
        rendered_frame(page)
        canvas.screenshot(path=str(output / f"{prefix}-editor-draft-f.png"))
        page.keyboard.press("Escape")
        settle_gesture(page)
        after_typing = node_snapshot(page, canvas, output, f"{prefix}-editor-after-f", editor_region)
        assert_translation(before_typing, after_typing, 0, 0)
        open_input(page)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
            page.keyboard.press("Control+Enter")
        reply = complete_submission(page, response.value)
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
                before_png = canvas.screenshot()
                before_node = screenshot_metrics(page, before_png, graph_region)
                before_code = screenshot_metrics(page, before_png, region)
                assert before_node["bounds"], f"missing node before {name} scroll"
                page.mouse.move(*pointer)
                page.mouse.wheel(0, 220)
                settle_gesture(page)
                page.mouse.move(2, 2)
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
        browser.close()
    return results

RPC_PROVIDERS = [
    {"id": "pending-fixture", "name": "Pending acquisition"},
    {"id": "world-fixture", "name": "World snapshot"},
]
RPC_TEST_TOKEN = "rpc-value-shown-in-provider-selector"


def open_rpc_input(page, canvas):
    open_input(page)
    canvas.click(position={"x": 465, "y": 386})
    settle_gesture(page)


def submit_rpc(page, canvas, rpc, output: Path, name: str, provider_index: int, providers):
    expect(page.locator("#analysis-status")).to_contain_text(
        f"RPC provider: {providers[0]['name']} ({providers[0]['endpoint']})")
    open_rpc_input(page, canvas)
    if provider_index:
        canvas.click(position={"x": 600, "y": 404})
        rendered_frame(page)
        canvas.screenshot(path=str(output / f"{name}-providers.png"))
        canvas.click(position={"x": 460, "y": 430 + provider_index * 23})
    provider = providers[provider_index]
    expect(page.locator("#analysis-status")).to_contain_text(
        f"RPC provider: {provider['name']} ({provider['endpoint']})")
    canvas.click(position={"x": 600, "y": 444})
    page.keyboard.press("Control+A")
    page.keyboard.type(rpc.root_address, delay=3)
    rendered_frame(page)
    canvas.screenshot(path=str(output / f"{name}-input.png"))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
        page.keyboard.press("Control+Enter")
    admitted = response.value
    request = admitted.request.post_data_json
    assert request["input"] == {"Rpc": {"provider_id": provider["id"], "address": rpc.root_address, "block": "Latest", "accounts": []}}, request
    assert rpc.endpoint not in admitted.request.post_data
    assert RPC_TEST_TOKEN not in admitted.request.post_data
    assert request["environment"]["number"] is None
    return admitted


def world_interactions(browser, url: str, output: Path, rpc, pending_rpc, providers):
    results = {}
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    failures = []
    page.on("pageerror", lambda error: failures.append(str(error)))
    page.goto(url, wait_until="networkidle")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    admitted = submit_rpc(page, canvas, rpc, output, "world", provider_index=1, providers=providers)
    reply = complete_submission(page, admitted)
    envelope = reply.json()
    (output / "world-task-reply.json").write_text(json.dumps({"admission_http_status": reply.admission_status, "observed_http_status": reply.status, "reply": envelope}, indent=2) + "\n")
    assert reply.status == 200 and "Ok" in envelope["result"], f"RPC world task failed: {json.dumps(envelope, sort_keys=True)}"
    report = envelope["result"]["Ok"]
    assert len(report["programs"]) >= 2, "RPC call failed to capture the child program"
    assert {program["code_address"] for program in report["programs"]} == {rpc.root_address, rpc.child_address}
    assert max(block["frame_depth"] for block in report["cfg"]) == 2
    snapshot = report["metadata"]["snapshot"]
    assert int(snapshot["chain_id"], 16) == int(rpc.chain_id, 16) and snapshot["block_hash"] == rpc.block_hash
    assert report["acquisition"]["requests"] > 0
    assert report["outcomes"] and report["stores"] and report["byte_arrays"]
    assert any(state["exit"] and len(state["exit"]["frames"]) == 2 for state in report["states"])
    child = next(program for program in report["programs"] if program["code_address"] == rpc.child_address)
    child_state = next(block for block in report["cfg"] if block["program"] == child["id"])
    child_frames = [frame for state in report["states"] for point in (state["entry"], state["exit"]) if point for frame in point["frames"] if frame["program"] == child["id"]]
    def contains(value, number):
        return value["constants"] is not None and number in [int(word, 16) for word in value["constants"]]
    def byte_at(array, offset, number):
        return any(cell["offset"] == offset and contains(array["values"][cell["value"]], number) for cell in array["cells"])
    assert any(contains(report["byte_arrays"][frame["calldata"]]["length"], 1) and byte_at(report["byte_arrays"][frame["calldata"]], 0, 0xab) for frame in child_frames)
    assert any(byte_at(report["byte_arrays"][frame["memory"]], 0, 0xab) for frame in child_frames)
    child_accounts = [account for store in report["stores"] for account in store["accounts"] if account["address"] == rpc.child_address]
    assert any(any(int(entry["slot"], 16) == 3 and contains(entry["value"], 99) for entry in account["storage"]) for account in child_accounts)
    assert any(any(int(entry["slot"], 16) == 2 and contains(entry["value"], 7) for entry in account["transient"]) for account in child_accounts)
    # Select the actual child program from the native directory. It must
    # update code, SSA, graph selection and the active frame together.
    settle_gesture(page)
    canvas.screenshot(path=str(output / "world-overview.png"))
    canvas.click(position={"x": 80, "y": 135})
    expect(status).to_contain_text(f"source P{child['id']}")
    expect(status).to_contain_text(f"Selected S{child_state['id']}")
    expect(status).to_contain_text("Frame 1")
    canvas.screenshot(path=str(output / "world-child-frame.png"))
    screenshots = {}
    tabs = [(80, "Stack"), (137, "Memory"), (201, "Storage"), (268, "Transient"),
            (335, "Calldata"), (408, "Returndata"), (488, "Outcomes"),
            (567, "Acquisition"), (654, "Environment"), (741, "Diagnostics"), (30, "Frame")]
    for x, tab in tabs:
        canvas.click(position={"x": x, "y": 778})
        expect(status).to_contain_text(tab)
        rendered_frame(page)
        png = canvas.screenshot(path=str(output / f"world-{tab.lower()}.png"))
        screenshots[tab] = hashlib.sha256(png).hexdigest()
    assert len(set(screenshots.values())) == len(tabs), "inspector controls did not expose distinct views"
    canvas.click(position={"x": 166, "y": 801})
    expect(status).to_contain_text("Observed exit")
    canvas.click(position={"x": 137, "y": 778})
    expect(status).to_contain_text("Memory")
    canvas.screenshot(path=str(output / "world-child-exit-memory.png"))
    # All acquisition reads remain on one canonical snapshot.
    reads = [request for request in rpc.requests if request["method"] in ("eth_getCode", "eth_getStorageAt", "eth_getBalance", "eth_getTransactionCount")]
    assert reads and all(request["params"][-1] == {"blockHash": rpc.block_hash, "requireCanonical": True} for request in reads)
    assert sum(request["method"] == "eth_getBlockByNumber" and request["params"][0] == "latest" for request in rpc.requests) == 1
    assert not pending_rpc.requests, "selecting the second provider contacted the default provider"
    assert RPC_TEST_TOKEN not in json.dumps(report), "provider URL escaped into the analysis report"
    (output / "world-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    results["analysis"] = {"programs": len(report["programs"]), "states": len(report["states"]), "outcomes": len(report["outcomes"]), "rpc_requests": len(rpc.requests), "inspector_views": screenshots, "renderer": webgpu_evidence(page)}
    assert not failures, failures
    page.close()
    rpc = pending_rpc
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    page.goto(url, wait_until="networkidle")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    page.evaluate("""() => {
        window.taskStatusHistory = [];
        new MutationObserver(() => window.taskStatusHistory.push(document.querySelector('#analysis-status').textContent))
            .observe(document.querySelector('#analysis-status'), {childList:true,subtree:true,characterData:true});
    }""")
    held_polls = []
    aborted = []
    def hold_first_poll(route):
        if route.request.method == "GET" and re.search(r"/api/tasks/\d+$", route.request.url) and not held_polls:
            held_polls.append(route)
        else:
            route.continue_()
    page.route("**/api/tasks/*", hold_first_poll)
    page.on("requestfailed", lambda request: aborted.append({"url": request.url, "failure": request.failure}))
    admitted = submit_rpc(page, canvas, rpc, output, "cancel", provider_index=0, providers=providers)
    assert admitted.status == 202, admitted.text()
    assert rpc.started.wait(5), "fixture did not enter a real pending RPC request"
    expect(status).to_contain_text("showing previous result")
    for _ in range(50):
        if held_polls:
            break
        page.wait_for_timeout(100)
    assert held_polls, "fixture did not hold an actual browser status fetch"
    canvas.screenshot(path=str(output / "cancel-pending.png"))
    with page.expect_response(lambda response: response.request.method == "DELETE" and "/api/tasks/" in response.url) as cancelled:
        canvas.click(position={"x": 30, "y": 988})
    assert cancelled.value.ok
    assert cancelled.value.json()["result"]["Ok"]["state"] == "Cancelling"
    assert rpc.disconnected.wait(5), "cooperative cancellation did not close pending acquisition"
    expect(status).to_contain_text("Cancelled", timeout=22000)
    assert any("/api/tasks/" in failure["url"] for failure in aborted), "hung HTTP poll was never aborted"
    history = page.evaluate("window.taskStatusHistory")
    assert any("Cancelling" in text for text in history), history
    assert "showing previous result" in status.text_content()
    canvas.screenshot(path=str(output / "cancel-acknowledged.png"))
    results["cancellation"] = {"history": history, "pending_rpc_closed": rpc.disconnected.is_set(), "aborted_http": aborted, "status": status.text_content()}
    # Resolve the test harness interception after the application itself
    # proved it aborted/recovered, so Playwright has no pending handler.
    held_polls[0].abort()
    page.unroute_all(behavior="wait")
    page.close()
    return results


def configured_world_interactions(browser, args):
    # Endpoints must exist before the server loads its provider configuration.
    # Both tasks use the real catalogue and ID resolution, including cancellation.
    with RpcFixture(block_method="eth_getCode") as pending, RpcFixture() as rpc:
        with TemporaryDirectory(prefix="web-rpc-providers-") as directory:
            configuration = Path(directory) / "providers.json"
            providers = [
                {**provider, "endpoint": fixture.endpoint + "/rpc?token=" + RPC_TEST_TOKEN}
                for provider, fixture in zip(RPC_PROVIDERS, [pending, rpc], strict=True)
            ]
            configuration.write_text(json.dumps({"providers": providers}) + "\n")
            with analysis_server(args, configuration) as url:
                page = browser.new_page()
                catalogue = page.request.get(url + "/api/rpc-providers")
                assert catalogue.ok, catalogue.text()
                assert catalogue.json() == {"result": {"Ok": providers}}
                page.close()
                result = world_interactions(browser, url, args.output, rpc, pending, providers)
                result["providers"] = providers
                result["selected_provider_id"] = RPC_PROVIDERS[1]["id"]
                result["endpoint_displayed_in_selector"] = True
                result["submission_uses_provider_id"] = True
                return result


def url_configured_world_interactions(browser, args):
    with RpcFixture() as rpc:
        endpoint = rpc.endpoint + "/rpc?token=" + RPC_TEST_TOKEN
        providers = [{"id": "default", "name": "Default RPC", "endpoint": endpoint}]
        with analysis_server(args, endpoint) as url:
            assert not rpc.requests, "loading a URL unexpectedly contacted the RPC"
            page = browser.new_page(viewport={"width": 1440, "height": 1000})
            catalogue = page.request.get(url + "/api/rpc-providers")
            assert catalogue.ok, catalogue.text()
            assert catalogue.json() == {"result": {"Ok": providers}}
            page.goto(url, wait_until="networkidle")
            expect(page.locator("#analysis-status")).to_contain_text("Ready:", timeout=60000)
            admitted = submit_rpc(page, page.locator("#evm-canvas"), rpc, args.output,
                                  "url-default", provider_index=0, providers=providers)
            reply = complete_submission(page, admitted)
            envelope = reply.json()
            assert reply.status == 200 and "Ok" in envelope["result"], envelope
            report = envelope["result"]["Ok"]
            assert {program["code_address"] for program in report["programs"]} == {rpc.root_address, rpc.child_address}
            assert report["metadata"]["snapshot"]["block_hash"] == rpc.block_hash
            assert report["acquisition"]["requests"] > 0 and rpc.requests
            assert RPC_TEST_TOKEN not in json.dumps(report)
            assert endpoint not in json.dumps(report)
            (args.output / "url-default-task-reply.json").write_text(json.dumps(envelope, indent=2) + "\n")
            page.close()
            return {"providers": providers, "selected_provider_id": "default",
                    "rpc_requests": len(rpc.requests), "endpoint_displayed_in_selector": True,
                    "submission_uses_provider_id": True,
                    "status": report["status"]}


def assert_rpc_disabled(page, canvas, output: Path, name: str, analyze_y: int):
    submissions = []
    def observe_submission(request):
        if request.method == "POST" and request.url.endswith("/api/tasks"):
            submissions.append(request.post_data_json)
    page.on("request", observe_submission)
    try:
        page.keyboard.press("Control+Enter")
        settle_gesture(page)
        canvas.click(position={"x": 382, "y": analyze_y})
        settle_gesture(page)
        assert not submissions, f"{name} allowed an RPC task without an available provider: {submissions}"
        canvas.screenshot(path=str(output / f"{name}.png"))
    finally:
        page.remove_listener("request", observe_submission)


def unconfigured_rpc_interactions(browser, url: str, output: Path):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    with page.expect_response(lambda response: response.url.endswith("/api/rpc-providers")) as catalogue:
        page.goto(url, wait_until="networkidle")
    assert catalogue.value.ok
    assert catalogue.value.json() == {"result": {"Ok": []}}
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    expect(status).to_contain_text("No RPC providers are configured on the server.")
    canvas = page.locator("#evm-canvas")
    bytecode = submit_bytecode(page, canvas, "600160020100", output / "rpc-empty-bytecode.png")
    assert bytecode.status == 200 and bytecode.json()["result"]["Ok"]["status"] == "Converged"
    open_rpc_input(page, canvas)
    assert_rpc_disabled(page, canvas, output, "rpc-empty-disabled", analyze_y=667)
    with page.expect_response(lambda response: response.url.endswith("/api/rpc-providers")) as retry:
        canvas.click(position={"x": 372, "y": 414})
    assert retry.value.json() == {"result": {"Ok": []}}
    expect(status).to_contain_text("No RPC providers are configured on the server.")
    page.close()

    # Fail the initial catalogue, then delay a retry after bytecode is ready.
    # This starts the production fetch deadline immediately before the loading
    # assertions, rather than letting initial analysis consume that deadline.
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    held_catalogues = []
    catalogue_attempts = 0
    def delay_catalogue(route):
        nonlocal catalogue_attempts
        catalogue_attempts += 1
        if catalogue_attempts == 1:
            route.fulfill(status=503, content_type="application/json", body='{"result":{"Ok":[]}}')
        else:
            held_catalogues.append(route)
    page.route("**/api/rpc-providers", delay_catalogue)
    page.goto(url, wait_until="networkidle")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    expect(status).to_contain_text("Could not load RPC providers:")
    canvas = page.locator("#evm-canvas")
    bytecode = submit_bytecode(page, canvas, "600160020100", output / "rpc-failed-bytecode.png")
    assert bytecode.status == 200
    open_rpc_input(page, canvas)
    assert_rpc_disabled(page, canvas, output, "rpc-failed-disabled", analyze_y=667)
    canvas.click(position={"x": 372, "y": 414})
    expect(status).to_contain_text("Loading RPC providers")
    assert len(held_catalogues) == 1
    assert_rpc_disabled(page, canvas, output, "rpc-loading-disabled", analyze_y=660)
    expect(status).to_contain_text("Loading RPC providers")
    held_catalogues[0].fulfill(status=503, content_type="application/json", body='{"result":{"Ok":[]}}')
    expect(status).to_contain_text("Could not load RPC providers:")
    page.unroute("**/api/rpc-providers", delay_catalogue)
    with page.expect_response(lambda response: response.url.endswith("/api/rpc-providers")) as retry:
        canvas.click(position={"x": 372, "y": 414})
    assert retry.value.ok and retry.value.json() == {"result": {"Ok": []}}
    expect(status).to_contain_text("No RPC providers are configured on the server.")
    canvas.screenshot(path=str(output / "rpc-retry-recovered.png"))
    page.close()
    return {"unconfigured_catalogue": [], "bytecode_available": True,
            "disabled_rpc_states": ["empty", "loading", "failed"],
            "checked_submission_paths": ["Analyze", "Control+Enter"],
            "catalogue_retry": "real server returned empty catalogue"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", required=True)
    parser.add_argument("--assets", required=True)
    parser.add_argument("--browser", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    with analysis_server(args) as url:
        with sync_playwright() as playwright:
            browser = launch_browser(playwright.chromium, args.browser)
            page = browser.new_page(viewport={"width": 1440, "height": 1000}, color_scheme="light")
            observe_webgpu(page)
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.on(
                "console",
                lambda message: errors.append(message.text)
                if message.type == "error"
                and not (message.location.get("url", "").endswith("/api/tasks") and "400" in message.text)
                else None,
            )
            with page.expect_response(lambda response: response.url.endswith("/api/tasks")) as response:
                page.goto(url, wait_until="networkidle")
            reply = complete_submission(page, response.value)
            assert reply.status == 200, f"initial analysis failed: {reply.status}"
            result = reply.json()
            analysis = result["result"]["Ok"]
            assert analysis["schema_version"] == 5
            assert sum(len(block["instructions"]) for block in analysis["disassembly"]) > 0
            assert len(analysis["cfg"]) > 1, "example did not produce a graph"
            assert {"BranchTrue", "BranchFalse"}.issubset({edge["kind"] for edge in analysis["edges"]})
            assert analysis["ssa"]["blocks"], "example did not produce SSA"
            (args.output / "analysis.json").write_text(json.dumps(result, indent=2) + "\n")
            status = page.locator("#analysis-status")
            expect(status).to_contain_text(re.compile(r"ready", re.IGNORECASE), timeout=60000)
            expect(status).to_contain_text("CFG nodes: SSA")
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
                    if request.url.endswith("/api/tasks") else None)
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
            assert invalid.admission_status == 202
            assert invalid.status in (200, 202), "worker failures belong to typed task status"
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
                responsive_input[f"{width}x{height}"] = posted_bytecode(reply)
            renderer = webgpu_evidence(page)
            unavailable = webgpu_unavailable(browser, url, args.output)
            budgets = budget_interactions(browser, url, args.output)
            rpc_providers = unconfigured_rpc_interactions(browser, url, args.output)
            world = configured_world_interactions(browser, args)
            url_world = url_configured_world_interactions(browser, args)
            panes = pane_interactions(browser, url, args.output)
            gestures = graph_interactions(playwright.chromium, args.browser, url, args.output)
            assert not errors, "browser errors: " + "\n".join(errors)
            report = {
                "browser": browser.version,
                "renderer": renderer,
                "webgpu_unavailable": unavailable,
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
                "pane_interactions": panes,
                "world_interactions": world,
                "url_configured_world": url_world,
                "rpc_providers": rpc_providers,
                "budget_interactions": budgets,
                "platform": "Linux headless Chromium with Xvfb and SwiftShader WebGPU; synthesized browser gestures, not physical macOS hardware",
                "browser_errors": errors,
            }
            (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
            browser.close()


if __name__ == "__main__":
    main()
