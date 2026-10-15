"""Panes browser acceptance cases."""

from pathlib import Path
from playwright.sync_api import expect
from web_test_graph_helpers import workspace_panes
from web_test_harness import observe_webgpu, rendered_frame, resize_view, settle_gesture, submit_bytecode, webgpu_evidence
from web_test_inspector_helpers import hide_inspector
from web_test_pixels import assert_translation, graph_screenshot_metrics


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


def run(context):
    return pane_interactions(context.browser, context.url, context.output)
