"""Address acquisition stays code-only and qualifies cache reuse by real content."""

from contextlib import ExitStack
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.parse import urlencode, urlsplit

from playwright.sync_api import expect
from web_test_decompile_helpers import (assert_decompiler_isolated, bytecode_bytes,
                                       decompiler_page, open_decompiler, origin)
from web_test_decompile_links import open_cached, opened_result
from web_test_harness import (analysis_server, click_control, consume_http_error, edit_control,
                              expect_http_error, ui_control, webgpu_evidence)
from web_test_rpc import RpcFixture


FIRST_PROVIDER = "z-chain-one"
SECOND_PROVIDER = "a-chain-two"
ALLOWED_METHODS = {"eth_chainId", "eth_getBlockByNumber", "eth_getBlockByHash", "eth_getCode"}


def address_page(context, base, rpc, query, provider, *, cache_hit):
    page = decompiler_page(context)
    url = origin(base) + "/decompiler?" + urlencode({"address": rpc.root_address, **query})
    artifact, source, evidence = opened_result(page, lambda: page.goto(url, wait_until="networkidle"), cache_hit=cache_hit)
    expected = {"provider_id": provider, "address": rpc.root_address.lower(),
                "chain_id": str(int(rpc.chain_id, 16)), "block_hash": rpc.block_hash,
                "block_number": str(int(rpc.block_number, 16))}
    assert source["origin"] == expected, source
    assert artifact["input"]["runtime_bytecode"] == list(bytecode_bytes(rpc.root_code))
    status = page.locator("#analysis-status")
    for field, value in [("address", expected["address"]), ("provider", provider),
                         ("chain", expected["chain_id"]), ("block", expected["block_number"])]:
        expect(status).to_contain_text("Source " + field + ": " + value)
    expect(status).to_contain_text(expected["block_hash"])
    assert rpc.endpoint not in page.url and "route-secret" not in page.url
    assert rpc.endpoint not in json.dumps(evidence) and "route-secret" not in json.dumps(evidence)
    assert_decompiler_isolated(page)
    return page, artifact, source, evidence


def acquired_code(requests, rpc):
    assert requests and all(item["method"] in ALLOWED_METHODS for item in requests), requests
    code = [item for item in requests if item["method"] == "eth_getCode"]
    assert code, requests
    for item in code:
        assert item["params"] == [rpc.root_address, {"blockHash": rpc.block_hash, "requireCanonical": True}], item
    return requests


def failed_address(context, base, rpc, query, *, code, status):
    page = decompiler_page(context)
    endpoint = origin(base) + "/api/decompile/open"
    expected = expect_http_error(page, method="POST", url=endpoint, status=status)
    with page.expect_response(lambda response: response.request.method == "POST"
                              and urlsplit(response.url).path == "/api/decompile/open") as received:
        page.goto(origin(base) + "/decompiler?" + urlencode({"address": rpc.root_address, **query}),
                  wait_until="networkidle")
    error = received.value.json()["result"]["Err"]
    assert received.value.status == status and error["code"] == code, error
    expect(page.locator("#analysis-status")).to_contain_text("Workspace: Decompiler")
    expect(page.locator("#analysis-status")).to_contain_text(error["message"])
    assert_decompiler_isolated(page)
    observed = {"error": error, "http_error": consume_http_error(page, expected)}
    assert rpc.endpoint not in json.dumps(observed) and "route-secret" not in json.dumps(observed)
    page.close()
    return observed


def run(context):
    evidence = {}
    with ExitStack() as stack:
        first = stack.enter_context(RpcFixture())
        second = stack.enter_context(RpcFixture())
        first.root_code = second.root_code = "0x600160005500"
        second.chain_id = "0x2"
        second.block_hash = "0x" + "66" * 32
        directory = Path(stack.enter_context(TemporaryDirectory(prefix="decompiler-address-browser-")))
        configuration = directory / "providers.json"
        configuration.write_text(json.dumps({"providers": [
            {"id": FIRST_PROVIDER, "name": "Chain one", "endpoint": first.endpoint + "?token=route-secret-one"},
            {"id": SECOND_PROVIDER, "name": "Chain two", "endpoint": second.endpoint + "?token=route-secret-two"},
        ]}) + "\n")
        with analysis_server(context.args, configuration, decompile_cache_dir=directory / "cache") as base:
            page, artifact, source, evidence["default_provider"] = address_page(
                context, base, first, {}, FIRST_PROVIDER, cache_hit=False,
            )
            evidence["first_rpc"] = acquired_code(first.requests[:], first)
            evidence["renderer"] = webgpu_evidence(page)
            page.locator("#evm-canvas").screenshot(path=str(context.output / "address-source-provenance.png"))
            before = (len(first.requests), len(second.requests))
            restored, hash_source, evidence["hash_reload"] = opened_result(
                page, lambda: page.reload(wait_until="networkidle"), cache_hit=True,
            )
            assert restored == artifact and hash_source["origin"] is None
            assert before == (len(first.requests), len(second.requests)), "a content link must not reacquire an address"
            page.close()

            # Identical code on another chain shares the artifact but reports
            # this acquisition's chain and provider, never the first owner's.
            page, same, second_source, evidence["other_chain_same_code"] = address_page(
                context, base, second, {"provider": SECOND_PROVIDER, "chain": "2", "block": "latest"},
                SECOND_PROVIDER, cache_hit=True,
            )
            assert same == artifact and second_source["sha256"] == source["sha256"]
            evidence["second_rpc"] = acquired_code(second.requests[:], second)
            page.close()

            # The independent address form uses the same source-open contract.
            page, canvas, _, _ = open_decompiler(context, base)
            panel = ui_control(page, "Decompiler input", role="decompile-input")
            click_control(page, canvas, "Contract address", role="tab", within=panel)
            click_control(page, canvas, "RPC provider", role="combobox")
            ui_control(page, "Chain one", role="tab", selected=True)
            click_control(page, canvas, "Chain two", role="tab")
            edit_control(page, canvas, "Contract address", second.root_address)
            edit_control(page, canvas, "Source block", "latest")
            edit_control(page, canvas, "Expected chain ID", "2")
            manual, manual_source, evidence["address_form"] = opened_result(
                page, lambda: click_control(page, canvas, "Decompile", role="action"), cache_hit=True,
            )
            assert manual == artifact and manual_source["origin"]["provider_id"] == SECOND_PROVIDER
            assert manual_source["origin"]["chain_id"] == "2"
            assert_decompiler_isolated(page)
            page.close()

            # Latest moves to different code at the same address. Its new hash
            # misses, while the old immutable content URL remains restorable.
            second.parent_hash = second.block_hash
            second.block_hash = "0x" + "77" * 32
            second.block_number = "0x2b"
            second.root_code = "0x600260005500"
            start = len(second.requests)
            page, changed, changed_source, evidence["latest_changed_code"] = address_page(
                context, base, second, {"provider": SECOND_PROVIDER, "chain": "2"},
                SECOND_PROVIDER, cache_hit=False,
            )
            assert changed_source["sha256"] != source["sha256"]
            evidence["changed_rpc"] = acquired_code(second.requests[start:], second)
            page.close()
            before = (len(first.requests), len(second.requests))
            old, restored, _, evidence["old_content_after_latest_change"] = open_cached(
                context, base, source, expected_artifact=artifact,
            )
            assert restored != changed
            assert before == (len(first.requests), len(second.requests))
            old.close()

            page, pinned, pinned_source, evidence["explicit_block_hash"] = address_page(
                context, base, second, {"provider": SECOND_PROVIDER, "chain": "2", "block": second.block_hash},
                SECOND_PROVIDER, cache_hit=True,
            )
            assert pinned == changed and pinned_source["sha256"] == changed_source["sha256"]
            page.close()
            page, pinned, _, evidence["explicit_block_number"] = address_page(
                context, base, first, {"provider": FIRST_PROVIDER, "chain": "1", "block": "42"},
                FIRST_PROVIDER, cache_hit=True,
            )
            assert pinned == artifact
            page.close()

            evidence["chain_mismatch"] = failed_address(
                context, base, second, {"provider": SECOND_PROVIDER, "chain": "1"}, code="Rpc", status=502,
            )
            second.parent_hash = second.block_hash
            second.block_hash = "0x" + "88" * 32
            second.block_number = "0x2c"
            second.root_code = "0x"
            evidence["empty_account"] = failed_address(
                context, base, second, {"provider": SECOND_PROVIDER, "chain": "2"},
                code="InvalidBytecode", status=400,
            )
            assert all(item["method"] in ALLOWED_METHODS for item in first.requests + second.requests)

    (context.output / "address-cache-evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    return evidence
