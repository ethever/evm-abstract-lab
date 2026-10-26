"""Harness browser acceptance helpers."""

from pathlib import Path
from contextlib import contextmanager
from contextvars import ContextVar
from playwright.sync_api import expect
import base64
import json
import re
import selectors
import subprocess
import time
from web_test_hybrid import assert_detail_policy, job_result_report, namespace_available


_browser_errors = ContextVar("browser_acceptance_errors", default=None)


@contextmanager
def capture_browser_errors():
    """Collect real page failures across every browser launched by one case."""
    errors = []
    token = _browser_errors.set(errors)
    try:
        yield errors
    finally:
        _browser_errors.reset(token)


def capture_console_errors(page):
    # Preserve the original startup/input console-error acceptance. A typed
    # InvalidBytecode HTTP failure is expected in the submissions case.
    errors = _browser_errors.get()
    if errors is not None:
        page.on("console", lambda message: errors.append(message.text)
                if message.type == "error"
                and not (message.location.get("url", "").endswith("/api/tasks")
                         and "400" in message.text) else None)


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
def analysis_server(args, rpc_config: Path | str | None = None, *, workers: int | None = None,
                    postprocess_work: int | None = None):
    command = [args.server, "--assets", args.assets, "--bind", "127.0.0.1:0"]
    if workers is not None:
        command.extend(["--workers", str(workers), "--queue-capacity", str(workers * 2)])
    if rpc_config is not None:
        command.extend(["--rpc-config", str(rpc_config)])
    if postprocess_work is not None:
        command.extend(["--postprocess-work", str(postprocess_work)])
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
    # eframe can present one already prepared frame before running logic again.
    # Two real submissions after the last text event therefore include the
    # frame that consumes it; browser rAF callbacks alone do not establish this.
    page.wait_for_function("""() => {
        const pending = window.__webTestInputFrame;
        return pending === undefined || window.__webgpuEvidence.submissions >= pending + 2;
    }""")
    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")

class AnalysisResponse:
    """Task result wire reply or terminal typed error, plus its form submission."""
    def __init__(self, response, envelope, status, job=None):
        self.request = response.request
        self.status = status
        self.admission_status = response.status
        self.envelope = envelope
        self.job = job

    def json(self):
        return self.envelope

    @property
    def report(self):
        return job_result_report(self.envelope, job_id=self.job["id"],
                                 publication=self.job["publication"])

TIMING_PHASES = ("validating_ns", "pinning_ns", "acquiring_ns", "analyzing_ns",
                 "building_ssa_ns", "projecting_ns")

def measured_backend_timings(report, *, require_ssa=True):
    timing = report["timings"]
    assert set(timing) == {"queue_ns", "processing_ns", *TIMING_PHASES}, timing
    assert all(type(value) is int and value >= 0 for value in timing.values()), timing
    assert sum(timing[field] for field in TIMING_PHASES) == timing["processing_ns"], timing
    assert timing["processing_ns"] > 0, timing
    assert timing["analyzing_ns"] > 0, timing
    if require_ssa:
        assert timing["building_ssa_ns"] > 0, timing
    assert timing["projecting_ns"] > 0, timing
    return timing

def wait_for_report_publication(page, job_id, publication, timeout_ms=60000):
    """Follow IDs only after Web activates the exact received publication."""
    status = page.locator("#analysis-status")
    fields = {"Job ID": str(job_id), "Publication": publication["version"],
              "Minimum model": publication["minimum_version"], "Global graph": publication["global_version"],
              "Native graph": publication["native_version"], "Closed model": str(publication["model_closed"]).lower()}
    for label, value in fields.items():
        shown = "unavailable" if value is None else value
        expect(status).to_contain_text(
            re.compile(rf"(?:^|; ){re.escape(label)}: {re.escape(shown)}(?=;|$)"), timeout=timeout_ms)


def hide_world_directory(page, canvas):
    """Close the painted directory before tests that need the full Native canvas."""
    def control():
        page.mouse.move(2, 2)
        settle_gesture(page)
        return page.evaluate("""async png => {
            const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
            const c=document.createElement('canvas');c.width=image.width;c.height=image.height;
            const ctx=c.getContext('2d');ctx.drawImage(image,0,0);
            const scale=image.width/innerWidth;
            if(innerWidth<800){
                // Compact chrome puts the open directory in a floating window.
                // Its real grey titlebar can occlude the namespace selectors.
                const h=Math.min(c.height,Math.ceil(140*scale));
                const pixels=ctx.getImageData(0,0,c.width,h).data;
                let left=c.width,right=-1,top=h,bottom=-1,count=0;
                for(let y=0;y<h;y++){
                    let row=0,rowLeft=c.width,rowRight=-1;
                    for(let x=0;x<c.width;x++){
                        const i=(y*c.width+x)*4;
                        if(pixels[i]===45&&pixels[i+1]===45&&pixels[i+2]===45){
                            rowLeft=Math.min(rowLeft,x);rowRight=Math.max(rowRight,x);row++;
                        }
                    }
                    if(row>200*scale){
                        left=Math.min(left,rowLeft);right=Math.max(right,rowRight);
                        top=Math.min(top,y);bottom=Math.max(bottom,y);count+=row;
                    }
                }
                const width=(right-left+1)/scale,height=(bottom-top+1)/scale;
                return count>1500*scale*scale&&width>=200&&height>=24&&height<=40 ?
                    {x:(right+1)/scale-17,y:(top+bottom+1)/2/scale}:null;
            }
            const p=ctx.getImageData(Math.round((innerWidth-202)*scale),Math.round(4*scale),
                Math.round(44*scale),Math.round(16*scale)).data;
            let count=0;
            for(let i=0;i<p.length;i+=4)if(p[i]===33&&p[i+1]===65&&p[i+2]===75)count++;
            return count>100*scale*scale ? {x:innerWidth-180,y:12}:null;
        }""", base64.b64encode(canvas.screenshot()).decode())
    target=control()
    if target is not None:
        canvas.click(position=target)
        assert control() is None, "World directory did not close"


def select_report_namespace(page, report, namespace):
    """Use the real selector before following IDs in a particular graph."""
    assert namespace_available(report, namespace), namespace
    if report["hybrid"] is not None:
        label, x = {"Baseline": ("Global model", 50), "Native": ("Detailed paths", 148)}[namespace]
        if namespace == "Baseline" and report["minimum"] is not None:
            expected = f"Global model {report['minimum']['status']}"
        else:
            expected = f"View: {label}"
        status = page.locator("#analysis-status")
        if expected not in status.text_content():
            page.locator("#evm-canvas").click(position={"x": x, "y": 38})
        expect(status).to_contain_text(expected)
        settle_gesture(page)


def complete_submission(page, admitted, timeout_seconds: int = 60, *, namespace=None, close_directory=False,
                        expected_detail_regions=1):
    # Parse raw HTTP text in Python so u64 Job IDs never pass through JS Number.
    reply = json.loads(admitted.text())["result"]
    if "Err" in reply:
        return AnalysisResponse(admitted, {"result": reply}, admitted.status)
    job = reply["Ok"]
    job_id = job["id"]
    observed_status = admitted.status
    task_url = admitted.url + "/" + str(job_id)
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        state = job["state"]
        if state == "Completed":
            result = page.request.get(task_url + "/result")
            assert result.ok, result.text()
            envelope = json.loads(result.text())
            report = job_result_report(envelope, job_id=job_id, publication=job["publication"])
            if report["hybrid"] is not None:
                # Only this D5 field has the authoritative omission default1.
                # Preserve raw request bytes and compare the effective policy.
                effective = admitted.request.post_data_json["limits"].get("max_detail_regions", 1)
                assert type(effective) is int and effective == expected_detail_regions
                assert_detail_policy(report, effective)
            timing = measured_backend_timings(report, require_ssa=namespace == "Native")
            assert timing == job["timings"], "report and completed job disagree on measured time"
            expect(page.locator("#analysis-status")).to_contain_text("Ready:", timeout=timeout_seconds * 1000)
            wait_for_report_publication(page, job_id, job["publication"], timeout_seconds * 1000)
            if close_directory:
                hide_world_directory(page, page.locator("#evm-canvas"))
            if namespace is not None:
                select_report_namespace(page, report, namespace)
            return AnalysisResponse(admitted, envelope, result.status, job)
        if isinstance(state, dict) and "Failed" in state:
            error = state["Failed"]
            expect(page.locator("#analysis-status")).to_contain_text("Error:", timeout=60000)
            return AnalysisResponse(admitted, {"result": {"Err": error}}, observed_status, job)
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
    # The modal requests its TextEdit focus after painting the controls. Wait
    # until eframe has applied that request to its real DOM text agent before
    # sending selection/paste events, which a focused canvas would discard.
    page.wait_for_function("""() => {
        const canvas = document.querySelector('#evm-canvas');
        const agent = canvas?.nextElementSibling;
        if (!(agent instanceof HTMLInputElement) || document.activeElement !== agent) return false;
        const x = parseFloat(agent.style.left), y = parseFloat(agent.style.top);
        return x > canvas.offsetLeft && y > canvas.offsetTop &&
               x <= canvas.offsetLeft + canvas.clientWidth &&
               y <= canvas.offsetTop + canvas.clientHeight;
    }""")

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
    return complete_submission(page, admitted, namespace="Native")

def settle_gesture(page) -> None:
    """Wait for consumed input, including the tail of an actual smooth wheel.

    Buttons and keys use rendered-frame readiness. A wheel's tail still needs
    egui's 400 ms smoothing interval; subsequent calls do not wait it again.
    Animation/timing acceptance retains its own sampling intervals.
    """
    page.evaluate("""() => new Promise(resolve => {
        const pending = window.__webTestLastWheel ?? -Infinity;
        function settled() {
            if (performance.now() - pending < 400) requestAnimationFrame(settled);
            else requestAnimationFrame(() => requestAnimationFrame(resolve));
        }
        settled();
    })""")

def resize_view(page, width: int, height: int) -> None:
    page.set_viewport_size({"width": width, "height": height})
    page.wait_for_function(
        "() => { const c = document.querySelector('#evm-canvas'); return c.width === innerWidth * devicePixelRatio && c.height === innerHeight * devicePixelRatio; }")
    rendered_frame(page)

def observe_webgpu(page, unavailable: str | None = None) -> None:
    """Observe the application's real browser API calls before WASM starts.

    Wrappers call the native implementations unchanged. The test never supplies
    a renderer or creates its own GPU device on the successful application path.
    """
    errors = _browser_errors.get()
    if errors is not None:
        page.on("pageerror", lambda error: errors.append(str(error)))
    page.add_init_script("""(() => {
        const unavailable = UNAVAILABLE;
        window.__webTestLastWheel = -Infinity;
        addEventListener('wheel', () => { window.__webTestLastWheel = performance.now(); },
                         {capture: true, passive: true});
        const evidence = {contexts: [], adapter_requests: 0, adapters: [],
                          device_requests: 0, devices: 0, configurations: [], textures: 0,
                          submissions: 0, indexed_draws: 0, errors: []};
        window.__webgpuEvidence = evidence;
        addEventListener('input', event => {
            const canvas = document.querySelector('#evm-canvas');
            if (event.target instanceof HTMLInputElement && event.target === canvas?.nextElementSibling)
                window.__webTestInputFrame = evidence.submissions;
        }, {capture: true});
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
                evidence.device_requests++;
                if (unavailable === 'device-rejected')
                    throw new DOMException('Injected WebGPU device rejection', 'OperationError');
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
