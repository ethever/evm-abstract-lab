"""Ending helpers browser acceptance helpers."""

import base64
import re


def selected_ending(page):
    status=page.locator("#analysis-status").text_content()
    match=re.search(r"Selected CFG ending: (.+?); (\d+) native members; root normal (\d+), REVERT (\d+), exception (\d+); frame normal (\d+), REVERT (\d+), exception (\d+); continuing (\d+), unresolved (\d+), non-current (\d+), unobserved (\d+)",status)
    assert match,status
    keys=['members','root_normal','root_revert','root_failure','frame_normal','frame_revert','frame_failure','continuing','unresolved','non_current','unobserved']
    return {'kind':match[1],**dict(zip(keys,map(int,match.groups()[1:]),strict=True))}

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
