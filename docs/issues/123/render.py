#!/usr/bin/env python3
"""Render the four local HTML figures using Playwright Chromium.

Requires the Python package playwright. Use its standard installed Chromium,
or pass --chromium /path/to/chromium (also CHROMIUM_EXECUTABLE).
All HTML/CSS resources remain relative to this script's directory.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct

from playwright.sync_api import sync_playwright


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chromium", default=os.environ.get("CHROMIUM_EXECUTABLE"),
                        help="Chromium executable; defaults to standard Playwright browser")
    parser.add_argument("--max-height", type=int, default=1400, help="Maximum figure height in pixels")
    parser.add_argument("--output-dir", type=Path,
                        help="PNG directory; defaults to the HTML source directory")
    parser.add_argument("--verification-json", type=Path,
                        help="Optional local runtime verification; may contain machine paths")
    args = parser.parse_args()
    source_dir = Path(__file__).resolve().parent
    output_dir = args.output_dir or source_dir
    output_dir.mkdir(parents=True, exist_ok=True)
    stems = ["01-loop-widening", "02-recursive-summary", "03-recursion-relation", "04-rollback-context"]
    evidence = {"chromium_executable_argument": args.chromium, "figures": [],
                "viewport_width": 1120, "device_scale_factor": 1,
                "maximum_allowed_height": args.max_height}
    errors = []
    with sync_playwright() as playwright:
        launch = {"headless": True, "args": ["--disable-dev-shm-usage"]}
        if args.chromium:
            launch["executable_path"] = args.chromium
        browser = playwright.chromium.launch(**launch)
        evidence["browser_version"] = browser.version
        context = browser.new_context(viewport={"width": 1120, "height": 1200},
                                      device_scale_factor=1, locale="zh-CN", color_scheme="light")
        for stem in stems:
            page = context.new_page()
            page.goto((source_dir / (stem + ".html")).as_uri(), wait_until="load")
            page.evaluate("document.fonts.ready")
            canvas = page.locator(".canvas")
            layout = page.evaluate("""() => {
              const root = document.querySelector('.canvas');
              const bounds = root.getBoundingClientRect();
              const errors = [];
              for (const e of root.querySelectorAll('h1,h2,p,pre,footer,.panel,.context-card,.node,.call-site')) {
                const r = e.getBoundingClientRect();
                if (r.left < bounds.left - 1 || r.right > bounds.right + 1 ||
                    r.top < bounds.top - 1 || r.bottom > bounds.bottom + 1 ||
                    e.scrollWidth > e.clientWidth + 1 || e.scrollHeight > e.clientHeight + 1) {
                  errors.push({tag:e.tagName, class:e.className, text:e.textContent.trim(),
                               width:r.width, height:r.height, scrollWidth:e.scrollWidth,
                               clientWidth:e.clientWidth, scrollHeight:e.scrollHeight,
                               clientHeight:e.clientHeight});
                }
              }
              return {width:bounds.width, height:bounds.height,
                      page_scroll_width:document.documentElement.scrollWidth,
                      font_family:getComputedStyle(root).fontFamily, errors};
            }""")
            cdp = context.new_cdp_session(page)
            cdp.send("DOM.enable")
            cdp.send("CSS.enable")
            document = cdp.send("DOM.getDocument")
            node = cdp.send("DOM.querySelector", {"nodeId": document["root"]["nodeId"], "selector": "h1"})
            fonts = cdp.send("CSS.getPlatformFontsForNode", {"nodeId": node["nodeId"]})["fonts"]
            output = output_dir / (stem + ".png")
            canvas.screenshot(path=str(output), animations="disabled", type="png")
            data = output.read_bytes()
            assert data[:8] == b"\x89PNG\r\n\x1a\n"
            width, height = struct.unpack(">II", data[16:24])
            row = {"html": stem + ".html", "png": output.name, "width": width, "height": height,
                   "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
                   "layout": layout, "title_platform_fonts": fonts}
            evidence["figures"].append(row)
            if width != 1120 or height > args.max_height or layout["errors"]:
                errors.append(row)
            print(json.dumps({"png": output.name, "width": width, "height": height,
                              "layout_error_count": len(layout["errors"])}, ensure_ascii=False), flush=True)
            page.close()
        context.close()
        browser.close()
    evidence["all_size_and_layout_checks_passed"] = not errors
    if args.verification_json:
        args.verification_json.parent.mkdir(parents=True, exist_ok=True)
        args.verification_json.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n")
    if errors:
        raise SystemExit("One or more figures exceed bounds or contain overflowing elements; see local verification JSON.")


if __name__ == "__main__":
    main()
