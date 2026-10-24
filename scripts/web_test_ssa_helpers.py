"""Ssa helpers browser acceptance helpers."""

from playwright.sync_api import expect
from web_test_harness import settle_gesture


SSA_DETAILS_CODE = "60036007819001905060005200"

def ssa_details_toggle(page, canvas, mode):
    # At the existing 640px responsive fixture the explicit root condition wraps
    # once; the actual metadata control moves down one 16px label line.
    canvas.click(position={"x":page.viewport_size["width"]-45,
                           "y":72 if page.viewport_size["width"]<1000 else 56})
    page.mouse.move(2,2);settle_gesture(page)
    expect(page.locator("#analysis-status")).to_contain_text(f"SSA details: {mode}")
