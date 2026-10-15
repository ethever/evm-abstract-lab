"""Pixels browser acceptance helpers."""

from pathlib import Path
import base64
import math
from statistics import median
import re


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
            const fourSides=Object.values(outlineSides).every(score=>score>=0.45);
            const ox0=fourSides?Math.min(...positions.left):x0,oy0=fourSides?Math.min(...positions.top):y0;
            const ox1=fourSides?Math.max(...positions.right):x1,oy1=fourSides?Math.max(...positions.bottom):y1;
            // Card outlines have rounded corners. A canvas clip at the straight
            // header separator can mimic four sides, but lacks these cutouts.
            const roundedCorners=fourSides && [[ox0,oy0],[ox1,oy0],[ox0,oy1],[ox1,oy1]].every(([x,y])=>
                x>=0 && x<width && y>=0 && y<height && !outline(x,y) && fillAt[y*width+x]<0);
            const complete=fourSides && roundedCorners;
            // Tiny selected fills can have fewer than 30 exact-color pixels.
            // Keep the ordinary cutoff; require a complete rounded selection
            // outline with all four accent sides and the same pixel threshold.
            const tinySelected=complete && visibleSides.length===4 && borderPixels>=30*scale*scale;
            const measurable=(write>=30*scale*scale || tinySelected)
                && Math.max(x1-x0+1,bx1-bx0+1)>=10*scale
                && Math.max(y1-y0+1,by1-by0+1)>=8*scale;
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

def workspace_content_bottom(page, screenshot: bytes, *, above_inspector=False):
    """Measure the status panel's top, optionally stopping above a docked Inspector."""
    status = (page.locator("#analysis-status").text_content() or "") if above_inspector else ""
    panels = 2 if re.search(r"(?:^|; )Inspector:", status) and " · Maximized" not in status else 1
    return page.evaluate("""async ({png,panels}) => {
        const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();
        const c=document.createElement('canvas');c.width=image.width;c.height=image.height;
        const ctx=c.getContext('2d');ctx.drawImage(image,0,0);
        const data=ctx.getImageData(0,0,c.width,c.height).data,scale=c.width/innerWidth;
        const border=(x,y)=>{const i=(y*c.width+x)*4;
            return data[i]===47 && data[i+1]===62 && data[i+2]===80;};
        // Native bottom Panels paint full-width separators. Interior rules,
        // cards and text do not reach both canvas edges. Collapse HiDPI bands.
        const lines=[];
        for(let y=c.height-1;y>=0;y--) {
            if(!border(0,y) || !border(c.width-1,y))continue;
            let x=1;while(x<c.width-1 && border(x,y))x++;
            if(x<c.width-1)continue;
            if(lines.length && lines[lines.length-1]===y+1)lines[lines.length-1]=y;
            else lines.push(y);
        }
        if(lines.length<panels)throw new Error(`missing bottom panel separators: ${lines}`);
        return lines[panels-1]/scale;
    }""", {"png":base64.b64encode(screenshot).decode(),"panels":panels})

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
    # below this palette threshold. Exclude status separators, which also match.
    return page.evaluate("""async ({png,contentBottom}) => {
        const image = new Image(); image.src = `data:image/png;base64,${png}`; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width=image.width; canvas.height=image.height;
        const context=canvas.getContext('2d'); context.drawImage(image,0,0);
        const scale=image.width/innerWidth;
        const top=Math.round(123*scale), bottom=Math.round(contentBottom*scale);
        const data=context.getImageData(0,top,image.width,bottom-top).data;
        let left=image.width,right=-1,first=bottom,last=-1,count=0;
        for(let y=0;y<bottom-top;y++) for(let x=4*scale;x<image.width-4*scale;x++) {
            const i=(y*image.width+x)*4, r=data[i],g=data[i+1],b=data[i+2];
            if(g>=48 && g-r>=10 && b>=g-25) {
                left=Math.min(left,x);right=Math.max(right,x);first=Math.min(first,y+top);last=Math.max(last,y+top);count++;
            }
        }
        return {bounds:count?[left/scale,first/scale,(right+1)/scale,(last+1)/scale]:null,pixels:count};
    }""", {"png":base64.b64encode(png).decode(),
           "contentBottom":workspace_content_bottom(page, png, above_inspector=True)})

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
