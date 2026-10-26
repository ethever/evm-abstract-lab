"""Real staged HTTP results, GPU interaction and retained Global after cancellation."""

import hashlib
import json
from pathlib import Path
import re
import time
from playwright.sync_api import expect
from web_test_harness import click_control, edit_control, observe_webgpu, open_input, rendered_frame, select_report_namespace, settle_gesture, ui_control, webgpu_evidence
from web_test_graph_helpers import pick_may_point, workspace_route
from web_test_hybrid import process_control_relation, source_program


def process_model(report, require_hub=False):
    """Qualify actual wire receipts; the directory alone proves no admission."""
    directory = {row["id"]: row for row in report["process_controls"]}
    hubs = []
    regions = 0
    for summary in report["minimum"]["summaries"]:
        for node in summary["relation"]["regional_cfg"]["nodes"]:
            frame = node["frame"]["process"]
            transfer = node["transfer"]
            if frame is None:
                continue
            assert node["frame"]["basic_block"] is None
            assert frame["location"] in ("Any", "End", "JumpTargets") or set(frame["location"]) == {"Block"}
            if transfer is None:
                continue
            assert transfer["visited_block"] is False
            process = transfer["process"]
            if process is None or process["control"] is None:
                continue  # Real CALL/STOP boundaries can have no pure region.
            region = process["control"]
            source = directory[region["relation"]]
            assert source["source"] == frame["source"] and source["epoch"] == frame["epoch"]
            assert region["entry"] == frame["location"]
            regions += 1
            if "JumpTargets" in region["through"]:
                assert source["jump_targets"]
                hubs.append((node["id"], source["id"], source["jump_targets"]))
    if require_hub:
        assert hubs, "the actual selected Process model lacks its unknown-jump hub"
    return {"directory_rows": len(directory), "regions": regions, "hubs": hubs}


def select_may_source(page, canvas, report, output):
    """Keep the actual qualified MAY hub while navigating its decoded Code."""
    model = report["minimum"]
    assert len(model["summaries"]) == 1, "choose an explicit entry relation"
    relation = model["summaries"][0]["relation"]
    procedure = relation["root_procedure"]
    root = relation["procedures"][procedure]["process"]
    assert root is not None and root["source"] == model["declared_root_source"]
    candidates = []
    for node in relation["regional_cfg"]["nodes"]:
        if node["procedure"] != procedure or node["transfer"] is None:
            continue
        key = node["frame"]["process"]
        process = node["transfer"]["process"]
        if key is None or process is None or process["control"] is None:
            continue
        if (key["epoch"], key["owner"], key["source"], key["is_static"]) != (
                root["epoch"], root["owner"], root["source"], root["is_static"]):
            continue
        control = process["control"]
        if "JumpTargets" in control["through"]:
            candidates.append((node, process_control_relation(report, key, control)))
    assert candidates, "the actual root context has no retained unknown-jump through receipt"
    # Pick a particular retained receipt deterministically; do not join its
    # state facts with other origins sharing the same displayed source hub.
    node, certificate = min(candidates, key=lambda item: item[0]["id"])
    card = pick_may_point(page, canvas, report, summary=0, procedure=procedure,
                          region=node["id"], location="JumpTargets")
    workspace_route(page, "Code")
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Selected MAY source: unknown-jump hub")
    ui_control(page, "Copy code", role="action")
    program = source_program(report, certificate["program"])
    page.context.grant_permissions(["clipboard-read", "clipboard-write"])
    def copied(expected):
        deadline = time.monotonic() + 5
        while True:
            value = page.evaluate("navigator.clipboard.readText()")
            if expected(value):
                return value
            assert time.monotonic() < deadline, ("decoded source clipboard did not match", value)
            page.wait_for_timeout(25)
    # Two distinct real copy actions prevent an old clipboard value passing
    # the exact source-byte oracle. These buttons belong to the painted Code route.
    click_control(page, canvas, "Copy code", role="action")
    copied(lambda text: "CALLDATASIZE" in text)
    click_control(page, canvas, "Copy bytecode", role="action")
    copied(lambda text: text == program["bytecode"])
    canvas.screenshot(path=str(output / "may-source-navigation.png"))
    expect(status).to_contain_text("Selected MAY source: unknown-jump hub")
    assert "Selected σ" not in status.text_content(), status.text_content()
    return [card]


def detail_policy_form(page, canvas, reference_request, reference_report, output):
    """Edit the real D5 policy with the same source, environment and payer."""
    from web_test_harness import complete_submission
    from web_test_hybrid import assert_detail_policy, assert_root_source, source_instructions, source_program
    from web_test_layout_helpers import LAYOUT_DEFAULT_LIMITS, layout_detail_regions

    assert reference_request["limits"] == LAYOUT_DEFAULT_LIMITS
    assert reference_request["limits"]["max_detail_regions"] == 1
    previous = assert_detail_policy(reference_report, 1)
    status = page.locator("#analysis-status")
    results = []
    (output / "detail-policy-reference-request.json").write_text(json.dumps(reference_request, indent=2) + "\n")
    (output / "detail-policy-reference-report.json").write_text(json.dumps(reference_report, indent=2) + "\n")

    def policy_text(coverage, requested):
        return f"Detailed code sources: {len(coverage['detail_regions'])} selected · limit {requested}"

    previous_text = policy_text(previous, 1)
    reference_environment = dict(reference_report["minimum"]["environment"])
    scopes = {reference_environment.pop("input_scope")}
    for requested in [0, 2]:
        open_input(page, mode="Bytecode")
        layout_detail_regions(page, canvas, output, f"detail-policy-{requested}", requested)
        # The new draft cannot relabel the still-displayed report or its payer.
        expect(status).to_contain_text(previous_text)
        with page.expect_response(lambda response: response.url.endswith("/api/tasks")
                                  and response.request.method == "POST") as submitted:
            page.keyboard.press("Control+Enter")
        reply = complete_submission(page, submitted.value, expected_detail_regions=requested)
        (output / f"detail-policy-{requested}-request.json").write_text(reply.request.post_data + "\n")
        (output / f"detail-policy-{requested}-result.json").write_text(json.dumps(reply.json(), indent=2) + "\n")
        assert reply.admission_status == 202 and reply.status == 200, reply.json()
        expected = {**reference_request,
                    "limits": {**reference_request["limits"], "max_detail_regions": requested}}
        assert reply.request.post_data_json == expected, reply.request.post_data_json
        report = reply.report
        coverage = assert_detail_policy(report, requested)
        minimum = report["minimum"]
        assert minimum["current"] and minimum["status"] == "Converged"
        assert minimum["fork"] == reference_request["fork"]
        assert minimum["numeric_model"] == reference_request["limits"]["numeric_model"]
        environment = dict(minimum["environment"])
        scope = environment.pop("input_scope")
        # Each actual job owns a fresh symbolic-input namespace. Its physical
        # input bounds still agree exactly; this identity must never be reused.
        assert type(scope) is str and scope.isdecimal() and scope not in scopes
        scopes.add(scope)
        assert environment == reference_environment
        assert coverage["work"]["total"] <= reference_request["limits"]["max_work"]
        roots = []
        for graph in [report["global"], report["native"]]:
            if graph is None:
                continue
            metadata = graph["metadata"]
            assert metadata["limits"] == expected["limits"]
            assert_root_source(report, metadata)
            program = source_program(report, metadata["root_program"])
            assert program["bytecode"].removeprefix("0x") == reference_request["input"]["Bytecode"]["bytecode"]
            assert program["fork"] == reference_request["fork"]
            roots.append((metadata["declared_root_source"], program["id"]))
        if len(roots) == 2 and roots[0][0] != roots[1][0]:
            assert roots[0][1] != roots[1][1], "different qualified source aliases share a program ID"
        if report["global"] is not None:
            assert all(not block["executed_pcs"] for block in report["global"]["cfg"])
        controls = process_model(report, require_hub=True)
        previous_text = policy_text(coverage, requested)
        expect(status).to_contain_text(previous_text)
        expect(status).to_contain_text("View: Global model")
        (output / f"detail-policy-{requested}-status.txt").write_text(status.text_content() + "\n")
        # New reports select this program's actual entry source, rather than
        # carrying the previously clicked virtual hub or a Native state ID.
        expect(status).to_contain_text("Selected MAY source: block index 0")
        assert "Selected σ" not in status.text_content(), status.text_content()
        if requested == 0:
            expect(status).to_contain_text("detailed paths disabled")
            ui_control(page, "Detailed paths", role="tab", enabled=False)
            click_control(page, canvas, "Detailed paths", role="tab", enabled=False)
            expect(status).to_contain_text("View: Global model")
        else:
            assert len(coverage["detail_regions"]) == 1, "one captured source in this unchanged program"
            assert any(block["executed_pcs"] for block in report["native"]["cfg"])
            for block in report["native"]["cfg"]:
                if block["executed_pcs"]:
                    assert set(block["executed_pcs"]) <= {instruction["pc"] for instruction in source_instructions(report, block)}
            select_report_namespace(page, report, "Native")
            expect(status).to_contain_text("View: Detailed paths")
            select_report_namespace(page, report, "Baseline")
            expect(status).to_contain_text("View: Global model")
        rendered_frame(page)
        canvas.screenshot(path=str(output / f"detail-policy-{requested}-result.png"))
        results.append({"requested": requested, "selected": len(coverage["detail_regions"]),
                        "job": str(reply.job["id"]), "controls": controls,
                        "input_scope": scope, "work": coverage["work"], "status": status.text_content()})
    return results


def run(context, mode="cancel"):
    if mode=="refusal": return refusal(context)
    from tempfile import TemporaryDirectory
    from web_test_rpc import RpcFixture
    from web_test_harness import analysis_server
    class PrecisionRpc(RpcFixture):
        def _result(self,method,params):
            # Initial pinned root admission remains live. The next code request
            # is optional callee precision, after the real closed minimum.
            if method=="eth_getCode" and params[0].lower()==self.root_address:
                self.block_method="eth_getCode"
            return super()._result(method,params)
    with PrecisionRpc() as rpc, TemporaryDirectory(prefix="minimum-rpc-") as directory:
        config=Path(directory)/"providers.json"
        config.write_text(json.dumps({"providers":[{"id":"minimum-precision","name":"Minimum precision","endpoint":rpc.endpoint}]}))
        with analysis_server(context.args,config) as url:
            return cancellation(context,url,rpc)


def cancellation(context,url,rpc):
    from web_test_hybrid import assert_detail_policy
    page = context.browser.new_page(viewport={"width": 1440, "height": 1000})
    observe_webgpu(page)
    results, submissions = [], []
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))

    def fixed_stress_input(route):
        if route.request.method != "POST" or submissions:
            route.continue_()
            return
        # A real pinned provider blocks only optional callee precision. The
        # source/model, body transfer, renderer and cancellation remain real.
        request = route.request.post_data_json
        request["input"] = {"Rpc":{"provider_id":"minimum-precision","address":rpc.root_address,"block":"Latest","accounts":[]}}
        request["limits"]["max_work"] = 4_000_000_000
        request["limits"]["root_terminal_coverage"] = False
        submissions.append(request)
        route.continue_(post_data=json.dumps(request))

    def actual_result(response):
        if response.url.endswith("/result") and response.request.method == "GET" and response.ok:
            results.append(json.loads(response.text())["result"]["Ok"])

    page.route("**/api/tasks", fixed_stress_input)
    page.on("response", actual_result)
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as admitted:
        page.goto(url, wait_until="domcontentloaded")
    job = json.loads(admitted.value.text())["result"]["Ok"]
    job_id = job["id"]
    task_url = url + "/api/tasks/" + str(job_id)
    status = page.locator("#analysis-status")
    expect(status).to_contain_text("Publication: 1", timeout=60000)
    expect(status).to_contain_text("Running")
    expect(status).to_contain_text("View: Global model")
    assert len(submissions) == 1 and results
    assert rpc.started.wait(5),"optional precision never reached the real pending callee code request"
    first = results[0]
    assert str(first["id"]) == str(job_id)
    assert first["publication"]["model_closed"] is True
    assert first["publication"]["minimum_version"] is not None
    assert first["publication"]["global_version"] is None
    assert first["publication"]["native_version"] is None
    (context.output / "first-publication.json").write_text(json.dumps(first,indent=2)+"\n")
    report = first["report"]
    assert submissions[0]["limits"]["max_detail_regions"] == 1
    assert_detail_policy(report, 1)
    first_controls = process_model(report)
    assert report["selected_view"] == "Baseline" and report["minimum"] is not None and report["global"] is None and report["native"] is None
    assert report["hybrid"]["coverage"]["baseline_current"] is True
    assert report["minimum"]["environment"]["caller"].get("Symbolic")
    supplied = next(account for account in report["minimum"]["accounts"] if account["address"].lower() == rpc.root_address)
    root = next(program for program in report["programs"] if program["id"] == supplied["program"])
    assert root["bytecode"].removeprefix("0x") == rpc.root_code.removeprefix("0x")
    canvas = page.locator("#evm-canvas")
    views = {}
    for key in ["1", "3", "2"]:
        page.keyboard.press(key)
        rendered_frame(page)
        png = canvas.screenshot(path=str(context.output / f"running-view-{key}.png"))
        assert len(png) > 10000, "the real result view did not paint"
        views[key] = hashlib.sha256(png).hexdigest()
        current = page.request.get(task_url).json()["result"]["Ok"]
        assert current["state"] == "Running", "the stress control did not leave Native running for interaction"
    assert len(set(views.values())) == 3
    # A real Ctrl+wheel pinch changes the graph camera; cancel must retain it.
    cdp = page.context.new_cdp_session(page)
    page.mouse.move(720, 300)
    cdp.send("Input.dispatchMouseEvent", {"type": "mouseWheel", "x": 720, "y": 300, "deltaX": 0, "deltaY": -150, "modifiers": 2})
    settle_gesture(page)
    page.mouse.move(2, 2)
    rendered_frame(page)
    region = {"x": 10, "y": 180, "width": 1420, "height": 240}
    before = page.screenshot(path=str(context.output / "global-camera-before-cancel.png"), clip=region)
    assert len(before) > 1000
    # Use the actual canvas Cancel control and observe its real DELETE request.
    with page.expect_response(lambda response: response.request.method == "DELETE" and response.url == task_url) as cancellation:
        click_control(page, canvas, "Cancel", role="action")
    assert cancellation.value.ok
    expect(status).to_contain_text("Cancelled", timeout=30000)
    assert rpc.disconnected.wait(5),"cancellation did not close the actual optional RPC operation"
    expect(status).to_contain_text("Publication: 1")
    expect(status).to_contain_text("View: Global model")
    page.mouse.move(2, 2)
    settle_gesture(page)
    after = page.screenshot(path=str(context.output / "global-camera-after-cancel.png"), clip=region)
    assert before == after, "cancellation moved or repacked the retained Global scene"
    retained = page.request.get(task_url + "/result").json()["result"]["Ok"]
    assert retained["publication"]["minimum_version"] == first["publication"]["minimum_version"]
    assert retained["report"]["minimum"] == first["report"]["minimum"], "cancellation changed the retained qualified minimum"
    # A subsequent real form submission replaces the cancelled report; namespace
    # buttons must use the new report even when its numeric state IDs repeat.
    open_input(page, mode="Bytecode")
    # The original tiny unknown-jump/loop program exercises the selected D4 route.
    replacement_code = "36565b36565b00"
    edit_control(page, canvas, "Runtime bytecode", replacement_code)
    canvas.screenshot(path=str(context.output / "replacement-input-modal.png"))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as replacement:
        page.keyboard.press("Control+Enter")
    replacement_request=replacement.value.request.post_data_json
    assert replacement_request["input"]=={"Bytecode":{"bytecode":replacement_code,"address":None}},replacement_request
    replacement_id = json.loads(replacement.value.text())["result"]["Ok"]["id"]
    assert str(replacement_id) != str(job_id)
    expect(status).to_contain_text("Ready:", timeout=60000)
    expect(status).to_contain_text(re.compile(rf"(?:^|; )Job ID: {replacement_id}(?=;|$)"))
    assert "Publication: 2" in status.text_content()
    replacement_result = page.request.get(url + "/api/tasks/" + str(replacement_id) + "/result").json()["result"]["Ok"]
    assert_detail_policy(replacement_result["report"], 1)
    replacement_controls = process_model(replacement_result["report"], require_hub=True)
    # Through/admission was checked above against the actual Process receipt;
    # the scoped display reports its hub count without calling it Native expansion.
    expect(status).to_contain_text("; 1 unknown-jump hubs;")
    cards = select_may_source(page, canvas, replacement_result["report"], context.output)
    canvas.screenshot(path=str(context.output / "selected-may-hub-source.png"))
    page.keyboard.press("2")
    rendered_frame(page)
    select_report_namespace(page, replacement_result["report"], "Native")
    expect(status).to_contain_text("View: Detailed paths")
    select_report_namespace(page, replacement_result["report"], "Baseline")
    expect(status).to_contain_text("View: Global model")
    policies = detail_policy_form(page, canvas, replacement_request, replacement_result["report"], context.output)
    assert not errors, errors
    renderer = webgpu_evidence(page)
    (context.output / "request.json").write_text(json.dumps(submissions[0], indent=2) + "\n")
    (context.output / "publications.json").write_text(json.dumps(results, indent=2) + "\n")
    result = {"pending_optional_rpc_closed":rpc.disconnected.is_set(),"requests":rpc.requests,"first_job": str(job_id), "replacement_job": str(replacement_id), "first_publication": first["publication"], "renderer": renderer, "running_views": views, "camera_sha256": hashlib.sha256(before).hexdigest(), "first_controls": first_controls, "replacement_controls": replacement_controls, "painted_cards": cards, "detail_policies": policies, "status": status.text_content()}
    page.close()
    return result


def refusal(context):
    from web_test_harness import analysis_server
    from web_test_hybrid import assert_detail_policy
    from web_test_inspector_helpers import inspector_selected_tab, inspector_title_action, select_inspector_tab
    def page_for(url,request=None):
        page=context.browser.new_page(viewport={"width":1440,"height":1000})
        observe_webgpu(page)
        requests,results=[],[]
        def submit(route):
            if route.request.method!="POST": route.continue_();return
            body=request or route.request.post_data_json
            if request is None: body["input"]={"Bytecode":{"bytecode":"600160020100","address":None}}
            requests.append(body)
            route.continue_(post_data=json.dumps(body))
        def received(response):
            if response.ok and response.request.method=="GET" and response.url.endswith("/result"):
                results.append(json.loads(response.text())["result"]["Ok"])
        page.route("**/api/tasks",submit);page.on("response",received)
        page.goto(url,wait_until="domcontentloaded")
        expect(page.locator("#analysis-status")).to_contain_text("Ready:",timeout=60000)
        assert results and len(requests)==1
        return page,requests[0],results[-1]
    baseline,request,complete=page_for(context.url)
    assert request["limits"]["max_detail_regions"] == 1
    assert_detail_policy(complete["report"], 1)
    assert complete["report"]["rich_export"]=="Complete"
    minimum_work=complete["report"]["minimum"]["export_usage"]["work"]
    full_work=complete["report"]["postprocessing_work"]
    cap=(minimum_work+full_work)//2
    assert minimum_work<cap<full_work,(minimum_work,cap,full_work)
    baseline.close()
    with analysis_server(context.args,postprocess_work=cap) as url:
        page,replayed,actual=page_for(url,request)
        assert replayed==request,"the refusal control changed execution input/policy"
        report=actual["report"];minimum=report["minimum"]
        assert_detail_policy(report, 1)
        controls = process_model(report)
        assert actual["publication"]["model_closed"] and minimum["current"] and minimum["status"]=="Converged"
        assert actual["publication"]["minimum_version"] is not None
        assert actual["publication"]["global_version"] is None and actual["publication"]["native_version"] is None
        assert report["global"] is None and report["native"] is None
        assert report["rich_export"]["Refused"]["code"]=="Export"
        assert minimum["postprocessing_max_work"]==cap and report["postprocessing_work"]<=cap
        status=page.locator("#analysis-status")
        expect(status).to_contain_text("presentation refused")
        expect(status).to_contain_text("View: Global model")
        canvas=page.locator("#evm-canvas")
        views={}
        for key in ["1","3","2"]:
            page.keyboard.press(key);rendered_frame(page)
            png=canvas.screenshot(path=str(context.output/f"refused-view-{key}.png"))
            assert len(png)>10000
            views[key]=hashlib.sha256(png).hexdigest()
        assert len(set(views.values()))==3
        select_inspector_tab(page, canvas, "Frame", minimum=True)
        controls = inspector_selected_tab(page, canvas, "Frame", minimum=True)
        inspector_title_action(page,canvas,"Maximize",controls)
        rendered_frame(page)
        canvas.screenshot(path=str(context.output/"refused-minimum-inputs.png"))
        renderer=webgpu_evidence(page)
        (context.output/"refusal-request.json").write_text(json.dumps(request,indent=2)+"\n")
        (context.output/"refusal-result.json").write_text(json.dumps(actual,indent=2)+"\n")
        value={"minimum_work":minimum_work,"cap":cap,"full_work":full_work,"spent":report["postprocessing_work"],"publication":actual["publication"],"renderer":renderer,"views":views,"controls":controls,"status":status.text_content()}
        page.close();return value
