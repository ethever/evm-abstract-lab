"""Harness browser acceptance helpers."""

from pathlib import Path
from contextlib import contextmanager
from contextvars import ContextVar
from playwright.sync_api import expect
import json
import math
import re
import selectors
import subprocess
import time
from web_test_hybrid import assert_detail_policy, job_result_report, namespace_available, native_entry_context, source_program


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


def ui_controls(page):
    """Read opt-in geometry from actual egui responses in the accepted UI pass."""
    status = page.locator("#analysis-status")
    expect(status).to_have_attribute("data-controls", re.compile(r"^\["))
    controls = json.loads(status.get_attribute("data-controls"))
    assert type(controls) is list, controls
    for control in controls:
        assert type(control) is dict and set(control) == {
            "id", "label", "role", "selected", "enabled", "rect"}, control
        assert type(control["id"]) is str and control["id"].isascii() and control["id"].isdecimal(), control
        assert str(int(control["id"])) == control["id"] and int(control["id"]) < 2**64, control
        assert type(control["label"]) is str and type(control["role"]) is str, control
        assert control["selected"] is None or type(control["selected"]) is bool, control
        assert type(control["enabled"]) is bool, control
        rect = control["rect"]
        assert type(rect) is list and len(rect) == 4, control
        assert all(type(value) in (int, float) and math.isfinite(value) for value in rect), control
        assert rect[0] < rect[2] and rect[1] < rect[3], control
    return controls


def ui_control(page, label=None, *, role=None, selected=None, enabled=True, id=None, within=None):
    # A disabled background control behind a modal cannot satisfy an enabled
    # target. All interaction still enters through the real canvas.
    assert label is not None or id is not None, "a control requires a rendered label or exact response ID"
    fields = {} if label is None else dict(label=label)
    fields.update((key, value) for key, value in
                  [("role", role), ("selected", selected), ("enabled", enabled), ("id", id)]
                  if value is not None)
    # Parse the real attribute structurally: rendered values can contain JSON
    # braces or quotes. Polling this DOM state also works on idle WebGPU pages
    # without requiring a rAF callback merely to evaluate the predicate.
    # Repeated Exact/Unknown tabs are qualified by their real enclosing row,
    # never by an ordinal or an unrelated first matching response.
    bounds = within["rect"] if isinstance(within, dict) else within
    deadline = time.monotonic() + 5
    while True:
        candidates = [control for control in ui_controls(page)
                      if all(control[key] == value for key, value in fields.items())
                      and (bounds is None or (
                          bounds[0] - 0.5 <= control["rect"][0] < control["rect"][2] <= bounds[2] + 0.5
                          and bounds[1] - 0.5 <= control["rect"][1] < control["rect"][3] <= bounds[3] + 0.5))]
        assert len(candidates) <= 1, f"ambiguous rendered control {fields}: {candidates}"
        if candidates:
            return candidates[0]
        assert time.monotonic() < deadline, f"no rendered control {fields} within {bounds}"
        page.wait_for_timeout(25)  # Poll actual response geometry, including idle WebGPU pages.


def click_control(page, canvas, label, **query):
    control = ui_control(page, label, **query)
    left, top, right, bottom = control["rect"]
    assert 0 <= left < right <= page.viewport_size["width"], control
    assert 0 <= top < bottom <= page.viewport_size["height"], control
    bounds = canvas.bounding_box()
    assert bounds is not None, "application canvas is not visible"
    assert bounds["x"] <= left < right <= bounds["x"] + bounds["width"], control
    assert bounds["y"] <= top < bottom <= bounds["y"] + bounds["height"], control
    canvas.click(position=dict(x=(left + right) / 2 - bounds["x"],
                              y=(top + bottom) / 2 - bounds["y"]))
    settle_gesture(page)
    return control


def set_control(page, canvas, label, selected, *, role="toggle", **query):
    """Set one real toggle, checkbox or section without accidentally reopening it."""
    assert type(selected) is bool
    control = ui_control(page, label, role=role, **query)
    assert type(control["selected"]) is bool, control
    if control["selected"] != selected:
        click_control(page, canvas, label, role=role, **query)
    return ui_control(page, label, role=role, selected=selected, **query)


def focus_text_control(page, canvas, label, *, touch=False, **query):
    """Focus one observed editor through an actual pointer or Chromium touch."""
    if touch:
        control = ui_control(page, label, role="text", **query)
        left, top, right, bottom = control["rect"]
        assert 0 <= left < right <= page.viewport_size["width"], control
        assert 0 <= top < bottom <= page.viewport_size["height"], control
        session = page.context.new_cdp_session(page)
        try:
            session.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [
                {"x": (left + right) / 2, "y": (top + bottom) / 2, "radiusX": 3, "radiusY": 3}]})
            session.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
        finally:
            session.detach()
        settle_gesture(page)
    else:
        control = click_control(page, canvas, label, role="text", **query)
    page.wait_for_function("""rect => {
        const canvas = document.querySelector('#evm-canvas');
        const agent = canvas?.nextElementSibling;
        if (!(agent instanceof HTMLInputElement) || document.activeElement !== agent) return false;
        const x = parseFloat(agent.style.left), y = parseFloat(agent.style.top);
        return Number.isFinite(x) && Number.isFinite(y) &&
            rect[0] - 4 <= x && x <= rect[2] + 4 && rect[1] - 4 <= y && y <= rect[3] + 4;
    }""", arg=control["rect"], polling=25)
    return control


def edit_control(page, canvas, label, text, **query):
    """Replace a real egui text field after its DOM text agent takes focus."""
    control = focus_text_control(page, canvas, label, **query)
    page.keyboard.press("Control+A")
    rendered_frame(page)
    page.keyboard.insert_text(str(text))
    rendered_frame(page)
    return control


def select_input_mode(page, canvas, mode):
    assert mode in {"Address", "Bytecode"}, mode
    control = ui_control(page, mode, role="tab")
    if not control["selected"]:
        click_control(page, canvas, mode, role="tab")
    ui_control(page, mode, role="tab", selected=True)
    expect(page.locator("#analysis-status")).to_contain_text(f"Input editor: {mode}")


def hide_world_directory(page, canvas):
    """Close the anchored Sources lane through its actual header toggle."""
    control = ui_control(page, "Sources", role="toggle")
    if control["selected"]:
        click_control(page, canvas, "Sources", role="toggle", selected=True)
        ui_control(page, "Sources", role="toggle", selected=False)


def select_report_context(page, report, namespace, *, state=None, summary=None, procedure=None,
                          close_directory=True):
    """Open one original context through Sources, never by a display ordinal."""
    assert namespace_available(report, namespace), namespace
    canvas = page.locator("#evm-canvas")
    label = {"Baseline": "Global model", "Native": "Detailed paths"}[namespace]
    if namespace == "Native" or report["minimum"] is None:
        assert summary is None and procedure is None
        assert type(state) is int and state >= 0, state
        selected = report[{"Baseline": "global", "Native": "native"}[namespace]]
        nodes = [node for node in selected["cfg"] if type(node["id"]) is int and node["id"] == state]
        assert len(nodes) == 1, (namespace, state, nodes)
        program = None if nodes[0]["program"] is None else source_program(report, nodes[0]["program"])
        query = f"Graph {{ state: {state} }}"
    else:
        assert state is None
        minimum = report["minimum"]
        assert type(summary) is int and 0 <= summary < len(minimum["summaries"]), summary
        relation = minimum["summaries"][summary]["relation"]
        assert type(procedure) is int and 0 <= procedure < len(relation["procedures"]), procedure
        target = relation["procedures"][procedure]
        key = target["process"]
        if key is not None:
            captures = [item for item in report["process_controls"]
                        if item["source"] == key["source"] and item["epoch"] == key["epoch"]]
            programs = {item["program"] for item in captures if item["program"] is not None}
            assert len(programs) <= 1, programs
            program = source_program(report, programs.pop()) if programs else None
        else:
            captures = [item for item in report["programs"]
                        if (item["code_address"], item["code_hash"], item["kind"], item["fork"]) ==
                        (target["code_address"], target["code_hash"], target["kind"], minimum["fork"])]
            assert len(captures) <= 1, captures
            program = captures[0] if captures else None
        query = f"Minimum {{ summary: {summary}, procedure: {procedure} }}"
    click_control(page, canvas, label, role="tab")
    # An opaque/program=None context has no captured source row to choose.
    # Listing every context is an explicit presentation action; it does not
    # change the canonical focus until the exact context below is picked.
    set_control(page, canvas, "All contexts", program is None)
    if program is not None:
        # The original template and its conditional Native alias may have the
        # same bytes. Choose the actual alias before enumerating its contexts.
        source = program["code_address"] or (
            "Constructor code" if program["kind"] == "Initcode" else "Supplied input program")
        # An address can retain old and new code in the same publication.
        # Hash search plus the actual alias caption keeps those sources apart.
        edit_control(page, canvas, "source_context_filter", program["code_hash"])
        caption = source if len(source) <= 22 else source[:10] + "…" + source[-8:]
        click_control(page, canvas, caption, role="source")
        set_control(page, canvas, "Sources", True)
    edit_control(page, canvas, "source_context_filter", query)
    contexts = [control for control in ui_controls(page) if control["role"] == "context" and control["enabled"]]
    assert len(contexts) == 1, (namespace, query, contexts)
    click_control(page, canvas, contexts[0]["label"], role="context", id=contexts[0]["id"])
    expect(page.locator("#analysis-status")).to_contain_text(f"View: {label}")
    # Namespace text alone could accept a different row in the same graph.
    # Reopen a narrow sheet and verify the canonical selection through the
    # actual selected context and source responses after navigation completes.
    set_control(page, canvas, "Sources", True)
    chosen = ui_control(page, contexts[0]["label"], role="context", selected=True)
    if program is not None:
        edit_control(page, canvas, "source_context_filter", program["code_hash"])
        ui_control(page, caption, role="source", selected=True)
        edit_control(page, canvas, "source_context_filter", query)
        chosen = ui_control(page, contexts[0]["label"], role="context", selected=True)
    if close_directory:
        hide_world_directory(page, canvas)
    return chosen


def select_report_namespace(page, report, namespace):
    """Preserve an active namespace; switching requires a qualified root context."""
    assert namespace_available(report, namespace), namespace
    if report["hybrid"] is not None:
        label = {"Baseline": "Global model", "Native": "Detailed paths"}[namespace]
        expected = f"View: {label}"
        status = page.locator("#analysis-status")
        if expected not in status.text_content():
            if namespace == "Native":
                select_report_context(page, report, namespace, state=native_entry_context(report)["id"])
            else:
                minimum = report["minimum"]
                assert minimum is not None and len(minimum["summaries"]) == 1, "choose an explicit Global relation"
                relation = minimum["summaries"][0]["relation"]
                select_report_context(page, report, namespace, summary=0, procedure=relation["root_procedure"])
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
            if namespace is not None:
                select_report_namespace(page, report, namespace)
            if close_directory:
                hide_world_directory(page, page.locator("#evm-canvas"))
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

def open_input(page, *, mode=None):
    """Open the modal and explicitly focus its real source field."""
    page.keyboard.press("Control+Enter")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text(re.compile(r"(?:^|; )Input editor: (Address|Bytecode)(?=;|$)"))
    settle_gesture(page)
    canvas = page.locator("#evm-canvas")
    if mode is not None:
        select_input_mode(page, canvas, mode)
    else:
        match = re.search(r"(?:^|; )Input editor: (Address|Bytecode)(?=;|$)", status.text_content())
        assert match, status.text_content()
        mode = match[1]
    # Mobile mode buttons may keep canvas focus. A real editor tap is the
    # meaningful focus transition before callers send selection or paste keys.
    focus_text_control(page, canvas, "Runtime bytecode" if mode == "Bytecode" else "rpc_address")


def posted_bytecode(reply):
    return reply.request.post_data_json["input"]["Bytecode"]["bytecode"]

def submit_bytecode(page, canvas, bytecode: str, evidence: Path, *, close_directory=False):
    open_input(page, mode="Bytecode")
    edit_control(page, canvas, "Runtime bytecode", bytecode)
    canvas.screenshot(path=str(evidence))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
        page.keyboard.press("Control+Enter")
    admitted = response.value
    posted = posted_bytecode(admitted)
    assert posted == bytecode, f"editor input did not reach the typed request: expected {bytecode!r}, got {posted!r}"
    return complete_submission(page, admitted, namespace="Native", close_directory=close_directory)

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
        const observeControls = () => {
            const status=document.querySelector('#analysis-status');
            if (!status) return false;
            status.setAttribute('data-observe-controls', '');
            return true;
        };
        if (!observeControls()) {
            const observer=new MutationObserver(() => {
                if (observeControls()) observer.disconnect();
            });
            observer.observe(document, {childList:true, subtree:true});
        }
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
        for (const kind of ['keydown', 'keyup', 'pointerdown', 'pointerup', 'compositionstart', 'compositionend']) {
            addEventListener(kind, event => {
                const canvas = document.querySelector('#evm-canvas');
                if (canvas && (event.target === canvas || event.target === canvas.nextElementSibling ||
                               (kind.startsWith('key') && event.target === document.body)))
                    window.__webTestInputFrame = evidence.submissions;
            }, {capture: true});
        }
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
