"""Rpc url browser acceptance cases."""

from playwright.sync_api import expect
import json
from web_test_harness import analysis_server, complete_submission, observe_webgpu, webgpu_evidence
from web_test_rpc_helpers import EntryRpcFixture, RPC_TEST_TOKEN, automatic_rpc_entry, submit_rpc


def url_configured_world_interactions(browser, args):
    with EntryRpcFixture() as rpc:
        endpoint = rpc.endpoint + "/rpc?token=" + RPC_TEST_TOKEN
        providers = [{"id": "default", "name": "Default RPC", "endpoint": endpoint}]
        with analysis_server(args, endpoint) as url:
            assert not rpc.requests, "loading a URL unexpectedly contacted the RPC"
            page = browser.new_page(viewport={"width": 1440, "height": 1000})
            catalogue = page.request.get(url + "/api/rpc-providers")
            assert catalogue.ok, catalogue.text()
            assert catalogue.json() == {"result": {"Ok": providers}}
            observe_webgpu(page)
            automatic_rpc_entry(page, url, providers[0], args.output, "url-startup")
            expect(page.locator("#analysis-status")).to_contain_text("Ready:", timeout=60000)
            admitted = submit_rpc(page, page.locator("#evm-canvas"), rpc, args.output,
                                  "url-default", provider_index=0, providers=providers)
            reply = complete_submission(page, admitted)
            envelope = reply.json()
            assert reply.status == 200 and "Ok" in envelope["result"], envelope
            full_report = envelope["result"]["Ok"]
            assert full_report["hybrid"]["coverage"]["baseline_current"]
            report = full_report["hybrid"]["detail"]
            assert {program["code_address"] for program in report["programs"]} == {rpc.root_address, rpc.child_address}
            assert report["metadata"]["snapshot"]["block_hash"] == rpc.block_hash
            assert full_report["acquisition"]["requests"] > 0 and rpc.requests
            assert RPC_TEST_TOKEN not in json.dumps(report)
            assert endpoint not in json.dumps(report)
            (args.output / "url-default-task-reply.json").write_text(json.dumps(envelope, indent=2) + "\n")
            renderer = webgpu_evidence(page)
            page.close()
            return {"providers": providers, "selected_provider_id": "default",
                    "rpc_requests": len(rpc.requests), "endpoint_displayed_in_selector": True,
                    "submission_uses_provider_id": True,
                    "status": report["status"], "renderer": renderer}


def run(context):
    return url_configured_world_interactions(context.browser, context.args)
