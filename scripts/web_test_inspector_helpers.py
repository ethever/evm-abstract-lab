"""Inspector helpers browser acceptance helpers."""

import base64
from playwright.sync_api import expect
import re
from web_test_harness import settle_gesture


INSPECTOR_TAB_POSITIONS = {
    "Frame": 30, "Stack": 80, "SSA value": 141, "Memory": 210,
    "Storage": 274, "Transient": 341, "Calldata": 408, "Returndata": 481,
    "Outcomes": 561, "Acquisition": 640, "Environment": 727, "Timing": 799, "Diagnostics": 866,
}

def hide_inspector(page, canvas):
    # Native geometry tests cover this right-aligned header control; the action
    # is exercised with an actual pointer and leaves more canvas for gestures.
    canvas.click(position={"x": page.viewport_size["width"] - 124, "y": 12})
    settle_gesture(page)

def select_inspector_tab(page, canvas, tab: str, y: float | None = None):
    # Compact shared controls, measured in the packaged 1440x1000 WASM app.
    # Preserve caller-supplied geometry after inspector resizing or dragging.
    if y is None:
        selected = inspector_selected_tab(page, canvas)
        y = (selected[1] + selected[3]) / 2
    canvas.click(position={"x": INSPECTOR_TAB_POSITIONS[tab], "y": y})
    expect(page.locator("#analysis-status")).to_contain_text(f"Inspector: {tab}")

def inspector_painted_controls(page, canvas):
    """Measure compact selected tabs and inactive title actions from their actual fills."""
    page.mouse.move(2, 2)
    settle_gesture(page)
    png = canvas.screenshot()
    return page.evaluate("""async png => {
        const image = new Image(); image.src = `data:image/png;base64,${png}`;
        await image.decode();
        const c = document.createElement('canvas'); c.width=image.width; c.height=image.height;
        const ctx=c.getContext('2d'); ctx.drawImage(image,0,0);
        const pixels=ctx.getImageData(0,0,c.width,c.height).data;
        const visited=new Uint8Array(c.width*c.height), scale=c.width/innerWidth;
        const result={selected:[],actions:[]};
        // palette::SELECTED and inactive action RAISED; move the pointer away
        // before capture so a hovered action cannot be mistaken for a tab.
        const kind=i=>pixels[i*4]===33 && pixels[i*4+1]===65 && pixels[i*4+2]===75 ? 0 :
            pixels[i*4]===29 && pixels[i*4+1]===40 && pixels[i*4+2]===56 ? 1 : -1;
        for (let y=Math.ceil(25*scale);y<c.height;y++) for(let x=0;x<c.width;x++) {
            const start=y*c.width+x;
            const group=kind(start);
            if(visited[start] || group<0)continue;
            const queue=[start];visited[start]=1;let left=x,right=x,top=y,bottom=y,count=0;
            for(let at=0;at<queue.length;at++) {
                const i=queue[at],px=i%c.width,py=Math.floor(i/c.width);count++;
                left=Math.min(left,px);right=Math.max(right,px);top=Math.min(top,py);bottom=Math.max(bottom,py);
                for(const next of [px>0?i-1:-1,px+1<c.width?i+1:-1,py>0?i-c.width:-1,py+1<c.height?i+c.width:-1]) {
                    if(next>=0 && !visited[next] && kind(next)===group){visited[next]=1;queue.push(next);}
                }
            }
            const w=(right-left+1)/scale,h=(bottom-top+1)/scale;
            if(w>=(group===0?40:30) && w<=100 && h>=14 && h<=20 && count>120*scale*scale)
                result[group===0?'selected':'actions'].push([left/scale,top/scale,(right+1)/scale,(bottom+1)/scale]);
        }
        for(const rectangles of Object.values(result))rectangles.sort((a,b)=>a[1]-b[1]);
        return result;
    }""", base64.b64encode(png).decode())

def inspector_selected_tab(page, canvas, tab: str | None = None):
    """Locate the status-selected tab without assuming the inspector's height."""
    labels = "|".join(re.escape(label) for label in INSPECTOR_TAB_POSITIONS)
    selected = re.findall(rf"(?:^|; )Inspector: ({labels})(?= ·| %|;|$)",
                          page.locator("#analysis-status").text_content() or "")
    assert len(selected) == 1, f"expected one selected Inspector label, got {selected}"
    if tab is not None:
        assert selected == [tab], f"expected Inspector: {tab}, got {selected}"
    tab = selected[0]
    controls = inspector_painted_controls(page, canvas)
    # The inspector alone has adjacent right-aligned Maximize/Restore and Hide
    # actions. Match their painted row, not another selected control at the same
    # x (notably graph Blocks beside the Memory tab's x coordinate).
    titlebars = [hide for hide in controls["actions"]
                 if 2 <= page.viewport_size["width"] - hide[2] <= 10
                 and 30 <= hide[2] - hide[0] <= 45
                 and any(45 <= action[2] - action[0] <= 85
                         and 2 <= hide[0] - action[2] <= 9
                         and abs((hide[1] + hide[3] - action[1] - action[3]) / 2) <= 1
                         for action in controls["actions"])]
    assert len(titlebars) == 1, f"expected one Inspector Hide/Maximize titlebar, got {titlebars}"
    titlebar = titlebars[0]
    candidates = [rect for rect in controls["selected"]
                  if abs((rect[0] + rect[2]) / 2 - INSPECTOR_TAB_POSITIONS[tab]) <= 8
                  and abs((rect[1] + rect[3] - titlebar[1] - titlebar[3]) / 2) <= 1
                  and (tab != "Frame" or (4 <= rect[0] <= 10 and 40 <= rect[2] - rect[0] <= 50))]
    assert len(candidates) == 1, f"expected one selected {tab} tab, got {candidates}"
    return candidates[0]

def inspector_frame_tab(page, canvas):
    """Keep Frame-specific assertions for drag, resize and restore callers."""
    expect(page.locator("#analysis-status")).to_contain_text("Inspector: Frame")
    return inspector_selected_tab(page, canvas, "Frame")

def inspector_block_entry_control(page, canvas):
    """Find the selected Block entry control immediately below the Frame toolbar."""
    expect(page.locator("#analysis-status")).to_contain_text("Inspector: Frame · Block entry")
    toolbar = inspector_frame_tab(page, canvas)
    candidates = [rect for rect in inspector_painted_controls(page, canvas)["selected"]
                  if 20 <= rect[0] <= 50 and 70 <= rect[2] - rect[0] <= 85
                  and 0 <= rect[1] - toolbar[3] <= 10]
    assert len(candidates) == 1, f"expected one selected Block entry control, got {candidates}"
    return candidates[0]

def inspector_diagnostics_region(page, canvas):
    """The existing default 210px dock has no observation row on Diagnostics."""
    title = inspector_selected_tab(page, canvas, "Diagnostics")
    # Start after the actual titlebar and its 3px item gap. This keeps the same
    # 176px content window inside the default dock, above the status/Job ID panel.
    top = title[3] + 3
    return (8, top, 1410, top + 176)

def inspector_title_action(page, canvas, action: str, selected_tab):
    """Use the real right-aligned titlebar at its painted vertical position."""
    inset = {"Maximize": 79, "Restore": 73, "Hide": 24}[action]
    canvas.click(position={"x": page.viewport_size["width"] - inset,
                           "y": (selected_tab[1] + selected_tab[3]) / 2})
    settle_gesture(page)
