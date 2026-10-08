#!/usr/bin/env python3
"""Exercise the packaged WASM app against its real same-origin analysis server."""

import argparse
import hashlib
import json
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
    # At this fixed viewport the two-line egui bytecode editor sits below the
    # toolbar. Use real pointer/keyboard input so serialization is tested too.
    canvas.click(position={"x": 300, "y": 85})
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
            browser = playwright.chromium.launch(
                executable_path=args.browser,
                headless=True,
                args=[
                    "--no-sandbox",
                    "--use-gl=angle",
                    "--use-angle=swiftshader",
                    "--enable-unsafe-swiftshader",
                ],
            )
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
            page.keyboard.press("0")
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
