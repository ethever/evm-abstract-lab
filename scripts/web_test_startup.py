"""Startup browser acceptance cases."""

from playwright.sync_api import expect
import hashlib
import json
import re
from web_test_example_helpers import example_page
from web_test_hybrid import assert_hybrid, displayed_graph
from web_test_harness import rendered_frame, webgpu_evidence


def run_startup(context):
    page, canvas, reply = example_page(context.browser, context.url)
    result = reply.json()
    full_analysis = result["result"]["Ok"]
    assert_hybrid(full_analysis)
    analysis = displayed_graph(full_analysis)
    assert analysis["schema_version"] == 21
    assert analysis["metadata"]["postprocessing_max_work"] == 2_000_000_000
    assert 0 <= analysis["metadata"]["postprocessing_work"] <= 2_000_000_000
    assert sum(len(block["instructions"]) for block in analysis["disassembly"]) > 0
    assert len(analysis["cfg"]) > 1, "example did not produce a graph"
    assert {"BranchTrue", "BranchFalse"}.issubset({edge["kind"] for edge in analysis["edges"]})
    assert analysis["ssa"]["blocks"], "example did not produce SSA"
    (context.output / "analysis.json").write_text(json.dumps(result, indent=2) + "\n")
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
        screenshot = canvas.screenshot(path=str(context.output / f"{name}.png"))
        assert len(screenshot) > 10000, f"{name} did not paint a substantial canvas"
        screenshots[name] = hashlib.sha256(screenshot).hexdigest()
    assert len(set(screenshots.values())) == 4, "view shortcuts did not repaint distinct custom views"
    renderer = webgpu_evidence(page)
    result = {"branch": analysis["status"], "screenshots": screenshots,
              "renderer": renderer, "status": status.text_content()}
    page.close()
    return result


def run(context):
    return run_startup(context)
