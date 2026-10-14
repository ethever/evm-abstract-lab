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
import zlib

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


TIMING_PHASES = ("validating_ns", "pinning_ns", "acquiring_ns", "analyzing_ns",
                 "building_ssa_ns", "projecting_ns")


def measured_backend_timings(report):
    timing = report["metadata"]["timings"]
    assert set(timing) == {"queue_ns", "processing_ns", *TIMING_PHASES}, timing
    assert all(type(value) is int and value >= 0 for value in timing.values()), timing
    assert sum(timing[field] for field in TIMING_PHASES) == timing["processing_ns"], timing
    assert timing["processing_ns"] > 0, timing
    assert timing["analyzing_ns"] > 0 and timing["building_ssa_ns"] > 0, timing
    assert timing["projecting_ns"] > 0, timing
    return timing


def complete_submission(page, admitted, timeout_seconds: int = 60):
    reply = admitted.json()["result"]
    if "Err" in reply:
        return AnalysisResponse(admitted, {"result": reply}, admitted.status)
    job = reply["Ok"]
    observed_status = admitted.status
    task_url = admitted.url + "/" + str(job["id"])
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        state = job["state"]
        if state == "Completed":
            result = page.request.get(task_url + "/result")
            envelope = result.json()
            timing = measured_backend_timings(envelope["result"]["Ok"])
            assert timing == job["timings"], "report and completed job disagree on measured time"
            expect(page.locator("#analysis-status")).to_contain_text("Ready:", timeout=timeout_seconds * 1000)
            return AnalysisResponse(admitted, envelope, result.status)
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


def compression_interactions(browser, url: str, output: Path):
    """Exercise real HTTP negotiation and transparent decoding in the Wasm app."""
    cases = [("zstd", ["zstd", "br", "gzip"]),
             ("br", ["br", "gzip"]), ("gzip", ["gzip"]), ("identity", [])]
    results = {}
    for expected, accepted in cases:
        page = browser.new_page(viewport={"width": 1440, "height": 1000})
        observe_webgpu(page)
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        cdp = page.context.new_cdp_session(page)
        cdp.send("Network.enable")
        cdp.send("Network.setAcceptedEncodings", {"encodings": accepted})
        try:
            with page.expect_response(lambda response: response.url.endswith("/result")
                                      and response.request.method == "GET") as fetched:
                with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                          and response.request.method == "POST") as submitted:
                    page.goto(url, wait_until="networkidle")
                completed = complete_submission(page, submitted.value)
            response = fetched.value
            assert response.status == 200, response.text()
            headers = response.all_headers()
            observed = headers.get("content-encoding", "identity")
            assert observed == expected, (expected, headers)
            assert "accept-encoding" in headers.get("vary", "").lower(), headers
            assert headers["content-type"].startswith("application/json"), headers
            assert headers["cache-control"] == "no-store", headers
            envelope = response.json()
            report = envelope["result"]["Ok"]
            assert report == completed.json()["result"]["Ok"]
            assert report["schema_version"] == 12
            assert "max_output_bytes" not in report["metadata"]["export_limits"]
            assert report["cfg"] and report["ssa"]["blocks"]
            expect(page.locator("#analysis-status")).to_contain_text("Ready:")
            page.wait_for_function(
                "url => performance.getEntriesByName(url).some(entry => entry.responseEnd > 0)",
                arg=response.url)
            sizes = page.evaluate("""url => {
                const entry = performance.getEntriesByName(url).find(entry => entry.responseEnd > 0);
                return {encoded: entry.encodedBodySize, decoded: entry.decodedBodySize};
            }""", response.url)
            assert sizes["decoded"] == len(response.body()), sizes
            if expected == "identity":
                assert sizes["encoded"] == sizes["decoded"] > 0, sizes
            else:
                assert 0 < sizes["encoded"] < sizes["decoded"], sizes
            assert not errors, errors
            results[expected] = {"accepted": accepted, "content_encoding": observed,
                                 "encoded_bytes": sizes["encoded"], "decoded_bytes": sizes["decoded"],
                                 "status": report["status"], "wasm_installed_report": True}
            (output / "compression-report.json").write_text(json.dumps(results, indent=2) + "\n")
            print(f"HTTP compression {expected}: {sizes['encoded']} wire bytes, "
                  f"{sizes['decoded']} decoded bytes; Wasm report ready", flush=True)
        finally:
            cdp.detach()
            page.close()
    return results


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
    # Unbounded export accounting can exceed Wasm's usize and JavaScript's
    # exact Number range. Let the real Wasm client decode an exact u64 total.
    reported_export_work = 9_007_199_254_740_993
    decoded_export_work = []

    def large_export_work(route):
        response = route.fetch()
        envelope = response.json()
        metadata = envelope["result"]["Ok"]["metadata"]
        assert "max_work" not in metadata["export_limits"], metadata
        metadata["export_usage"]["work"] = reported_export_work
        # The intercepted body was decoded before editing. Do not label the
        # replacement plain JSON with the upstream response's content coding.
        headers = dict(response.headers)
        headers.pop("content-encoding", None)
        headers.pop("content-length", None)
        route.fulfill(status=response.status, headers=headers, body=json.dumps(envelope))
        decoded_export_work.append(reported_export_work)

    page.route("**/api/tasks/*/result", large_export_work)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    initial_reply = complete_submission(page, initial.value)
    assert initial_reply.status == 200, initial_reply.json()
    initial_request = json.loads(initial_reply.request.post_data)
    initial_report = initial_reply.json()["result"]["Ok"]
    assert initial_report["schema_version"] == 12
    assert "max_work" not in initial_report["metadata"]["export_limits"]
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
    assert report["schema_version"] == 12
    assert report["status"] == "Converged", report["frontiers"]
    assert "max_work" not in report["metadata"]["export_limits"]
    expected = {**defaults, **edited}
    for field, value in expected.items():
        assert request["limits"][field] == value, (field, request["limits"])
        assert report["metadata"]["limits"][field] == value, (field, report["metadata"])
    assert not errors, errors
    assert len(decoded_export_work) >= 2, decoded_export_work
    canvas.screenshot(path=str(output / "large-budget-result.png"))
    (output / "large-budget-request.json").write_text(reply.request.post_data + "\n")
    (output / "large-budget-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    result = {"defaults": defaults, "edited": edited, "status": report["status"],
              "metadata_preserves_exact_integers": True,
              "wasm_decoded_export_work": reported_export_work,
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


def node_snapshot(page, canvas, output: Path, name: str, region=(4, 123, 1436, 970), measure_nodes=False, min_selected_pixels=20):
    screenshot = canvas.screenshot(path=str(output / f"{name}.png"))
    metrics = graph_screenshot_metrics(page, screenshot, region, measure_nodes)
    assert metrics["bounds"] and metrics["selected_pixels"] >= min_selected_pixels, f"selected CFG node missing: {name}: {metrics}"
    return metrics


def graph_screenshot_metrics(page, screenshot: bytes, region, measure_nodes=False):
    """Measure CFG cards by their semantic fill and actual selection outline.

    Source tables still use screenshot_metrics and the ordinary SELECTED fill.
    A long continuous accent border distinguishes the selected card from its
    accent-colored title, endpoint arrows and highlighted incident edges.
    """
    return page.evaluate("""async ({png,region,measureNodes}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        const scale=image.width/innerWidth;
        const [left,top,right,bottom]=region.map(value=>Math.round(value*scale));
        const width=right-left,height=bottom-top;
        const pixels=context.getImageData(left,top,width,height).data;
        const fills=[[21,29,42],[20,52,42],[48,30,62],[62,27,34]];
        const kinds=['ordinary','normal','revert','failure'];
        const mask=new Uint8Array(width*height),fillAt=new Int8Array(width*height);fillAt.fill(-1);
        for(let at=0;at<mask.length;at++) {
            const i=at*4;
            const fill=fills.findIndex(color=>pixels[i]===color[0] && pixels[i+1]===color[1] && pixels[i+2]===color[2]);
            fillAt[at]=fill;
            if(fill>=0 || (pixels[i]===47 && pixels[i+1]===62 && pixels[i+2]===80)) mask[at]=1;
        }
        const accent=(x,y)=>{
            if(x<0 || x>=width || y<0 || y>=height) return false;
            const i=(y*width+x)*4,r=pixels[i],g=pixels[i+1],b=pixels[i+2];
            return r>=45 && r<=125 && g>=130 && g<=235 && b>=120 && b<=225 && g-r>=60 && b-r>=50;
        };
        const outline=(x,y)=>{
            if(accent(x,y)) return true;
            if(x<0 || x>=width || y<0 || y>=height) return false;
            const i=(y*width+x)*4,r=pixels[i],g=pixels[i+1],b=pixels[i+2];
            const border=r>=30 && r<=90 && g>=40 && g<=110 && b>=55 && b<=140
                && g-r>=5 && g-r<=35 && b-g>=8 && b-g<=40;
            const hover=r>=70 && g>=95 && b>=140 && g-r>=30 && b-g>=35 && b-r>=70;
            return border || hover;
        };
        const run=(start,end,fixed,horizontal,match=accent)=>{
            let current=0,best=0;
            for(let at=start;at<=end;at++) {
                current=(horizontal?match(at,fixed):match(fixed,at))?current+1:0;
                best=Math.max(best,current);
            }
            return best;
        };
        const records=[],queue=new Int32Array(width*height);
        for(let start=0;start<mask.length;start++) {
            if(!mask[start]) continue;
            let read=0,write=1,x0=width,y0=height,x1=0,y1=0;
            let fx0=width,fy0=height,fx1=0,fy1=0,fillCount=0;
            const fillCounts=[0,0,0,0];queue[0]=start;mask[start]=0;
            while(read<write) {
                const at=queue[read++],x=at%width,y=Math.floor(at/width);
                x0=Math.min(x0,x);y0=Math.min(y0,y);x1=Math.max(x1,x);y1=Math.max(y1,y);
                if(fillAt[at]>=0) {
                    fillCounts[fillAt[at]]++;fillCount++;
                    fx0=Math.min(fx0,x);fy0=Math.min(fy0,y);fx1=Math.max(fx1,x);fy1=Math.max(fy1,y);
                }
                for(const next of [x>0?at-1:-1,x+1<width?at+1:-1,y>0?at-width:-1,y+1<height?at+width:-1]) {
                    if(next>=0 && mask[next]) {mask[next]=0;queue[write++]=next;}
                }
            }
            if(!fillCount || x1-x0<2*scale || y1-y0<2*scale) continue;
            let borderPixels=0,bx0=width,by0=height,bx1=-1,by1=-1;
            const margin=Math.ceil(3*scale);
            const sides={top:0,bottom:0,left:0,right:0};
            const outlineSides={top:0,bottom:0,left:0,right:0};
            const positions={top:[],bottom:[],left:[],right:[]};
            for(let offset=-margin;offset<=margin;offset++) {
                sides.top=Math.max(sides.top,run(x0,x1,y0+offset,true)/(x1-x0+1));
                sides.bottom=Math.max(sides.bottom,run(x0,x1,y1+offset,true)/(x1-x0+1));
                sides.left=Math.max(sides.left,run(y0,y1,x0+offset,false)/(y1-y0+1));
                sides.right=Math.max(sides.right,run(y0,y1,x1+offset,false)/(y1-y0+1));
                for(const [side,start,end,fixed,horizontal] of [
                    ['top',x0,x1,y0+offset,true],['bottom',x0,x1,y1+offset,true],
                    ['left',y0,y1,x0+offset,false],['right',y0,y1,x1+offset,false]]) {
                    const score=run(start,end,fixed,horizontal,outline)/(end-start+1);
                    outlineSides[side]=Math.max(outlineSides[side],score);
                    if(score>=0.45) positions[side].push(fixed);
                }
            }
            for(let y=Math.max(0,y0-margin);y<=Math.min(height-1,y1+margin);y++)
                for(let x=Math.max(0,x0-margin);x<=Math.min(width-1,x1+margin);x++)
                    if((x<=x0+margin || x>=x1-margin || y<=y0+margin || y>=y1-margin) && accent(x,y)) {
                        borderPixels++;bx0=Math.min(bx0,x);by0=Math.min(by0,y);bx1=Math.max(bx1,x);by1=Math.max(by1,y);
                    }
            const css=(a,b,c,d)=>[(left+a)/scale,(top+b)/scale,(left+c+1)/scale,(top+d+1)/scale];
            const fill=fillCounts.indexOf(Math.max(...fillCounts));
            const regionClipped={left:fx0===0,top:fy0===0,right:fx1===width-1,bottom:fy1===height-1};
            const viewportClipped={left:left+fx0===0,top:top+fy0===0,right:left+fx1===image.width-1,bottom:top+fy1===image.height-1};
            const visibleSides=Object.keys(sides).filter(side=>!regionClipped[side] && sides[side]>=0.45);
            const measurable=write>=30*scale*scale && Math.max(x1-x0+1,bx1-bx0+1)>=10*scale
                && Math.max(y1-y0+1,by1-by0+1)>=8*scale;
            const fourSides=Object.values(outlineSides).every(score=>score>=0.45);
            const ox0=fourSides?Math.min(...positions.left):x0,oy0=fourSides?Math.min(...positions.top):y0;
            const ox1=fourSides?Math.max(...positions.right):x1,oy1=fourSides?Math.max(...positions.bottom):y1;
            // Card outlines have rounded corners. A canvas clip at the straight
            // header separator can mimic four sides, but lacks these cutouts.
            const roundedCorners=fourSides && [[ox0,oy0],[ox1,oy0],[ox0,oy1],[ox1,oy1]].every(([x,y])=>
                x>=0 && x<width && y>=0 && y<height && !outline(x,y) && fillAt[y*width+x]<0);
            const complete=fourSides && roundedCorners;
            const outlineBounds=css(ox0,oy0,ox1,oy1);
            records.push({bounds:css(x0,y0,x1,y1),fill_bounds:css(fx0,fy0,fx1,fy1),outline_bounds:outlineBounds,
                          outer_bounds:borderPixels?css(bx0,by0,bx1,by1):css(x0,y0,x1,y1),
                          kind:kinds[fill],fill_pixels:fillCount,border_pixels:borderPixels,side_scores:sides,
                          outline_side_scores:outlineSides,
                          visible_selection_sides:visibleSides,region_clipped:regionClipped,viewport_clipped:viewportClipped,
                          full_outline_visible:complete,
                          rounded_corners_visible:roundedCorners,
                          potential_canvas_clipping:Object.fromEntries(Object.entries(outlineSides).map(([side,score])=>[side,score<0.45])),
                          measurable});
        }
        records.sort((a,b)=>a.outline_bounds[0]-b.outline_bounds[0] || a.outline_bounds[1]-b.outline_bounds[1]);
        // Letter counters contain small islands of the same background color.
        // They belong to the surrounding card, never to another CFG node.
        const cards=records.filter(record=>record.measurable && !records.some(other=>other!==record
            && other.measurable && other.bounds[0]<=record.bounds[0] && other.bounds[1]<=record.bounds[1]
            && other.bounds[2]>=record.bounds[2] && other.bounds[3]>=record.bounds[3]
            && (other.bounds[2]-other.bounds[0])*(other.bounds[3]-other.bounds[1])
                >(record.bounds[2]-record.bounds[0])*(record.bounds[3]-record.bounds[1])));
        const candidates=cards.filter(record=>record.visible_selection_sides.length>=2);
        const selected=candidates.length===1?candidates[0]:null;
        const digest=await crypto.subtle.digest('SHA-256',pixels);
        return {bounds:selected?.fill_bounds ?? null,selected_pixels:selected?.border_pixels ?? 0,
                selected_kind:selected?.kind ?? null,selected_record:selected ?? null,
                selection_candidates:candidates.length,nodes:measureNodes?cards.map(record=>record.outline_bounds):[],
                node_records:cards,region,viewport:[innerWidth,innerHeight],scale,dividers:[],
                hash:Array.from(new Uint8Array(digest),n=>n.toString(16).padStart(2,'0')).join('')};
    }""", {"png":base64.b64encode(screenshot).decode(),"region":region,"measureNodes":measure_nodes})


def flow_pixels(page, screenshot: bytes, regions, screenshot_box):
    """Locate bright moving arrowheads in real PNG pixels, outside node cards."""
    return page.evaluate("""async ({png, regions, box}) => {
        const image = new Image(); image.src = `data:image/png;base64,${png}`; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width=image.width; canvas.height=image.height;
        const context=canvas.getContext('2d'); context.drawImage(image,0,0);
        const scale=image.width/(box[2]-box[0]);
        const result={};
        for(const [name,region] of Object.entries(regions)) {
            const [left,top,right,bottom]=region.map((v,axis)=>Math.round((v-box[axis%2])*scale));
            const width=right-left,height=bottom-top;
            const pixels=context.getImageData(left,top,width,height).data;
            const mask=new Uint8Array(width*height), queue=new Int32Array(width*height);
            for(let at=0;at<mask.length;at++) {
                const i=at*4;
                // Blue/green moving heads are brighter than their kind-colored
                // base line, labels, spatial annotations, and the dark grid.
                if(pixels[i]>=150 && pixels[i+1]>=185 && pixels[i+2]>=205) mask[at]=1;
            }
            const markers=[];
            for(let start=0;start<mask.length;start++) {
                if(!mask[start]) continue;
                let read=0,write=1,sumX=0,sumY=0;
                queue[0]=start;mask[start]=0;
                while(read<write) {
                    const at=queue[read++],x=at%width,y=Math.floor(at/width);
                    sumX+=x;sumY+=y;
                    for(let dy=-1;dy<=1;dy++) for(let dx=-1;dx<=1;dx++) {
                        const nx=x+dx,ny=y+dy;
                        if(nx<0 || nx>=width || ny<0 || ny>=height) continue;
                        const next=ny*width+nx;
                        if(mask[next]) { mask[next]=0;queue[write++]=next; }
                    }
                }
                if(write>=3*scale*scale) markers.push({
                    center:[box[0]+(left+sumX/write)/scale,box[1]+(top+sumY/write)/scale],pixels:write});
            }
            const digest=await crypto.subtle.digest('SHA-256',pixels);
            result[name]={markers,bounds:region,hash:Array.from(new Uint8Array(digest),n=>n.toString(16).padStart(2,'0')).join('')};
        }
        return result;
    }""", {"png":base64.b64encode(screenshot).decode(),"regions":regions,"box":screenshot_box})


def flow_frames(page, canvas, output: Path, name: str, regions):
    captures=[]
    # Crop the actual browser capture, rather than downsampling a full PNG.
    # Full SwiftShader screenshots can take half a second: that would alias a
    # periodic moving head and make forward movement look like wraparound.
    box=[math.floor(min(region[0] for region in regions.values())),
         math.floor(min(region[1] for region in regions.values())),
         math.ceil(max(region[2] for region in regions.values())),
         math.ceil(max(region[3] for region in regions.values()))]
    canvas.screenshot(path=str(output/f"{name}-scene.png"))
    start=time.monotonic()
    max_interval=min(56,min(region[3]-region[1] for region in regions.values()))/(2*60)
    valid_pairs=0
    # Capture consecutive real compositor frames first. PNG decoding, pixel
    # extraction and browser round trips belong after sampling, otherwise
    # they can consume the entire conservative half-cycle timing allowance.
    # Capture the real Chromium surface directly. Playwright's screenshot
    # preparation can otherwise spend the half-cycle allowance on frame waits.
    # Retain the same conservative wall-clock bounds and lossless PNG evidence.
    cdp=page.context.new_cdp_session(page)
    try:
        for index in range(24):
            capture_begin=time.monotonic()-start
            captured=cdp.send("Page.captureScreenshot",{
                "format":"png","fromSurface":True,"captureBeyondViewport":False,
                "clip":{"x":box[0],"y":box[1],"width":box[2]-box[0],"height":box[3]-box[1],"scale":1}})
            capture_end=time.monotonic()-start
            png=base64.b64decode(captured["data"])
            (output/f"{name}-{index:02}.png").write_bytes(png)
            if captures and capture_end-captures[-1][0]["capture_begin_seconds"]<max_interval:
                valid_pairs+=1
            captures.append(({"capture_begin_seconds":capture_begin,"capture_end_seconds":capture_end,"screenshot_box":box},png))
            # Six timing-valid pairs provide several opportunities to see a head
            # fully inside the gap. Direction still requires its own two different
            # advancing frame pairs below; timing or presence alone cannot pass it.
            if len(captures)>=10 and valid_pairs>=6: break
    finally:
        cdp.detach()
    frames=[]
    for frame,png in captures:
        frame["regions"]=flow_pixels(page,png,regions,box)
        frames.append(frame)
    return frames


def assert_flow_direction(frames, region: str, direction):
    """Track nearby heads between actual frames; wraparound starts a new head."""
    advances=[]
    backwards=[]
    advanced_pairs=[]
    skipped_pairs=[]
    height=frames[0]["regions"][region]["bounds"][3]-frames[0]["regions"][region]["bounds"][1]
    # Normal routes move at 60 CSS pixels/s. A visible gap is no longer than
    # its route, so this conservative half-cycle limit prevents a reversed
    # periodic head from wrapping forward under a heavily loaded compositor.
    max_interval=min(56,height)/(2*60)
    for index,(before,after) in enumerate(zip(frames,frames[1:])):
        interval=after["capture_end_seconds"]-before["capture_begin_seconds"]
        if interval>=max_interval:
            skipped_pairs.append({"index":index,"capture_interval_upper_bound":interval})
            continue
        old=before["regions"][region]["markers"]
        new=after["regions"][region]["markers"]
        advanced=False
        for marker in old:
            candidates=[next_marker for next_marker in new
                        if 1.5<math.dist(marker["center"],next_marker["center"])<25]
            if not candidates: continue
            nearest=min(candidates,key=lambda candidate:math.dist(marker["center"],candidate["center"]))
            delta=[b-a for a,b in zip(marker["center"],nearest["center"],strict=True)]
            along=sum(a*b for a,b in zip(delta,direction,strict=True))
            across=abs(delta[0]*direction[1]-delta[1]*direction[0])
            if across>2.0: continue
            if along>1.5:
                advances.append(along);advanced=True
            elif along<-1.5: backwards.append(along)
        if advanced: advanced_pairs.append(index)
    assert len(advanced_pairs)>=2 and len(backwards)<=len(advances)//3, (
        f"rendered heads did not move source to target in {region}: "
        f"forward={advances}, backward={backwards}, frames={frames}")
    return {"screen_direction":direction,"forward_steps":advances,"backward_steps":backwards,
            "advanced_pairs":advanced_pairs,"max_capture_interval":max_interval,"skipped_pairs":skipped_pairs}


def vertical_edge_region(source, target):
    assert source[3]+8<target[1], (source,target)
    return (max(source[0],target[0]),source[3]+4,
            min(source[2],target[2]),target[1]-4)


def loop_return_region(page, png, cards):
    """Find the upward return using its static target arrow at the upper card."""
    return page.evaluate("""async ({png,cards}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        const scale=image.width/innerWidth,left=Math.floor(Math.min(...cards.map(card=>card[0]))*scale);
        const top=Math.floor(cards[0][3]*scale),bottom=Math.ceil(cards[1][1]*scale);
        const width=Math.ceil(Math.max(...cards.map(card=>card[2]))*scale)-left,height=bottom-top;
        const data=context.getImageData(left,top,width,height).data;
        const columns=Array.from({length:width},()=>({count:0,first:height,last:0}));
        const mask=new Uint8Array(width*height);
        for(let y=0;y<height;y++) for(let x=0;x<width;x++) {
            const i=(y*width+x)*4,r=data[i],g=data[i+1],b=data[i+2];
            // The destination arrow retains the green kind color; moving
            // heads are brighter and must not affect destination detection.
            if(r<150 && g>=150 && g-r>=50 && b>=130) {
                mask[y*width+x]=1;
                const column=columns[x];column.count++;column.first=Math.min(column.first,y);column.last=Math.max(column.last,y);
            }
        }
        const targetScore=x=>{
            let score=0;
            for(let y=0;y<Math.min(height,10*scale);y++) for(let dx=-5*scale;dx<=5*scale;dx++) {
                if(Math.abs(dx)<=1.5*scale || x+dx<0 || x+dx>=width) continue;
                score+=mask[y*width+x+dx];
            }
            return score;
        };
        const candidates=columns.map((column,x)=>({x,score:targetScore(x),count:column.count}))
            .filter(column=>column.count>=20*scale);
        candidates.sort((a,b)=>b.score-a.score || b.count-a.count);
        const x=candidates[0]?.x ?? 0;
        const leg=columns[x];
        return {pixels:leg.count,target_arrow_pixels:targetScore(x),
                region:[(left+x)/scale-9,(top+leg.first)/scale+9,
                        (left+x)/scale+9,(top+leg.last)/scale-4]};
    }""", {"png":base64.b64encode(png).decode(),"cards":cards})


def edge_flow_interactions(browser, url: str, output: Path):
    page,canvas,errors=layout_page(browser,url)
    code,metadata=layout_fixture("linear-chain")
    report=layout_submission(page,canvas,code,output,"flow-chain")
    assert report["status"]=="Converged" and len(report["cfg"])==metadata["expected_native_states"]
    graph_mode(page,canvas,"States")
    page.keyboard.press("f");settle_gesture(page);page.mouse.move(2,2)
    scene=node_snapshot(page,canvas,output,"flow-chain-before",measure_nodes=True)
    cards=sorted(scene["nodes"],key=lambda card:card[1])
    assert len(cards)==4, cards
    # Match the rendered downward chain to the actual typed graph, rather than
    # assuming state IDs or substituting hand-authored routes for ELK's output.
    current=next(node["id"] for node in report["cfg"]
                 if not any(edge["to"]==node["id"] for edge in report["edges"]))
    states=[current]
    while outgoing:=[edge for edge in report["edges"] if edge["from"]==current]:
        assert len(outgoing)==1
        current=outgoing[0]["to"];states.append(current)
    assert len(states)==len(cards)
    requests=[]
    page.on("request",lambda request:requests.append(request.url)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    def select(index):
        card=cards[index]
        canvas.click(position={"x":(card[0]+card[2])/2,"y":(card[1]+card[3])/2})
        page.mouse.move(2,2);settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ',states[index])};")
    select(1)
    regions={"incoming":vertical_edge_region(cards[0],cards[1]),
             "outgoing":vertical_edge_region(cards[1],cards[2])}
    incident=flow_frames(page,canvas,output,"flow-chain-selected-middle",regions)
    incoming=assert_flow_direction(incident,"incoming",[0,1])
    outgoing=assert_flow_direction(incident,"outgoing",[0,1])
    select(3)
    changed=flow_frames(page,canvas,output,"flow-chain-selected-last",
                        {**regions,"new_incoming":vertical_edge_region(cards[2],cards[3])})
    for name in regions:
        assert len({frame["regions"][name]["hash"] for frame in changed})==1, f"old {name} edge kept animating"
        assert all(not frame["regions"][name]["markers"] for frame in changed), f"old {name} edge remained highlighted"
    new_incoming=assert_flow_direction(changed,"new_incoming",[0,1])
    before=node_snapshot(page,canvas,output,"flow-chain-before-zoom",measure_nodes=True)
    pointer=(720,500)
    page.mouse.move(*pointer);page.keyboard.down("Control");page.mouse.wheel(0,35);page.keyboard.up("Control")
    page.mouse.move(2,2);settle_gesture(page)
    zoomed=node_snapshot(page,canvas,output,"flow-chain-zoomed",measure_nodes=True)
    camera=assert_layout_same_geometry(before,zoomed)
    assert camera["camera_scale"]<0.95, camera
    resize_view(page,1600,1100)
    resized=node_snapshot(page,canvas,output,"flow-chain-resized",(4,123,1596,1070),True)
    assert_translation(zoomed,resized,0,0,tolerance=1.0)
    assert abs(assert_layout_same_geometry(zoomed,resized)["camera_scale"]-1)<0.01
    zoom_cards=sorted(resized["nodes"],key=lambda card:card[1])
    zoom_frames=flow_frames(page,canvas,output,"flow-chain-zoom-resize",
                            {"new_incoming":vertical_edge_region(zoom_cards[2],zoom_cards[3])})
    zoom_flow=assert_flow_direction(zoom_frames,"new_incoming",[0,1])
    assert not requests, "selection, animation, zoom or resize reran analysis"
    resize_view(page,1440,1000)
    loop=layout_submission(page,canvas,"5b6004565b600056",output,"flow-loop")
    assert loop["status"]=="Converged" and len(loop["edges"])>=128
    assert {node["start_pc"] for node in loop["cfg"]}=={0,4}
    pcs={node["id"]:node["start_pc"] for node in loop["cfg"]}
    assert {(pcs[edge["from"]],pcs[edge["to"]]) for edge in loop["edges"]}=={(0,4),(4,0)}
    graph_mode(page,canvas,"Blocks");page.keyboard.press("f");settle_gesture(page);page.mouse.move(2,2)
    card=node_snapshot(page,canvas,output,"flow-loop-before",measure_nodes=True)
    assert len(card["nodes"])==2,card
    loop_cards=sorted(card["nodes"],key=lambda bounds:bounds[1])
    selected=next(node for node in loop["cfg"]
                  if f"Selected {symbol('σ',node['id'])};" in page.locator("#analysis-status").text_content())
    selected_index=next(index for index,bounds in enumerate(loop_cards)
                        if bounds[0]<=card["bounds"][0] and bounds[1]<=card["bounds"][1]
                        and bounds[2]>=card["bounds"][2] and bounds[3]>=card["bounds"][3])
    # Either cycle member may be above the other after ELK cycle breaking.
    # Resolve the upper/lower PCs from the actual selected card, then verify
    # the typed edge really returns from the lower source to the upper target.
    upper_pc=selected["start_pc"] if selected_index==0 else 4-selected["start_pc"]
    lower_pc=4-upper_pc
    assert any(pcs[edge["from"]]==lower_pc and pcs[edge["to"]]==upper_pc for edge in loop["edges"])
    leg=loop_return_region(page,canvas.screenshot(),loop_cards)
    assert leg["pixels"]>30 and leg["target_arrow_pixels"]>=2 and leg["region"][3]-leg["region"][1]>25,leg
    returning=flow_frames(page,canvas,output,"flow-loop-return",{"loop_return":leg["region"]})
    loop_flow=assert_flow_direction(returning,"loop_return",[0,-1])
    loop_flow.update(source_pc=lower_pc,target_pc=upper_pc,target_arrow_pixels=leg["target_arrow_pixels"])
    assert len(requests)==1, "loop animation or graph scope reran analysis"
    assert not errors,errors
    result={"chain_native_states":states,"selected_middle":states[1],"incoming":incoming,"outgoing":outgoing,
            "selected_last":states[3],"old_edges_stop":True,"new_incoming":new_incoming,
            "camera":camera,"zoom_resize_flow":zoom_flow,"loop_return":loop_flow,
            "loop_original_edges":len(loop["edges"]),"incident_frames":incident,
            "selection_frames":changed,"zoom_resize_frames":zoom_frames,"loop_frames":returning,
            "renderer":webgpu_evidence(page)}
    (output/"edge-flow-report.json").write_text(json.dumps(result,indent=2)+"\n")
    page.close()
    return result


def compact_ssa_interactions(browser, url: str, output: Path):
    page=browser.new_page(viewport={"width":1440,"height":1000});observe_webgpu(page)
    page.goto(url,wait_until="networkidle")
    expect(page.locator("#analysis-status")).to_contain_text("Ready:",timeout=60000)
    canvas=page.locator("#evm-canvas");hide_inspector(page,canvas)
    requests=[]
    page.on("request",lambda request:requests.append(request.url)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    results={}
    for width,name,ssa_x in [(1024,"medium",118),(640,"narrow",198)]:
        resize_view(page,width,800);page.keyboard.press("0");page.mouse.move(2,2);settle_gesture(page)
        region=(8,100,172,740)
        def source_hash(label):
            page.mouse.move(2,2);page.wait_for_timeout(800);rendered_frame(page)
            png=canvas.screenshot(path=str(output/f"workspace-{name}-{label}.png"))
            metrics=screenshot_metrics(page,png,region)
            assert metrics["selected_pixels"]>100, f"missing selected source block: {name}/{label}"
            return metrics["hash"]
        initial=source_hash("initial-ssa")
        canvas.click(position={"x":ssa_x,"y":65});settle_gesture(page)
        explicit_ssa=source_hash("explicit-ssa")
        assert initial==explicit_ssa, f"{name} workspace did not initially open SSA"
        canvas.click(position={"x":52,"y":65});settle_gesture(page)
        disasm=source_hash("disassembly")
        assert disasm!=explicit_ssa, f"{name} Disassembly tab did not show different source content"
        canvas.click(position={"x":ssa_x,"y":65});settle_gesture(page)
        assert source_hash("ssa-return")==initial, f"{name} SSA tab did not restore its initial source content"
        canvas.click(position={"x":52,"y":65});settle_gesture(page)
        assert source_hash("disassembly-return")==disasm, f"{name} Disassembly tab did not restore source content"
        page.keyboard.press("2");settle_gesture(page);page.keyboard.press("0");settle_gesture(page)
        assert source_hash("view-return-disassembly")==disasm, f"{name} tab choice was lost across views"
        resize_view(page,1440,1000);settle_gesture(page);resize_view(page,width,800);settle_gesture(page)
        assert source_hash("resize-return-disassembly")==disasm, f"{name} tab choice was lost across width classes"
        results[name]={"initial_is_ssa":True,"ssa_hash":explicit_ssa,"disassembly_hash":disasm,
                       "preserves_manual_tab_across_views_and_widths":True}
    resize_view(page,1024,800);settle_gesture(page)
    returned=screenshot_metrics(page,canvas.screenshot(path=str(output/"workspace-medium-after-narrow.png")),(8,100,172,740))
    assert returned["hash"]==results["medium"]["disassembly_hash"], "narrow workspace overwrote the medium tab preference"
    assert not requests,"compact source tabs or workspace resizing reran analysis"
    results["renderer"]=webgpu_evidence(page)
    (output/"compact-ssa-report.json").write_text(json.dumps(results,indent=2)+"\n")
    page.close()
    return results


def diagnostics_interactions(browser, url: str, args):
    """Real native receipts survive HTTP/WASM and retain independent source jumps."""
    output = args.output
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors, posts = [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    complete_submission(page, initial.value)
    canvas, status = page.locator("#evm-canvas"), page.locator("#analysis-status")
    select_inspector_tab(page, canvas, "Diagnostics")

    def submit(code, fork, name):
        open_input(page)
        page.keyboard.press("Control+A")
        page.keyboard.insert_text(code)
        rendered_frame(page)
        modal = layout_modal(page, canvas)
        # The actual fork selector sits beside Bytecode and RPC account.
        canvas.click(position={"x": modal[0] + 220, "y": modal[1] + 44})
        settle_gesture(page)
        canvas.click(position={"x": modal[0] + 220,
                               "y": modal[1] + 70 + {"Cancun": 0, "Prague": 1, "Osaka": 2}[fork] * 23})
        settle_gesture(page)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                  and response.request.method == "POST") as submitted:
            page.keyboard.press("Control+Enter")
        reply = complete_submission(page, submitted.value)
        request = reply.request.post_data_json
        assert request["fork"] == fork and posted_bytecode(reply) == code, request
        assert reply.status == 200 and "Ok" in reply.json()["result"], reply.json()
        report = reply.json()["result"]["Ok"]
        assert report["schema_version"] == 12 and report["status"] == "Converged", report
        (output / f"diagnostics-{name}-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
        return report

    def snapshot(name, minimum_rows):
        page.mouse.move(2, 2)
        settle_gesture(page)
        png = canvas.screenshot(path=str(output / f"diagnostics-{name}.png"))
        # Measure the painted summary, state/opcode receipt and explanatory cause.
        pixels = text_pixels(page, png, (8, 794, 1410, 970))
        assert len(pixels["rows"]) >= minimum_rows and pixels["white_pixels"] > 50, pixels
        return pixels

    result = {}
    for name, code, fork, opcode, mnemonic, reason, cause in [
        ("explicit", "fe", "Osaka", 0xfe, "INVALID", "ExplicitInvalid", "Explicit exceptional halt instruction"),
        ("undefined", "ef", "Osaka", 0xef, None, "UndefinedOpcode", "Undefined ordinary bytecode opcode"),
        ("clz-cancun", "5f1e00", "Cancun", 0x1e, "CLZ", {"UnavailableInFork": "Cancun"}, "Instruction is not enabled in fork Cancun"),
    ]:
        report = submit(code, fork, name)
        diagnostics = [item for item in report["diagnostics"] if item["kind"] == "InvalidOpcode"]
        assert len(diagnostics) == 1, diagnostics
        diagnostic = diagnostics[0]
        assert diagnostic["invalid_opcode"] == {"Instruction": {"opcode": opcode, "mnemonic": mnemonic, "reason": reason}}, diagnostic
        label = f"{symbol('σ', diagnostic['state'])} · PC 0x{diagnostic['pc']:04x} · InvalidOpcode · 0x{opcode:02x}"
        if mnemonic is not None:
            label += f" ({mnemonic})"
        expect(status).to_contain_text(label)
        expect(status).to_contain_text(cause)
        expect(status).to_contain_text("counts are abstract-state entries, not instructions or transactions")
        if mnemonic is None:
            assert "0xef (INVALID)" not in status.text_content()
        pixels = snapshot(name, 3)
        # Receipt rows remain real links to the native state/source location.
        row = pixels["rows"][1]
        submissions = len(posts)
        canvas.click(position={"x": 120, "y": sum(row) / 2})
        expect(status).to_contain_text(f"Selected {symbol('σ', diagnostic['state'])};")
        assert len(posts) == submissions, "diagnostic selection submitted another analysis"
        result[name] = {"diagnostic": diagnostic, "painted": pixels}
    assert len({case["painted"]["hash"] for case in result.values()}) == 3

    for name, code in [("clz-osaka", "5f1e00"), ("unreachable", "00fe")]:
        report = submit(code, "Osaka", name)
        assert not any(item["kind"] == "InvalidOpcode" for item in report["diagnostics"]), report["diagnostics"]
        expect(status).to_contain_text("0 abstract-state diagnostic entries")
        if name == "unreachable":
            assert any(instruction["opcode"] == 0xfe for block in report["disassembly"] for instruction in block["instructions"])
        result[name] = {"status": report["status"], "diagnostics": report["diagnostics"]}

    repeated = submit("5f35600a5760016010565b60026010565bfe", "Osaka", "repeated-pc")
    receipts = [item for item in repeated["diagnostics"] if item["kind"] == "InvalidOpcode"]
    assert len(receipts) == 2 and len({item["state"] for item in receipts}) == 2, receipts
    assert {item["pc"] for item in receipts} == {17}, receipts
    expect(status).to_contain_text("2 abstract-state diagnostic entries")
    pixels = snapshot("repeated-pc", 5)
    for index, receipt in enumerate(receipts):
        expect(status).to_contain_text(f"{symbol('σ', receipt['state'])} · PC 0x0011")
        row = pixels["rows"][1 + index * 2]
        submissions = len(posts)
        canvas.click(position={"x": 120, "y": sum(row) / 2})
        expect(status).to_contain_text(f"Selected {symbol('σ', receipt['state'])};")
        assert len(posts) == submissions
    result["repeated-pc"] = {"diagnostics": receipts, "painted": pixels, "all_state_links_work": True}
    assert not errors, errors
    result["renderer"] = webgpu_evidence(page)
    page.close()

    class InvalidChildRpc(EntryRpcFixture):
        root_code = "0x5f5f5f5f5f73" + "22" * 20 + "61fffff100"
        child_code = "0xfe"

    with InvalidChildRpc() as rpc, TemporaryDirectory(prefix="diagnostics-rpc-") as directory:
        provider = {"id": "invalid-child", "name": "Invalid child fixture", "endpoint": rpc.endpoint}
        config = Path(directory) / "providers.json"
        config.write_text(json.dumps({"providers": [provider]}) + "\n")
        with analysis_server(args, config) as rpc_url:
            child_page = browser.new_page(viewport={"width": 1440, "height": 1000})
            observe_webgpu(child_page)
            automatic_rpc_entry(child_page, rpc_url, provider, output, "diagnostics-child-startup")
            child_canvas = child_page.locator("#evm-canvas")
            admitted = submit_rpc(child_page, child_canvas, rpc, output, "diagnostics-child", 0, [provider])
            reply = complete_submission(child_page, admitted)
            report = reply.json()["result"]["Ok"]
            diagnostic = next(item for item in report["diagnostics"] if item["kind"] == "InvalidOpcode")
            node = next(item for item in report["cfg"] if item["id"] == diagnostic["state"])
            source = next(item for item in report["programs"] if item["id"] == node["program"])
            root = next(item for item in report["programs"] if item["id"] == report["metadata"]["root_program"])
            assert diagnostic["pc"] == 0 and source["code_address"] == rpc.child_address
            assert bytes.fromhex(root["bytecode"].removeprefix("0x"))[0] == 0x5f
            assert diagnostic["invalid_opcode"]["Instruction"]["opcode"] == 0xfe
            select_inspector_tab(child_page, child_canvas, "Diagnostics")
            child_status = child_page.locator("#analysis-status")
            expect(child_status).to_contain_text("0xfe (INVALID)")
            child_page.mouse.move(2, 2)
            settle_gesture(child_page)
            png = child_canvas.screenshot(path=str(output / "diagnostics-child.png"))
            pixels = text_pixels(child_page, png, (8, 794, 1410, 970))
            assert len(pixels["rows"]) >= 3, pixels
            row = pixels["rows"][1]
            child_canvas.click(position={"x": 120, "y": sum(row) / 2})
            expect(child_status).to_contain_text(f"Selected {symbol('σ', diagnostic['state'])}; source {symbol('P', source['id'])};")
            result["child-source"] = {"diagnostic": diagnostic, "program": source["id"], "root_pc0_opcode": 0x5f, "correct_source_jump": True}
            (output / "diagnostics-child-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
            child_page.close()
    (output / "diagnostics-report.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


SSA_DETAILS_CODE = "60036007819001905060005200"


def text_pixels(page, screenshot: bytes, region):
    """Count actual painted text bands and semantic colors in a PNG region."""
    return page.evaluate("""async ({png,region}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        const scale=image.width/innerWidth;
        const [left,top,right,bottom]=region.map(value=>Math.round(value*scale));
        const width=right-left,height=bottom-top;
        const data=context.getImageData(left,top,width,height).data;
        const counts=new Uint32Array(height);
        let white=0,muted=0,red=0;
        for(let y=0;y<height;y++) for(let x=0;x<width;x++) {
            const i=(y*width+x)*4,r=data[i],g=data[i+1],b=data[i+2];
            if(Math.max(r,g,b)>=110) counts[y]++;
            if(r>180 && g>190 && b>205) white++;
            if(Math.abs(r-145)<=15 && Math.abs(g-164)<=15 && Math.abs(b-186)<=15) muted++;
            if(r>=180 && r-g>=65 && r-b>=50) red++;
        }
        const rows=[];
        for(let y=0;y<height;y++) {
            // Thin source dividers and selected-row accent strokes are not text.
            if(counts[y]<8*scale) continue;
            const at=(top+y)/scale;
            if(rows.length && at-rows[rows.length-1][1]<=4) rows[rows.length-1][1]=at+1/scale;
            else rows.push([at,at+1/scale]);
        }
        const digest=await crypto.subtle.digest('SHA-256',data);
        return {rows,white_pixels:white,muted_pixels:muted,error_pixels:red,
                hash:Array.from(new Uint8Array(digest),n=>n.toString(16).padStart(2,'0')).join('')};
    }""", {"png":base64.b64encode(screenshot).decode(),"region":region})


def save_png_crop(page, screenshot: bytes, region, path: Path):
    """Save the same screenshot crop used by the pixel assertions for review."""
    encoded = page.evaluate("""async ({png,region}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const scale=image.width/innerWidth;
        const [left,top,right,bottom]=region.map(value=>Math.round(value*scale));
        const crop=document.createElement('canvas');crop.width=right-left;crop.height=bottom-top;
        crop.getContext('2d').drawImage(image,left,top,crop.width,crop.height,0,0,crop.width,crop.height);
        return crop.toDataURL('image/png').split(',')[1];
    }""", {"png":base64.b64encode(screenshot).decode(),"region":region})
    path.write_bytes(base64.b64decode(encoded))


def ssa_text_snapshot(page, canvas, output, name):
    page.mouse.move(2,2);page.wait_for_timeout(800);rendered_frame(page)
    png=canvas.screenshot(path=str(output/f"{name}.png"))
    size=page.viewport_size
    selected=screenshot_metrics(page,png,(4,75,size["width"]-4,size["height"]-30))
    assert selected["bounds"], f"missing selected native SSA header: {name}"
    # The selected state header supplies the actual start of the virtual table,
    # independently of the global metadata bar's height or responsive layout.
    region=(4,selected["bounds"][1],size["width"]-4,size["height"]-30)
    return {**text_pixels(page,png,region),"selected":selected,"region":region}


def details_graph_snapshot(page, canvas, output, name):
    page.mouse.move(2,2);settle_gesture(page)
    png=canvas.screenshot(path=str(output/f"{name}.png"))
    size=page.viewport_size
    node=graph_screenshot_metrics(page,png,(4,123,size["width"]-4,size["height"]-30),True)
    assert node["bounds"] and len(node["nodes"])==1,node
    left,top,right,bottom=node["bounds"]
    # Exclude the selected card's border, title and source-location header.
    body=text_pixels(page,png,(left+6,top+34,right-6,bottom-6))
    return {"node":node,"body":body}


def ssa_details_toggle(page, canvas, mode):
    canvas.click(position={"x":page.viewport_size["width"]-45,"y":32})
    page.mouse.move(2,2);settle_gesture(page)
    expect(page.locator("#analysis-status")).to_contain_text(f"SSA details: {mode}")


def ssa_details_interactions(browser, url: str, output: Path):
    page,canvas,errors=layout_page(browser,url)
    status=page.locator("#analysis-status")
    expect(status).to_contain_text("SSA details: Compact")
    requests=[]
    page.on("request",lambda request:requests.append(request.post_data_json)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    report=layout_submission(page,canvas,SSA_DETAILS_CODE,output,"ssa-details-core")
    assert report["status"]=="Converged" and report["ssa"]["complete"]
    assert len(report["cfg"])==len(report["ssa"]["blocks"])==1
    block=report["ssa"]["blocks"][0]
    instructions=block["instructions"]
    assert [(item["pc"],item["name"]) for item in instructions]==[
        (0,"PUSH1"),(2,"PUSH1"),(4,"DUP2"),(5,"SWAP1"),(6,"ADD"),
        (7,"SWAP1"),(8,"POP"),(9,"PUSH1"),(11,"MSTORE"),(12,"STOP")]
    assert not block["phis"]
    by_pc={item["pc"]:item for item in instructions}
    hidden_pcs=[4,5,7,8]
    assert all(by_pc[pc]["progress"]=="Completed" and not by_pc[pc]["fault"] for pc in hidden_pcs)
    # Bookkeeping retains the actual original SSA identities. MSTORE still
    # consumes the offset definition and the ADD result after the hidden ops.
    assert by_pc[4]["operands"]==by_pc[0]["results"] and not by_pc[4]["results"]
    assert by_pc[8]["operands"]==by_pc[0]["results"]
    assert by_pc[11]["operands"]==by_pc[9]["results"]+by_pc[6]["results"]
    graph_mode(page,canvas,"States");page.keyboard.press("f");settle_gesture(page)
    compact_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-compact")
    expect(status).to_contain_text("CFG detail: SSA preview")
    page.keyboard.press("3");settle_gesture(page)
    compact=ssa_text_snapshot(page,canvas,output,"ssa-details-pane-compact")
    assert len(compact["rows"])==9,compact
    ssa_details_toggle(page,canvas,"Full")
    full=ssa_text_snapshot(page,canvas,output,"ssa-details-pane-full")
    assert len(full["rows"])==13,full
    assert full["muted_pixels"]>compact["muted_pixels"]*1.4, "ordinary μ/completion details did not return"
    page.keyboard.press("2");settle_gesture(page)
    full_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-full")
    assert_translation(compact_graph["node"],full_graph["node"],0,0,tolerance=1.0)
    assert full_graph["body"]["hash"]!=compact_graph["body"]["hash"]
    assert full_graph["body"]["white_pixels"]>compact_graph["body"]["white_pixels"]*1.2
    ssa_details_toggle(page,canvas,"Compact")
    restored_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-restored")
    assert restored_graph["body"]["hash"]==compact_graph["body"]["hash"]
    assert_translation(compact_graph["node"],restored_graph["node"],0,0,tolerance=1.0)

    # Click the real DUP2 PC in the independent disassembly table. Its selected
    # block row and native 22pt row stride locate the control from PNG evidence.
    page.keyboard.press("1");settle_gesture(page);page.mouse.move(2,2)
    source_png=canvas.screenshot(path=str(output/"ssa-details-disassembly-before.png"))
    source=screenshot_metrics(page,source_png,(4,75,1436,970))
    assert source["bounds"],source
    source_order=report["programs"][0]["blocks"][0]["instructions"]
    index=next(index for index,item in enumerate(source_order) if item["pc"]==4)
    clicked_y=source["bounds"][3]+22*(index+0.5)
    canvas.click(position={"x":80,"y":clicked_y});page.mouse.move(2,2);settle_gesture(page)
    selected_source=screenshot_metrics(page,canvas.screenshot(path=str(output/"ssa-details-disassembly-pc4.png")),(4,75,1436,970))
    assert selected_source["selected_pixels"]>source["selected_pixels"]*1.8
    assert abs(selected_source["bounds"][3]-(clicked_y+11))<=2, (source,selected_source,clicked_y)
    expect(status).to_contain_text(f"Selected {symbol('σ',block['state'])};")
    page.keyboard.press("3");settle_gesture(page)
    revealed=ssa_text_snapshot(page,canvas,output,"ssa-details-pane-pc4-revealed")
    expect(status).to_contain_text("SSA details: Compact")
    assert len(revealed["rows"])==10,revealed
    assert revealed["selected"]["selected_pixels"]>compact["selected"]["selected_pixels"]*1.8
    page.keyboard.press("2");settle_gesture(page)
    revealed_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-pc4-revealed")
    assert revealed_graph["body"]["hash"]!=compact_graph["body"]["hash"]
    assert_translation(compact_graph["node"],revealed_graph["node"],0,0,tolerance=1.0)
    page.keyboard.press("3");settle_gesture(page)
    # The SSA header clears only the selected PC. The temporary DUP2 row must
    # disappear from both views while the global mode remains Compact.
    canvas.click(position={"x":30,"y":revealed["selected"]["bounds"][1]+11})
    page.mouse.move(2,2);settle_gesture(page)
    cleared=ssa_text_snapshot(page,canvas,output,"ssa-details-pane-pc-cleared")
    assert len(cleared["rows"])==9 and cleared["hash"]==compact["hash"],cleared
    page.keyboard.press("2");settle_gesture(page)
    cleared_graph=details_graph_snapshot(page,canvas,output,"ssa-details-graph-pc-cleared")
    assert cleared_graph["body"]["hash"]==compact_graph["body"]["hash"]
    assert len(requests)==1, "reading mode or linked PC selection reran analysis"

    ssa_details_toggle(page,canvas,"Full")
    for key in ["1","0","2","3"]:
        page.keyboard.press(key);settle_gesture(page)
        expect(status).to_contain_text("SSA details: Full")
    resize_view(page,640,800);settle_gesture(page)
    narrow_full=ssa_text_snapshot(page,canvas,output,"ssa-details-narrow-full")
    assert len(narrow_full["rows"])==13,narrow_full
    ssa_details_toggle(page,canvas,"Compact")
    narrow_compact=ssa_text_snapshot(page,canvas,output,"ssa-details-narrow-compact")
    assert len(narrow_compact["rows"])==9,narrow_compact
    ssa_details_toggle(page,canvas,"Full")
    resize_view(page,1440,1000);settle_gesture(page)
    assert len(requests)==1, "responsive detail controls submitted another analysis"
    repeated=layout_submission(page,canvas,SSA_DETAILS_CODE,output,"ssa-details-reanalysis")
    assert repeated["ssa"]["blocks"][0]["instructions"]==instructions
    expect(status).to_contain_text("SSA details: Full")
    after_reanalysis=ssa_text_snapshot(page,canvas,output,"ssa-details-full-after-reanalysis")
    assert len(after_reanalysis["rows"])==13,after_reanalysis

    # Two cheap real safety examples complement native filtering checks:
    # JUMPDEST remains visible, and an underflowing DUP is not bookkeeping.
    ssa_details_toggle(page,canvas,"Compact")
    jumpdest=layout_submission(page,canvas,"5b600160020100",output,"ssa-details-jumpdest")
    jump_ops=jumpdest["ssa"]["blocks"][0]["instructions"]
    assert jump_ops[0]["name"]=="JUMPDEST" and jump_ops[0]["progress"]=="Completed"
    jump_pixels=ssa_text_snapshot(page,canvas,output,"ssa-details-jumpdest-compact")
    assert len(jump_pixels["rows"])==len(jump_ops)+3,jump_pixels
    fault=layout_submission(page,canvas,"80",output,"ssa-details-faulted-dup")
    fault_ops=fault["ssa"]["blocks"][0]["instructions"]
    assert len(fault_ops)==1 and fault_ops[0]["name"]=="DUP1" and fault_ops[0]["fault"]
    assert fault_ops[0]["progress"]=="Faulted"
    fault_compact=ssa_text_snapshot(page,canvas,output,"ssa-details-fault-pane-compact")
    assert len(fault_compact["rows"])==4 and fault_compact["error_pixels"]>20,fault_compact
    page.keyboard.press("2");settle_gesture(page);page.keyboard.press("f");settle_gesture(page)
    fault_graph=details_graph_snapshot(page,canvas,output,"ssa-details-fault-graph-compact")
    assert fault_graph["body"]["error_pixels"]>20,fault_graph
    ssa_details_toggle(page,canvas,"Full")
    fault_graph_full=details_graph_snapshot(page,canvas,output,"ssa-details-fault-graph-full")
    assert fault_graph_full["body"]["hash"]==fault_graph["body"]["hash"]
    page.keyboard.press("3");settle_gesture(page)
    fault_full=ssa_text_snapshot(page,canvas,output,"ssa-details-fault-pane-full")
    assert fault_full["hash"]==fault_compact["hash"]
    assert len(requests)==4, "SSA detail display created an extra backend task"
    assert not errors,errors
    result={"fixture":SSA_DETAILS_CODE,"hidden_completed_pcs":hidden_pcs,"instructions":instructions,
            "compact":compact,"full":full,"compact_graph":compact_graph,"full_graph":full_graph,
            "revealed":revealed,"revealed_graph":revealed_graph,"cleared":cleared,
            "source_selection":{"pc":4,"before":source,"after":selected_source},
            "narrow_compact":narrow_compact,"narrow_full":narrow_full,
            "full_after_reanalysis":after_reanalysis,"jumpdest":jump_pixels,
            "fault_compact":fault_compact,"fault_full":fault_full,"fault_graph":fault_graph,
            "analysis_submissions":len(requests),"renderer":webgpu_evidence(page)}
    (output/"ssa-details-report.json").write_text(json.dumps(result,indent=2)+"\n")
    page.close()
    return result


def selected_ending(page):
    status=page.locator("#analysis-status").text_content()
    match=re.search(r"Selected CFG ending: (.+?); (\d+) native members; root normal (\d+), REVERT (\d+), exception (\d+); frame normal (\d+), REVERT (\d+), exception (\d+); continuing (\d+), unresolved (\d+), non-current (\d+), unobserved (\d+)",status)
    assert match,status
    keys=['members','root_normal','root_revert','root_failure','frame_normal','frame_revert','frame_failure','continuing','unresolved','non_current','unobserved']
    return {'kind':match[1],**dict(zip(keys,map(int,match.groups()[1:]),strict=True))}


def ssa_definition_tokens(page, png, top):
    """Locate real purple definition tokens; ordinary operand links are blue."""
    return page.evaluate("""async ({png,top}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        const scale=image.width/innerWidth,left=Math.round(4*scale),right=Math.round(350*scale);
        const first=Math.round(top*scale),last=Math.round((innerHeight-30)*scale);
        const width=right-left,height=last-first;
        const pixels=context.getImageData(left,first,width,height).data,rows=[];
        for(let y=0;y<height;y++) {
            let x0=width,x1=-1,count=0;
            for(let x=0;x<width;x++) {
                const i=(y*width+x)*4,r=pixels[i],g=pixels[i+1],b=pixels[i+2];
                if(r>=150 && g>=100 && g<=205 && b>=210 && b-r>=25 && r-g>=20) {
                    x0=Math.min(x0,x);x1=Math.max(x1,x);count++;
                }
            }
            if(!count) continue;
            const current=[(left+x0)/scale,(first+y)/scale,(left+x1+1)/scale,(first+y+1)/scale];
            const previous=rows.at(-1);
            if(previous && current[1]-previous[3]<=4) {
                previous[0]=Math.min(previous[0],current[0]);previous[2]=Math.max(previous[2],current[2]);previous[3]=current[3];
            } else rows.push(current);
        }
        return rows;
    }""",{'png':base64.b64encode(png).decode(),'top':top})


def ssa_values_interactions(browser, url, output):
    page,canvas,errors=layout_page(browser,url)
    status=page.locator('#analysis-status')
    posts=[]
    page.on('request',lambda request:posts.append(request.url)
            if request.method=='POST' and request.url.endswith('/api/tasks') else None)
    results={}
    cases=[('fold','600260030100','Converged'),
           ('copy','345f0100','Converged'),
           ('partial','60026003015a5f5ff0','Incomplete')]
    for name,code,completion in cases:
        if 'Inspector hidden' not in status.text_content():hide_inspector(page,canvas)
        report=layout_submission(page,canvas,code,output,f'ssa-values-{name}')
        assert report['schema_version']==12 and report['status']==completion
        ssa=report['ssa']
        assert len(ssa['values'])==ssa['value_count']
        assert [value['id'] for value in ssa['values']]==list(range(ssa['value_count']))
        block=next(block for block in ssa['blocks'] if any(item['opcode']==1 for item in block['instructions']))
        operation=next(item for item in block['instructions'] if item['opcode']==1)
        value=ssa['values'][operation['results'][0]]
        assert operation['progress']=='Completed' and operation['effect_result'] is not None
        assert value['dependencies']==operation['operands']
        if name=='copy':
            caller=next(item['results'][0] for item in block['instructions'] if item['opcode']==0x34)
            assert operation['operation']=={'Copy':{'Value':caller}}
            assert value['equivalent']==caller
        else:
            assert operation['operation']=={'Constant':'0x5'}
            assert value['equivalent'] is None
            assert any(fact['scope']=='Definition' and fact['constant']=='0x5' for fact in value['facts'])
        if name=='partial':
            assert any(fact['scope']=={'State':block['state']} for fact in value['facts'])
            assert any(frontier['kind']=='Creation' and 'UnknownInitCode' in frontier['detail'] for frontier in report['frontiers'])
        page.keyboard.press('3');settle_gesture(page);page.mouse.move(2,2)
        png=canvas.screenshot(path=str(output/f'ssa-values-{name}-table.png'))
        selected=screenshot_metrics(page,png,(4,75,1436,970))
        assert selected['bounds'],selected
        tokens=ssa_definition_tokens(page,png,selected['bounds'][3])
        ids=[phi['result'] for phi in block['phis']]
        ids.extend(result for item in block['instructions'] for result in item['results'])
        index=ids.index(value['id'])
        assert len(tokens)>index,(tokens,ids)
        box=tokens[index]
        canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2})
        page.mouse.move(2,2);settle_gesture(page)
        expect(status).to_contain_text(f"Inspector: SSA value %{value['id']}")
        expect(status).to_contain_text('COPY' if name=='copy' else 'CONST 0x5')
        if name=='partial':expect(status).to_contain_text(f"State {symbol('σ',block['state'])}")
        canvas.screenshot(path=str(output/f'ssa-values-{name}-inspector.png'))
        before=status.text_content()
        ssa_details_toggle(page,canvas,'Full')
        expect(status).to_contain_text(f"Inspector: SSA value %{value['id']}")
        ssa_details_toggle(page,canvas,'Compact')
        navigation=None
        if name=='fold':
            # The selected inspector tab anchors its public ValueId editor and
            # the first-row source button without exposing egui rectangles.
            png=canvas.screenshot()
            inspector=screenshot_metrics(page,png,(4,650,1436,970))
            assert inspector['bounds'],inspector
            tab_bottom=inspector['bounds'][3]
            native_region=(4,selected['bounds'][3]+1,1436,450)
            native_before=screenshot_metrics(page,png,native_region)
            assert native_before['bounds'],native_before
            canvas.dblclick(position={'x':85,'y':tab_bottom+12})
            page.keyboard.press('Control+A');page.keyboard.insert_text('0');page.keyboard.press('Enter')
            page.mouse.move(2,2);settle_gesture(page)
            expect(status).to_contain_text('Inspector: SSA value %0')
            expect(status).to_contain_text('CONST 0x2')
            inspected=screenshot_metrics(page,canvas.screenshot(),native_region)
            assert inspected['bounds']==native_before['bounds'], 'inspecting another value moved the selected native instruction'
            canvas.click(position={'x':100,'y':tab_bottom+44})
            page.mouse.move(2,2);settle_gesture(page)
            source=screenshot_metrics(page,canvas.screenshot(path=str(output/'ssa-values-source-navigation.png')),native_region)
            assert source['bounds'],source
            first_y=(tokens[0][1]+tokens[0][3])/2
            assert source['bounds'][1]<=first_y<=source['bounds'][3], 'Go to source did not select the original PUSH instruction'
            navigation={'inspector_tab':inspector['bounds'],'before':native_before['bounds'],
                        'inspected':inspected['bounds'],'source':source['bounds']}
        results[name]={'value':value,'original_instruction':operation,'definition_token':box,
                       'inspector':before,'navigation':navigation,
                       'cfg_states':len(report['cfg']),'frontiers':len(report['frontiers'])}
    assert len(posts)==len(cases),'value inspection or reading-mode changes reran analysis'
    assert not errors,errors
    results['renderer']=webgpu_evidence(page)
    results['analysis_submissions']=len(posts)
    (output/'ssa-values-report.json').write_text(json.dumps(results,indent=2)+'\n')
    page.close()
    return results


def unknown_world_interactions(browser, url, output):
    """Run an actual closed unknown relation through the form, SSA and inspector."""
    page, canvas, errors = layout_page(browser, url)
    posts = []
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    code = "602a5f555f5f5f5f6102015afa505f545f5260205ff3"
    entry = {"address": "0x0000000000000000000000000000000000000101",
             "caller": "0x0000000000000000000000000000000000000900",
             "call_value": "0x0", "calldata": "0x"}
    report = layout_submission(page, canvas, code, output, "unknown-world", entry=entry,
                               source_height_offset=-62)
    assert report["schema_version"] == 12 and report["status"] == "Converged"
    assert report["frontiers"] == [] and report["ssa"]["complete"]
    summaries = report["abstract_summaries"]
    assert summaries and all(summary["closed"] and summary["procedures"] for summary in summaries)
    assert any(edge["kind"] == "SummaryReturn" for edge in report["edges"])
    unknown = [state for state in report["states"]
               if state["entry"]["frames"][-1]["kind"] == "AbstractEnvironment"]
    assert unknown
    for state in unknown:
        frame = state["entry"]["frames"][-1]
        block = next(block for block in report["cfg"] if block["id"] == state["state"])
        assert frame["program"] is None
        assert block["instructions"] == [] and block["executed_pcs"] == []
    returns = [outcome for outcome in report["outcomes"] if outcome["kind"] == "Return"]
    assert returns
    for outcome in returns:
        data = report["byte_arrays"][outcome["data"]]
        assert data["length"]["constants"] == ["0x20"]
        cells = {cell["offset"]: data["values"][cell["value"]] for cell in data["cells"]}
        for offset in range(32):
            assert cells[offset]["constants"] == ["0x2a" if offset == 31 else "0x0"]
    graph_mode(page, canvas, "States")
    page.keyboard.press("f")
    settle_gesture(page)
    scene = node_snapshot(page, canvas, output, "unknown-world-summary-graph", measure_nodes=True)
    source = summaries[0]["source_state"]
    status = page.locator("#analysis-status")
    for record in scene["node_records"]:
        left, top, right, bottom = record["bounds"]
        canvas.click(position={"x": (left + right) / 2, "y": (top + bottom) / 2})
        page.mouse.move(2, 2)
        settle_gesture(page)
        if f"Selected {symbol('σ', source)};" in status.text_content():
            break
    expect(status).to_contain_text(f"Selected {symbol('σ', source)};")
    hide_inspector(page, canvas)
    expect(status).to_contain_text("Inspector: Frame")
    expect(status).to_contain_text("Abstract interactions")
    expect(status).to_contain_text("closed")
    tab = inspector_frame_tab(page, canvas)
    inspector_title_action(page, canvas, "Maximize", tab)
    page.mouse.move(2, 2)
    settle_gesture(page)
    png = canvas.screenshot(path=str(output / "unknown-world-summary-inspector.png"))
    pixels = text_pixels(page, png, (8, 80, 1410, 970))
    assert len(pixels["rows"]) >= 10 and pixels["white_pixels"] > 100, pixels
    assert len(posts) == 1, "summary selection or inspection reran analysis"
    assert not errors, errors
    result = {"status": report["status"], "ssa_complete": report["ssa"]["complete"],
              "summaries": summaries, "unknown_states": [state["state"] for state in unknown],
              "source_state": source, "inspector": status.text_content(),
              "painted": pixels, "analysis_submissions": len(posts),
              "renderer": webgpu_evidence(page)}
    (output / "unknown-world-report.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result


def warning_pixels(page, png, card):
    """Inspect the independent inset warning dashes above the card's text."""
    return page.evaluate("""async ({png,card}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        const scale=image.width/innerWidth;
        const left=Math.floor((card[0]+6)*scale),right=Math.ceil((card[2]-6)*scale);
        const top=Math.floor((card[1]-1)*scale),height=Math.ceil(6*scale),width=right-left;
        const data=context.getImageData(left,top,width,height).data;
        let best={pixels:0,runs:0};
        for(let y=0;y<height;y++) {
            let count=0,runs=0,previous=false;
            for(let x=0;x<width;x++) {
                const i=(y*width+x)*4,r=data[i],g=data[i+1],b=data[i+2];
                const warning=r>=180 && g>=140 && g<=215 && b>=60 && b<=165 && r-g>=30;
                if(warning) {count++;if(!previous) runs++;}previous=warning;
            }
            if(count>best.pixels) best={pixels:count,runs};
        }
        return best;
    }""", {'png':base64.b64encode(png).decode(),'card':card})


def termination_interactions(browser, url, args):
    page,canvas,errors=layout_page(browser,url)
    output=args.output
    posts=[]
    page.on('request',lambda request:posts.append(request.url)
            if request.method=='POST' and request.url.endswith('/api/tasks') else None)
    results={}
    for name,code,kind,color in [('normal','6003565b00','Return','normal'),
                                ('revert','6003565b5f5ffd','Revert','revert'),
                                ('failure','6003565bfe','Failure','failure')]:
        report=layout_submission(page,canvas,code,output,f'termination-{name}')
        target=next(node for node in report['cfg'] if node['start_pc']==3)
        assert {item['kind'] for item in report['outcomes'] if item['state']==target['id']}=={kind}
        graph_mode(page,canvas,'States');page.keyboard.press('f');settle_gesture(page)
        before=node_snapshot(page,canvas,output,f'termination-{name}-unselected',measure_nodes=True)
        candidate=next(record for record in before['node_records'] if record['kind']==color)
        left,top,right,bottom=candidate['bounds']
        canvas.click(position={'x':(left+right)/2,'y':(top+bottom)/2});page.mouse.move(2,2);settle_gesture(page)
        expect(page.locator('#analysis-status')).to_contain_text(f"Selected {symbol('σ',target['id'])};")
        selected=node_snapshot(page,canvas,output,f'termination-{name}-selected',measure_nodes=True)
        ending=selected_ending(page)
        assert ending['members']==1 and ending['continuing']==ending['unresolved']==0,ending
        assert ending[{'normal':'root_normal','revert':'root_revert','failure':'root_failure'}[color]]==1,ending
        assert selected['selected_kind']==color,selected
        assert candidate['kind']==selected['selected_kind'], 'selection covered the semantic background'
        low=None
        if name=='normal':
            pointer=((left+right)/2,(top+bottom)/2)
            for _ in range(12):
                if 'CFG detail: identities' in page.locator('#analysis-status').text_content():break
                page.mouse.move(*pointer);page.keyboard.down('Control');page.mouse.wheel(0,120);page.keyboard.up('Control');settle_gesture(page)
            page.mouse.move(2,2);settle_gesture(page)
            expect(page.locator('#analysis-status')).to_contain_text('CFG detail: identities')
            low=node_snapshot(page,canvas,output,'termination-normal-low-zoom',min_selected_pixels=1)
            assert low['selected_kind']=='normal',low
            assert selected_ending(page)['root_normal']==1
        results[name]={'typed_outcome':kind,'target_state':target['id'],'before':candidate,'selected':selected,'ending':ending,'low_zoom':low}
        # S0 deliberately retains the model's entry-failure alternative plus
        # the real observed JUMP. It must stay neutral and label continuation.
        page.keyboard.press('f');settle_gesture(page)
        scene=node_snapshot(page,canvas,output,f'termination-{name}-refit',measure_nodes=True)
        entry=next(record for record in scene['node_records'] if record['kind']=='ordinary')
        box=entry['bounds'];canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2})
        page.mouse.move(2,2);settle_gesture(page)
        expect(page.locator('#analysis-status')).to_contain_text('Selected σ₀;')
        mixed=selected_ending(page)
        assert mixed['kind']=='mixed / possible ending' and mixed['root_failure']==mixed['continuing']==1,mixed
        neutral=node_snapshot(page,canvas,output,f'termination-{name}-entry-mixed')
        assert neutral['selected_kind']=='ordinary',neutral
        results[name]['entry_mixed']=mixed

    mixed_report=layout_submission(page,canvas,'346007576009565b5f5b5000',output,'termination-mixed-block')
    members={node['id'] for node in mixed_report['cfg'] if node['start_pc']==9}
    assert len(members)==2
    observed={member:tuple(sorted({item['kind'] for item in mixed_report['outcomes'] if item['state']==member})) for member in members}
    assert set(observed.values())=={('Return',),('Failure',)},observed
    graph_mode(page,canvas,'Blocks');page.keyboard.press('f');settle_gesture(page)
    scene=node_snapshot(page,canvas,output,'termination-mixed-block-before',measure_nodes=True)
    box=max(scene['nodes'],key=lambda rect:(rect[1]+rect[3])/2)
    canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2});page.mouse.move(2,2);settle_gesture(page)
    assert any(f"Selected {symbol('σ',member)};" in page.locator('#analysis-status').text_content() for member in members)
    summary=selected_ending(page)
    assert summary['members']==2 and summary['root_normal']==summary['root_failure']==1 and summary['continuing']==0,summary
    mixed=node_snapshot(page,canvas,output,'termination-mixed-block-selected',measure_nodes=True)
    assert mixed['selected_kind']=='ordinary',mixed
    results['mixed_aggregation']={'members':sorted(members),'summary':summary,'pixels':mixed}

    partial=layout_submission(page,canvas,'5a5f5ff0',output,'termination-frontier')
    assert partial['status']=='Incomplete' and partial['frontiers']
    assert any(frontier['kind']=='Creation' and 'UnknownInitCode' in frontier['detail'] for frontier in partial['frontiers'])
    graph_mode(page,canvas,'States');page.keyboard.press('f');settle_gesture(page)
    frontier=node_snapshot(page,canvas,output,'termination-frontier-selected',measure_nodes=True)
    summary=selected_ending(page)
    assert summary['unresolved']>0 and frontier['selected_kind']=='ordinary',summary
    dashes=warning_pixels(page,canvas.screenshot(),frontier['bounds'])
    assert dashes['pixels']>20 and dashes['runs']>=3,dashes
    assert frontier['selected_pixels']>20, 'warning replaced the independent selection outline'
    results['frontier']={'summary':summary,'pixels':frontier,'warning_dashes':dashes}
    assert len(posts)==5, 'selection or semantic zoom reran termination analyses'
    assert not errors,errors
    results['renderer']=webgpu_evidence(page)
    page.close()
    results['child_frames']=termination_call_interactions(browser,args)
    (output/'termination-report.json').write_text(json.dumps(results,indent=2)+'\n')
    return results


def termination_call_interactions(browser,args):
    class TerminalRpc(EntryRpcFixture):
        root_code='0x6003565b5f5f5f5f5f73'+'22'*20+'61fffff100'
        child_code='0x00'
    with TerminalRpc() as rpc,TemporaryDirectory(prefix='termination-rpc-') as directory:
        providers=[{'id':'terminal-world','name':'Termination child frame fixture','endpoint':rpc.endpoint}]
        config=Path(directory)/'providers.json';config.write_text(json.dumps({'providers':providers})+'\n')
        with analysis_server(args,config) as url:
            page=browser.new_page(viewport={'width':1440,'height':1000});observe_webgpu(page)
            automatic_rpc_entry(page,url,providers[0],args.output,'termination-call-startup')
            canvas=page.locator('#evm-canvas')
            admitted=submit_rpc(page,canvas,rpc,args.output,'termination-child-call',0,providers)
            reply=complete_submission(page,admitted)
            assert reply.status==200 and 'Ok' in reply.json()['result'],reply.json()
            report=reply.json()['result']['Ok']
            (args.output/'termination-child-call-analysis.json').write_text(json.dumps(report,indent=2)+'\n')
            depths={node['id']:node['frame_depth'] for node in report['cfg']}
            failure=next(edge for edge in report['edges'] if edge['kind']=='Failure' and depths[edge['from']]==depths[edge['to']]==1)
            caller=failure['from']
            assert not any(outcome['state']==caller for outcome in report['outcomes'])
            returning=next(edge for edge in report['edges'] if edge['kind']=='Return' and depths[edge['from']]==2 and depths[edge['to']]==1)
            canvas.click(position={'x':1440-180,'y':12});hide_inspector(page,canvas)
            page.keyboard.press('2');settle_gesture(page);graph_mode(page,canvas,'States');page.keyboard.press('f');settle_gesture(page)
            submitted=[]
            page.on('request',lambda request:submitted.append(request.url) if request.method=='POST' and request.url.endswith('/api/tasks') else None)
            # The one green child frame and one green resumed root are verified
            # by real clicks and original state IDs, not inferred from opcode.
            scene=node_snapshot(page,canvas,args.output,'termination-call-overview',measure_nodes=True)
            frame=None
            for record in scene['node_records']:
                if record['kind']!='normal':continue
                box=record['bounds'];canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2});page.mouse.move(2,2);settle_gesture(page)
                if f"Selected {symbol('σ',returning['from'])};" not in page.locator('#analysis-status').text_content():continue
                summary=selected_ending(page)
                assert summary['frame_normal']==1 and summary['root_normal']==summary['root_failure']==0,summary
                frame={'summary':summary,'pixels':node_snapshot(page,canvas,args.output,'termination-frame-return-selected')}
                break
            assert frame is not None, 'real child RETURN edge did not paint a normal frame ending'
            neutral=None
            for record in scene['node_records']:
                if record['kind']!='ordinary':continue
                box=record['bounds'];canvas.click(position={'x':(box[0]+box[2])/2,'y':(box[1]+box[3])/2});page.mouse.move(2,2);settle_gesture(page)
                if f"Selected {symbol('σ',caller)};" not in page.locator('#analysis-status').text_content():continue
                summary=selected_ending(page)
                assert summary['kind']=='no termination evidence' and summary['continuing']==1,summary
                assert summary['root_failure']==summary['frame_failure']==0,summary
                neutral={'summary':summary,'pixels':node_snapshot(page,canvas,args.output,'termination-call-failure-continuing')}
                break
            assert neutral is not None, 'same-depth call failure was falsely classified as a terminal ending'
            assert not submitted, 'terminal node selection reran the RPC analysis'
            result={'frame_return':frame,'recoverable_call_failure':neutral,'typed_failure_edge':failure,'typed_return_edge':returning,'renderer':webgpu_evidence(page)}
            page.close()
            return result


def require_selected_complete(metrics):
    record=metrics['selected_record']
    assert record and not any(record['region_clipped'].values()), 'geometry requires the complete selected fill'
    assert not any(record['viewport_clipped'].values()), 'geometry requires a visible complete card'
    assert record['full_outline_visible'], 'geometry requires four real outlines, including the internal graph painter clip'


def require_scene_complete(metrics):
    assert len(metrics['nodes'])==len(metrics['node_records'])
    for record in metrics['node_records']:
        assert record['full_outline_visible'],f"scene geometry needs all four real outlines: {record}"
        assert not any(record['region_clipped'].values()) and not any(record['viewport_clipped'].values()),record


def assert_translation(before, after, dx: float, dy: float, tolerance: float = 3.0) -> None:
    for metrics in [before,after]:require_selected_complete(metrics)
    initial, final = before["bounds"], after["bounds"]
    expected = [dx, dy, dx, dy]
    for old, new, delta in zip(initial, final, expected, strict=True):
        assert abs(new - old - delta) <= tolerance, f"wheel must pan without resizing nodes: {initial} -> {final}, expected {expected}"


def assert_anchored_zoom(before, after, pointer) -> float:
    for metrics in [before,after]:require_selected_complete(metrics)
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
    require_scene_complete(before);require_scene_complete(after)
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
        def contains(card, fill):
            return (card[0] <= fill[0] and card[1] <= fill[1]
                    and card[2] >= fill[2] and card[3] >= fill[3])

        selected_before = contains(original, before["bounds"])
        selected_after = contains(fitted, after["bounds"])
        assert selected_before == selected_after, "Fit changed the selected card's scene association"
        # The selected card retains its semantic fill. Compare its pure fill
        # in both modes, with all centers measured from actual outer outlines.
        original_measure = before["bounds"] if selected_before else original
        fitted_measure = after["bounds"] if selected_after else fitted
        for axis in [0, 1]:
            original_center = (original[axis] + original[axis + 2]) / 2
            fitted_center = (fitted[axis] + fitted[axis + 2]) / 2
            expected = translation[axis] + original_center * scale
            assert abs(fitted_center - expected) <= 4.0, f"Fit rearranged the scene: {before['nodes']} -> {after['nodes']}"
            original_size = original_measure[axis + 2] - original_measure[axis]
            fitted_size = fitted_measure[axis + 2] - fitted_measure[axis]
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
                                (4, 123, width - 4, height - 30), inspect_scene)
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


def select_inspector_tab(page, canvas, tab: str, y: float = 778):
    # Compact shared controls, measured in the packaged 1440x1000 WASM app.
    # Callers that resize the inspector pass its measured toolbar center.
    positions = {
        "Frame": 30, "Stack": 80, "SSA value": 141, "Memory": 210,
        "Storage": 274, "Transient": 341, "Calldata": 408, "Returndata": 481,
        "Outcomes": 561, "Acquisition": 640, "Environment": 727, "Timing": 799, "Diagnostics": 866,
    }
    canvas.click(position={"x": positions[tab], "y": y})
    expect(page.locator("#analysis-status")).to_contain_text(f"Inspector: {tab}")


def inspector_frame_tab(page, canvas):
    """Measure the selected Frame tab from its painted fill, including after drag."""
    expect(page.locator("#analysis-status")).to_contain_text("Inspector: Frame")
    page.mouse.move(2, 2)
    settle_gesture(page)
    png = canvas.screenshot()
    candidates = page.evaluate("""async png => {
        const image = new Image(); image.src = `data:image/png;base64,${png}`;
        await image.decode();
        const c = document.createElement('canvas'); c.width=image.width; c.height=image.height;
        const ctx=c.getContext('2d'); ctx.drawImage(image,0,0);
        const pixels=ctx.getImageData(0,0,c.width,c.height).data;
        const visited=new Uint8Array(c.width*c.height), scale=c.width/innerWidth, result=[];
        const selected=i=>pixels[i*4]===33 && pixels[i*4+1]===65 && pixels[i*4+2]===75;
        for (let y=Math.ceil(25*scale);y<c.height;y++) for(let x=0;x<c.width;x++) {
            const start=y*c.width+x;
            if(visited[start] || !selected(start))continue;
            const queue=[start];visited[start]=1;let left=x,right=x,top=y,bottom=y,count=0;
            for(let at=0;at<queue.length;at++) {
                const i=queue[at],px=i%c.width,py=Math.floor(i/c.width);count++;
                left=Math.min(left,px);right=Math.max(right,px);top=Math.min(top,py);bottom=Math.max(bottom,py);
                for(const next of [px>0?i-1:-1,px+1<c.width?i+1:-1,py>0?i-c.width:-1,py+1<c.height?i+c.width:-1]) {
                    if(next>=0 && !visited[next] && selected(next)){visited[next]=1;queue.push(next);}
                }
            }
            const w=(right-left+1)/scale,h=(bottom-top+1)/scale;
            // Frame is the first compact tab, not a selected SSA row or the
            // similarly colored Block entry observation control beneath it.
            if(left/scale>=4 && left/scale<=10 && w>=40 && w<=50 && h>=14 && h<=20 && count>120*scale*scale)
                result.push([left/scale,top/scale,(right+1)/scale,(bottom+1)/scale]);
        }
        return result.sort((a,b)=>a[1]-b[1]);
    }""", base64.b64encode(png).decode())
    assert len(candidates) == 1, f"expected one selected Frame tab, got {candidates}"
    return candidates[0]


def inspector_title_action(page, canvas, action: str, frame_tab):
    """Use the real right-aligned titlebar at its painted vertical position."""
    inset = {"Maximize": 79, "Restore": 73, "Hide": 24}[action]
    canvas.click(position={"x": page.viewport_size["width"] - inset,
                           "y": (frame_tab[1] + frame_tab[3]) / 2})
    settle_gesture(page)


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


STORAGE_SLOT_WORD = "0x1234567890abcdef234567890abcdef134567890abcdef124567890190abcdef"


def storage_table_pixels(page, screenshot: bytes, region):
    """Find the painted divider, header and cells, without reading egui state."""
    # White glyphs are distinct from muted labels and the native border.
    return page.evaluate("""async ({png,region}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const c=document.createElement('canvas');c.width=image.width;c.height=image.height;
        const ctx=c.getContext('2d');ctx.drawImage(image,0,0);
        const scale=image.width/innerWidth,data=ctx.getImageData(0,0,c.width,c.height).data;
        const [left,top,right,bottom]=region.map(n=>Math.round(n*scale));
        const color=(x,y)=>Array.from(data.subarray((y*c.width+x)*4,(y*c.width+x)*4+3));
        const border=(x,y)=>{const [r,g,b]=color(x,y);return r>=36&&r<=50&&g>=48&&g<=66&&b>=63&&b<=84;};
        const ink=(x,y)=>{const [r,g,b]=color(x,y);return r>=128&&b>=g&&g>=r&&b-r<=35;};
        const lines=[];
        for(let x=Math.max(left,Math.round(40*scale));x<right;x++) {
            let begin=top;
            for(let y=top;y<=bottom;y++) {
                if(y<bottom&&border(x,y))continue;
                if(y-begin>=65*scale)lines.push({x,top:begin,bottom:y,length:y-begin});
                begin=y+1;
            }
        }
        assert(lines.length>0,'missing painted storage column separator');
        lines.sort((a,b)=>b.length-a.length);
        const longest=lines[0];
        const same=lines.filter(n=>Math.abs(n.x-longest.x)<=2*scale&&Math.abs(n.top-longest.top)<=2*scale);
        const separator=same.reduce((sum,n)=>sum+n.x+0.5,0)/same.length/scale;
        const tableTop=longest.top,tableBottom=longest.bottom;
        const bands=[];
        for(let y=tableTop;y<tableBottom;y++) {
            let count=0;for(let x=left;x<right;x++)if(ink(x,y))count++;
            if(count<5*scale)continue;
            if(bands.length&&y-bands[bands.length-1][1]<=3*scale)bands[bands.length-1][1]=y+1;
            else bands.push([y,y+1]);
        }
        async function cell(x0,x1,y0,y1) {
            let minX=x1,minY=y1,maxX=-1,maxY=-1,count=0;
            for(let y=y0;y<y1;y++)for(let x=x0;x<x1;x++)if(ink(x,y)){
                minX=Math.min(minX,x);maxX=Math.max(maxX,x);minY=Math.min(minY,y);maxY=Math.max(maxY,y);count++;
            }
            if(!count)return null;
            const width=maxX-minX+1,height=maxY-minY+1,mask=new Uint8Array(width*height);
            const rgb=new Uint8Array(width*height*3);
            for(let y=minY;y<=maxY;y++)for(let x=minX;x<=maxX;x++)if(ink(x,y))mask[(y-minY)*width+x-minX]=1;
            for(let y=minY;y<=maxY;y++)for(let x=minX;x<=maxX;x++)rgb.set(color(x,y),((y-minY)*width+x-minX)*3);
            const digest=await crypto.subtle.digest('SHA-256',mask);
            return {bounds:[minX/scale,minY/scale,(maxX+1)/scale,(maxY+1)/scale],pixels:count,
                mask_size:[width,height],rgb_raster:btoa(String.fromCharCode(...rgb)),
                mask_sha256:Array.from(new Uint8Array(digest),n=>n.toString(16).padStart(2,'0')).join('')};
        }
        const rows=[];
        for(const [y0,y1] of bands)rows.push({band:[y0/scale,y1/scale],
            slot:await cell(left,Math.floor(separator*scale)-1,Math.max(tableTop,y0-2*scale),Math.min(tableBottom,y1+2*scale)),
            value:await cell(Math.ceil(separator*scale)+1,right,Math.max(tableTop,y0-2*scale),Math.min(tableBottom,y1+2*scale))});
        return {separator,table_bounds:[left/scale,tableTop/scale,right/scale,tableBottom/scale],
            header:rows[0],rows:rows.slice(1),divider_candidates:lines.length};
        function assert(ok,message){if(!ok)throw new Error(message);}
    }""", {"png":base64.b64encode(screenshot).decode(),"region":region})


def storage_table_snapshot(page, canvas, output: Path, name: str, tab_bottom: float, count: int):
    page.mouse.move(2,2)
    settle_gesture(page)
    png=canvas.screenshot(path=str(output/f"{name}.png"))
    size=page.viewport_size
    table=storage_table_pixels(page,png,(4,tab_bottom+52,size["width"]-24,size["height"]-30))
    assert table["header"]["slot"] and table["header"]["value"], f"Slot/Value headers are missing: {storage_pixel_report(table)}"
    assert len(table["rows"])==count, f"expected {count} body rows distinct from the header: {storage_pixel_report(table)}"
    assert all(row["slot"] and row["value"] for row in table["rows"]), storage_pixel_report(table)
    origins=[row["value"]["bounds"][0] for row in table["rows"]]
    assert max(origins)-min(origins)<=1, f"Value origins depend on slot text length: {origins}"
    ends=[row["slot"]["bounds"][2] for row in table["rows"]]
    # Different glyphs have slightly different ink bearings inside their
    # monospace cells; their low-order character positions must still agree.
    assert max(ends)-min(ends)<=2, f"Slot low-order ends are not right aligned: {ends}"
    assert all(row["slot"]["bounds"][2]<table["separator"] for row in table["rows"]), storage_pixel_report(table)
    assert min(origins)>table["separator"], f"Slot labels invaded Value: {storage_pixel_report(table)}"
    save_png_crop(page,png,table["table_bounds"],output/f"{name}-table.png")
    return table


def drag_storage_separator(page, table, target_x: float):
    """Drag the separator located in the actual PNG, using browser pointer input."""
    y=sum(table["header"]["band"])/2
    page.mouse.move(table["separator"],y)
    rendered_frame(page)
    page.mouse.down()
    page.mouse.move(target_x,y,steps=16)
    rendered_frame(page)
    page.mouse.up()
    settle_gesture(page)


def storage_ink_pixels(page, screenshot: bytes, region):
    """Retain the glyph mask and every RGB sample of the text and its region."""
    return page.evaluate("""async ({png,region}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const c=document.createElement('canvas');c.width=image.width;c.height=image.height;
        const ctx=c.getContext('2d');ctx.drawImage(image,0,0);
        const data=ctx.getImageData(0,0,c.width,c.height).data,scale=image.width/innerWidth;
        const [left,top,right,bottom]=region.map(n=>Math.round(n*scale));
        const ink=(x,y)=>{const i=(y*c.width+x)*4,r=data[i],g=data[i+1],b=data[i+2];
            return r>=128&&b>=g&&g>=r&&b-r<=35;};
        let minX=right,minY=bottom,maxX=-1,maxY=-1,count=0;
        for(let y=top;y<bottom;y++)for(let x=left;x<right;x++)if(ink(x,y)){
            minX=Math.min(minX,x);maxX=Math.max(maxX,x);minY=Math.min(minY,y);maxY=Math.max(maxY,y);count++;
        }
        if(!count)return null;
        const width=maxX-minX+1,height=maxY-minY+1,mask=new Uint8Array(width*height);
        const rgb=(x0,y0,x1,y1)=>{
            const samples=[];
            for(let y=y0;y<y1;y++)for(let x=x0;x<x1;x++){
                const i=(y*c.width+x)*4;samples.push(data[i],data[i+1],data[i+2]);
            }
            let binary='';
            for(let i=0;i<samples.length;i+=16384)binary+=String.fromCharCode(...samples.slice(i,i+16384));
            return btoa(binary);
        };
        for(let y=minY;y<=maxY;y++)for(let x=minX;x<=maxX;x++)if(ink(x,y))mask[(y-minY)*width+x-minX]=1;
        const digest=await crypto.subtle.digest('SHA-256',mask);
        return {bounds:[minX/scale,minY/scale,(maxX+1)/scale,(maxY+1)/scale],pixels:count,
            mask_size:[width,height],mask_sha256:Array.from(new Uint8Array(digest),n=>n.toString(16).padStart(2,'0')).join(''),
            rgb_raster:rgb(minX,minY,maxX+1,maxY+1),
            region:{bounds:region,mask_size:[right-left,bottom-top],rgb_raster:rgb(left,top,right,bottom)}};
    }""", {"png":base64.b64encode(screenshot).decode(),"region":region})


def storage_hover_snapshot(page, canvas, output: Path, name: str, cell):
    left,top,right,bottom=cell["bounds"]
    page.mouse.move((left+min(right,left+40))/2,(top+bottom)/2)
    page.wait_for_timeout(1400)
    rendered_frame(page)
    png=canvas.screenshot(path=str(output/f"{name}.png"))
    # Tooltips use the panel fill. Find their real rectangle below the native
    # titlebar, rather than assuming a fixed placement beside the pointer.
    bounds=page.evaluate("""async png => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const c=document.createElement('canvas');c.width=image.width;c.height=image.height;
        const ctx=c.getContext('2d');ctx.drawImage(image,0,0);
        const data=ctx.getImageData(0,0,c.width,c.height).data,scale=image.width/innerWidth;
        const runs=[];
        for(let y=80*scale;y<c.height-30*scale;y++) {
            let start=0;
            for(let x=0;x<=c.width;x++) {
                const i=(y*c.width+x)*4;
                if(x<c.width&&data[i]===21&&data[i+1]===29&&data[i+2]===42)continue;
                if(x-start>200*scale)runs.push([start,y,x,y+1]);
                start=x+1;
            }
        }
        if(!runs.length)return null;
        const maxWidth=Math.max(...runs.map(r=>r[2]-r[0]));
        const wide=runs.filter(r=>r[2]-r[0]>=maxWidth-8*scale);
        return [Math.min(...wide.map(r=>r[0]))/scale,Math.min(...wide.map(r=>r[1]))/scale,
            Math.max(...wide.map(r=>r[2]))/scale,Math.max(...wide.map(r=>r[3]))/scale];
    }""",base64.b64encode(png).decode())
    assert bounds, f"no rendered hover details for {name}"
    metrics=text_pixels(page,png,bounds)
    save_png_crop(page,png,bounds,output/f"{name}-tooltip.png")
    ink=storage_ink_pixels(page,png,bounds)
    return {"bounds":bounds,"text":metrics,"raster":ink.pop("region"),"ink":ink}


def assert_storage_cell_text(before, after, label="Value"):
    """Compare every rendered RGB sample, allowing only compositor rounding."""
    assert before["mask_size"]==after["mask_size"], (before["bounds"],after["bounds"])
    first,last=base64.b64decode(before["rgb_raster"]),base64.b64decode(after["rgb_raster"])
    maximum=max(abs(a-b) for a,b in zip(first,last,strict=True))
    # Reviewed translated full-word PNGs vary by at most two intensity steps;
    # a changed glyph fails at individual pixels, even at the end of a long word.
    assert maximum<=2,f"rendered {label} text changed: maximum RGB difference {maximum}"
    return {"rgb_samples":len(first),"maximum_rgb_difference":maximum}


def storage_reviewed_raster(key, text, size, mask_sha256):
    """Load independent historical RGB pixels and verify their reviewed mask."""
    fixture=json.loads(Path(__file__).with_name("web_test_storage_literals.json").read_text())["literals"][key]
    assert fixture["text"]==text and fixture["mask_size"]==size, key
    raster=zlib.decompress(base64.b64decode(fixture["rgb_zlib_base64"],validate=True))
    assert len(raster)==size[0]*size[1]*3, key
    assert hashlib.sha256(raster).hexdigest()==fixture["rgb_sha256"], key
    left,top,right,bottom=fixture["ink_bounds"]
    mask=bytearray()
    for y in range(top,bottom):
        for x in range(left,right):
            r,g,b=raster[(y*size[0]+x)*3:(y*size[0]+x+1)*3]
            mask.append(int(r>=128 and b>=g>=r and b-r<=35))
    assert fixture["mask_sha256"]==mask_sha256==hashlib.sha256(mask).hexdigest(), key
    return {"bounds":[0,0,*size],"mask_size":size,"rgb_raster":base64.b64encode(raster).decode()}


def assert_storage_reviewed_pixels(cell, key, text, size, mask_sha256):
    reference=storage_reviewed_raster(key,text,size,mask_sha256)
    return {"expected_text":text,"mask_size":size,"reviewed_mask_sha256":mask_sha256,
            **assert_storage_cell_text(reference,cell,text)}


def storage_crop_cell(cell, left: int, top: int, right: int, bottom: int):
    """Crop a real glyph raster and normalize its remaining ink bounds."""
    width,height=cell["mask_size"]
    assert 0<=left<right<=width and 0<=top<bottom<=height,(cell["mask_size"],left,top,right,bottom)
    source=base64.b64decode(cell["rgb_raster"])
    ink=[]
    for y in range(top,bottom):
        for x in range(left,right):
            r,g,b=source[(y*width+x)*3:(y*width+x+1)*3]
            if r>=128 and b>=g>=r and b-r<=35:
                ink.append((x,y))
    assert ink,"glyph crop is empty"
    x0,x1=min(x for x,y in ink),max(x for x,y in ink)+1
    y0,y1=min(y for x,y in ink),max(y for x,y in ink)+1
    raster=b"".join(source[(y*width+x0)*3:(y*width+x1)*3] for y in range(y0,y1))
    scale=width/(cell["bounds"][2]-cell["bounds"][0])
    mask=bytes(int(r>=128 and b>=g>=r and b-r<=35) for r,g,b in zip(raster[::3],raster[1::3],raster[2::3],strict=True))
    return {"bounds":[cell["bounds"][0]+x0/scale,cell["bounds"][1]+y0/scale,
                      cell["bounds"][0]+x1/scale,cell["bounds"][1]+y1/scale],
            "mask_size":[x1-x0,y1-y0],"rgb_raster":base64.b64encode(raster).decode(),
            "mask_sha256":hashlib.sha256(mask).hexdigest(),"pixels":len(ink)}


def storage_glyph_cells(cell):
    """Separate the ink runs of the bundled monospace hexadecimal glyphs.

    Integer glyph placement can change the inter-character gap by one pixel
    when a suffix starts a new galley. Compare every glyph's RGB samples while
    keeping the actual character count and order, rather than shifting a word.
    """
    width,height=cell["mask_size"]
    raster=base64.b64decode(cell["rgb_raster"])
    runs=[]
    for x in range(width):
        colored=False
        for y in range(height):
            r,g,b=raster[(y*width+x)*3:(y*width+x+1)*3]
            if r>=128 and b>=g>=r and b-r<=35:
                colored=True
                break
        if colored:
            if runs and x==runs[-1][1]:
                runs[-1][1]=x+1
            else:
                runs.append([x,x+1])
    return [storage_crop_cell(cell,left,0,right,height) for left,right in runs]


def assert_storage_literal_pixels(cell, text: str):
    """Require independent reviewed DPR1 pixels of the complete literal."""
    reviewed = {
        "0x0": ([22,9], "70dcbd48818467183033b408aeb867e1b1385300a64ace6d0a1dfcd02fcb5c33"),
        "0xf": ([21,10], "c15143a43ec1f811109009d82060c7230250bd467072ccf11b0a976465cd9b26"),
        "0x1234": ([46,9], "f526234bce81a61b6813593d5fc81d4bcc29fffe717d2ab644219bfc06d42e6a"),
        STORAGE_SLOT_WORD: ([514,10], "7f1ebe5c7ae6c51205b1c0ab84b5f70a7f2d6e0c00c2254e8bfb973e378fb7b0"),
        "{"+STORAGE_SLOT_WORD+"}": ([531,12], "441542c8fa375969f42f890a0240ae01222ff506ba5da7a71cfbee95695caa2e"),
        "…abcdef": ([54,10], "f3f2b65c34ee7f1d1bd1851b0aeaa7f509d1ccc515cff792b2b718de558c5805"),
        "…7890190abcdef": ([108,10], "8b9dc9c7c872399ea7f0c8aa2c1be3e03610fcb1bd7224af9ffa8b13b9a6dda9"),
        "…90abcdef": ([69,10], "400ff596c148f1a88017f63a1ccc0cc31cd655306ba58e6c52950f79415e81b2"),
        "…90190abcdef": ([93,10], "ff5b7b798e75e4de049ff8445836b31c155ad8d3a08f07e3625c8450b46767cd"),
    }
    size,expected=reviewed[text]
    if text=="{"+STORAGE_SLOT_WORD+"}":
        # One historical red127 sample rounds to128 in current Chromium.
        # Compare the complete independent raster with the existing <=2 bound.
        return assert_storage_reviewed_pixels(cell,"full_value",text,size,expected)
    assert cell["mask_size"]==size and cell["mask_sha256"]==expected, f"rendered {text} changed: {storage_pixel_report(cell)}"
    return {"expected_text":text,"mask_size":size,"mask_sha256":expected}


def assert_storage_suffix(narrowed, word: str, retained: int):
    """Require exactly a leading ellipsis plus the requested real tail glyphs."""
    literal=assert_storage_literal_pixels(narrowed,"…"+word[-retained:])
    actual=storage_glyph_cells(narrowed)
    assert len(actual)==retained+3,f"expected …{word[-retained:]}, got {len(actual)} ink runs"
    dots=actual[:3]
    assert all(dot["mask_size"][1]<=3 for dot in dots),"omission marker is not three dots"
    assert max(dot["bounds"][3] for dot in dots)-min(dot["bounds"][1] for dot in dots)<=3,"ellipsis dots have different baselines"
    assert min(dot["bounds"][1] for dot in dots)>=narrowed["bounds"][3]-4,"omission marker is not a leading baseline ellipsis"
    return {**literal,"retained_characters":retained,
            "tail_glyph_mask_sha256":[glyph["mask_sha256"] for glyph in actual[3:]],
            "leading_ellipsis_components":[dot["bounds"] for dot in dots]}


def storage_pixel_report(value):
    if isinstance(value,dict):
        return {key:storage_pixel_report(item) for key,item in value.items() if key!="rgb_raster"}
    if isinstance(value,(list,tuple)):
        return [storage_pixel_report(item) for item in value]
    return value


def storage_separator_offset(table):
    # The left-aligned header stays at the column start when right-aligned
    # body keys move, and travels with the table across inspector margins.
    return table["separator"]-table["header"]["slot"]["bounds"][0]


def assert_storage_preference(table, expected):
    actual=storage_separator_offset(table)
    assert abs(actual-expected)<=1,{"expected_span":expected,"actual_span":actual,
                                  "separator":table["separator"]}


def storage_table_interactions(browser, url: str, output: Path):
    """Real WASM full defaults, suffix resizing, hovers and retained preferences."""
    page=browser.new_page(viewport={"width":1440,"height":1000})
    observe_webgpu(page)
    errors,posts=[],[]
    page.on("pageerror",lambda error:errors.append(str(error)))
    page.on("request",lambda request:posts.append(request.url)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    page.goto(url,wait_until="networkidle")
    status=page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:",timeout=60000)
    canvas=page.locator("#evm-canvas")
    word=STORAGE_SLOT_WORD
    assert len(word)==66
    slots=[0,0xf,0x1234,int(word,16)]

    def fixture(revision, long_slots=True, literal_word=word):
        expected={kind:{} for kind in ["storage","transient"]}
        instructions=[]
        fixture_slots=[*slots[:3],int(literal_word,16)] if long_slots else slots[:3]
        for index,slot in enumerate(fixture_slots):
            key=f"7f{slot:064x}" if slot>0xffff else f"61{slot:04x}"
            for kind,opcode,base in [("storage","55",0x3100),("transient","5d",0x4200)]:
                value=int(("cd" if kind=="storage" else "ef")*32,16) if index==3 else base+revision*16+index
                if index==1:
                    value=0xf
                elif index==2 and long_slots:
                    value=int(literal_word,16)
                expected[kind][hex(slot)]=hex(value)
                push=f"7f{value:064x}" if value>0xffff else f"61{value:04x}"
                instructions.append(push+key+opcode)
        return "".join(instructions)+"00",expected

    def submit_fixture(revision, long_slots=True, literal_word=word):
        code,expected=fixture(revision,long_slots,literal_word)
        reply=submit_bytecode(page,canvas,code,output/f"storage-table-input-{revision}.png")
        assert reply.status==200,reply.json()
        report=reply.json()["result"]["Ok"]
        assert report["status"]=="Converged" and len(report["states"])==1
        machine=report["states"][0]["exit"]
        account=next(account for account in report["stores"][machine["store"]]["accounts"]
                     if account["address"]==machine["frames"][0]["storage_address"])
        for kind in expected:
            assert {entry["slot"]:entry["value"]["constants"] for entry in account[kind]}=={
                slot:[value] for slot,value in expected[kind].items()},account[kind]
            for entry in account[kind]:
                literal=expected[kind][entry["slot"]]
                number=int(literal,16)
                assert entry["value"]=={
                    "summary":"{"+literal+"}","constants":[literal],
                    "known_zero":hex(((1<<256)-1)^number),"known_one":literal,
                    "nonzero":number!=0,"unsigned":{"lower":literal,"upper":literal},
                    "signed":{"lower":literal,"upper":literal},"congruence":{"Exact":literal},
                    "origins":["Constant"],"code_address_role":False,"identity":None,
                    "expression":None,"symbolic_limit":False},entry
        (output/f"storage-table-analysis-{revision}.json").write_text(json.dumps(report,indent=2)+"\n")
        return expected

    submit_fixture(-1,long_slots=False)
    canvas.click(position={"x":720,"y":600})
    page.keyboard.press("3")
    tab=inspector_frame_tab(page,canvas)
    page.mouse.move(720,tab[1]-3)
    page.mouse.down()
    page.mouse.move(720,350,steps=16)
    page.mouse.up()
    tab=inspector_frame_tab(page,canvas)
    toolbar_y=sum(tab[1::2])/2
    results={}
    preferences={}
    short_defaults={}
    for title,kind in [("Storage","storage"),("Transient","transient")]:
        select_inspector_tab(page,canvas,title,toolbar_y)
        canvas.click(position={"x":145,"y":tab[3]+14})
        expect(status).to_contain_text("Observed exit")
        short_defaults[kind]=storage_table_snapshot(page,canvas,output,
            f"storage-table-{kind}-short-default",tab[3],3)
    # These tables were actually rendered before report replacement and have
    # never been resized. Their default must now accommodate the new long key.
    expected=submit_fixture(0)
    for title,kind in [("Storage","storage"),("Transient","transient")]:
        select_inspector_tab(page,canvas,title,toolbar_y)
        canvas.click(position={"x":145,"y":tab[3]+14})
        expect(status).to_contain_text("Observed exit")
        initial=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-initial",tab[3],len(slots))
        for slot,row in zip(slots[:3],initial["rows"][:3],strict=True):
            assert_storage_literal_pixels(row["slot"],hex(slot))
            assert len(storage_glyph_cells(row["slot"]))==len(hex(slot)),f"short Slot {hex(slot)} was padded or clipped"
        full=initial["rows"][-1]["slot"]
        full_comparison=assert_storage_literal_pixels(full,word)
        full_value_comparison=assert_storage_literal_pixels(initial["rows"][2]["value"],"{"+word+"}")
        assert full["bounds"][2]-full["bounds"][0]>500,storage_pixel_report(full)
        full_hover=storage_hover_snapshot(page,canvas,output,f"storage-table-{kind}-full-slot-hover",initial["rows"][-1]["slot"])
        # This varied literal fits one proportional-font tooltip line. Require
        # its complete reviewed raster, including the original 0x prefix.
        assert len(full_hover["text"]["rows"])==1 and full_hover["ink"]["mask_size"]==[465,10],storage_pixel_report(full_hover)
        full_hover_comparison=assert_storage_reviewed_pixels(full_hover["raster"],"slot_hover",word,[476,23],
            "c24455921e771f31c4dff3e171ab62b12271f5c29b07f653f4492e6b0092488a")
        if kind=="transient":
            assert initial["rows"][0]["value"]["mask_sha256"]!=results["storage"]["initial"]["rows"][0]["value"]["mask_sha256"], "Transient rendered the persistent value"
        drag_storage_separator(page,initial,initial["separator"]+320)
        wide=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-wide",tab[3],len(slots))
        assert wide["separator"]-initial["separator"]>=280, storage_pixel_report((initial,wide))
        assert wide["rows"][-1]["slot"]["bounds"][2]-wide["rows"][-1]["slot"]["bounds"][0]>300,storage_pixel_report(wide)
        advance=(full["bounds"][2]-full["bounds"][0]
                 -(initial["rows"][1]["slot"]["bounds"][2]-initial["rows"][1]["slot"]["bounds"][0]))/(len(word)-3)
        current=wide
        suffixes=[]
        for retained,fraction in [(6,0.25),(13,0.625),(8 if kind=="storage" else 11,0.375)]:
            # Include both 2pt cell margins and half the 5pt column gap.
            # Aim inside the one-glyph fitting interval so divider raster
            # rounding cannot push a fractional drag below its intended width.
            target=current["header"]["slot"]["bounds"][0]+(retained+1.5)*advance+4+2.5+fraction
            drag_storage_separator(page,current,target)
            narrow=storage_table_snapshot(page,canvas,output,
                f"storage-table-{kind}-suffix-{retained}",tab[3],len(slots))
            assert abs(narrow["separator"]-target)<=3,(target,storage_pixel_report(narrow))
            suffix=assert_storage_suffix(narrow["rows"][-1]["slot"],word,retained)
            suffix.update(target_separator=target,table=narrow)
            suffixes.append(suffix)
            for first,last in zip(initial["rows"][:3],narrow["rows"][:3],strict=True):
                assert_storage_cell_text(first["slot"],last["slot"],"un-padded short Slot")
            current=narrow
        assert narrow["separator"]<initial["separator"]-20,storage_pixel_report((initial,narrow))
        assert_storage_cell_text(initial["rows"][-1]["slot"],wide["rows"][-1]["slot"],"complete widened Slot")
        value_comparisons=[assert_storage_cell_text(first["value"],last["value"])
            for first,last in zip(initial["rows"],narrow["rows"],strict=True)]
        preferences[kind]=storage_separator_offset(narrow)
        slot_hover=storage_hover_snapshot(page,canvas,output,f"storage-table-{kind}-slot-hover",narrow["rows"][-1]["slot"])
        # Full original text remains identical to the untruncated initial
        # key's hover, including every glyph of the original single line.
        assert len(slot_hover["text"]["rows"])==1 and slot_hover["ink"]["mask_sha256"]==full_hover["ink"]["mask_sha256"],storage_pixel_report(slot_hover)
        page.mouse.move(2,2)
        settle_gesture(page)
        value_hover=storage_hover_snapshot(page,canvas,output,f"storage-table-{kind}-value-hover",narrow["rows"][-1]["value"])
        # Retain the eight original numeric/provenance rows, plus the new
        # proved-nonzero row. Its wire fact was checked against the full U256.
        nonzero_rows=int(int(expected[kind][hex(slots[-1])],16)!=0)
        assert len(value_hover["text"]["rows"])==8+nonzero_rows and value_hover["text"]["muted_pixels"]>100,storage_pixel_report(value_hover)
        select_inspector_tab(page,canvas,"Frame",toolbar_y)
        frame=inspector_frame_tab(page,canvas)
        page.mouse.move(720,frame[1]-3)
        page.mouse.down()
        page.mouse.move(720,680,steps=16)
        page.mouse.up()
        small_tab=inspector_frame_tab(page,canvas)
        small_toolbar_y=sum(small_tab[1::2])/2
        select_inspector_tab(page,canvas,title,small_toolbar_y)
        canvas.click(position={"x":120,"y":small_tab[3]+80})
        page.mouse.move(80,small_tab[3]+115)
        page.mouse.wheel(0,600)
        settle_gesture(page)
        page.mouse.move(2,2)
        scrolled_png=canvas.screenshot(path=str(output/f"storage-table-{kind}-scrolled.png"))
        scrolled=storage_table_pixels(page,scrolled_png,
            (4,small_tab[3]+52,1416,970))
        assert_storage_preference(scrolled,preferences[kind])
        scrolled_value=assert_storage_cell_text(narrow["rows"][-1]["value"],scrolled["rows"][-1]["value"])
        page.mouse.move(80,small_tab[3]+115)
        page.mouse.wheel(0,-600)
        settle_gesture(page)
        canvas.click(position={"x":120,"y":small_tab[3]+80})
        select_inspector_tab(page,canvas,"Frame",small_toolbar_y)
        frame=inspector_frame_tab(page,canvas)
        page.mouse.move(720,frame[1]-3)
        page.mouse.down()
        page.mouse.move(720,350,steps=16)
        page.mouse.up()
        tab=inspector_frame_tab(page,canvas)
        toolbar_y=sum(tab[1::2])/2
        select_inspector_tab(page,canvas,title,toolbar_y)
        scroll_returned=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-scroll-return",tab[3],len(slots))
        assert_storage_preference(scroll_returned,preferences[kind])
        results[kind]={"expected":expected[kind],"short_default":short_defaults[kind],
                       "full_slot_raster":full,"full_slot_comparison":full_comparison,"full_value_comparison":full_value_comparison,
                       "full_slot_hover":full_hover,"full_hover_comparison":full_hover_comparison,
                       "suffixes":suffixes,"initial":initial,"wide":wide,"narrow":narrow,
                       "slot_hover":slot_hover,"value_hover":value_hover,"scrolled":scrolled,"scroll_return":scroll_returned}
        results[kind].update(value_comparisons=value_comparisons,scrolled_value=scrolled_value)

    # Returning from another inspector tab, workspace views, hidden/maximized
    # inspector and a new report must preserve each independently dragged width.
    for title,kind in [("Storage","storage"),("Transient","transient")]:
        select_inspector_tab(page,canvas,"Memory",toolbar_y)
        select_inspector_tab(page,canvas,title,toolbar_y)
        returned=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-tab-return",tab[3],len(slots))
        assert_storage_preference(returned,preferences[kind])
        for key in ["0","1","2","3"]:
            page.keyboard.press(key)
            rendered_frame(page)
        viewed=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-view-return",tab[3],len(slots))
        assert_storage_preference(viewed,preferences[kind])
        page.keyboard.press("Control+j")
        expect(status).to_contain_text("Inspector hidden")
        page.keyboard.press("Control+j")
        expect(status).to_contain_text(f"Inspector: {title}")
        hidden=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-hidden-return",tab[3],len(slots))
        assert_storage_preference(hidden,preferences[kind])
        select_inspector_tab(page,canvas,"Frame",toolbar_y)
        frame=inspector_frame_tab(page,canvas)
        inspector_title_action(page,canvas,"Maximize",frame)
        maximized_tab=inspector_frame_tab(page,canvas)
        select_inspector_tab(page,canvas,title,sum(maximized_tab[1::2])/2)
        maximized=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-maximized",maximized_tab[3],len(slots))
        assert_storage_preference(maximized,preferences[kind])
        select_inspector_tab(page,canvas,"Frame",sum(maximized_tab[1::2])/2)
        inspector_title_action(page,canvas,"Restore",inspector_frame_tab(page,canvas))
        tab=inspector_frame_tab(page,canvas)
        toolbar_y=sum(tab[1::2])/2
        select_inspector_tab(page,canvas,title,toolbar_y)
        restored=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-restored",tab[3],len(slots))
        assert_storage_preference(restored,preferences[kind])
        results[kind].update(tab_return=returned,view_return=viewed,hidden_return=hidden,
                             maximized=maximized,restored=restored)
    assert len(posts)==3,"table inspection unexpectedly reran analysis"
    expected_new=submit_fixture(1)
    select_inspector_tab(page,canvas,"Frame",toolbar_y)
    tab=inspector_frame_tab(page,canvas)
    for title,kind in [("Storage","storage"),("Transient","transient")]:
        select_inspector_tab(page,canvas,title,toolbar_y)
        canvas.click(position={"x":145,"y":tab[3]+14})
        expect(status).to_contain_text("Observed exit")
        updated=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-new-report",tab[3],len(slots))
        assert_storage_preference(updated,preferences[kind])
        assert updated["rows"][0]["value"]["mask_sha256"]!=results[kind]["narrow"]["rows"][0]["value"]["mask_sha256"], "new report retained stale values"
        results[kind].update(new_report=updated,expected_new=expected_new[kind])
    assert len(posts)==4,"expected startup, short fixture, long fixture and one controlled new report"
    # Submit a real incorrect last digit, after the complete original matrix.
    # Its Value has the same dimensions; the RGB oracle must reject its tail.
    wrong_word=word[:-1]+"e"
    assert wrong_word[:-1]==word[:-1] and wrong_word[-1]!=word[-1]
    submit_fixture(2,literal_word=wrong_word)
    select_inspector_tab(page,canvas,"Storage",toolbar_y)
    canvas.click(position={"x":145,"y":tab[3]+14})
    expect(status).to_contain_text("Observed exit")
    wrong=storage_table_snapshot(page,canvas,output,"storage-table-wrong-last-glyph",tab[3],len(slots))
    wrong_value=wrong["rows"][2]["value"]
    reference=storage_reviewed_raster("full_value","{"+word+"}",[531,12],
        "441542c8fa375969f42f890a0240ae01222ff506ba5da7a71cfbee95695caa2e")
    assert wrong_value["mask_size"]==reference["mask_size"],storage_pixel_report(wrong_value)
    first,last=base64.b64decode(reference["rgb_raster"]),base64.b64decode(wrong_value["rgb_raster"])
    differences=[abs(a-b) for a,b in zip(first,last,strict=True)]
    changed=[(i//3)%531 for i,difference in enumerate(differences) if difference>2]
    assert changed and min(changed)>=531-30,"negative fixture changed more than its final glyph"
    try:
        assert_storage_literal_pixels(wrong_value,"{"+word+"}")
    except AssertionError as error:
        assert "maximum RGB difference" in str(error),str(error)
        negative={"actual_text":"{"+wrong_word+"}","expected_text":"{"+word+"}",
                  "mask_size":wrong_value["mask_size"],"maximum_rgb_difference":max(differences),
                  "changed_tail_columns":[min(changed),max(changed)],"rejection":str(error)}
    else:
        raise AssertionError("reviewed complete Value accepted an incorrect last glyph")
    assert len(posts)==5,"expected exactly one additional real negative analysis"
    assert not errors,errors
    result=storage_pixel_report({"stores":results,"wrong_last_glyph":negative,
        "analysis_submissions":len(posts),"renderer":webgpu_evidence(page)})
    (output/"storage-table-report.json").write_text(json.dumps(result,indent=2)+"\n")
    page.close()
    return result


def storage_scroll_interactions(browser, url: str, output: Path):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors, posts = [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    page.goto(url, wait_until="networkidle")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    count = 96
    code = "".join(f"61{0x1000+i:04x}61{i:04x}5561{0x2000+i:04x}61{i:04x}5d"
                   for i in range(count)) + "00"
    reply = submit_bytecode(page, canvas, code, output / "storage-scroll-input.png")
    report = reply.json()["result"]["Ok"]
    assert report["status"] == "Converged" and len(report["states"]) == 1
    machine = report["states"][0]["exit"]
    account = next(account for account in report["stores"][machine["store"]]["accounts"]
                   if account["address"] == machine["frames"][0]["storage_address"])
    store=report["stores"][machine["store"]]
    graph=report["symbolic"]["nodes"]
    ordered_roots={}
    for kind, value_base in [("storage", 0x1000), ("transient", 0x2000)]:
        assert len(account[kind]) == count
        last = next(entry for entry in account[kind] if int(entry["slot"], 16) == count - 1)
        assert last["value"]["constants"] == [hex(value_base + count - 1)]
        # The new prefix describes this shared ordered DAG, independently of
        # sparse observations. Preserve every update and the unknown/zero base.
        node=account[kind+"_root"]["node"]
        for slot in reversed(range(count)):
            update=graph[node]["Store"]
            assert graph[update["key"]["node"]]=={"Constant":hex(slot)},update
            assert graph[update["value"]["node"]]=={"Constant":hex(value_base+slot)},update
            node=update["previous"]["node"]
        base=graph[node]
        default=account[kind+"_default"]
        if kind=="storage":
            source=base["SlotsInput"]
            assert source["bindings"]==[] and "Fresh" in graph[source["source"]["token"]["node"]],source
            assert int(source["source"]["namespace"],16)==int(account["address"],16)<<8,source
            assert default["constants"] is None and default["known_zero"]==default["known_one"]=="0x0",default
            assert default["unsigned"]=={"lower":"0x0","upper":hex((1<<256)-1)},default
            assert default["signed"]=={"lower":hex(1<<255),"upper":hex((1<<255)-1)},default
            assert default["congruence"]=="Any" and default["origins"] is None and not default["nonzero"],default
        else:
            assert base=="SlotsZero" and default["constants"]==["0x0"],(base,default)
            assert default["known_zero"]==hex((1<<256)-1) and default["known_one"]=="0x0",default
            assert default["unsigned"]==default["signed"]=={"lower":"0x0","upper":"0x0"},default
            assert default["congruence"]=={"Exact":"0x0"} and default["origins"]==["Constant"] and not default["nonzero"],default
        assert not default["code_address_role"] and default["identity"] is None and default["expression"] is None and not default["symbolic_limit"],default
        assert store[kind+"_facts"]==[],store[kind+"_facts"]
        ordered_roots[kind]={"root":account[kind+"_root"],"writes":count,"base_node":node,"base":base,"default":default}
    (output / "storage-scroll-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    canvas.click(position={"x": 720, "y": 600})
    page.keyboard.press("3")
    tab = inspector_frame_tab(page, canvas)
    inspector_title_action(page, canvas, "Maximize", tab)
    expect(status).to_contain_text(re.compile("maximized", re.IGNORECASE))
    tab = inspector_frame_tab(page, canvas)
    toolbar_y = (tab[1] + tab[3]) / 2
    evidence = {}
    # Exact glyph masks from the reviewed DPR1 "0x5f" and "{0x105f}" /
    # "{0x205f}" crops. Separate cells preserve the old exact-text acceptance
    # despite the adjustable gap, without allowing an off-by-one final slot.
    last_cell_hashes = {
        "slot": "1f94c5cbc9d80be833ac49b7540e5baa49cd503a8144a920697946262ad7e2d8",
        "storage": "fd1ab3bd3503fbd583c7f1443389c9cd9c436bfc915f8bb5ae4d0b187b5a5f22",
        "transient": "01a9f16f529282cffa2f49e72b9c1686e9ca3d452e2ce788636eaa5c83592176",
    }
    for title, kind in [("Storage", "storage"), ("Transient", "transient")]:
        select_inspector_tab(page, canvas, title, toolbar_y)
        canvas.click(position={"x": 145, "y": tab[3] + 14})
        expect(status).to_contain_text("Observed exit")
        page.mouse.move(2, 2)
        settle_gesture(page)
        default_png = canvas.screenshot(path=str(output / f"{kind}-facts-default-width.png"))
        default_table = storage_table_pixels(page, default_png,
            (4,tab[3]+52,page.viewport_size["width"]-24,page.viewport_size["height"]-30))
        assert default_table["header"]["slot"] and default_table["header"]["value"],storage_pixel_report(default_table)
        # Preserve the reviewed final-row hashes at their original Value x=138.
        # The larger default moves Value to x=531, where one unchanged glyph
        # sample rounds from 127 to 128 and crosses the exact ink-mask threshold.
        # A real native drag also lets this scroll case retain an explicit width.
        drag_storage_separator(page,default_table,134.5)
        page.mouse.move(2,2)
        settle_gesture(page)
        collapsed = canvas.screenshot(path=str(output / f"{kind}-facts-collapsed.png"))
        table = storage_table_pixels(page, collapsed,
            (4,tab[3]+52,page.viewport_size["width"]-24,page.viewport_size["height"]-30))
        assert abs(table["separator"]-134.5)<=1,storage_pixel_report(table)
        assert table["header"]["slot"] and table["header"]["value"],storage_pixel_report(table)
        # Address, ordered-state root, closed account header and base default
        # precede the measured table. The new DAG row shifts the old click/crop.
        collapsed_prefix_region=(4,tab[3]+55,600,table["table_bounds"][1])
        collapsed_prefix=text_pixels(page,collapsed,collapsed_prefix_region)
        assert len(collapsed_prefix["rows"])==4,"account facts were already expanded or the ordered-state prefix is missing"
        address_band,root_band,account_band,default_band=collapsed_prefix["rows"]
        collapsed_facts_region=(145,account_band[1]+1,650,default_band[0])
        collapsed_facts = text_pixels(page, collapsed, collapsed_facts_region)
        assert not collapsed_facts["rows"], "account facts were already expanded before the header click"
        canvas.click(position={"x":120,"y":sum(account_band)/2})
        page.mouse.move(2, 2)
        settle_gesture(page)
        expanded = canvas.screenshot(path=str(output / f"{kind}-facts-expanded.png"))
        expanded_table=storage_table_pixels(page,expanded,
            (4,tab[3]+52,page.viewport_size["width"]-24,page.viewport_size["height"]-30))
        expanded_prefix=text_pixels(page,expanded,(4,tab[3]+55,600,expanded_table["table_bounds"][1]))
        assert len(expanded_prefix["rows"])==4+8,"header click did not preserve the prefix and reveal all eight account facts"
        assert expanded_prefix["rows"][:3]==collapsed_prefix["rows"][:3],(collapsed_prefix,expanded_prefix)
        facts_region=(145,account_band[1]+1,650,expanded_prefix["rows"][-1][0])
        expanded_facts = text_pixels(page, expanded, facts_region)
        assert len(expanded_facts["rows"]) == 8, "header click did not reveal all eight account facts"
        prefix_region=(4,address_band[0]-1,600,account_band[1]+1)
        prefix = text_pixels(page, expanded, prefix_region)
        assert len(prefix["rows"])==3,"account address, ordered-state root and full facts heading were not painted"
        save_png_crop(page, expanded, prefix_region, output / f"{kind}-prefix-expanded.png")
        # The wheel starts over the facts prefix. One shared scroll must carry
        # it all the way through the slot list, rather than trap it in 80 points.
        wheel_y=sum(expanded_facts["rows"][0])/2
        page.mouse.move(80,wheel_y)
        page.mouse.wheel(0, 6000)
        settle_gesture(page)
        page.mouse.move(2, 2)
        settled = canvas.screenshot(path=str(output / f"{kind}-last-slots.png"))
        scrolled_prefix = text_pixels(page, settled, prefix_region)
        assert prefix["hash"] != scrolled_prefix["hash"], "wheel remained trapped inside facts; account prefix did not move"
        save_png_crop(page, settled, prefix_region, output / f"{kind}-prefix-scrolled.png")
        scrolled_table = storage_table_pixels(page, settled,
            (4,tab[3]+52,page.viewport_size["width"]-24,page.viewport_size["height"]-30))
        assert abs(scrolled_table["separator"]-table["separator"])<=1,"scrolling reset the Slot width"
        final_cells=scrolled_table["rows"][-1]
        last_row_region = (4,final_cells["band"][0]-3,
                           scrolled_table["separator"]+100,final_cells["band"][1]+3)
        last_row = text_pixels(page, settled, last_row_region)
        save_png_crop(page, settled, last_row_region, output / f"{kind}-last-row.png")
        assert len(last_row["rows"]) == 1 and last_row["white_pixels"] > 100, "last slot row was clipped or blank"
        for cell,expected_hash in [("slot",last_cell_hashes["slot"]),("value",last_cell_hashes[kind])]:
            actual=final_cells[cell]
            assert actual and actual["mask_sha256"]==expected_hash,f"last rendered {title} {cell} changed: {storage_pixel_report(actual)}"
            left,top,right,bottom=actual["bounds"]
            save_png_crop(page,settled,(left-1,top-1,right+1,bottom+1),output/f"{kind}-last-{cell}.png")
        page.mouse.move(80,wheel_y)
        page.mouse.wheel(0, -6000)
        settle_gesture(page)
        page.mouse.move(2, 2)
        returned = canvas.screenshot(path=str(output / f"{kind}-facts-returned.png"))
        returned_prefix = text_pixels(page, returned, prefix_region)
        assert prefix["hash"] == returned_prefix["hash"], "returning to the top did not restore account facts"
        save_png_crop(page, returned, prefix_region, output / f"{kind}-prefix-returned.png")
        evidence[kind] = {"entries": count, "expected_last_slot": hex(count - 1),
            "expected_last_value": account[kind][-1]["value"]["constants"],
            "expected_last_cell_mask_sha256": {"slot":last_cell_hashes["slot"],"value":last_cell_hashes[kind]},
            "facts_region": facts_region, "collapsed_facts": collapsed_facts,
            "collapsed_facts_region":collapsed_facts_region,
            "collapsed_prefix":collapsed_prefix,"expanded_prefix":expanded_prefix,
            "account_header_band":account_band,"ordered_header_band":root_band,"ordered_state":ordered_roots[kind],
            "expanded_facts": expanded_facts, "last_row_region": last_row_region, "last_row": last_row,
            "default_table":default_table,"table":table,"scrolled_table":scrolled_table,"last_cells":final_cells,
            "prefix_region": prefix_region, "prefix": prefix,
            "scrolled_prefix": scrolled_prefix, "returned_prefix": returned_prefix,
            "expanded_screenshot": f"{kind}-facts-expanded.png",
            "scrolled_screenshot": f"{kind}-last-slots.png"}
    assert len(posts) == 2, "storage inspection reran analysis"
    assert not errors, errors
    result = storage_pixel_report({"stores": evidence, "analysis_submissions": len(posts),
        "renderer": webgpu_evidence(page)})
    (output / "storage-scroll-report.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result


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
    before = graph_screenshot_metrics(page, before_png, graph_region)
    assert before["bounds"], before
    page.mouse.move(before_panes["left"], 600)
    page.mouse.down()
    page.mouse.move(before_panes["left"] + 48, 600, steps=6)
    page.mouse.up()
    rendered_frame(page)
    after_panes, after_png = workspace_panes(page, canvas, output, "panes-after-divider")
    shift = after_panes["left"] - before_panes["left"]
    assert shift > 30, (before_panes, after_panes)
    after = graph_screenshot_metrics(page, after_png, (after_panes["left"] + 4, 160, after_panes["right"] - 4, 970))
    assert_translation(before, after, shift, 0, tolerance=1.0)
    resize_view(page, 1128, 700)
    canvas.screenshot(path=str(output / "panes-manual-clipped.png"))
    resize_view(page, 2560, 1000)
    restored_panes, restored_png = workspace_panes(page, canvas, output, "panes-manual-restored")
    for field in ["disassembly_width", "ssa_width"]:
        assert abs(after_panes[field] - restored_panes[field]) <= 1, (after_panes, restored_panes)
    restored = graph_screenshot_metrics(page, restored_png, (restored_panes["left"] + 4, 160, restored_panes["right"] - 4, 970))
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


def graph_mode(page, canvas, mode: str):
    # Product toolbar labels, exercised through real pointer events. Its first
    # content/navigation row stays unchanged; scopes occupy the second row.
    x = {"Blocks": 29, "States": 84, "Local": 133}[mode]
    canvas.click(position={"x": x, "y": 109})
    expect(page.locator("#analysis-status")).to_contain_text(f"CFG view: {mode}")
    settle_gesture(page)


def graph_scope(page, mode: str, nodes: int, shown: int, total: int):
    status = page.locator("#analysis-status")
    for phrase in [f"CFG view: {mode}", f"{nodes} shown nodes", f"{shown}/{total} states"]:
        expect(status).to_contain_text(phrase)
    match = re.search(r"CFG view: (\w+); (\d+) shown nodes; (\d+)/(\d+) states; (\d+) shown edges; (\d+) native edges; (\d+) hidden states; (\d+) boundary edges; ([^;]+)", status.text_content())
    assert match, status.text_content()
    fields = ["nodes", "shown_states", "total_states", "edges", "native_edges", "hidden_states", "boundary_edges"]
    return {"mode": match[1], **dict(zip(fields, map(int, match.group(2,3,4,5,6,7,8)), strict=True)), "scope": match[9]}


def symbol(base: str, index: int):
    return base + str(index).translate(str.maketrans("0123456789", "₀₁₂₃₄₅₆₇₈₉"))


def pick_graph_instance(page, canvas, state: int, query: str | None = None):
    # Real native TextEdit and virtual list coordinates from the egui geometry
    # probe. Search accepts an original state ID, not a display-node ordinal.
    canvas.click(position={"x": 200, "y": 108})
    page.keyboard.press("Control+A")
    page.keyboard.insert_text(query if query is not None else symbol("σ", state))
    settle_gesture(page)
    canvas.click(position={"x": 110, "y": 131})
    expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ', state)}; source {symbol('P', 0)}")
    settle_gesture(page)


def block_overview_interactions(browser, url: str, output: Path):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    initial_reply = complete_submission(page, initial.value)
    report = initial_reply.json()["result"]["Ok"]
    assert len(report["cfg"]) == len(report["ssa"]["blocks"]) == 5
    identities = {state["id"]: (state["program"], state["basic_block"], state["start_pc"]) for state in report["cfg"]}
    assert len(set(identities.values())) == 4
    aggregate_edges = {(identities[edge["from"]], identities[edge["to"]], edge["kind"]) for edge in report["edges"]}
    assert len(aggregate_edges) == 4
    submitted = []
    page.on("request", lambda request: submitted.append(request.post_data_json)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    canvas = page.locator("#evm-canvas")
    hide_inspector(page, canvas)
    page.keyboard.press("2")
    settle_gesture(page)
    blocks = graph_scope(page, "Blocks", 4, 5, 5)
    assert blocks["edges"] == 4 and blocks["native_edges"] == len(report["edges"])
    block_pixels = node_snapshot(page, canvas, output, "overview-blocks", measure_nodes=True)
    assert len(block_pixels["nodes"]) == 4, block_pixels
    graph_mode(page, canvas, "States")
    states = graph_scope(page, "States", 5, 5, 5)
    assert states["edges"] == states["native_edges"] == len(report["edges"])
    state_pixels = node_snapshot(page, canvas, output, "overview-states", measure_nodes=True)
    assert len(state_pixels["nodes"]) == 5, state_pixels
    graph_mode(page, canvas, "Local")
    local = graph_scope(page, "Local", 5, 5, 5)
    assert local["scope"] == "Cross-program neighborhood"
    canvas.screenshot(path=str(output / "overview-local-branch.png"))
    graph_mode(page, canvas, "Blocks")
    assert not submitted, "changing graph scope unexpectedly reran analysis"

    loop_reply = submit_bytecode(page, canvas, "5b600056", output / "overview-loop-input.png")
    assert loop_reply.status == 200 and "Ok" in loop_reply.json()["result"], loop_reply.json()
    loop = loop_reply.json()["result"]["Ok"]
    assert loop["metadata"]["limits"]["context_depth"] == 128
    assert loop["status"] == "Converged" and loop["ssa"]["complete"]
    assert len(loop["cfg"]) == len(loop["ssa"]["blocks"]) == 129
    assert {state["id"] for state in loop["cfg"]} == set(range(129))
    assert len(loop["edges"]) == 129
    settle_gesture(page)
    grouped = graph_scope(page, "Blocks", 1, 129, 129)
    assert grouped["edges"] == 1 and grouped["native_edges"] == 129
    one_card = node_snapshot(page, canvas, output, "overview-loop-129-instances", measure_nodes=True)
    assert len(one_card["nodes"]) == 1, one_card
    left, top, right, bottom = one_card["bounds"]
    canvas.click(position={"x": (left + right) / 2, "y": (top + bottom) / 2})
    settle_gesture(page)
    canvas.screenshot(path=str(output / "overview-loop-instances.png"))
    search_forms = [(127, "127"), (128, "σ₁₂₈"), (127, "sigma_127"), (128, "S128")]
    for index, (state, query) in enumerate(search_forms):
        if index:
            canvas.click(position={"x": 300, "y": 109})  # Instances toolbar
            settle_gesture(page)
        pick_graph_instance(page, canvas, state, query)
    canvas.screenshot(path=str(output / "overview-loop-symbol-search.png"))
    graph_scope(page, "Blocks", 1, 129, 129)
    # Expose both the native inspector and the full SSA table for the selected
    # instance. A virtualized table must finish its long focus animation.
    hide_inspector(page, canvas)  # toggle the initially hidden inspector on
    page.keyboard.press("3")
    page.wait_for_timeout(800)
    rendered_frame(page)
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Selected σ₁₂₈; source P₀")
    expect(status).to_contain_text("Inspector: Frame")
    expect(status).to_contain_text("Frame f₀")
    png = canvas.screenshot(path=str(output / "overview-loop-native-ssa.png"))
    ssa_pixels = screenshot_metrics(page, png, (4, 75, 1436, 760))
    assert ssa_pixels["selected_pixels"] > 100, "the selected σ₁₂₈ SSA block was not scrolled into view"
    selected = next(state for state in loop["states"] if state["state"] == 128)
    assert len(selected["entry"]["frames"][0]["context"]) == 128
    page.keyboard.press("2")
    settle_gesture(page)
    canvas.click(position={"x": 300, "y": 109})  # Instances toolbar
    settle_gesture(page)
    pick_graph_instance(page, canvas, 128)
    graph_mode(page, canvas, "Local")
    neighborhood = graph_scope(page, "Local", 3, 3, 129)
    assert neighborhood["hidden_states"] == 126 and neighborhood["boundary_edges"] == 1
    assert neighborhood["edges"] == 3
    expect(status).to_contain_text("Converged")
    canvas.screenshot(path=str(output / "overview-loop-local-boundary.png"))
    assert len(submitted) == 1, "instance selection or local display changed the backend analysis"
    assert not errors, errors
    result = {"blocks": blocks, "states": states, "local": local, "loop": grouped,
              "loop_local": neighborhood, "selected_native_state": 128,
              "block_pixels": block_pixels, "state_pixels": state_pixels,
              "loop_pixels": one_card, "selected_ssa_pixels": ssa_pixels["selected_pixels"],
              "renderer": webgpu_evidence(page)}
    (output / "overview-loop-analysis.json").write_text(json.dumps(loop, indent=2) + "\n")
    page.close()
    return result


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

        # Start in the product default SSA, then exercise both directions using
        # real toolbar controls. Status and measured node widths verify the
        # representation change independently of screenshot hashes.
        expect(status).to_contain_text("CFG nodes: SSA")
        canvas.click(position={"x": 28, "y": 86})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: Disasm")
        disasm = node_snapshot(page, canvas, output, f"{prefix}-nodes-disasm")
        require_selected_complete(disasm);require_selected_complete(safari_zoom)
        disasm_width = disasm["bounds"][2] - disasm["bounds"][0]
        assert disasm["hash"] != safari_zoom["hash"], "Disasm mode retained SSA pixels"
        assert safari_zoom["bounds"][2] - safari_zoom["bounds"][0] > disasm_width + 10, "Disasm mode did not replace the fixture's SSA definitions/effects"
        canvas.click(position={"x": 78, "y": 86})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: SSA")
        ssa = node_snapshot(page, canvas, output, f"{prefix}-nodes-ssa")
        require_selected_complete(ssa)
        assert ssa["hash"] != disasm["hash"], "SSA mode retained disassembly pixels"
        ssa_width = ssa["bounds"][2] - ssa["bounds"][0]
        assert ssa_width > disasm_width + 10, "SSA definitions/effects did not expand the fixture's rendered node"
        canvas.click(position={"x": 143, "y": 86})
        settle_gesture(page)
        expect(status).to_contain_text("CFG nodes: SSA")
        ssa_fit = node_snapshot(page, canvas, output, f"{prefix}-nodes-ssa-fit")
        require_selected_complete(ssa_fit)
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
                actual_width = measured["bounds"][2] - measured["bounds"][0]
                observed_zoom = float(re.search(r"; CFG zoom: ([0-9.]+)%", status.text_content())[1])
                assert abs(observed_zoom / fitted_zoom - scale) < 0.005, f"incorrect camera zoom at DPR {dpr}: {scale}: {observed_zoom}/{fitted_zoom}"
                # The inside stroke and antialiasing retain physical width;
                # tiny fill rectangles have a fixed pixel bias, independent of
                # their scale. Keep a visible selected fill and compare actual
                # pixels as well as the product's precise camera percentage.
                assert abs(actual_width - fitted_width * scale) <= 4.0, f"incorrect screenshot zoom at DPR {dpr}: {scale}: {measured}"
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
                before_node = graph_screenshot_metrics(page, before_png, graph_region)
                before_code = screenshot_metrics(page, before_png, region)
                assert before_node["bounds"], f"missing node before {name} scroll"
                page.mouse.move(*pointer)
                page.mouse.wheel(0, 220)
                settle_gesture(page)
                page.mouse.move(2, 2)
                settle_gesture(page)
                after_png = canvas.screenshot(path=str(output / f"adjacent-scroll-{name}.png"))
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
        browser.close()
    return results

RPC_PROVIDERS = [
    {"id": "pending-fixture", "name": "Pending acquisition"},
    {"id": "world-fixture", "name": "World snapshot"},
]
RPC_TEST_TOKEN = "rpc-value-shown-in-provider-selector"
USDC_ENTRY = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
WETH_ENTRY = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
USDT_ENTRY = "0xdAC17F958D2ee523a2206206994597C13D831ec7"
CUSTOM_ENTRY = "0x3333333333333333333333333333333333333333"


class EntryRpcFixture(RpcFixture):
    """Preset addresses execute tiny fixture code, never live token contracts."""
    def _result(self, method, params):
        account_methods = ("eth_getCode", "eth_getBalance", "eth_getTransactionCount", "eth_getStorageAt")
        if method in account_methods and params[0].lower() in (USDC_ENTRY.lower(), WETH_ENTRY.lower(), USDT_ENTRY.lower(), CUSTOM_ENTRY):
            assert params[-1] == {"blockHash": self.block_hash, "requireCanonical": True}
            return {"eth_getCode": "0x00", "eth_getBalance": "0x100000", "eth_getTransactionCount": "0x1", "eth_getStorageAt": "0x" + "00" * 32}[method]
        return super()._result(method, params)


def automatic_rpc_entry(page, url: str, provider, output: Path, name: str):
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    reply = complete_submission(page, initial.value)
    request = reply.request.post_data_json
    assert request["input"] == {"Rpc": {"provider_id": provider["id"], "address": USDC_ENTRY, "block": "Latest", "accounts": []}}, request
    assert reply.status == 200 and "Ok" in reply.json()["result"], reply.json()
    report = reply.json()["result"]["Ok"]
    assert report["metadata"]["entry_address"].lower() == USDC_ENTRY.lower()
    assert len(report["cfg"]) == 1 and report["programs"][0]["bytecode"] in ("00", "0x00"), "preset fixture unexpectedly analyzed live code"
    (output / f"{name}-automatic-usdc.json").write_text(json.dumps({"request": request, "entry_address": report["metadata"]["entry_address"], "status": report["status"]}, indent=2) + "\n")
    return request


def wait_for_job(page, url, job_id, predicate, timeout_seconds=15):
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        response = page.request.get(f"{url}/api/tasks/{job_id}")
        assert response.ok, response.text()
        snapshot = response.json()["result"]["Ok"]
        if predicate(snapshot):
            return snapshot
        page.wait_for_timeout(50)
    raise AssertionError(f"task did not reach the expected state: {snapshot}")


def visible_seconds(page, label):
    text = page.locator("#analysis-status").text_content()
    match = re.search(rf"(?:^|; ){re.escape(label)}: ([0-9.]+) (ms|s)(?:;|$)", text)
    assert match, f"missing {label} in accessible timing: {text}"
    return float(match[1]) / (1000 if match[2] == "ms" else 1)


def timing_interactions(browser, args):
    """Measure real queued work, delayed result transport and terminal freezing."""
    with EntryRpcFixture() as rpc, TemporaryDirectory(prefix="web-timings-") as directory:
        providers = [{"id": "timing-fixture", "name": "Timing fixture", "endpoint": rpc.endpoint}]
        configuration = Path(directory) / "providers.json"
        configuration.write_text(json.dumps({"providers": providers}) + "\n")
        with analysis_server(args, configuration) as url:
            page = browser.new_page(viewport={"width": 1440, "height": 1000})
            observe_webgpu(page)
            baseline = automatic_rpc_entry(page, url, providers[0], args.output, "timing-startup")
            canvas = page.locator("#evm-canvas")
            status = page.locator("#analysis-status")
            # Two real pending RPC reads occupy both workers. The visible task
            # must queue, rather than estimating queue time from a delayed poll.
            rpc.block_method = "eth_getCode"
            request = json.loads(json.dumps(baseline))
            request["input"]["Rpc"]["address"] = rpc.root_address
            occupied = []
            for _ in range(2):
                response = page.request.post(url + "/api/tasks", data=request)
                assert response.status == 202, response.text()
                occupied.append(response.json()["result"]["Ok"]["id"])
            for job_id in occupied:
                wait_for_job(page, url, job_id,
                             lambda job: job["progress"]["phase"] == "Acquiring")
            admitted = submit_rpc(page, canvas, rpc, args.output, "timing-queued", 0, providers)
            job_id = admitted.json()["result"]["Ok"]["id"]
            queued = wait_for_job(page, url, job_id, lambda job: job["state"] == "Queued")
            expect(status).to_contain_text("Queued")
            before = visible_seconds(page, "Elapsed")
            page.wait_for_timeout(400)
            after = visible_seconds(page, "Elapsed")
            assert after > before, "queued user wait stopped advancing"
            queued_later = wait_for_job(page, url, job_id, lambda job: job["state"] == "Queued")
            assert queued_later["timings"]["queue_ns"] > queued["timings"]["queue_ns"]
            assert queued_later["timings"]["processing_ns"] == 0
            canvas.screenshot(path=str(args.output / "timing-queued.png"))
            page.set_viewport_size({"width": 390, "height": 844})
            rendered_frame(page)
            assert visible_seconds(page, "Elapsed") >= after
            canvas.screenshot(path=str(args.output / "timing-queued-narrow.png"))
            page.set_viewport_size({"width": 1440, "height": 1000})
            rendered_frame(page)
            # Acknowledged cancellation frees one actual worker. The displayed
            # job then reaches its own pending RPC, with a frozen queue interval.
            assert page.request.delete(f"{url}/api/tasks/{occupied[0]}").ok
            wait_for_job(page, url, occupied[0], lambda job: job["state"] == "Cancelled")
            running = wait_for_job(page, url, job_id,
                                   lambda job: job["progress"]["phase"] == "Acquiring")
            expect(status).to_contain_text("Running")
            page.wait_for_timeout(200)
            running_later = wait_for_job(page, url, job_id,
                                         lambda job: job["progress"]["phase"] == "Acquiring")
            assert running_later["timings"]["queue_ns"] == running["timings"]["queue_ns"]
            assert running_later["timings"]["acquiring_ns"] > running["timings"]["acquiring_ns"]
            # Make CFG the current view. Completion must include its first real
            # layout, while later focus/zoom actions must not restart the timer.
            page.keyboard.press("2")
            rendered_frame(page)
            held_results = []
            page.route("**/api/tasks/*/result", lambda route: held_results.append(route))
            rpc.release.set()
            completed = wait_for_job(page, url, job_id, lambda job: job["state"] == "Completed")
            deadline = time.monotonic() + 15
            while not held_results and time.monotonic() < deadline:
                page.wait_for_timeout(50)
            assert len(held_results) == 1, "browser did not fetch exactly one result"
            expect(status).to_contain_text("Loading result")
            fetch_before = visible_seconds(page, "Elapsed")
            page.wait_for_timeout(400)
            fetch_after = visible_seconds(page, "Elapsed")
            assert fetch_after > fetch_before, "user wait stopped before result transport finished"
            frozen_backend = wait_for_job(page, url, job_id, lambda job: job["state"] == "Completed")
            assert frozen_backend["timings"] == completed["timings"]
            held_results[0].continue_()
            expect(status).to_contain_text("Ready:", timeout=60000)
            expect(status).to_contain_text("Frontend preparation:")
            report = page.request.get(f"{url}/api/tasks/{job_id}/result").json()["result"]["Ok"]
            backend = measured_backend_timings(report)
            assert backend == completed["timings"]
            assert backend["queue_ns"] > 0 and backend["acquiring_ns"] > 0 and backend["pinning_ns"] > 0
            assert report["acquisition"]["rounds"] > 1, "fixture did not exercise RPC analysis reruns"
            total = visible_seconds(page, "Total wait")
            preparation = visible_seconds(page, "Frontend preparation")
            assert total >= fetch_after, "completed user wait omitted the delayed result"
            assert preparation >= 0.3, "frontend preparation omitted the held transport"
            assert visible_seconds(page, "Result transfer") >= 0.3
            assert visible_seconds(page, "JSON parsing") >= 0
            assert visible_seconds(page, "First view setup") >= 0
            page.wait_for_timeout(300)
            page.keyboard.press("f")
            rendered_frame(page)
            assert visible_seconds(page, "Total wait") == total, "completed user wait kept advancing"
            assert visible_seconds(page, "Frontend preparation") == preparation
            canvas.click(position={"x": 75, "y": 978})
            expect(status).to_contain_text("Inspector: Timing")
            inspector_title_action(page, canvas, "Maximize", (0, 778, 0, 778))
            expect(status).to_contain_text(re.compile("maximized", re.IGNORECASE))
            canvas.screenshot(path=str(args.output / "timing-completed.png"))
            page.unroute_all(behavior="wait")
            rpc.release.clear()
            rpc.started.clear()
            rpc.disconnected.clear()
            cancelled = submit_rpc(page, canvas, rpc, args.output, "timing-cancel", 0, providers)
            cancelled_id = cancelled.json()["result"]["Ok"]["id"]
            assert rpc.started.wait(5), "cancelled task never entered its own real RPC read"
            expect(status).to_contain_text("Elapsed:")
            with page.expect_response(lambda response: response.request.method == "DELETE"
                                      and response.url.endswith(f"/api/tasks/{cancelled_id}")):
                canvas.click(position={"x": 30, "y": 988})
            expect(status).to_contain_text("Cancelled", timeout=22000)
            cancellation_wait = visible_seconds(page, "Total wait")
            cancelled_backend = wait_for_job(page, url, cancelled_id, lambda job: job["state"] == "Cancelled")
            page.wait_for_timeout(300)
            assert visible_seconds(page, "Total wait") == cancellation_wait
            assert wait_for_job(page, url, cancelled_id, lambda job: job["state"] == "Cancelled")["timings"] == cancelled_backend["timings"]
            assert rpc.disconnected.wait(5), "cancelled acquisition did not close its connection"
            canvas.screenshot(path=str(args.output / "timing-cancelled.png"))
            # A typed worker failure freezes the new task, while the successful
            # report still retains its original backend timing evidence.
            open_input(page)
            canvas.click(position={"x": 380, "y": 361})  # Bytecode in the taller RPC modal
            page.keyboard.press("Escape")
            settle_gesture(page)
            failed = submit_bytecode(page, canvas, "not-bytecode", args.output / "timing-error-input.png")
            assert failed.json()["result"]["Err"]["code"] == "InvalidBytecode"
            failure_wait = visible_seconds(page, "Total wait")
            page.wait_for_timeout(300)
            assert visible_seconds(page, "Total wait") == failure_wait
            frozen_report = page.request.get(f"{url}/api/tasks/{job_id}/result").json()["result"]["Ok"]
            assert frozen_report["metadata"]["timings"] == backend
            canvas.screenshot(path=str(args.output / "timing-failed.png"))
            result = {"queue_samples": [queued["timings"], queued_later["timings"]],
                      "running_samples": [running["timings"], running_later["timings"]],
                      "completed": backend, "rpc_rounds": report["acquisition"]["rounds"],
                      "user_total_s": total, "frontend_preparation_s": preparation,
                      "cancellation_wait_s": cancellation_wait, "cancelled_backend": cancelled_backend["timings"],
                      "failure_wait_s": failure_wait, "renderer": webgpu_evidence(page)}
            (args.output / "timing-acceptance-report.json").write_text(json.dumps(result, indent=2) + "\n")
            page.close()
            return result


def http_transport_interactions(browser, url: str, output: Path):
    """Use real page fetches for HTTP persistence and typed route boundaries."""
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    network = page.context.new_cdp_session(page)
    tagged_requests = {}
    connections = []

    def tag_request(event):
        headers = {name.lower(): value for name, value in event["request"].get("headers", {}).items()}
        tag = headers.get("x-evm-http-case")
        if tag is not None:
            tagged_requests[event["requestId"]] = tag

    def observe_response(event):
        tag = tagged_requests.get(event["requestId"])
        if tag is not None:
            response = event["response"]
            connections.append({"case": tag, "connection_id": response["connectionId"],
                                "reused": response["connectionReused"], "status": response["status"]})

    network.on("Network.requestWillBeSent", tag_request)
    network.on("Network.responseReceived", observe_response)
    network.send("Network.enable")
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    complete_submission(page, initial.value)
    request = initial.value.request.post_data_json

    def fetch(path, *, method="GET", body=None, content_type=None, tag=None):
        response = page.evaluate("""async input => {
            const headers = {};
            if (input.content_type) headers['Content-Type'] = input.content_type;
            if (input.tag) headers['X-Evm-Http-Case'] = input.tag;
            const response = await fetch(input.path, {method: input.method, headers,
                ...(input.body === null ? {} : {body: input.body})});
            const text = await response.text();
            return {status: response.status, headers: Object.fromEntries(response.headers.entries()), text};
        }""", {"path": path, "method": method, "body": body,
                 "content_type": content_type, "tag": tag})
        # Job IDs are u64; JavaScript's JSON.parse would round values above
        # 2**53 before the next task URL is constructed. Keep the wire text.
        response["reply"] = json.loads(response["text"]) \
            if response["headers"].get("content-type", "").startswith("application/json") else None
        return response

    providers = [fetch("/api/rpc-providers", tag=f"keepalive-{index}") for index in range(2)]
    assert all(response["status"] == 200 and response["reply"] == {"result": {"Ok": []}}
               for response in providers)
    assert len(connections) == 2, connections
    assert any(connection["reused"] for connection in connections), \
        f"sequential browser requests never reused an HTTP connection: {connections}"
    shell = fetch("/")
    assert shell["status"] == 200 and "evm-canvas" in shell["text"]
    assert shell["headers"]["content-type"].startswith("text/html")
    for response in [*providers, shell]:
        assert response["headers"]["cache-control"] == "no-store"
        assert response["headers"]["x-content-type-options"] == "nosniff"

    admitted = fetch("/api/tasks", method="POST", body=json.dumps(request), content_type="application/json")
    assert admitted["status"] == 202 and "Ok" in admitted["reply"]["result"], admitted
    job_id = admitted["reply"]["result"]["Ok"]["id"]
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        polled = fetch(f"/api/tasks/{job_id}")
        assert polled["status"] == 200, polled
        snapshot = polled["reply"]["result"]["Ok"]
        if snapshot["state"] == "Completed":
            break
        assert snapshot["state"] in ("Queued", "Running"), snapshot
        page.wait_for_timeout(30)
    else:
        raise AssertionError(f"browser fetch task did not complete: {snapshot}")
    result = fetch(f"/api/tasks/{job_id}/result")
    assert result["status"] == 200, result
    timing = measured_backend_timings(result["reply"]["result"]["Ok"])
    assert timing == snapshot["timings"]

    errors = {}
    for name, path, method, body, content_type, http_status, code in [
        ("invalid_json", "/api/tasks", "POST", "{", "application/json", 400, "InvalidRequest"),
        ("wrong_content_type", "/api/tasks", "POST", json.dumps(request), "text/plain", 400, "InvalidRequest"),
        ("oversized_body", "/api/tasks", "POST", " " * 262145, "application/json", 413, "RequestTooLarge"),
        ("encoded_path_traversal", "/%2e%2e%2fCargo.toml", "GET", None, None, 404, "NotFound"),
    ]:
        response = fetch(path, method=method, body=body, content_type=content_type)
        assert response["status"] == http_status, (name, response)
        error = response["reply"]["result"]["Err"]
        assert error["code"] == code, (name, error)
        assert response["headers"]["cache-control"] == "no-store"
        assert response["headers"]["x-content-type-options"] == "nosniff"
        errors[name] = {"http_status": response["status"], "code": error["code"],
                        "details": error["details"]}

    evidence = {"browser_connections": connections, "typed_errors": errors,
                "task_id": job_id, "backend_timings": timing,
                "static_headers": shell["headers"], "renderer": webgpu_evidence(page)}
    (output / "http-transport-report.json").write_text(json.dumps(evidence, indent=2) + "\n")
    page.close()
    return evidence


def open_rpc_input(page, canvas, *, from_bytecode=False):
    open_input(page)
    if from_bytecode:
        canvas.click(position={"x": 465, "y": 386})
        settle_gesture(page)


def select_rpc_provider(page, canvas, index: int, output: Path, name: str):
    # Real packaged 1440x1000 RPC modal: regular controls retain the
    # original 20-point rows and 23-point stride.
    canvas.click(position={"x": 600, "y": 402})
    rendered_frame(page)
    canvas.screenshot(path=str(output / f"{name}-providers.png"))
    canvas.click(position={"x": 460, "y": 428 + index * 23})
    settle_gesture(page)


def submit_rpc(page, canvas, rpc, output: Path, name: str, provider_index: int, providers):
    expect(page.locator("#analysis-status")).to_contain_text(
        f"RPC provider: {providers[0]['name']} ({providers[0]['endpoint']})")
    open_rpc_input(page, canvas)
    if provider_index:
        select_rpc_provider(page, canvas, provider_index, output, name)
    provider = providers[provider_index]
    expect(page.locator("#analysis-status")).to_contain_text(
        f"RPC provider: {provider['name']} ({provider['endpoint']})")
    canvas.click(position={"x": 600, "y": 448})
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


def world_graph_scopes(page, canvas, report, child, child_state, output: Path):
    # Keep the native child selected while narrowing visibility. Switching to
    # Local must cross the real CALL/RETURN links even with P1 still selected.
    states = report["cfg"]
    edges = report["edges"]
    child_ids = {state["id"] for state in states if state["program"] == child["id"]}
    assert len(states) == 3 and child_ids == {child_state["id"]}
    boundary = [edge for edge in edges if (edge["from"] in child_ids) != (edge["to"] in child_ids)]
    assert len(boundary) == 2 and {edge["kind"] for edge in boundary} == {"Call", "Return"}
    submitted = []
    page.on("request", lambda request: submitted.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    canvas.click(position={"x": page.viewport_size["width"] - 180, "y": 12})  # World directory
    hide_inspector(page, canvas)
    page.keyboard.press("2")
    settle_gesture(page)
    graph_mode(page, canvas, "States")
    full = graph_scope(page, "States", 3, 3, 3)
    assert full["scope"] == "All programs" and full["native_edges"] == len(edges)
    canvas.click(position={"x": 205, "y": 109})  # native program ComboBox
    rendered_frame(page)
    canvas.screenshot(path=str(output / "world-graph-program-menu.png"))
    canvas.click(position={"x": 205, "y": 181})  # P1 · Runtime (native menu geometry)
    settle_gesture(page)
    filtered = graph_scope(page, "States", 1, 1, 3)
    assert filtered["scope"] == symbol("P", child["id"])
    assert filtered["edges"] == filtered["native_edges"] == 0
    assert filtered["hidden_states"] == 2 and filtered["boundary_edges"] == 2
    status = page.locator("#analysis-status")
    expect(status).to_contain_text(f"Selected {symbol('σ', child_state['id'])}; source {symbol('P', child['id'])}")
    child_pixels = node_snapshot(page, canvas, output, "world-graph-child-program", measure_nodes=True)
    assert len(child_pixels["nodes"]) == 1, child_pixels
    graph_mode(page, canvas, "Local")
    local = graph_scope(page, "Local", 3, 3, 3)
    assert local["scope"] == "Cross-program neighborhood"
    assert local["hidden_states"] == local["boundary_edges"] == 0
    assert local["edges"] == local["native_edges"] == len(edges)
    expect(status).to_contain_text(f"Selected {symbol('σ', child_state['id'])}; source {symbol('P', child['id'])}")
    expect(status).to_contain_text("Converged")
    local_pixels = node_snapshot(page, canvas, output, "world-graph-local-cross-program", measure_nodes=True)
    assert len(local_pixels["nodes"]) == 3, local_pixels
    assert not submitted, "program filtering or local exploration reran analysis"
    return {"all_programs": full, "child_program": filtered, "cross_program_local": local,
            "selected_native_state": child_state["id"], "child_pixels": child_pixels,
            "local_pixels": local_pixels}


def world_interactions(browser, url: str, output: Path, rpc, pending_rpc, providers):
    results = {}
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    failures = []
    page.on("pageerror", lambda error: failures.append(str(error)))
    startup = automatic_rpc_entry(page, url, providers[0], output, "world-startup")
    pending_rpc.requests.clear()
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
    expect(status).to_contain_text(f"source {symbol('P', child['id'])}")
    expect(status).to_contain_text(f"Selected {symbol('σ', child_state['id'])}")
    expect(status).to_contain_text("Frame f₁")
    canvas.screenshot(path=str(output / "world-child-frame.png"))
    screenshots = {}
    tabs = ["Stack", "Memory", "Storage", "Transient", "Calldata", "Returndata",
            "Outcomes", "Acquisition", "Environment", "Diagnostics", "Frame"]
    for tab in tabs:
        select_inspector_tab(page, canvas, tab)
        expect(status).to_contain_text(tab)
        rendered_frame(page)
        png = canvas.screenshot(path=str(output / f"world-{tab.lower()}.png"))
        screenshots[tab] = hashlib.sha256(png).hexdigest()
    assert len(set(screenshots.values())) == len(tabs), "inspector controls did not expose distinct views"
    canvas.click(position={"x": 166, "y": 801})
    expect(status).to_contain_text("Observed exit")
    select_inspector_tab(page, canvas, "Memory")
    expect(status).to_contain_text("Memory")
    canvas.screenshot(path=str(output / "world-child-exit-memory.png"))
    graph_scopes = world_graph_scopes(page, canvas, report, child, child_state, output)
    # All acquisition reads remain on one canonical snapshot.
    reads = [request for request in rpc.requests if request["method"] in ("eth_getCode", "eth_getStorageAt", "eth_getBalance", "eth_getTransactionCount")]
    assert reads and all(request["params"][-1] == {"blockHash": rpc.block_hash, "requireCanonical": True} for request in reads)
    assert sum(request["method"] == "eth_getBlockByNumber" and request["params"][0] == "latest" for request in rpc.requests) == 1
    assert not pending_rpc.requests, "selecting the second provider contacted the default provider"
    assert RPC_TEST_TOKEN not in json.dumps(report), "provider URL escaped into the analysis report"
    (output / "world-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    results["analysis"] = {"programs": len(report["programs"]), "states": len(report["states"]), "outcomes": len(report["outcomes"]), "rpc_requests": len(rpc.requests), "inspector_views": screenshots, "graph_scopes": graph_scopes, "renderer": webgpu_evidence(page)}
    assert not failures, failures
    page.close()
    rpc = pending_rpc
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    automatic_rpc_entry(page, url, providers[0], output, "cancel-startup")
    rpc.block_method = "eth_getCode"
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
    cancelled_wait = visible_seconds(page, "Total wait")
    cancelled_id = admitted.json()["result"]["Ok"]["id"]
    frozen_cancelled = wait_for_job(page, url, cancelled_id, lambda job: job["state"] == "Cancelled")
    page.wait_for_timeout(300)
    assert visible_seconds(page, "Total wait") == cancelled_wait, "cancelled user wait kept advancing"
    assert wait_for_job(page, url, cancelled_id, lambda job: job["state"] == "Cancelled")["timings"] == frozen_cancelled["timings"]
    canvas.screenshot(path=str(output / "cancel-acknowledged.png"))
    results["cancellation"] = {"history": history, "pending_rpc_closed": rpc.disconnected.is_set(), "aborted_http": aborted, "status": status.text_content(), "total_wait_s": cancelled_wait, "backend_timings": frozen_cancelled["timings"]}
    # Resolve the test harness interception after the application itself
    # proved it aborted/recovered, so Playwright has no pending handler.
    held_polls[0].abort()
    page.unroute_all(behavior="wait")
    page.close()
    results["automatic_entry"] = startup["input"]
    return results


def entry_example_interactions(browser, url: str, output: Path, providers, rpc):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    baseline = automatic_rpc_entry(page, url, providers[0], output, "examples-startup")
    canvas = page.locator("#evm-canvas")
    submitted = []
    page.on("request", lambda request: submitted.append(request.post_data_json)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    results = {"automatic_entry": baseline["input"], "selected_addresses": [], "snapshot_pins": {}}
    for ordinal, (label, address, menu_y) in enumerate([("weth", WETH_ENTRY, 487), ("usdt", USDT_ENTRY, 510), ("usdc", USDC_ENTRY, 464)]):
        open_rpc_input(page, canvas)
        if ordinal == 0:
            # The entry choice must preserve an independently chosen provider.
            select_rpc_provider(page, canvas, 1, output, "entry-weth")
            expect(page.locator("#analysis-status")).to_contain_text(
                f"RPC provider: {providers[1]['name']} ({providers[1]['endpoint']})")
        canvas.click(position={"x": 454, "y": 425})
        rendered_frame(page)
        canvas.screenshot(path=str(output / f"entry-{label}-options.png"))
        canvas.click(position={"x": 460, "y": menu_y})
        settle_gesture(page)
        assert len(submitted) == ordinal, "choosing an entry unexpectedly submitted analysis"
        canvas.screenshot(path=str(output / f"entry-{label}-selected.png"))
        request_offset = len(rpc.requests)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
            page.keyboard.press("Control+Enter")
        reply = complete_submission(page, response.value)
        assert reply.status == 200 and "Ok" in reply.json()["result"], reply.json()
        requests = rpc.requests[request_offset:]
        pins = sum(request["method"] == "eth_getBlockByNumber" and request["params"][0] == "latest" for request in requests)
        assert pins == 1, f"{label} must pin latest once: {requests}"
        results["snapshot_pins"][label] = pins
        expected = json.loads(json.dumps(baseline))
        expected["input"]["Rpc"]["provider_id"] = providers[1]["id"]
        expected["input"]["Rpc"]["address"] = address
        assert reply.request.post_data_json == expected, "entry selection reset unrelated request fields"
        assert reply.json()["result"]["Ok"]["metadata"]["entry_address"].lower() == address.lower()
        results["selected_addresses"].append(address)
    open_rpc_input(page, canvas)
    canvas.click(position={"x": 600, "y": 448})
    page.keyboard.press("Control+A")
    page.keyboard.type(CUSTOM_ENTRY, delay=3)
    rendered_frame(page)
    canvas.screenshot(path=str(output / "entry-custom-edited.png"))
    page.keyboard.press("Escape")
    settle_gesture(page)
    open_rpc_input(page, canvas)
    canvas.screenshot(path=str(output / "entry-custom-reopened.png"))
    assert len(submitted) == 3
    request_offset = len(rpc.requests)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
        page.keyboard.press("Control+Enter")
    reply = complete_submission(page, response.value)
    assert reply.status == 200 and "Ok" in reply.json()["result"], reply.json()
    requests = rpc.requests[request_offset:]
    pins = sum(request["method"] == "eth_getBlockByNumber" and request["params"][0] == "latest" for request in requests)
    assert pins == 1, f"custom entry must pin latest once: {requests}"
    results["snapshot_pins"]["custom"] = pins
    expected = json.loads(json.dumps(baseline))
    expected["input"]["Rpc"]["provider_id"] = providers[1]["id"]
    expected["input"]["Rpc"]["address"] = CUSTOM_ENTRY
    assert reply.request.post_data_json == expected, "reopening the form replaced the hand-edited address"
    results["custom_reopened_address"] = CUSTOM_ENTRY
    results["renderer"] = webgpu_evidence(page)
    page.close()
    return results


def configured_world_interactions(browser, args):
    # Endpoints must exist before the server loads its provider configuration.
    # Both tasks use the real catalogue and ID resolution, including cancellation.
    with EntryRpcFixture() as pending, EntryRpcFixture() as rpc:
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
                examples = entry_example_interactions(browser, url, args.output, providers, rpc)
                # Each scenario audits its own pinned acquisition, independently
                # of the completed WETH/USDT/USDC/custom-address example tasks.
                rpc.requests.clear()
                result = world_interactions(browser, url, args.output, rpc, pending, providers)
                result["entry_examples"] = examples
                result["providers"] = providers
                result["selected_provider_id"] = RPC_PROVIDERS[1]["id"]
                result["endpoint_displayed_in_selector"] = True
                result["submission_uses_provider_id"] = True
                return result


def url_configured_world_interactions(browser, args):
    with EntryRpcFixture() as rpc:
        endpoint = rpc.endpoint + "/rpc?token=" + RPC_TEST_TOKEN
        providers = [{"id": "default", "name": "Default RPC", "endpoint": endpoint}]
        with analysis_server(args, endpoint) as url:
            assert not rpc.requests, "loading a URL unexpectedly contacted the RPC"
            page = browser.new_page(viewport={"width": 1440, "height": 1000})
            catalogue = page.request.get(url + "/api/rpc-providers")
            assert catalogue.ok, catalogue.text()
            assert catalogue.json() == {"result": {"Ok": providers}}
            automatic_rpc_entry(page, url, providers[0], args.output, "url-startup")
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
    open_rpc_input(page, canvas, from_bytecode=True)
    assert_rpc_disabled(page, canvas, output, "rpc-empty-disabled", analyze_y=669)
    with page.expect_response(lambda response: response.url.endswith("/api/rpc-providers")) as retry:
        canvas.click(position={"x": 372, "y": 410.5})
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
    open_rpc_input(page, canvas, from_bytecode=True)
    assert_rpc_disabled(page, canvas, output, "rpc-failed-disabled", analyze_y=669)
    canvas.click(position={"x": 372, "y": 410.5})
    expect(status).to_contain_text("Loading RPC providers")
    assert len(held_catalogues) == 1
    assert_rpc_disabled(page, canvas, output, "rpc-loading-disabled", analyze_y=663)
    expect(status).to_contain_text("Loading RPC providers")
    held_catalogues[0].fulfill(status=503, content_type="application/json", body='{"result":{"Ok":[]}}')
    expect(status).to_contain_text("Could not load RPC providers:")
    page.unroute("**/api/rpc-providers", delay_catalogue)
    with page.expect_response(lambda response: response.url.endswith("/api/rpc-providers")) as retry:
        canvas.click(position={"x": 372, "y": 410.5})
    assert retry.value.ok and retry.value.json() == {"result": {"Ok": []}}
    expect(status).to_contain_text("No RPC providers are configured on the server.")
    canvas.screenshot(path=str(output / "rpc-retry-recovered.png"))
    page.close()
    return {"unconfigured_catalogue": [], "bytecode_available": True,
            "disabled_rpc_states": ["empty", "loading", "failed"],
            "checked_submission_paths": ["Analyze", "Control+Enter"],
            "catalogue_retry": "real server returned empty catalogue"}


LAYOUT_FIXTURES = Path(__file__).resolve().parent.parent / "crates/evm-abstract-web/tests/fixtures/layout"
LAYOUT_DEFAULT_LIMITS = {
    "max_states":100_000, "max_transfers":10_000_000, "context_depth":128,
    "max_constants":512, "max_work":1_000_000_000_000, "max_call_depth":1025,
    "max_memory_bytes":64*1024*1024, "use_summaries":True, "domain_profile":"Product",
    "reduction_rounds":16, "max_facts":4096, "relations_enabled":True,
    "max_constraints":2048, "max_expression_nodes":16_384, "max_expression_depth":256,
    "smt_provider":"Z3", "smt_rlimit":10_000_000, "rpc_max_accounts":4096,
    "rpc_max_requests":1_000_000, "rpc_max_response_bytes":64*1024*1024,
    "rpc_timeout_ms":120_000,
}


def layout_fixture(name: str):
    code = (LAYOUT_FIXTURES / f"{name}.hex").read_text().strip()
    metadata = json.loads((LAYOUT_FIXTURES / f"{name}.json").read_text())
    assert len(bytes.fromhex(code)) == metadata["byte_length"]
    return code, metadata


def layout_modal(page, canvas):
    viewport = page.viewport_size
    boxes = screenshot_metrics(page, canvas.screenshot(),
                               (0, 0, viewport["width"], viewport["height"]), True)["nodes"]
    return max(boxes, key=lambda rect: (rect[2]-rect[0])*(rect[3]-rect[1]))


def layout_entry_form(page, canvas, entry, source_height_offset=0):
    """Edit actual egui controls; the typed POST below verifies every choice."""
    root_offset = source_height_offset + (10 if "address" in entry else 0)
    if "address" in entry:
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+18, "y": modal[1]+173+source_height_offset})
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+250, "y": modal[1]+198+source_height_offset})
        page.keyboard.insert_text(entry["address"])
        rendered_frame(page)
    modal = layout_modal(page, canvas)
    canvas.click(position={"x": modal[0]+100, "y": modal[1]+209+root_offset})
    settle_gesture(page)
    extra_rows = 0
    if "caller" in entry:
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+175, "y": modal[1]+237+root_offset})
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+250, "y": modal[1]+258+root_offset})
        page.keyboard.insert_text(entry["caller"])
        rendered_frame(page)
        extra_rows += 24
    if "call_value" in entry:
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+175, "y": modal[1]+293+root_offset+extra_rows})
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+250, "y": modal[1]+314+root_offset+extra_rows})
        page.keyboard.insert_text(entry["call_value"])
        rendered_frame(page)
        extra_rows += 24
    modal = layout_modal(page, canvas)
    canvas.click(position={"x": modal[0]+175, "y": modal[1]+337+root_offset+extra_rows})
    settle_gesture(page)
    modal = layout_modal(page, canvas)
    canvas.click(position={"x": modal[0]+250, "y": modal[1]+358+root_offset+extra_rows})
    page.keyboard.insert_text(entry["calldata"])
    rendered_frame(page)
    if "chain_id" in entry:
        # Original 20-point native rows, measured before and after modal scroll.
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+100, "y": modal[1]+439+root_offset})
        settle_gesture(page)
        page.mouse.move(800, 660)
        page.mouse.wheel(0, 240)
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+36, "y": modal[1]+466})
        settle_gesture(page)
        modal = layout_modal(page, canvas)
        canvas.click(position={"x": modal[0]+250, "y": modal[1]+489})
        page.keyboard.insert_text(entry["chain_id"])
        rendered_frame(page)


def layout_submission(page, canvas, code: str, output: Path, name: str, timeout_seconds=60, entry=None,
                      source_height_offset=0):
    # Exercise the real multiline editor as a paste. Typing every byte with a
    # delay would add minutes unrelated to analysis or layout for real runtimes.
    open_input(page)
    page.keyboard.press("Control+A")
    page.keyboard.insert_text(code)
    rendered_frame(page)
    if entry is not None:
        layout_entry_form(page, canvas, entry, source_height_offset)
    canvas.screenshot(path=str(output / f"{name}-input.png"))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as response:
        page.keyboard.press("Control+Enter")
    admitted = response.value
    assert posted_bytecode(admitted) == code
    request = admitted.request.post_data_json
    if entry is not None:
        assert request["environment"]["calldata"] == {"Exact": entry["calldata"]}, request
        if "caller" in entry:
            assert request["environment"]["caller"] == {"Concrete": entry["caller"]}, request
        if "call_value" in entry:
            assert request["environment"]["call_value"] == {"Concrete": entry["call_value"]}, request
        if "address" in entry:
            assert request["input"]["Bytecode"]["address"] == entry["address"], request
        if "chain_id" in entry:
            assert request["environment"]["chain_id"] == entry["chain_id"], request
        assert request["limits"] == LAYOUT_DEFAULT_LIMITS, request["limits"]
    (output / f"{name}-request.json").write_text(admitted.request.post_data + "\n")
    result = complete_submission(page, admitted, timeout_seconds)
    assert result.status == 200 and "Ok" in result.json()["result"], result.json()
    report = result.json()["result"]["Ok"]
    assert report["programs"][0]["bytecode"].removeprefix("0x") == code
    if entry is not None:
        assert report["metadata"]["limits"] == LAYOUT_DEFAULT_LIMITS
        environment = report["metadata"]["environment"]
        calldata = report["byte_arrays"][environment["calldata"]]
        expected = bytes.fromhex(entry["calldata"].removeprefix("0x"))
        assert [int(value,16) for value in calldata["length"]["constants"]] == [len(expected)]
        cells = {cell["offset"]:calldata["values"][cell["value"]] for cell in calldata["cells"]}
        for offset, byte in enumerate(expected):
            value = cells.get(offset, calldata["default"])
            assert [int(word,16) for word in value["constants"]] == [byte], (offset,value,byte)
        if "address" in entry:
            assert environment["to"]["Concrete"].lower() == entry["address"].lower()
        if "caller" in entry:
            assert environment["caller"]["Concrete"].lower() == entry["caller"].lower()
        if "call_value" in entry:
            assert [int(value,16) for value in environment["call_value"]["constants"]] == [int(entry["call_value"],16)]
        if "chain_id" in entry:
            assert [int(value,16) for value in environment["chain_id"]["constants"]] == [int(entry["chain_id"],16)]
    (output / f"{name}-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def layout_counts(page, report, expected_groups: int | None = None, mode="Blocks"):
    identities = {
        node["id"]: ((node["program"], node["basic_block"], node["start_pc"])
                     if mode == "Blocks" and node["program"] is not None and node["start_pc"] is not None
                     else ("state", node["id"]))
        for node in report["cfg"]
    }
    edge_count = len({(identities[edge["from"]], identities[edge["to"]], edge["kind"])
                      for edge in report["edges"]}) if mode == "Blocks" else len(report["edges"])
    scene = graph_scope(page, mode, len(set(identities.values())), len(report["cfg"]), len(report["cfg"]))
    assert scene["edges"] == edge_count and scene["native_edges"] == len(report["edges"]), scene
    status = page.locator("#analysis-status").text_content()
    assert "Layout unavailable:" not in status, status
    match = re.search(r"; (\d+) groups; CFG detail: (identities|SSA preview)", status)
    assert match, status
    scene.update(groups=int(match[1]), detail=match[2])
    if expected_groups is not None:
        assert scene["groups"] == expected_groups, scene
    return scene


def layout_page(browser, url: str):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                              and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    complete_submission(page, initial.value)
    canvas = page.locator("#evm-canvas")
    hide_inspector(page, canvas)
    page.keyboard.press("2")
    settle_gesture(page)
    return page, canvas, errors


def assert_layout_same_geometry(before, after):
    """Compare rendered leaf rectangles after removing uniform camera changes."""
    require_scene_complete(before);require_scene_complete(after)
    first = sorted(before["nodes"], key=lambda rect: (rect[1], rect[0]))
    second = sorted(after["nodes"], key=lambda rect: (rect[1], rect[0]))
    assert len(first) == len(second) >= 2, (first, second)
    pairs = list(zip(first, second, strict=True))
    centers = [([(a[0] + a[2]) / 2, (a[1] + a[3]) / 2],
                [(b[0] + b[2]) / 2, (b[1] + b[3]) / 2]) for a, b in pairs]
    scale = median(math.dist(centers[a][1], centers[b][1]) / math.dist(centers[a][0], centers[b][0])
                   for a in range(len(centers)) for b in range(a + 1, len(centers)))
    offset = [median(new[axis] - old[axis] * scale for old, new in centers) for axis in [0, 1]]
    for (a, b), (old, new) in zip(pairs, centers, strict=True):
        for axis in [0, 1]:
            assert abs(new[axis] - (old[axis] * scale + offset[axis])) <= 4.0, (first, second)
            assert abs((b[axis + 2] - b[axis]) - (a[axis + 2] - a[axis]) * scale) <= 3.0
    return {"camera_scale": scale, "camera_translation": offset}


def layout_ink(page, png, region):
    return page.evaluate("""async ({png, region}) => {
        const image = new Image(); image.src = `data:image/png;base64,${png}`; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
        const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
        const scale = image.width / innerWidth;
        const [left, top, right, bottom] = region.map(v => Math.round(v * scale));
        const pixels = context.getImageData(left, top, right-left, bottom-top).data;
        let preview = 0, annotation = 0;
        for (let i=0; i<pixels.length; i+=4) {
            const [r,g,b] = [pixels[i],pixels[i+1],pixels[i+2]];
            if (r>180 && g>190 && b>205) preview++;
            if (r>=60 && r<=165 && g-r>=10 && g-r<=45 && b-g>=10 && b-g<=40) annotation++;
        }
        return {preview_pixels:preview, annotation_pixels:annotation};
    }""", {"png":base64.b64encode(png).decode(), "region":region})


def layout_card_extent(page, png):
    # Include tiny cards below the ordinary connected-component size cutoff.
    # Solid fills become subpixel at this scale. Measure the brighter real card
    # outlines/edges instead; the background grid and dim group dashes remain
    # below this palette threshold.
    return page.evaluate("""async png => {
        const image = new Image(); image.src = `data:image/png;base64,${png}`; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width=image.width; canvas.height=image.height;
        const context=canvas.getContext('2d'); context.drawImage(image,0,0);
        const scale=image.width/innerWidth;
        const top=Math.round(123*scale), bottom=Math.round((innerHeight-30)*scale);
        const data=context.getImageData(0,top,image.width,bottom-top).data;
        let left=image.width,right=-1,first=bottom,last=-1,count=0;
        for(let y=0;y<bottom-top;y++) for(let x=4*scale;x<image.width-4*scale;x++) {
            const i=(y*image.width+x)*4, r=data[i],g=data[i+1],b=data[i+2];
            if(g>=48 && g-r>=10 && b>=g-25) {
                left=Math.min(left,x);right=Math.max(right,x);first=Math.min(first,y+top);last=Math.max(last,y+top);count++;
            }
        }
        return {bounds:count?[left/scale,first/scale,(right+1)/scale,(last+1)/scale]:null,pixels:count};
    }""", base64.b64encode(png).decode())


def assert_layout_camera_interval(actual, predicted):
    assert predicted[0] <= predicted[1], predicted
    error = max(predicted[0]-actual,actual-predicted[1],0.0)
    assert error <= 4.0, (actual,predicted,error)
    return {"actual":actual,"predicted_interval":predicted,"error":error}


def layout_camera_intervals(before, after, fit_zoom, zoom_out, pointer):
    # The public status rounds each percentage to three decimals. Propagate
    # that measurement precision without changing the four-point pixel bound.
    precision = 0.0005
    assert fit_zoom > precision and zoom_out > precision
    ratios = [(zoom_out-precision)/(fit_zoom+precision),
              (zoom_out+precision)/(fit_zoom-precision)]
    height = before["bounds"][3]-before["bounds"][1]
    measured = assert_layout_camera_interval(after["bounds"][3]-after["bounds"][1],
                                             [height*ratio for ratio in ratios])
    corners = []
    for axis in [0,1]:
        for corner in [axis,axis+2]:
            expected = sorted(pointer[axis]+(before["bounds"][corner]-pointer[axis])*ratio for ratio in ratios)
            corners.append({"corner":corner,**assert_layout_camera_interval(after["bounds"][corner],expected)})
    return {"display_precision_percent":precision,"zoom_ratio_interval":ratios,
            "height":measured,"corners":corners}


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
                   for node in report["cfg"] for instruction in node["instructions"])
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
        canvas.click(position={"x":225,"y":83});settle_gesture(page)
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
        canvas.click(position={"x":225,"y":83});settle_gesture(page)
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


def hierarchy_small_interactions(browser, url: str, output: Path):
    page, canvas, errors = layout_page(browser, url)
    results = {}
    for name in ["shortcut", "linear-chain"]:
        code, metadata = layout_fixture(name)
        report = layout_submission(page, canvas, code, output, name)
        assert report["status"] == "Converged", report["frontiers"]
        assert len(report["cfg"]) == metadata["expected_native_states"]
        assert len(report["edges"]) == metadata["expected_original_edges"]
        scene = layout_counts(page, report, metadata["expected_spatial_groups"])
        assert scene["nodes"] == metadata["expected_source_blocks"]
        pixels = node_snapshot(page, canvas, output, f"hierarchy-{name}", measure_nodes=True)
        assert len(pixels["nodes"]) == scene["nodes"], "spatial annotations were counted as real CFG states"
        if name == "shortcut":
            require_scene_complete(pixels)
            nodes = sorted(pixels["nodes"], key=lambda rect: rect[1])
            _, a, b = nodes
            assert a[3] < b[1], "shortcut target was placed beside/before its true predecessor"
            interval = (max(a[0],b[0]), a[3]+1, min(a[2],b[2]), b[1]-1)
            ink = layout_ink(page, canvas.screenshot(path=str(output / "hierarchy-shortcut-label.png")), interval)
            assert ink["annotation_pixels"] > 0, "the direct A to B annotation was not painted between its endpoints"
            results[name] = {"scene":scene,"pixels":pixels,"between_endpoints":ink}
        else:
            results[name] = {"scene":scene,"pixels":pixels}
            requests = []
            page.on("request", lambda request: requests.append(request.url)
                    if request.method=="POST" and request.url.endswith("/api/tasks") else None)
            pointer = ((pixels["bounds"][0]+pixels["bounds"][2])/2,(pixels["bounds"][1]+pixels["bounds"][3])/2)
            for _ in range(12):
                if "CFG detail: identities" in page.locator("#analysis-status").text_content(): break
                page.mouse.move(*pointer); page.keyboard.down("Control"); page.mouse.wheel(0,160); page.keyboard.up("Control"); settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text("CFG detail: identities")
            low_zoom=float(re.search(r"; CFG zoom: ([0-9.]+)%",page.locator("#analysis-status").text_content())[1])
            assert low_zoom <= 55
            page.mouse.move(2,2);settle_gesture(page)
            low = node_snapshot(page,canvas,output,"hierarchy-chain-identities",measure_nodes=True)
            assert len(low["nodes"])==4,low
            target = max(low["nodes"],key=lambda rect:rect[1])
            first_state = report["cfg"][0]["id"]
            state = first_state
            while (outgoing := [edge for edge in report["edges"] if edge["from"]==state]):
                assert len(outgoing)==1
                state=outgoing[0]["to"]
            pointer=((target[0]+target[2])/2,(target[1]+target[3])/2)
            canvas.click(position={"x":pointer[0],"y":pointer[1]}); page.mouse.move(2,2); settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text(f"Selected {symbol('σ',state)};")
            low_ink=layout_ink(page,canvas.screenshot(),target)
            assert low_ink["preview_pixels"]==0, "low zoom retained the white SSA instruction body"
            for _ in range(12):
                if "CFG detail: SSA preview" in page.locator("#analysis-status").text_content(): break
                page.mouse.move(*pointer);page.keyboard.down("Control");page.mouse.wheel(0,-120);page.keyboard.up("Control");settle_gesture(page)
            page.mouse.move(2,2);settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
            high_zoom=float(re.search(r"; CFG zoom: ([0-9.]+)%",page.locator("#analysis-status").text_content())[1])
            assert high_zoom >= 65
            high=node_snapshot(page,canvas,output,"hierarchy-chain-ssa")
            high_ink=layout_ink(page,canvas.screenshot(),high["bounds"])
            assert high_ink["preview_pixels"]>0, "high zoom did not restore real SSA instruction ink"
            assert layout_counts(page,report,1)["shown_states"]==4
            assert not requests,"semantic zoom or clicking a leaf reran analysis"
            results[name].update(low=low,high=high,low_ink=low_ink,high_ink=high_ink,
                                 low_zoom_percent=low_zoom,high_zoom_percent=high_zoom,selected_native_state=state)
    assert not errors,errors
    results["renderer"]=webgpu_evidence(page)
    page.close()
    # Fresh pages create the same layout for two genuinely different initial
    # viewports. This checks more than resizing an already-cached scene.
    samples=[]
    code,_=layout_fixture("shortcut")
    for width,height in [(1440,1000),(640,800)]:
        page=browser.new_page(viewport={"width":width,"height":height});observe_webgpu(page)
        with page.expect_response(lambda response:response.url.endswith("/api/tasks") and response.request.method=="POST") as initial:
            page.goto(url,wait_until="networkidle")
        complete_submission(page,initial.value)
        canvas=page.locator("#evm-canvas")
        # Inspector is right-aligned and omitted from the graph comparison.
        if width>=800: hide_inspector(page,canvas)
        else:
            canvas.click(position={"x":width-78,"y":12});rendered_frame(page)
            # Use the public Panels menu's State inspector checkbox.
            canvas.click(position={"x":width-80,"y":62});settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text("Inspector hidden")
        page.keyboard.press("2");settle_gesture(page)
        report=layout_submission(page,canvas,code,output,f"hierarchy-initial-{width}")
        layout_counts(page,report,0)
        samples.append(node_snapshot(page,canvas,output,f"hierarchy-initial-{width}-scene",(4,123,width-4,height-30),True))
        page.close()
    results["initial_viewports"]={"wide":samples[0],"narrow":samples[1],"normalization":assert_layout_same_geometry(*samples)}
    return results


class NestedLayoutRpcFixture(EntryRpcFixture):
    def __init__(self):
        super().__init__()
        self.world=json.loads((LAYOUT_FIXTURES/"nested-call-world.json").read_text())
        self.accounts={account["address"]:account for account in self.world["accounts"]}
        self.root_code=self.accounts[self.root_address]["code"]
        self.child_code=self.accounts[self.child_address]["code"]

    def _result(self,method,params):
        if method in ("eth_getCode","eth_getBalance","eth_getTransactionCount","eth_getStorageAt") and params[0].lower() in self.accounts:
            assert params[-1]=={"blockHash":self.block_hash,"requireCanonical":True}
            account=self.accounts[params[0].lower()]
            return {"eth_getCode":account["code"],"eth_getBalance":account["balance"],"eth_getTransactionCount":account["nonce"],"eth_getStorageAt":"0x"+"00"*32}[method]
        return super()._result(method,params)


def hierarchy_call_interactions(browser,args):
    with NestedLayoutRpcFixture() as rpc, TemporaryDirectory(prefix="layout-rpc-") as directory:
        providers=[{"id":"layout-world","name":"Synthetic nested layout calls","endpoint":rpc.endpoint}]
        config=Path(directory)/"providers.json";config.write_text(json.dumps({"providers":providers})+"\n")
        with analysis_server(args,config) as url:
            page=browser.new_page(viewport={"width":1440,"height":1000});observe_webgpu(page)
            automatic_rpc_entry(page,url,providers[0],args.output,"hierarchy-call-startup")
            canvas=page.locator("#evm-canvas")
            admitted=submit_rpc(page,canvas,rpc,args.output,"hierarchy-calls",0,providers)
            reply=complete_submission(page,admitted)
            assert reply.status==200 and "Ok" in reply.json()["result"],reply.json()
            report=reply.json()["result"]["Ok"]
            assert len(report["programs"])>=3
            assert max(state["frame_depth"] for state in report["cfg"])==3
            assert sum(edge["kind"]=="Call" for edge in report["edges"])>=2
            assert sum(edge["kind"]=="Return" for edge in report["edges"])>=2
            (args.output/"hierarchy-nested-calls-analysis.json").write_text(json.dumps(report,indent=2)+"\n")
            canvas.click(position={"x":1440-180,"y":12});hide_inspector(page,canvas)
            page.keyboard.press("2");settle_gesture(page);graph_mode(page,canvas,"States")
            scene=layout_counts(page,report,mode="States")
            assert scene["groups"]>=3
            pixels=node_snapshot(page,canvas,args.output,"hierarchy-nested-calls",measure_nodes=True)
            assert len(pixels["nodes"])==len(report["cfg"]),"program/chain frames became phantom selectable states"
            return {"scene":scene,"pixels":pixels,"programs":len(report["programs"]),"renderer":webgpu_evidence(page)}


def layout_pending_machines(report, entry):
    assert report["schema_version"] == 12
    pending = report["pending_machines"]
    assert len(pending) == entry["expected_pending_machines"]
    assert {frontier["payload"] for frontier in report["frontiers"]} == set(range(len(pending)))
    nodes = {node["id"]:node for node in report["cfg"]}
    programs = {program["id"] for program in report["programs"]}
    graph = report["symbolic"]["nodes"]
    word_kinds = {"Constant","Input","Fresh","Operation","Ite","ByteRead","WordRead","SlotRead","Keccak"}
    byte_kinds = {"BytesZero","BytesInput","BytesExact","WriteByte","WriteWord","Copy","Slice","BytesIte"}
    slot_kinds = {"SlotsZero","SlotsInput","Store","SlotsIte"}
    path_kinds = {"PathTrue","PathFalse","PathAtoms","PathAnd","PathIte"}
    def reference(value, kinds):
        index = value["node"]
        assert type(index) is int and 0 <= index < len(graph)
        node = graph[index]
        assert (node if isinstance(node,str) else next(iter(node))) in kinds
    def facts(value):
        assert set(value) == {"summary","constants","known_zero","known_one","nonzero","unsigned","signed",
                              "congruence","origins","code_address_role","identity","expression","symbolic_limit"}
        words = [value["known_zero"],value["known_one"],*value["unsigned"].values(),*value["signed"].values()]
        words.extend(value["constants"] or [])
        assert all(isinstance(word,str) and word.startswith("0x") and 0 <= int(word,16) < 2**256 for word in words)
        if value["expression"] is not None:reference(value["expression"],word_kinds)
    def pool(index, values):
        assert type(index) is int and 0 <= index < len(values)
        return values[index]
    buffers = set(); stores = set()
    for frontier in report["frontiers"]:
        assert frontier["from"] in nodes and frontier["pc"] is None and frontier["range"] is None
        assert frontier["reason"] == entry["expected_frontier_details"]
        machine = pool(frontier["payload"],pending)
        assert set(machine) == {"frames","store","path"} and machine["frames"]
        reference(machine["path"],path_kinds)
        pool(machine["store"],report["stores"]);stores.add(machine["store"])
        for index,frame in enumerate(machine["frames"]):
            assert set(frame) == {"index","program","code_address","code_hash","storage_address","address_value",
                                  "caller","address_operand","caller_operand","call_value","is_static","kind","basic_block","context","stack",
                                  "memory","calldata","returndata","rollback_store","continuation"}
            assert frame["index"] == index and frame["program"] in programs
            facts(frame["call_value"])
            for name in ["address_operand","caller_operand"]:
                if frame[name] is not None: facts(frame[name])
            for value in frame["stack"]:facts(value)
            for name in ["memory","calldata","returndata"]:
                pool(frame[name],report["byte_arrays"]);buffers.add(frame[name])
            pool(frame["rollback_store"],report["stores"]);stores.add(frame["rollback_store"])
            if frame["continuation"] is not None:
                facts(frame["continuation"]["output_offset"]);facts(frame["continuation"]["output_size"])
        active = machine["frames"][-1]
        assert any(node["program"] == active["program"] and node["basic_block"] == active["basic_block"]
                   and node["frame_depth"] == len(machine["frames"]) for node in nodes.values())
    for index in stores:
        store = report["stores"][index]
        assert set(store) == {"storage_default","transient_default","balance_default","accounts","logs",
                              "logs_unknown","storage_facts","transient_facts"}
        for name in ["storage_default","transient_default","balance_default"]:facts(store[name])
        for account in store["accounts"]:
            assert set(account) == {"address","existence","balance","nonce","code_kind","code_hash","program",
                                    "delegation_target","storage_default","storage_root","transient_root","storage",
                                    "transient_default","transient","created","pending_destruction"}
            assert account["program"] is None or account["program"] in programs
            for name in ["balance","nonce","storage_default","transient_default"]:facts(account[name])
            for name in ["storage_root","transient_root"]:reference(account[name],slot_kinds)
            for slot in account["storage"]+account["transient"]:
                assert slot["slot"].startswith("0x") and 0 <= int(slot["slot"],16) < 2**256
                facts(slot["value"])
        for binding in store["storage_facts"]+store["transient_facts"]:
            reference(binding["expression"],word_kinds);facts(binding["value"])
        for log in store["logs"]:
            pool(log["data"],report["byte_arrays"]);buffers.add(log["data"])
            for value in log["topics"]:facts(value)
    for index in buffers:
        buffer = report["byte_arrays"][index]
        reference(buffer["symbolic"],{"View"})
        view = graph[buffer["symbolic"]["node"]]["View"]
        reference(view["state"],byte_kinds)
        reference(view["offset"],word_kinds);reference(view["length"],word_kinds)
        for value in [buffer["length"],buffer["default"],*buffer["values"]]:facts(value)
        for cell in buffer["cells"]:pool(cell["value"],buffer["values"])
        for binding in buffer["symbolic_facts"]:
            reference(binding["expression"],word_kinds);facts(binding["value"])
    assert not report["ssa"]["complete"]
    assert {block["state"] for block in report["ssa"]["blocks"]} == set(nodes)
    assert all(not block["incoming_complete"] for block in report["ssa"]["blocks"])
    return {"pending_machines":len(pending),"byte_arrays":sorted(buffers),"stores":sorted(stores),
            "typed_reasons":[frontier["reason"] for frontier in report["frontiers"]]}


def hierarchy_real_runtime_interactions(browser,url,args,fixture_names=("ethereum-weth9","ethereum-uniswap-v2-router02","ethereum-multicall3")):
    results={}
    for name in fixture_names:
        page,canvas,errors=layout_page(browser,url)
        code,metadata=layout_fixture(name)
        assert metadata["chain_id"]==1 and metadata["requireCanonical"]
        entry=metadata["browser_entry"]
        print(f"real-runtime analysis start: {name}",flush=True)
        report=layout_submission(page,canvas,code,args.output,name,timeout_seconds=1800,entry=entry)
        print(f"real-runtime analysis done: {name} status={report['status']} states={len(report['cfg'])} "
              f"frontiers={[(frontier['kind'],frontier['detail']) for frontier in report['frontiers']]} "
              f"unknown_closed={[summary['closed'] for summary in report['abstract_summaries']]} "
              f"analyzing_ns={report['metadata']['timings']['analyzing_ns']}",flush=True)
        assert report["programs"][0]["code_hash"]==metadata["code_hash"]
        assert report["status"] == entry["expected_status"]
        assert len(report["cfg"]) >= entry["minimum_native_states"]
        assert len(report["frontiers"]) == entry["expected_frontiers"]
        if "expected_frontier_reason" in entry:
            assert all(frontier["kind"] == entry["expected_frontier_reason"] for frontier in report["frontiers"])
        if "expected_frontiers_by_kind" in entry:
            expected=entry["expected_frontiers_by_kind"]
            counts={kind:sum(frontier["kind"]==kind for frontier in report["frontiers"])
                    for kind in {frontier["kind"] for frontier in report["frontiers"]}}
            assert counts=={kind:details["count"] for kind,details in expected.items()},counts
            for frontier in report["frontiers"]:
                assert frontier["reason"]==expected[frontier["kind"]]["reason"]
                assert frontier["detail"]==expected[frontier["kind"]]["detail"]
        if "expected_closed_unknown_summaries" in entry:
            summaries=report["abstract_summaries"]
            assert len(summaries)==entry["expected_closed_unknown_summaries"]
            assert all(summary["closed"] for summary in summaries)
        if name == "ethereum-uniswap-v2-router02":
            for summary in report["abstract_summaries"]:
                assert summary["procedures"]
                for procedure in summary["procedures"]:
                    assert procedure["kind"]=="AbstractEnvironment" and procedure["is_static"]
                    assert not procedure["covered_blocks"]
                    assert set(procedure["outcomes"])=={"Return","Revert","Failure"}
        pending = layout_pending_machines(report,entry) if "expected_pending_machines" in entry else None
        scene=layout_counts(page,report)
        assert scene["nodes"] >= entry["minimum_source_blocks"]
        if report["status"]=="Incomplete":
            assert report["frontiers"] and not report["ssa"]["complete"]
            expect(page.locator("#analysis-status")).to_contain_text("Incomplete")
        # A full overview can contain many tiny cards. Focus is a camera action,
        # leaving the entire captured graph and its coverage untouched.
        canvas.screenshot(path=str(args.output/f"hierarchy-{name}-overview.png"))
        canvas.click(position={"x":225,"y":83});settle_gesture(page)
        expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
        focused=node_snapshot(page,canvas,args.output,f"hierarchy-{name}-focused")
        assert layout_counts(page,report)=={**scene,"detail":"SSA preview"}
        native_scene = None
        if name == "ethereum-multicall3":
            requests = []
            page.on("request", lambda request: requests.append(request.url)
                    if request.method == "POST" and request.url.endswith("/api/tasks") else None)
            graph_mode(page,canvas,"States")
            page.keyboard.press("f");settle_gesture(page)
            native_scene = layout_counts(page,report,mode="States")
            assert native_scene["nodes"] >= entry["minimum_native_states"] and native_scene["nodes"] == len(report["cfg"])
            assert native_scene["native_edges"] == len(report["edges"])
            canvas.screenshot(path=str(args.output/f"hierarchy-{name}-states-overview.png"))
            canvas.click(position={"x":225,"y":83});settle_gesture(page)
            expect(page.locator("#analysis-status")).to_contain_text("CFG detail: SSA preview")
            native_focus = node_snapshot(page,canvas,args.output,f"hierarchy-{name}-states-focused")
            selected = re.search(r"Selected (σ[^;]+);",page.locator("#analysis-status").text_content())[1]
            assert selected in [symbol("σ",node["id"]) for node in report["cfg"]]
            selected_node = next(node for node in report["cfg"] if symbol("σ",node["id"]) == selected)
            selected_ssa = next(block for block in report["ssa"]["blocks"] if block["state"] == selected_node["id"])
            assert selected_ssa["instructions"] and not selected_ssa["incoming_complete"]
            ink = layout_ink(page,canvas.screenshot(),native_focus["bounds"])
            assert ink["preview_pixels"] > 0, "real-runtime native focus did not paint SSA instructions"
            assert not requests, "native projection/focus triggered another real-runtime analysis"
            native_scene.update(focused=native_focus,selected_native_state=selected,ssa_instructions=len(selected_ssa["instructions"]),
                                ssa_ink=ink,unexpected_task_posts=requests)
        assert not errors,errors
        results[name]={"provenance":metadata,"status":report["status"],"scene":scene,"native_scene":native_scene,"focused":focused,"frontiers":len(report["frontiers"]),"pending":pending,"renderer":webgpu_evidence(page)}
        page.close()
    return results


def hierarchy_interactions(browser,url,args,include_real=True):
    result={"small":hierarchy_small_interactions(browser,url,args.output),
            "long_chain":hierarchy_long_chain_interactions(browser,url,args.output),
            "native_projection_600":hierarchy_native_projection_interactions(browser,url,args.output),
            "nested_calls":hierarchy_call_interactions(browser,args)}
    if include_real:result["real_runtimes"]=hierarchy_real_runtime_interactions(browser,url,args)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", required=True)
    parser.add_argument("--assets", required=True)
    parser.add_argument("--browser", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--layout-only",action="store_true",help="Run the complete hierarchical CFG browser acceptance cases")
    parser.add_argument("--layout-small-only",action="store_true",help="Run only synthetic hierarchy/zoom cases while real analyses are running")
    parser.add_argument("--flow-only",action="store_true",help="Run selected-edge motion and compact-workspace SSA acceptance cases")
    parser.add_argument("--ssa-details-only",action="store_true",help="Run shared SSA reading mode and source-PC reveal acceptance cases")
    parser.add_argument("--ssa-values-only",action="store_true",help="Run normalized SSA and scoped value inspector acceptance")
    parser.add_argument("--unknown-world-only", action="store_true", help="Run closed unknown interaction and summary inspector acceptance")
    parser.add_argument("--inspector-controls-only",action="store_true",help="Run inspector controls, resizable storage tables and whole-panel scrolling acceptance")
    parser.add_argument("--termination-only",action="store_true",help="Run semantic ending styles and existing selected-node flow/SSA acceptance")
    parser.add_argument("--timings-only",action="store_true",help="Run queued analysis, phase timing and delayed-result acceptance")
    parser.add_argument("--http-only",action="store_true",help="Run real browser HTTP persistence and typed route acceptance")
    parser.add_argument("--compression-only", action="store_true", help="Run zstd, Brotli, gzip and identity negotiation through the real Wasm client")
    parser.add_argument("--diagnostics-only", action="store_true", help="Run typed invalid-opcode causes and state/source navigation acceptance")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    with analysis_server(args) as url:
        with sync_playwright() as playwright:
            browser = launch_browser(playwright.chromium, args.browser)
            if args.compression_only:
                compression_interactions(browser, url, args.output)
                browser.close()
                return
            if args.unknown_world_only:
                unknown_world_interactions(browser, url, args.output)
                browser.close()
                return
            if args.diagnostics_only:
                diagnostics_interactions(browser, url, args)
                browser.close()
                return
            if args.http_only:
                http_transport_interactions(browser, url, args.output)
                browser.close()
                return
            if args.timings_only:
                timing_interactions(browser, args)
                browser.close()
                return
            if args.inspector_controls_only:
                inspector_controls_interactions(browser,url,args.output)
                storage_table_interactions(browser,url,args.output)
                storage_scroll_interactions(browser,url,args.output)
                browser.close()
                return
            if args.ssa_values_only:
                ssa_values_interactions(browser,url,args.output)
                browser.close()
                return
            if args.termination_only:
                report={'termination':termination_interactions(browser,url,args),
                        'edge_flow':edge_flow_interactions(browser,url,args.output),
                        'ssa_details':ssa_details_interactions(browser,url,args.output),
                        'graph_interactions':graph_interactions(playwright.chromium,args.browser,url,args.output)}
                (args.output/'termination-acceptance-report.json').write_text(json.dumps(report,indent=2)+'\n')
                browser.close()
                return
            if args.ssa_details_only:
                ssa_details_interactions(browser,url,args.output)
                browser.close()
                return
            if args.flow_only:
                report={"edge_flow":edge_flow_interactions(browser,url,args.output),
                        "compact_ssa":compact_ssa_interactions(browser,url,args.output)}
                (args.output/"flow-report.json").write_text(json.dumps(report,indent=2)+"\n")
                browser.close()
                return
            if args.layout_only or args.layout_small_only:
                report=hierarchy_interactions(browser,url,args,include_real=not args.layout_small_only)
                (args.output/"hierarchy-report.json").write_text(json.dumps(report,indent=2)+"\n")
                browser.close()
                return
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
            assert analysis["schema_version"] == 12
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

            partial = submit_bytecode(page, canvas, "5a5f5ff0", args.output / "uncovered-creation-input.png")
            assert partial.status == 200
            partial_reply = partial.json()
            incomplete = partial_reply["result"]["Ok"]
            assert incomplete["status"] == "Incomplete"
            assert any(frontier["kind"] == "Creation" and "UnknownInitCode" in frontier["detail"] for frontier in incomplete["frontiers"])
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
            compression = compression_interactions(browser, url, args.output)
            http_transport = http_transport_interactions(browser, url, args.output)
            timings = timing_interactions(browser, args)
            rpc_providers = unconfigured_rpc_interactions(browser, url, args.output)
            world = configured_world_interactions(browser, args)
            url_world = url_configured_world_interactions(browser, args)
            overview = block_overview_interactions(browser, url, args.output)
            panes = pane_interactions(browser, url, args.output)
            flow = edge_flow_interactions(browser, url, args.output)
            compact_ssa = compact_ssa_interactions(browser, url, args.output)
            ssa_details = ssa_details_interactions(browser, url, args.output)
            ssa_values = ssa_values_interactions(browser, url, args.output)
            unknown_world = unknown_world_interactions(browser, url, args.output)
            inspector_controls = inspector_controls_interactions(browser, url, args.output)
            storage_table = storage_table_interactions(browser, url, args.output)
            storage_scroll = storage_scroll_interactions(browser, url, args.output)
            termination = termination_interactions(browser, url, args)
            gestures = graph_interactions(playwright.chromium, args.browser, url, args.output)
            hierarchy = hierarchy_interactions(browser,url,args)
            diagnostics = diagnostics_interactions(browser, url, args)
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
                    "uncovered_creation": incomplete["status"],
                },
                "screenshots": screenshots,
                "viewport_screenshots": viewport_screenshots,
                "responsive_input": responsive_input,
                "graph_interactions": gestures,
                "pane_interactions": panes,
                "edge_flow": flow,
                "compact_ssa": compact_ssa,
                "ssa_details": ssa_details,
                "ssa_values": ssa_values,
                "unknown_world": unknown_world,
                "inspector_controls": inspector_controls,
                "storage_table": storage_table,
                "storage_scroll": storage_scroll,
                "termination": termination,
                "block_overview": overview,
                "world_interactions": world,
                "url_configured_world": url_world,
                "rpc_providers": rpc_providers,
                "budget_interactions": budgets,
                "compression": compression,
                "http_transport": http_transport,
                "timing_interactions": timings,
                "platform": "Linux headless Chromium with Xvfb and SwiftShader WebGPU; synthesized browser gestures, not physical macOS hardware",
                "browser_errors": errors,
                "hierarchical_cfg": hierarchy,
                "invalid_opcode_diagnostics": diagnostics,
            }
            (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
            browser.close()


if __name__ == "__main__":
    main()
