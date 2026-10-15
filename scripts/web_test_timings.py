"""Timings browser acceptance cases."""

from pathlib import Path
from tempfile import TemporaryDirectory
from playwright.sync_api import expect
import json
import re
import time
from web_test_harness import analysis_server, measured_backend_timings, observe_webgpu, open_input, rendered_frame, settle_gesture, submit_bytecode, webgpu_evidence
from web_test_inspector_helpers import inspector_selected_tab, inspector_title_action
from web_test_rpc_helpers import EntryRpcFixture, automatic_rpc_entry, submit_rpc, visible_seconds, wait_for_job


def timing_interactions(browser, args):
    """Measure real queued work, delayed result transport and terminal freezing."""
    with EntryRpcFixture() as rpc, TemporaryDirectory(prefix="web-timings-") as directory:
        providers = [{"id": "timing-fixture", "name": "Timing fixture", "endpoint": rpc.endpoint}]
        configuration = Path(directory) / "providers.json"
        configuration.write_text(json.dumps({"providers": providers}) + "\n")
        with analysis_server(args, configuration, workers=2) as url:
            page = browser.new_page(viewport={"width": 1440, "height": 1000})
            observe_webgpu(page)
            baseline = automatic_rpc_entry(page, url, providers[0], args.output, "timing-startup")
            canvas = page.locator("#evm-canvas")
            status = page.locator("#analysis-status")
            # Two real pending RPC reads occupy both workers. The visible task
            # must queue, rather than estimating queue time from a delayed poll.
            rpc.block_method = "eth_getCode"
            request = json.loads(json.dumps(baseline))
            request["input"]["Rpc"]["address"] = rpc.root_address
            occupied = []
            for _ in range(2):
                response = page.request.post(url + "/api/tasks", data=request)
                assert response.status == 202, response.text()
                occupied.append(response.json()["result"]["Ok"]["id"])
            for job_id in occupied:
                wait_for_job(page, url, job_id,
                             lambda job: job["progress"]["phase"] == "Acquiring")
            admitted = submit_rpc(page, canvas, rpc, args.output, "timing-queued", 0, providers)
            job_id = json.loads(admitted.text())["result"]["Ok"]["id"]
            queued = wait_for_job(page, url, job_id, lambda job: job["state"] == "Queued")
            expect(status).to_contain_text("Queued")
            before = visible_seconds(page, "Elapsed")
            page.wait_for_timeout(400)
            after = visible_seconds(page, "Elapsed")
            assert after > before, "queued user wait stopped advancing"
            queued_later = wait_for_job(page, url, job_id, lambda job: job["state"] == "Queued")
            assert queued_later["timings"]["queue_ns"] > queued["timings"]["queue_ns"]
            assert queued_later["timings"]["processing_ns"] == 0
            canvas.screenshot(path=str(args.output / "timing-queued.png"))
            page.set_viewport_size({"width": 390, "height": 844})
            rendered_frame(page)
            assert visible_seconds(page, "Elapsed") >= after
            canvas.screenshot(path=str(args.output / "timing-queued-narrow.png"))
            page.set_viewport_size({"width": 1440, "height": 1000})
            rendered_frame(page)
            # Acknowledged cancellation frees one actual worker. The displayed
            # job then reaches its own pending RPC, with a frozen queue interval.
            assert page.request.delete(f"{url}/api/tasks/{occupied[0]}").ok
            wait_for_job(page, url, occupied[0], lambda job: job["state"] == "Cancelled")
            running = wait_for_job(page, url, job_id,
                                   lambda job: job["progress"]["phase"] == "Acquiring")
            expect(status).to_contain_text("Running")
            page.wait_for_timeout(200)
            running_later = wait_for_job(page, url, job_id,
                                         lambda job: job["progress"]["phase"] == "Acquiring")
            assert running_later["timings"]["queue_ns"] == running["timings"]["queue_ns"]
            assert running_later["timings"]["acquiring_ns"] > running["timings"]["acquiring_ns"]
            # Make CFG the current view. Completion must include its first real
            # layout, while later focus/zoom actions must not restart the timer.
            page.keyboard.press("2")
            rendered_frame(page)
            held_results = []
            page.route("**/api/tasks/*/result", lambda route: held_results.append(route))
            rpc.release.set()
            completed = wait_for_job(page, url, job_id, lambda job: job["state"] == "Completed")
            deadline = time.monotonic() + 15
            while not held_results and time.monotonic() < deadline:
                page.wait_for_timeout(50)
            assert len(held_results) == 1, "browser did not fetch exactly one result"
            expect(status).to_contain_text("Loading result")
            fetch_before = visible_seconds(page, "Elapsed")
            page.wait_for_timeout(400)
            fetch_after = visible_seconds(page, "Elapsed")
            assert fetch_after > fetch_before, "user wait stopped before result transport finished"
            frozen_backend = wait_for_job(page, url, job_id, lambda job: job["state"] == "Completed")
            assert frozen_backend["timings"] == completed["timings"]
            held_results[0].continue_()
            expect(status).to_contain_text("Ready:", timeout=60000)
            expect(status).to_contain_text("Frontend preparation:")
            report = page.request.get(f"{url}/api/tasks/{job_id}/result").json()["result"]["Ok"]
            backend = measured_backend_timings(report)
            assert backend == completed["timings"]
            assert backend["queue_ns"] > 0 and backend["acquiring_ns"] > 0 and backend["pinning_ns"] > 0
            assert report["acquisition"]["rounds"] > 1, "fixture did not exercise RPC analysis reruns"
            total = visible_seconds(page, "Total wait")
            preparation = visible_seconds(page, "Frontend preparation")
            assert total >= fetch_after, "completed user wait omitted the delayed result"
            assert preparation >= 0.3, "frontend preparation omitted the held transport"
            assert visible_seconds(page, "Result transfer") >= 0.3
            assert visible_seconds(page, "JSON parsing") >= 0
            assert visible_seconds(page, "First view setup") >= 0
            page.wait_for_timeout(300)
            page.keyboard.press("f")
            rendered_frame(page)
            assert visible_seconds(page, "Total wait") == total, "completed user wait kept advancing"
            assert visible_seconds(page, "Frontend preparation") == preparation
            canvas.click(position={"x": 75, "y": 978})
            expect(status).to_contain_text("Inspector: Timing")
            inspector_title_action(page, canvas, "Maximize", inspector_selected_tab(page, canvas, "Timing"))
            expect(status).to_contain_text(re.compile("maximized", re.IGNORECASE))
            canvas.screenshot(path=str(args.output / "timing-completed.png"))
            page.unroute_all(behavior="wait")
            rpc.release.clear()
            rpc.started.clear()
            rpc.disconnected.clear()
            cancelled = submit_rpc(page, canvas, rpc, args.output, "timing-cancel", 0, providers)
            cancelled_id = json.loads(cancelled.text())["result"]["Ok"]["id"]
            assert rpc.started.wait(5), "cancelled task never entered its own real RPC read"
            expect(status).to_contain_text("Elapsed:")
            expect(status).to_contain_text(
                re.compile(rf"(?:^|; )Current Job ID: {cancelled_id}(?=;|$)"))
            expect(status).to_contain_text(
                re.compile(rf"(?:^|; )Result Job ID: {job_id}(?=;|$)"))
            with page.expect_response(lambda response: response.request.method == "DELETE"
                                      and response.url.endswith(f"/api/tasks/{cancelled_id}")):
                canvas.click(position={"x": 30, "y": 988})
            expect(status).to_contain_text("Cancelled", timeout=22000)
            cancellation_wait = visible_seconds(page, "Total wait")
            cancelled_backend = wait_for_job(page, url, cancelled_id, lambda job: job["state"] == "Cancelled")
            page.wait_for_timeout(300)
            assert visible_seconds(page, "Total wait") == cancellation_wait
            assert wait_for_job(page, url, cancelled_id, lambda job: job["state"] == "Cancelled")["timings"] == cancelled_backend["timings"]
            assert rpc.disconnected.wait(5), "cancelled acquisition did not close its connection"
            canvas.screenshot(path=str(args.output / "timing-cancelled.png"))
            # A typed worker failure freezes the new task, while the successful
            # report still retains its original backend timing evidence.
            open_input(page)
            canvas.click(position={"x": 380, "y": 361})  # Bytecode in the taller RPC modal
            page.keyboard.press("Escape")
            settle_gesture(page)
            failed = submit_bytecode(page, canvas, "not-bytecode", args.output / "timing-error-input.png")
            assert failed.json()["result"]["Err"]["code"] == "InvalidBytecode"
            failure_wait = visible_seconds(page, "Total wait")
            page.wait_for_timeout(300)
            assert visible_seconds(page, "Total wait") == failure_wait
            frozen_report = page.request.get(f"{url}/api/tasks/{job_id}/result").json()["result"]["Ok"]
            assert frozen_report["metadata"]["timings"] == backend
            canvas.screenshot(path=str(args.output / "timing-failed.png"))
            result = {"queue_samples": [queued["timings"], queued_later["timings"]],
                      "running_samples": [running["timings"], running_later["timings"]],
                      "completed": backend, "rpc_rounds": report["acquisition"]["rounds"],
                      "user_total_s": total, "frontend_preparation_s": preparation,
                      "cancellation_wait_s": cancellation_wait, "cancelled_backend": cancelled_backend["timings"],
                      "failure_wait_s": failure_wait, "renderer": webgpu_evidence(page)}
            (args.output / "timing-acceptance-report.json").write_text(json.dumps(result, indent=2) + "\n")
            page.close()
            return result


def run(context):
    return timing_interactions(context.browser, context.args)
