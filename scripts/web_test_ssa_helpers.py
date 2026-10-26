"""Ssa helpers browser acceptance helpers."""

from playwright.sync_api import expect
from web_test_harness import set_control, settle_gesture


SSA_DETAILS_CODE = "60036007819001905060005200"

def ssa_details_toggle(page, canvas, mode):
    assert mode in {"Compact", "Full"}, mode
    set_control(page, canvas, "SSA details", mode == "Full")
    page.mouse.move(2,2);settle_gesture(page)
    expect(page.locator("#analysis-status")).to_contain_text(f"SSA details: {mode}")
