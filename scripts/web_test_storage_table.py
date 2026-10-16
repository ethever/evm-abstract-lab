"""Storage table browser acceptance cases."""

from pathlib import Path
import base64
from playwright.sync_api import expect
import json
from web_test_harness import observe_webgpu, rendered_frame, settle_gesture, submit_bytecode, webgpu_evidence
from web_test_inspector_helpers import inspector_frame_tab, inspector_title_action, select_inspector_tab
from web_test_storage_helpers import STORAGE_SLOT_WORD, assert_storage_cell_text, assert_storage_literal_pixels, assert_storage_preference, assert_storage_reviewed_pixels, assert_storage_suffix, drag_storage_separator, storage_glyph_cells, storage_hover_snapshot, storage_pixel_report, storage_reviewed_raster, storage_scrolled_snapshot, storage_separator_offset, storage_table_snapshot


def storage_table_interactions(browser, url: str, output: Path):
    """Real WASM full defaults, suffix resizing, hovers and retained preferences."""
    page=browser.new_page(viewport={"width":1440,"height":1000})
    observe_webgpu(page)
    errors,posts=[],[]
    page.on("pageerror",lambda error:errors.append(str(error)))
    page.on("request",lambda request:posts.append(request.url)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    page.goto(url,wait_until="networkidle")
    status=page.locator("#analysis-status")
    expect(status).to_contain_text("Ready:",timeout=60000)
    canvas=page.locator("#evm-canvas")
    word=STORAGE_SLOT_WORD
    assert len(word)==66
    slots=[0,0xf,0x1234,int(word,16)]

    def fixture(revision, long_slots=True, literal_word=word):
        expected={kind:{} for kind in ["storage","transient"]}
        instructions=[]
        fixture_slots=[*slots[:3],int(literal_word,16)] if long_slots else slots[:3]
        for index,slot in enumerate(fixture_slots):
            key=f"7f{slot:064x}" if slot>0xffff else f"61{slot:04x}"
            for kind,opcode,base in [("storage","55",0x3100),("transient","5d",0x4200)]:
                value=int(("cd" if kind=="storage" else "ef")*32,16) if index==3 else base+revision*16+index
                if index==1:
                    value=0xf
                elif index==2 and long_slots:
                    value=int(literal_word,16)
                expected[kind][hex(slot)]=hex(value)
                push=f"7f{value:064x}" if value>0xffff else f"61{value:04x}"
                instructions.append(push+key+opcode)
        return "".join(instructions)+"00",expected

    def submit_fixture(revision, long_slots=True, literal_word=word):
        code,expected=fixture(revision,long_slots,literal_word)
        reply=submit_bytecode(page,canvas,code,output/f"storage-table-input-{revision}.png")
        assert reply.status==200,reply.json()
        report=reply.json()["result"]["Ok"]
        assert report["status"]=="Converged" and len(report["states"])==1
        machine=report["states"][0]["exit"]
        account=next(account for account in report["stores"][machine["store"]]["accounts"]
                     if account["address"]==machine["frames"][0]["storage_address"])
        for kind in expected:
            assert {entry["slot"]:entry["value"]["constants"] for entry in account[kind]}=={
                slot:[value] for slot,value in expected[kind].items()},account[kind]
            for entry in account[kind]:
                literal=expected[kind][entry["slot"]]
                number=int(literal,16)
                assert entry["value"]=={
                    "summary":"{"+literal+"}","constants":[literal],
                    "known_zero":hex(((1<<256)-1)^number),"known_one":literal,
                    "nonzero":number!=0,"unsigned":{"lower":literal,"upper":literal},
                    "signed":{"lower":literal,"upper":literal},"congruence":{"Exact":literal},
                    "origins":["Constant"],"code_address_role":False,"identity":None,
                    "expression":None,"symbolic_limit":False},entry
        (output/f"storage-table-analysis-{revision}.json").write_text(json.dumps(report,indent=2)+"\n")
        return expected

    submit_fixture(-1,long_slots=False)
    canvas.click(position={"x":720,"y":600})
    page.keyboard.press("3")
    tab=inspector_frame_tab(page,canvas)
    page.mouse.move(720,tab[1]-3)
    page.mouse.down()
    page.mouse.move(720,350,steps=16)
    page.mouse.up()
    tab=inspector_frame_tab(page,canvas)
    toolbar_y=sum(tab[1::2])/2
    results={}
    preferences={}
    short_defaults={}
    for title,kind in [("Storage","storage"),("Transient","transient")]:
        select_inspector_tab(page,canvas,title,toolbar_y)
        canvas.click(position={"x":145,"y":tab[3]+14})
        expect(status).to_contain_text("Observed exit")
        short_defaults[kind]=storage_table_snapshot(page,canvas,output,
            f"storage-table-{kind}-short-default",tab[3],3)
    # These tables were actually rendered before report replacement and have
    # never been resized. Their default must now accommodate the new long key.
    expected=submit_fixture(0)
    for title,kind in [("Storage","storage"),("Transient","transient")]:
        select_inspector_tab(page,canvas,title,toolbar_y)
        canvas.click(position={"x":145,"y":tab[3]+14})
        expect(status).to_contain_text("Observed exit")
        initial=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-initial",tab[3],len(slots))
        for slot,row in zip(slots[:3],initial["rows"][:3],strict=True):
            assert_storage_literal_pixels(row["slot"],hex(slot))
            assert len(storage_glyph_cells(row["slot"]))==len(hex(slot)),f"short Slot {hex(slot)} was padded or clipped"
        full=initial["rows"][-1]["slot"]
        full_comparison=assert_storage_literal_pixels(full,word)
        full_value_comparison=assert_storage_literal_pixels(initial["rows"][2]["value"],"{"+word+"}")
        assert full["bounds"][2]-full["bounds"][0]>500,storage_pixel_report(full)
        full_hover=storage_hover_snapshot(page,canvas,output,f"storage-table-{kind}-full-slot-hover",initial["rows"][-1]["slot"])
        # This varied literal fits one proportional-font tooltip line. Require
        # its complete reviewed raster, including the original 0x prefix.
        assert len(full_hover["text"]["rows"])==1 and full_hover["ink"]["mask_size"]==[465,10],storage_pixel_report(full_hover)
        full_hover_comparison=assert_storage_reviewed_pixels(full_hover["raster"],"slot_hover",word,[476,23],
            "c24455921e771f31c4dff3e171ab62b12271f5c29b07f653f4492e6b0092488a")
        if kind=="transient":
            assert initial["rows"][0]["value"]["mask_sha256"]!=results["storage"]["initial"]["rows"][0]["value"]["mask_sha256"], "Transient rendered the persistent value"
        drag_storage_separator(page,initial,initial["separator"]+320)
        wide=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-wide",tab[3],len(slots))
        assert wide["separator"]-initial["separator"]>=280, storage_pixel_report((initial,wide))
        assert wide["rows"][-1]["slot"]["bounds"][2]-wide["rows"][-1]["slot"]["bounds"][0]>300,storage_pixel_report(wide)
        advance=(full["bounds"][2]-full["bounds"][0]
                 -(initial["rows"][1]["slot"]["bounds"][2]-initial["rows"][1]["slot"]["bounds"][0]))/(len(word)-3)
        current=wide
        suffixes=[]
        for retained,fraction in [(6,0.25),(13,0.625),(8 if kind=="storage" else 11,0.375)]:
            # Include both 2pt cell margins and half the 5pt column gap.
            # Aim inside the one-glyph fitting interval so divider raster
            # rounding cannot push a fractional drag below its intended width.
            target=current["header"]["slot"]["bounds"][0]+(retained+1.5)*advance+4+2.5+fraction
            drag_storage_separator(page,current,target)
            narrow=storage_table_snapshot(page,canvas,output,
                f"storage-table-{kind}-suffix-{retained}",tab[3],len(slots))
            assert abs(narrow["separator"]-target)<=3,(target,storage_pixel_report(narrow))
            suffix=assert_storage_suffix(narrow["rows"][-1]["slot"],word,retained)
            suffix.update(target_separator=target,table=narrow)
            suffixes.append(suffix)
            for first,last in zip(initial["rows"][:3],narrow["rows"][:3],strict=True):
                assert_storage_cell_text(first["slot"],last["slot"],"un-padded short Slot")
            current=narrow
        assert narrow["separator"]<initial["separator"]-20,storage_pixel_report((initial,narrow))
        assert_storage_cell_text(initial["rows"][-1]["slot"],wide["rows"][-1]["slot"],"complete widened Slot")
        value_comparisons=[assert_storage_cell_text(first["value"],last["value"])
            for first,last in zip(initial["rows"],narrow["rows"],strict=True)]
        preferences[kind]=storage_separator_offset(narrow)
        slot_hover=storage_hover_snapshot(page,canvas,output,f"storage-table-{kind}-slot-hover",narrow["rows"][-1]["slot"])
        # Full original text remains identical to the untruncated initial
        # key's hover, including every glyph of the original single line.
        assert len(slot_hover["text"]["rows"])==1 and slot_hover["ink"]["mask_sha256"]==full_hover["ink"]["mask_sha256"],storage_pixel_report(slot_hover)
        page.mouse.move(2,2)
        settle_gesture(page)
        value_hover=storage_hover_snapshot(page,canvas,output,f"storage-table-{kind}-value-hover",narrow["rows"][-1]["value"])
        # Retain the eight original numeric/provenance rows, plus the new
        # proved-nonzero row. Its wire fact was checked against the full U256.
        nonzero_rows=int(int(expected[kind][hex(slots[-1])],16)!=0)
        assert len(value_hover["text"]["rows"])==8+nonzero_rows and value_hover["text"]["muted_pixels"]>100,storage_pixel_report(value_hover)
        select_inspector_tab(page,canvas,"Frame",toolbar_y)
        frame=inspector_frame_tab(page,canvas)
        page.mouse.move(720,frame[1]-3)
        page.mouse.down()
        page.mouse.move(720,680,steps=16)
        page.mouse.up()
        small_tab=inspector_frame_tab(page,canvas)
        small_toolbar_y=sum(small_tab[1::2])/2
        select_inspector_tab(page,canvas,title,small_toolbar_y)
        canvas.click(position={"x":120,"y":small_tab[3]+80})
        # The indented symbolic tree has its own scroll area. Target the outer
        # inspector's left margin so it receives the complete wheel delta.
        page.mouse.move(8,small_tab[3]+115)
        page.mouse.wheel(0,600)
        settle_gesture(page)
        # Keep the pointer in the outer scroll area while pending wheel frames
        # bring the complete table into view.
        scrolled=storage_scrolled_snapshot(page,output/f"storage-table-{kind}-scrolled.png",
            (4,small_tab[3]+52,1416,970),narrow)
        page.mouse.move(2,2)
        assert_storage_preference(scrolled,preferences[kind])
        scrolled_value=assert_storage_cell_text(narrow["rows"][-1]["value"],scrolled["rows"][-1]["value"])
        page.mouse.move(8,small_tab[3]+115)
        page.mouse.wheel(0,-600)
        settle_gesture(page)
        canvas.click(position={"x":120,"y":small_tab[3]+80})
        select_inspector_tab(page,canvas,"Frame",small_toolbar_y)
        frame=inspector_frame_tab(page,canvas)
        page.mouse.move(720,frame[1]-3)
        page.mouse.down()
        page.mouse.move(720,350,steps=16)
        page.mouse.up()
        tab=inspector_frame_tab(page,canvas)
        toolbar_y=sum(tab[1::2])/2
        select_inspector_tab(page,canvas,title,toolbar_y)
        scroll_returned=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-scroll-return",tab[3],len(slots))
        assert_storage_preference(scroll_returned,preferences[kind])
        results[kind]={"expected":expected[kind],"short_default":short_defaults[kind],
                       "full_slot_raster":full,"full_slot_comparison":full_comparison,"full_value_comparison":full_value_comparison,
                       "full_slot_hover":full_hover,"full_hover_comparison":full_hover_comparison,
                       "suffixes":suffixes,"initial":initial,"wide":wide,"narrow":narrow,
                       "slot_hover":slot_hover,"value_hover":value_hover,"scrolled":scrolled,"scroll_return":scroll_returned}
        results[kind].update(value_comparisons=value_comparisons,scrolled_value=scrolled_value)

    # Returning from another inspector tab, workspace views, hidden/maximized
    # inspector and a new report must preserve each independently dragged width.
    for title,kind in [("Storage","storage"),("Transient","transient")]:
        select_inspector_tab(page,canvas,"Memory",toolbar_y)
        select_inspector_tab(page,canvas,title,toolbar_y)
        returned=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-tab-return",tab[3],len(slots))
        assert_storage_preference(returned,preferences[kind])
        for key in ["0","1","2","3"]:
            page.keyboard.press(key)
            rendered_frame(page)
        viewed=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-view-return",tab[3],len(slots))
        assert_storage_preference(viewed,preferences[kind])
        page.keyboard.press("Control+j")
        expect(status).to_contain_text("Inspector hidden")
        page.keyboard.press("Control+j")
        expect(status).to_contain_text(f"Inspector: {title}")
        hidden=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-hidden-return",tab[3],len(slots))
        assert_storage_preference(hidden,preferences[kind])
        select_inspector_tab(page,canvas,"Frame",toolbar_y)
        frame=inspector_frame_tab(page,canvas)
        inspector_title_action(page,canvas,"Maximize",frame)
        maximized_tab=inspector_frame_tab(page,canvas)
        select_inspector_tab(page,canvas,title,sum(maximized_tab[1::2])/2)
        maximized=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-maximized",maximized_tab[3],len(slots))
        assert_storage_preference(maximized,preferences[kind])
        select_inspector_tab(page,canvas,"Frame",sum(maximized_tab[1::2])/2)
        inspector_title_action(page,canvas,"Restore",inspector_frame_tab(page,canvas))
        tab=inspector_frame_tab(page,canvas)
        toolbar_y=sum(tab[1::2])/2
        select_inspector_tab(page,canvas,title,toolbar_y)
        restored=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-restored",tab[3],len(slots))
        assert_storage_preference(restored,preferences[kind])
        results[kind].update(tab_return=returned,view_return=viewed,hidden_return=hidden,
                             maximized=maximized,restored=restored)
    assert len(posts)==3,"table inspection unexpectedly reran analysis"
    expected_new=submit_fixture(1)
    select_inspector_tab(page,canvas,"Frame",toolbar_y)
    tab=inspector_frame_tab(page,canvas)
    for title,kind in [("Storage","storage"),("Transient","transient")]:
        select_inspector_tab(page,canvas,title,toolbar_y)
        canvas.click(position={"x":145,"y":tab[3]+14})
        expect(status).to_contain_text("Observed exit")
        updated=storage_table_snapshot(page,canvas,output,f"storage-table-{kind}-new-report",tab[3],len(slots))
        assert_storage_preference(updated,preferences[kind])
        assert updated["rows"][0]["value"]["mask_sha256"]!=results[kind]["narrow"]["rows"][0]["value"]["mask_sha256"], "new report retained stale values"
        results[kind].update(new_report=updated,expected_new=expected_new[kind])
    assert len(posts)==4,"expected startup, short fixture, long fixture and one controlled new report"
    # Submit a real incorrect last digit, after the complete original matrix.
    # Its Value has the same dimensions; the RGB oracle must reject its tail.
    wrong_word=word[:-1]+"e"
    assert wrong_word[:-1]==word[:-1] and wrong_word[-1]!=word[-1]
    submit_fixture(2,literal_word=wrong_word)
    select_inspector_tab(page,canvas,"Storage",toolbar_y)
    canvas.click(position={"x":145,"y":tab[3]+14})
    expect(status).to_contain_text("Observed exit")
    wrong=storage_table_snapshot(page,canvas,output,"storage-table-wrong-last-glyph",tab[3],len(slots))
    wrong_value=wrong["rows"][2]["value"]
    reference=storage_reviewed_raster("full_value","{"+word+"}",[531,12],
        "441542c8fa375969f42f890a0240ae01222ff506ba5da7a71cfbee95695caa2e")
    assert wrong_value["mask_size"]==reference["mask_size"],storage_pixel_report(wrong_value)
    first,last=base64.b64decode(reference["rgb_raster"]),base64.b64decode(wrong_value["rgb_raster"])
    differences=[abs(a-b) for a,b in zip(first,last,strict=True)]
    changed=[(i//3)%531 for i,difference in enumerate(differences) if difference>2]
    assert changed and min(changed)>=531-30,"negative fixture changed more than its final glyph"
    try:
        assert_storage_literal_pixels(wrong_value,"{"+word+"}")
    except AssertionError as error:
        assert "maximum RGB difference" in str(error),str(error)
        negative={"actual_text":"{"+wrong_word+"}","expected_text":"{"+word+"}",
                  "mask_size":wrong_value["mask_size"],"maximum_rgb_difference":max(differences),
                  "changed_tail_columns":[min(changed),max(changed)],"rejection":str(error)}
    else:
        raise AssertionError("reviewed complete Value accepted an incorrect last glyph")
    assert len(posts)==5,"expected exactly one additional real negative analysis"
    assert not errors,errors
    result=storage_pixel_report({"stores":results,"wrong_last_glyph":negative,
        "analysis_submissions":len(posts),"renderer":webgpu_evidence(page)})
    (output/"storage-table-report.json").write_text(json.dumps(result,indent=2)+"\n")
    page.close()
    return result


def run(context):
    return storage_table_interactions(context.browser, context.url, context.output)
