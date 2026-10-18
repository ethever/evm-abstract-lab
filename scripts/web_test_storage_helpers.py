"""Storage helpers browser acceptance helpers."""

from pathlib import Path
import base64
import hashlib
import json
import time
import zlib
from playwright.sync_api import Error as PlaywrightError
from web_test_harness import rendered_frame, settle_gesture
from web_test_pixels import canvas_screenshot, save_png_crop, text_pixels


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

def storage_scrolled_snapshot(page, evidence: Path, region, previous):
    """Wait for the resized, scrolled table's actual painted rows."""
    deadline = time.monotonic() + 30
    while True:
        png = canvas_screenshot(page, path=str(evidence))
        try:
            table = storage_table_pixels(page, png, region)
        except PlaywrightError as error:
            if "missing painted storage column separator" not in str(error):
                raise
            problem = error
        else:
            rows = table["rows"]
            # This fixture moved the inspector below its previous entire table.
            # Require all original rows and the terminal Value's dimensions so
            # an old frame or a partially clipped table cannot satisfy readiness.
            ready = (table["table_bounds"][1] > previous["table_bounds"][3]
                     and len(rows) == len(previous["rows"])
                     and all(row["slot"] and row["value"] for row in rows)
                     and rows[-1]["value"]["mask_size"] == previous["rows"][-1]["value"]["mask_size"])
            if ready:
                return table
            problem = AssertionError(f"scrolled storage rows are not painted: {storage_pixel_report(table)}")
        if time.monotonic() >= deadline:
            raise problem
        rendered_frame(page)

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
        "{0x105f}": ([61,12], "fd1ab3bd3503fbd583c7f1443389c9cd9c436bfc915f8bb5ae4d0b187b5a5f22"),
        "{0x205f}": ([61,12], "01a9f16f529282cffa2f49e72b9c1686e9ca3d452e2ce788636eaa5c83592176"),
    }
    size,expected=reviewed[text]
    if text==STORAGE_SLOT_WORD:
        # The original Slot galley differs by at most one RGB step in the saved
        # current capture; its red127->128 sample changes the thresholded mask.
        return assert_storage_reviewed_pixels(cell,"full_slot",text,size,expected)
    if text=="{"+STORAGE_SLOT_WORD+"}":
        # One historical red127 sample rounds to128 in current Chromium.
        # Compare the complete independent raster with the existing <=2 bound.
        return assert_storage_reviewed_pixels(cell,"full_value",text,size,expected)
    if text in ("{0x105f}", "{0x205f}"):
        # Moving the scroll viewport can round a 127 sample to128. Verify every
        # RGB sample against the original short-literal raster, including braces.
        key = "last_storage_value" if text == "{0x105f}" else "last_transient_value"
        return assert_storage_reviewed_pixels(cell,key,text,size,expected)
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
