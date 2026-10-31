"""Real shared-scheduler queue cancellation, retained artifacts and wire guards."""

from copy import deepcopy
import json
from playwright.sync_api import expect
from web_test_decompile_helpers import (BRANCH, contains_ast, open_decompiler,
                                       prepare, stage, submit, wait_job)
from web_test_harness import analysis_server, click_control, ui_control, webgpu_evidence
from web_test_rpc_helpers import EntryRpcFixture, RPC_PROVIDERS


def run(context):
    with EntryRpcFixture() as rpc:
        configuration = context.output / "providers.json"
        configuration.write_text(json.dumps({"providers": [{**RPC_PROVIDERS[0], "endpoint": rpc.endpoint}]}) + "\n")
        with analysis_server(context.args, configuration, workers=1) as url:
            page, canvas, startup, _ = open_decompiler(context, url)
            baseline, _ = submit(context, page, canvas, BRANCH, "retained-baseline")
            status = page.locator("#analysis-status")
            rpc.block_method = "eth_getCode"
            occupying = deepcopy(startup)
            occupying["input"]["Rpc"]["address"] = rpc.root_address
            active = page.request.post(url + "/api/tasks", data=occupying)
            assert active.status == 202, active.text()
            active_id = active.json()["result"]["Ok"]["id"]
            assert rpc.started.wait(5), "the real shared worker was not occupied"
            prepare(page, canvas, "600160020100")
            with page.expect_response(lambda response: response.request.method == "POST"
                                      and response.url.endswith("/api/decompile/tasks")) as admitted:
                click_control(page, canvas, "Decompile", role="action")
            queued = admitted.value.json()["result"]["Ok"]
            assert queued["state"] == "Queued", queued
            expect(status).to_contain_text("Decompile task: Queued")
            expect(status).to_contain_text("Artifact: " + baseline["id"])
            canvas.screenshot(path=str(context.output / "queued-retained-artifact.png"))
            with page.expect_response(lambda response: response.request.method == "DELETE"
                                      and "/api/decompile/tasks/" in response.url) as cancelled:
                click_control(page, canvas, "Cancel decompile", role="action")
            assert cancelled.value.ok
            acknowledged = wait_job(page, url, queued["id"], lambda job: job["state"] == "Cancelled")
            expect(status).to_contain_text("Decompile task: Cancelled")
            expect(status).to_contain_text("Artifact: " + baseline["id"])
            assert acknowledged["publication"] is None
            canvas.screenshot(path=str(context.output / "cancelled-retained-artifact.png"))
            retained_selection = status.text_content().split("; Selected entity: ")[1].split(";")[0]
            missing = page.request.get(url + "/api/decompile/tasks/0")
            assert missing.status == 404 and missing.json()["result"]["Err"]["code"] == "TaskNotFound"
            missing_body = missing.text()

            def disappeared(route):
                if route.request.method == "GET":
                    route.fulfill(status=404, content_type="application/json", body=missing_body)
                else:
                    route.continue_()

            page.route("**/api/decompile/tasks/*", disappeared)
            prepare(page, canvas, "600560060100")
            with page.expect_response(lambda response: response.request.method == "POST"
                                      and response.url.endswith("/api/decompile/tasks")) as missing_admission:
                click_control(page, canvas, "Decompile", role="action")
            missing_id = missing_admission.value.json()["result"]["Ok"]["id"]
            expect(status).to_contain_text("Decompile task: Task unavailable")
            expect(status).to_contain_text("TaskNotFound")
            expect(status).to_contain_text("Artifact: " + baseline["id"])
            expect(status).to_contain_text("Selected entity: " + retained_selection)
            click_control(page, canvas, "Edit bytecode", role="action")
            ui_control(page, "Decompile", role="action", enabled=True)
            canvas.screenshot(path=str(context.output / "missing-task-unlocks-retained-workspace.png"))
            page.unroute("**/api/decompile/tasks/*", disappeared)
            assert page.request.delete(url + "/api/decompile/tasks/" + str(missing_id)).ok
            stopped = page.request.delete(url + "/api/tasks/" + str(active_id))
            assert stopped.ok
            assert rpc.disconnected.wait(5), "cancelling the occupying task did not release RPC"
            rpc.block_method = None
            rpc.release.set()

            def mismatched_identity(route):
                response = route.fetch()
                envelope = response.json()
                envelope["result"]["Ok"]["id"] = active_id
                headers = {key: value for key, value in response.headers.items()
                           if key.lower() not in {"content-encoding", "content-length"}}
                route.fulfill(status=response.status, headers=headers, body=json.dumps(envelope))

            page.route("**/api/decompile/tasks/*/result", mismatched_identity)
            prepare(page, canvas, "600360040100")
            with page.expect_response(lambda response: response.request.method == "GET"
                                      and "/api/decompile/tasks/" in response.url and response.url.endswith("/result")):
                click_control(page, canvas, "Decompile", role="action")
            expect(status).to_contain_text("Task response identity mismatch")
            expect(status).to_contain_text("Artifact: " + baseline["id"])
            canvas.screenshot(path=str(context.output / "rejected-result-keeps-artifact.png"))
            page.unroute_all(behavior="wait")
            unknown, _ = submit(context, page, canvas, "5f3556", "unresolved-local-jump")
            assert unknown["completion"] == "partial", unknown["diagnostics"]
            assert any(diagnostic["code"] == "unknown_jump" for diagnostic in unknown["diagnostics"])
            current = next(item for item in unknown["stages"] if item["id"] == unknown["current_stage"])
            assert current["ir"]["fact_scope"] == "recovered_paths"
            assert contains_ast(stage(unknown, "structured")["ir"]["structured"], "partial_exit") or contains_ast(current["ir"]["structured"], "indirect")
            expect(status).to_contain_text("Completion: Partial")
            expect(status).to_contain_text("Fact scope: RecoveredPaths")
            click_control(page, canvas, "Graphs", role="tab")
            canvas.screenshot(path=str(context.output / "unresolved-current-cfg.png"))
            result = {"queued": queued, "cancelled": acknowledged,
                      "missing_task": missing_id,
                      "retained_artifact": baseline["id"], "recovered_artifact": unknown["id"],
                      "renderer": webgpu_evidence(page)}
            page.close()
            return result
