"""Compact ssa browser acceptance cases."""

from pathlib import Path
import json
from web_test_example_helpers import example_page
from web_test_graph_helpers import assert_availability, compare_tab, compare_tab_control, default_entry_cfg, source_content_snapshot, workspace_route
from web_test_harness import measured_backend_timings, resize_view, select_report_namespace, set_control, webgpu_evidence
from web_test_hybrid import native_entry_context, oracle_graph, source_program


def compact_ssa_interactions(browser, url: str, output: Path):
    page,canvas,reply=example_page(browser,url,namespace=None,close_directory=False)
    report=reply.report
    entry=default_entry_cfg(page,canvas,report,output,"compact-default-entry-cfg")
    requests=[]
    page.on("request",lambda request:requests.append(request.url)
            if request.method=="POST" and request.url.endswith("/api/tasks") else None)
    results={"initial_entry":entry}
    workspace_route(page,"SSA")
    results["may_ssa_unavailable"]=assert_availability(page,"MAY control has no Native instruction or SSA receipt")
    workspace_route(page,"CFG")
    select_report_namespace(page,report,"Native")
    measured_backend_timings(report)
    set_control(page,canvas,"Inspect",False)
    root=native_entry_context(report)
    program=source_program(report,root["program"])
    native=oracle_graph(report,"Native")
    block=next(block for block in native["ssa"]["blocks"] if block["state"]==root["id"])
    for route,ssa in [("Code",None),("SSA",block)]:
        workspace_route(page,route)
        source_content_snapshot(page,canvas,program,output,f"compact-native-{route.lower()}",ssa=ssa)
    for width,name in [(1024,"medium"),(640,"narrow")]:
        resize_view(page,width,800);workspace_route(page,"Compare")
        def source_hash(label,*,ssa=False):
            return source_content_snapshot(page,canvas,program,output,f"workspace-{name}-{label}",
                                           ssa=block if ssa else None)["hash"]
        compare_tab(page,canvas,"SSA")
        explicit_ssa=source_hash("explicit-ssa",ssa=True)
        compare_tab(page,canvas,"Disassembly")
        disasm=source_hash("disassembly")
        assert disasm!=explicit_ssa, f"{name} Disassembly tab did not show different source content"
        compare_tab(page,canvas,"SSA")
        assert source_hash("ssa-return",ssa=True)==explicit_ssa, f"{name} SSA tab did not restore its source content"
        compare_tab(page,canvas,"Disassembly")
        assert source_hash("disassembly-return")==disasm, f"{name} Disassembly tab did not restore source content"
        workspace_route(page,"CFG");workspace_route(page,"Compare")
        compare_tab_control(page,"Disassembly",selected=True)
        assert source_hash("view-return-disassembly")==disasm, f"{name} tab choice was lost across views"
        resize_view(page,1440,1000);resize_view(page,width,800)
        compare_tab_control(page,"Disassembly",selected=True)
        assert source_hash("resize-return-disassembly")==disasm, f"{name} tab choice was lost across width classes"
        results[name]={"ssa_hash":explicit_ssa,"disassembly_hash":disasm,
                       "preserves_manual_tab_across_views_and_widths":True}
    # Leave different choices in the two width classes, then observe each
    # preference without clicking the expected tab and masking a lost choice.
    compare_tab(page,canvas,"SSA")
    resize_view(page,1024,800)
    compare_tab_control(page,"Disassembly",selected=True)
    returned=source_content_snapshot(page,canvas,program,output,"workspace-medium-after-narrow")
    assert returned["hash"]==results["medium"]["disassembly_hash"], "narrow workspace overwrote the medium tab preference"
    resize_view(page,640,800)
    compare_tab_control(page,"SSA",selected=True)
    narrow=source_content_snapshot(page,canvas,program,output,"workspace-narrow-after-medium",ssa=block)
    assert narrow["hash"]==results["narrow"]["ssa_hash"], "medium workspace overwrote the narrow tab preference"
    assert not requests,"compact source tabs or workspace resizing reran analysis"
    results["renderer"]=webgpu_evidence(page)
    (output/"compact-ssa-report.json").write_text(json.dumps(results,indent=2)+"\n")
    page.close()
    return results


def run(context):
    return compact_ssa_interactions(context.browser, context.url, context.output)
