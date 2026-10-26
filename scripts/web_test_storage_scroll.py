"""Storage scroll browser acceptance cases."""

from web_test_hybrid import oracle_graph, source_program
from pathlib import Path
from playwright.sync_api import expect
import json
import re
from web_test_harness import observe_webgpu, settle_gesture, submit_bytecode, webgpu_evidence
from web_test_inspector_helpers import inspector_frame_tab, inspector_title_action, select_inspector_tab
from web_test_pixels import save_png_crop, text_pixels
from web_test_storage_helpers import assert_storage_literal_pixels, drag_storage_separator, storage_pixel_report, storage_table_pixels


def storage_scroll_interactions(browser, url: str, output: Path):
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    errors, posts = [], []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on("request", lambda request: posts.append(request.url)
            if request.method == "POST" and request.url.endswith("/api/tasks") else None)
    page.goto(url, wait_until="networkidle")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:", timeout=60000)
    canvas = page.locator("#evm-canvas")
    count = 96
    code = "".join(f"61{0x1000+i:04x}61{i:04x}5561{0x2000+i:04x}61{i:04x}5d"
                   for i in range(count)) + "00"
    reply = submit_bytecode(page, canvas, code, output / "storage-scroll-input.png")
    report = reply.report
    (output / "storage-scroll-full-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    assert report["hybrid"]["coverage"]["baseline_current"]
    baseline=oracle_graph(report, "Baseline")
    original_entry=baseline["states"][0]["entry"]
    assert baseline["stores"][original_entry["store"]]["touched_default"] is None
    report=oracle_graph(report, "Native")
    assert report["status"] == "Converged" and len(report["states"]) == 1
    machine = report["states"][0]["exit"]
    account = next(account for account in report["stores"][machine["store"]]["accounts"]
                   if account["address"] == machine["frames"][0]["storage_address"])
    store=report["stores"][machine["store"]]
    # This fixture paints the default touch prefix and exactly nine account
    # facts: the captured program is present, with no delegation row.
    assert store["touched_default"] is False and account["touched"] is True
    root = source_program(report, report["metadata"]["root_program"])
    frame, = machine["frames"]
    assert account["program"] == frame["program"] == root["id"]
    assert root["code_address"] == frame["code_address"] == account["address"]
    assert root["code_hash"] == frame["code_hash"]
    assert root["kind"] == "Runtime" and root["fork"] == report["fork"]
    assert root["bytecode"].removeprefix("0x") == code and account["delegation_target"] is None
    assert account["created"] is False and account["pending_destruction"] is False
    graph=report["symbolic"]["nodes"]
    ordered_roots={}
    for kind, value_base in [("storage", 0x1000), ("transient", 0x2000)]:
        assert len(account[kind]) == count
        last = next(entry for entry in account[kind] if int(entry["slot"], 16) == count - 1)
        assert last["value"]["constants"] == [hex(value_base + count - 1)]
        # The new prefix describes this shared ordered DAG, independently of
        # sparse observations. Preserve every update and the unknown/zero base.
        node=account[kind+"_root"]["node"]
        for slot in reversed(range(count)):
            update=graph[node]["Store"]
            assert graph[update["key"]["node"]]=={"Constant":hex(slot)},update
            assert graph[update["value"]["node"]]=={"Constant":hex(value_base+slot)},update
            node=update["previous"]["node"]
        base=graph[node]
        default=account[kind+"_default"]
        if kind=="storage":
            source=base["SlotsInput"]
            assert source["bindings"]==[] and "Fresh" in graph[source["source"]["token"]["node"]],source
            assert int(source["source"]["namespace"],16)==int(account["address"],16)<<8,source
            assert default["constants"] is None and default["known_zero"]==default["known_one"]=="0x0",default
            assert default["unsigned"]=={"lower":"0x0","upper":hex((1<<256)-1)},default
            assert default["signed"]=={"lower":hex(1<<255),"upper":hex((1<<255)-1)},default
            assert default["congruence"]=="Any" and default["origins"] is None and not default["nonzero"],default
        else:
            assert base=="SlotsZero" and default["constants"]==["0x0"],(base,default)
            assert default["known_zero"]==hex((1<<256)-1) and default["known_one"]=="0x0",default
            assert default["unsigned"]==default["signed"]=={"lower":"0x0","upper":"0x0"},default
            assert default["congruence"]=={"Exact":"0x0"} and default["origins"]==["Constant"] and not default["nonzero"],default
        assert not default["code_address_role"] and default["identity"] is None and default["expression"] is None and not default["symbolic_limit"],default
        assert store[kind+"_facts"]==[],store[kind+"_facts"]
        ordered_roots[kind]={"root":account[kind+"_root"],"writes":count,"base_node":node,"base":base,"default":default}
    (output / "storage-scroll-analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    canvas.click(position={"x": 720, "y": 600})
    page.keyboard.press("3")
    tab = inspector_frame_tab(page, canvas)
    inspector_title_action(page, canvas, "Maximize", tab)
    expect(status).to_contain_text(re.compile("maximized", re.IGNORECASE))
    tab = inspector_frame_tab(page, canvas)
    toolbar_y = (tab[1] + tab[3]) / 2
    evidence = {}
    # Independent glyph rasters from the reviewed DPR1 "0x5f" and "{0x105f}" /
    # "{0x205f}" crops. Separate cells preserve the old exact-text acceptance
    # despite the adjustable gap, without allowing an off-by-one final slot.
    last_cell_hashes = {
        "slot": "1f94c5cbc9d80be833ac49b7540e5baa49cd503a8144a920697946262ad7e2d8",
        "storage": "fd1ab3bd3503fbd583c7f1443389c9cd9c436bfc915f8bb5ae4d0b187b5a5f22",
        "transient": "01a9f16f529282cffa2f49e72b9c1686e9ca3d452e2ce788636eaa5c83592176",
    }
    for title, kind in [("Storage", "storage"), ("Transient", "transient")]:
        select_inspector_tab(page, canvas, title, toolbar_y)
        canvas.click(position={"x": 145, "y": tab[3] + 14})
        expect(status).to_contain_text("Observed exit")
        page.mouse.move(2, 2)
        settle_gesture(page)
        default_png = canvas.screenshot(path=str(output / f"{kind}-facts-default-width.png"))
        default_table = storage_table_pixels(page, default_png,
            (4,tab[3]+52,page.viewport_size["width"]-24,page.viewport_size["height"]-30))
        assert default_table["header"]["slot"] and default_table["header"]["value"],storage_pixel_report(default_table)
        # Preserve the reviewed final-row glyph placement at original Value x=138.
        # The larger default moves Value to x=531, where one unchanged glyph
        # sample rounds from 127 to 128 and crosses the exact ink-mask threshold.
        # A real native drag also lets this scroll case retain an explicit width.
        drag_storage_separator(page,default_table,134.5)
        page.mouse.move(2,2)
        settle_gesture(page)
        collapsed = canvas.screenshot(path=str(output / f"{kind}-facts-collapsed.png"))
        table = storage_table_pixels(page, collapsed,
            (4,tab[3]+52,page.viewport_size["width"]-24,page.viewport_size["height"]-30))
        assert abs(table["separator"]-134.5)<=1,storage_pixel_report(table)
        assert table["header"]["slot"] and table["header"]["value"],storage_pixel_report(table)
        # Address, touch default, ordered-state root, closed account header and
        # base default precede the table, in the order painted by stores.rs.
        collapsed_prefix_region=(4,tab[3]+55,600,table["table_bounds"][1])
        collapsed_prefix=text_pixels(page,collapsed,collapsed_prefix_region)
        assert len(collapsed_prefix["rows"])==5,"account facts were already expanded or the touch/ordered-state prefix is missing"
        address_band,touch_band,root_band,account_band,default_band=collapsed_prefix["rows"]
        collapsed_facts_region=(145,account_band[1]+1,650,default_band[0])
        collapsed_facts = text_pixels(page, collapsed, collapsed_facts_region)
        assert not collapsed_facts["rows"], "account facts were already expanded before the header click"
        canvas.click(position={"x":120,"y":sum(account_band)/2})
        page.mouse.move(2, 2)
        settle_gesture(page)
        expanded = canvas.screenshot(path=str(output / f"{kind}-facts-expanded.png"))
        expanded_table=storage_table_pixels(page,expanded,
            (4,tab[3]+52,page.viewport_size["width"]-24,page.viewport_size["height"]-30))
        expanded_prefix=text_pixels(page,expanded,(4,tab[3]+55,600,expanded_table["table_bounds"][1]))
        assert len(expanded_prefix["rows"])==5+9,"header click did not preserve the prefix and reveal all nine account facts"
        assert expanded_prefix["rows"][:4]==collapsed_prefix["rows"][:4],(collapsed_prefix,expanded_prefix)
        facts_region=(145,account_band[1]+1,650,expanded_prefix["rows"][-1][0])
        expanded_facts = text_pixels(page, expanded, facts_region)
        assert len(expanded_facts["rows"]) == 9, "header click did not reveal all nine account facts"
        prefix_region=(4,address_band[0]-1,600,account_band[1]+1)
        prefix = text_pixels(page, expanded, prefix_region)
        assert len(prefix["rows"])==4,"account address, touch default, ordered-state root and full facts heading were not painted"
        save_png_crop(page, expanded, prefix_region, output / f"{kind}-prefix-expanded.png")
        # The wheel starts over the facts prefix. One shared scroll must carry
        # it all the way through the slot list, rather than trap it in 80 points.
        wheel_y=sum(expanded_facts["rows"][0])/2
        page.mouse.move(80,wheel_y)
        page.mouse.wheel(0, 6000)
        settle_gesture(page)
        page.mouse.move(2, 2)
        settled = canvas.screenshot(path=str(output / f"{kind}-last-slots.png"))
        scrolled_prefix = text_pixels(page, settled, prefix_region)
        assert prefix["hash"] != scrolled_prefix["hash"], "wheel remained trapped inside facts; account prefix did not move"
        save_png_crop(page, settled, prefix_region, output / f"{kind}-prefix-scrolled.png")
        scrolled_table = storage_table_pixels(page, settled,
            (4,tab[3]+52,page.viewport_size["width"]-24,page.viewport_size["height"]-30))
        assert abs(scrolled_table["separator"]-table["separator"])<=1,"scrolling reset the Slot width"
        final_cells=scrolled_table["rows"][-1]
        last_row_region = (4,final_cells["band"][0]-3,
                           scrolled_table["separator"]+100,final_cells["band"][1]+3)
        last_row = text_pixels(page, settled, last_row_region)
        save_png_crop(page, settled, last_row_region, output / f"{kind}-last-row.png")
        assert len(last_row["rows"]) == 1 and last_row["white_pixels"] > 100, "last slot row was clipped or blank"
        for cell,expected_hash in [("slot",last_cell_hashes["slot"]),("value",last_cell_hashes[kind])]:
            actual=final_cells[cell]
            if cell == "value":
                assert actual, f"last rendered {title} Value is missing"
                last_value_comparison=assert_storage_literal_pixels(
                    actual, "{0x105f}" if kind == "storage" else "{0x205f}")
            else:
                assert actual and actual["mask_sha256"]==expected_hash,f"last rendered {title} {cell} changed: {storage_pixel_report(actual)}"
            left,top,right,bottom=actual["bounds"]
            save_png_crop(page,settled,(left-1,top-1,right+1,bottom+1),output/f"{kind}-last-{cell}.png")
        page.mouse.move(80,wheel_y)
        page.mouse.wheel(0, -6000)
        settle_gesture(page)
        page.mouse.move(2, 2)
        returned = canvas.screenshot(path=str(output / f"{kind}-facts-returned.png"))
        returned_prefix = text_pixels(page, returned, prefix_region)
        assert prefix["hash"] == returned_prefix["hash"], "returning to the top did not restore account facts"
        save_png_crop(page, returned, prefix_region, output / f"{kind}-prefix-returned.png")
        returned_facts = text_pixels(page, returned, facts_region)
        assert returned_facts["hash"] == expanded_facts["hash"], "returning to the top changed the expanded account facts"
        canvas.click(position={"x":120,"y":sum(account_band)/2})
        page.mouse.move(2, 2)
        settle_gesture(page)
        reclosed = canvas.screenshot(path=str(output / f"{kind}-facts-reclosed.png"))
        reclosed_prefix = text_pixels(page, reclosed, collapsed_prefix_region)
        assert reclosed_prefix["hash"] == collapsed_prefix["hash"], "closing account facts did not restore the complete collapsed prefix"
        canvas.click(position={"x":120,"y":sum(account_band)/2})
        page.mouse.move(2, 2)
        settle_gesture(page)
        reopened = canvas.screenshot(path=str(output / f"{kind}-facts-reopened.png"))
        reopened_facts = text_pixels(page, reopened, facts_region)
        assert reopened_facts["hash"] == expanded_facts["hash"], "reopening account facts changed the nine rendered facts"
        evidence[kind] = {"entries": count, "expected_last_slot": hex(count - 1),
            "expected_last_value": account[kind][-1]["value"]["constants"],
            "expected_last_cell_mask_sha256": {"slot":last_cell_hashes["slot"],"value":last_cell_hashes[kind]},
            "last_value_rgb_comparison":last_value_comparison,
            "facts_region": facts_region, "collapsed_facts": collapsed_facts,
            "collapsed_facts_region":collapsed_facts_region,
            "collapsed_prefix":collapsed_prefix,"expanded_prefix":expanded_prefix,
            "account_header_band":account_band,"touch_default_band":touch_band,"ordered_header_band":root_band,"ordered_state":ordered_roots[kind],
            "expanded_facts": expanded_facts, "last_row_region": last_row_region, "last_row": last_row,
            "default_table":default_table,"table":table,"scrolled_table":scrolled_table,"last_cells":final_cells,
            "prefix_region": prefix_region, "prefix": prefix,
            "scrolled_prefix": scrolled_prefix, "returned_prefix": returned_prefix,
            "returned_facts":returned_facts,"reclosed_prefix":reclosed_prefix,"reopened_facts":reopened_facts,
            "expanded_screenshot": f"{kind}-facts-expanded.png",
            "scrolled_screenshot": f"{kind}-last-slots.png"}
    assert len(posts) == 2, "storage inspection reran analysis"
    assert not errors, errors
    result = storage_pixel_report({"stores": evidence, "analysis_submissions": len(posts),
        "renderer": webgpu_evidence(page)})
    (output / "storage-scroll-report.json").write_text(json.dumps(result, indent=2) + "\n")
    page.close()
    return result


def run(context):
    return storage_scroll_interactions(context.browser, context.url, context.output)
