"""Inspector helpers browser acceptance helpers."""

from playwright.sync_api import expect
import re
import time
from web_test_harness import click_control, set_control, settle_gesture, ui_control, ui_controls


INSPECTOR_GROUPS = {
    "Frame": "State", "Stack": "State", "SSA value": "State",
    "Memory": "Data", "Calldata": "Data", "Returndata": "Data",
    "Storage": "Effects", "Transient": "Effects", "Outcomes": "Effects",
    "Coverage": "Evidence", "Acquisition": "Evidence", "Environment": "Evidence",
    "Diagnostics": "Evidence", "Timing": "Timing",
}


def show_inspector(page, canvas):
    """Open the actual Inspector lane without resetting its tab or panel size."""
    return set_control(page, canvas, "Inspect", True)


def hide_inspector(page, canvas):
    """Keep an initially collapsed Inspector closed, including on narrow views."""
    return set_control(page, canvas, "Inspect", False)


def inspector_choice(page, canvas, menu, label, *, role="tab"):
    """Pick an actual popup response, disambiguated from the underlying tabs."""
    before = {control["id"] for control in ui_controls(page)}
    click_control(page, canvas, menu, role="combobox")
    deadline = time.monotonic() + 5
    while True:
        choices = [control for control in ui_controls(page)
                   if control["id"] not in before and control["label"] == label
                   and control["role"] == role and control["enabled"]]
        if choices or time.monotonic() >= deadline:
            break
        page.wait_for_timeout(25)
    assert len(choices) == 1, (menu, label, choices)
    return click_control(page, canvas, label, role=role, id=choices[0]["id"])


def select_inspector_scope(page, canvas, scope):
    """Keep original observations, joined inputs and paired outcomes distinct."""
    assert scope in {"Original observations", "Procedure input", "Joint outcome"}, scope
    show_inspector(page, canvas)
    if any(control["label"] == "inspector_observation_scope" and control["role"] == "combobox"
           for control in ui_controls(page)):
        inspector_choice(page, canvas, "inspector_observation_scope", scope)
    else:
        control = ui_control(page, scope, role="tab")
        if not control["selected"]:
            click_control(page, canvas, scope, role="tab")
        ui_control(page, scope, role="tab", selected=True)
    expect(page.locator("#analysis-status")).to_contain_text(scope)

def select_inspector_tab(page, canvas, tab: str, y: float | None = None, *, minimum=False):
    # Older callers pass the previous toolbar y after resizing. Navigation now
    # follows accepted response geometry; the previous row is never clickable.
    assert tab in INSPECTOR_GROUPS, tab
    show_inspector(page, canvas)
    group = INSPECTOR_GROUPS[tab]
    if any(control["label"] == "inspection_group" and control["role"] == "combobox"
           for control in ui_controls(page)):
        inspector_choice(page, canvas, "inspection_group", group)
    else:
        control = ui_control(page, group, role="tab")
        if not control["selected"]:
            click_control(page, canvas, group, role="tab")
        ui_control(page, group, role="tab", selected=True)
    inspector_choice(page, canvas, "inspector_tab", tab)
    expect(page.locator("#analysis-status")).to_contain_text(f"Inspector: {tab}")
    return inspector_selected_tab(page, canvas, tab, minimum=minimum)

def inspector_selected_tab(page, canvas, tab: str | None = None, *, minimum=False):
    """Measure the actual content selector; reading it never reopens a hidden pane."""
    ui_control(page, "Inspect", role="toggle", selected=True)
    labels = "|".join(re.escape(label) for label in INSPECTOR_GROUPS)
    selected = re.findall(rf"(?:^|; )Inspector: ({labels})(?= ·| %|;|$)",
                          page.locator("#analysis-status").text_content() or "")
    assert len(selected) == 1, f"expected one selected Inspector label, got {selected}"
    if tab is not None:
        assert selected == [tab], f"expected Inspector: {tab}, got {selected}"
    return ui_control(page, "inspector_tab", role="combobox")["rect"]

def inspector_frame_tab(page, canvas):
    """Keep Frame-specific assertions for drag, resize and restore callers."""
    expect(page.locator("#analysis-status")).to_contain_text("Inspector: Frame")
    return inspector_selected_tab(page, canvas, "Frame")

def inspector_block_entry_control(page, canvas):
    """Measure the actual Native before/after selector at a block entry."""
    expect(page.locator("#analysis-status")).to_contain_text("Inspector: Frame")
    expect(page.locator("#analysis-status")).to_contain_text("Block entry")
    inspector_frame_tab(page, canvas)
    return ui_control(page, "inspector_boundary", role="combobox")["rect"]


def select_inspector_boundary(page, canvas, point, *, historical=False):
    """Choose a retained Native boundary without upgrading a historical exit."""
    assert point in {"before", "after"}, point
    label = "Before block · entry snapshot" if point == "before" else (
        "After block · historical exit" if historical else "After block · observed exit")
    inspector_choice(page, canvas, "inspector_boundary", label)
    expected = "Block entry" if point == "before" else (
        "Historical exit" if historical else "Observed exit")
    expect(page.locator("#analysis-status")).to_contain_text(expected)
    return ui_control(page, "inspector_boundary", role="combobox")["rect"]


def select_inspector_frame(page, canvas, frame, *, active):
    """Frame indices come from the selected boundary's retained machine only."""
    assert type(frame) is int and frame >= 0, frame
    assert type(active) is bool, active
    label = f"Frame {frame + 1} · {'active' if active else 'suspended caller'}"
    inspector_choice(page, canvas, "inspector_frame", label)
    return ui_control(page, "inspector_frame", role="combobox")["rect"]

def inspector_diagnostics_region(page, canvas):
    """The existing default 210px dock has no observation row on Diagnostics."""
    title = inspector_selected_tab(page, canvas, "Diagnostics")
    # Start after the actual titlebar and its 3px item gap. This keeps the same
    # 176px content window inside the default dock, above the status/Job ID panel.
    top = title[3] + 3
    return (8, top, 1410, top + 176)

def inspector_title_action(page, canvas, action: str, selected_tab):
    """Use the actual panel action after a drag, resize or maximize."""
    assert action in {"Maximize", "Restore", "Hide"}, action
    click_control(page, canvas, action, role="action")
