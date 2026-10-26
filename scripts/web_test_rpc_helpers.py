"""Rpc helpers browser acceptance helpers."""

from pathlib import Path
from web_test_rpc import RpcFixture
from tempfile import TemporaryDirectory
from contextlib import contextmanager
from playwright.sync_api import expect
import json
import re
import time
from web_test_harness import analysis_server, complete_submission, open_input, rendered_frame, settle_gesture
from web_test_hybrid import oracle_graph, source_program


RPC_PROVIDERS = [
    {"id": "pending-fixture", "name": "Pending acquisition"},
    {"id": "world-fixture", "name": "World snapshot"},
]

RPC_TEST_TOKEN = "rpc-value-shown-in-provider-selector"

USDC_ENTRY = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"

WETH_ENTRY = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"

USDT_ENTRY = "0xdAC17F958D2ee523a2206206994597C13D831ec7"

RPC_ENTRY_EXAMPLES = (
    ("USDC", USDC_ENTRY),
    ("WETH", WETH_ENTRY),
    ("USDT", USDT_ENTRY),
    ("Multicall3", "0xcA11bde05977b3631167028862bE2a173976CA11"),
    ("Uniswap V2 Router02", "0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D"),
    ("Uniswap V2 Pair (USDC/WETH)", "0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc"),
    ("Uniswap V2 Factory", "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f"),
    ("Uniswap V4 PoolManager", "0x000000000004444c5dc75cB358380D2e3dE08A90"),
)

CUSTOM_ENTRY = "0x3333333333333333333333333333333333333333"

RPC_ENTRY_ADDRESSES = {address.lower() for _, address in RPC_ENTRY_EXAMPLES} | {CUSTOM_ENTRY}

class EntryRpcFixture(RpcFixture):
    """Preset addresses execute tiny fixture code, never live mainnet contracts."""
    def _result(self, method, params):
        account_methods = ("eth_getCode", "eth_getBalance", "eth_getTransactionCount", "eth_getStorageAt")
        if method in account_methods and params[0].lower() in RPC_ENTRY_ADDRESSES:
            assert params[-1] == {"blockHash": self.block_hash, "requireCanonical": True}
            return {"eth_getCode": "0x00", "eth_getBalance": "0x100000", "eth_getTransactionCount": "0x1", "eth_getStorageAt": "0x" + "00" * 32}[method]
        return super()._result(method, params)

def automatic_rpc_entry(page, url: str, provider, output: Path, name: str):
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as initial:
        page.goto(url, wait_until="networkidle")
    reply = complete_submission(page, initial.value, namespace="Native")
    request = reply.request.post_data_json
    assert request["input"] == {"Rpc": {"provider_id": provider["id"], "address": USDC_ENTRY, "block": "Latest", "accounts": []}}, request
    assert reply.status == 200 and "Ok" in reply.json()["result"], reply.json()
    report = oracle_graph(reply.report, "Native")
    assert report["metadata"]["entry_address"].lower() == USDC_ENTRY.lower()
    assert len(report["cfg"]) == 1 and source_program(report, report["metadata"]["root_program"])["bytecode"] in ("00", "0x00"), "preset fixture unexpectedly analyzed live code"
    (output / f"{name}-automatic-usdc.json").write_text(json.dumps({"request": request, "entry_address": report["metadata"]["entry_address"], "status": report["status"]}, indent=2) + "\n")
    return request

def wait_for_job(page, url, job_id, predicate, timeout_seconds=15):
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        response = page.request.get(f"{url}/api/tasks/{job_id}")
        assert response.ok, response.text()
        snapshot = response.json()["result"]["Ok"]
        if predicate(snapshot):
            return snapshot
        page.wait_for_timeout(50)
    raise AssertionError(f"task did not reach the expected state: {snapshot}")

def visible_seconds(page, label):
    text = page.locator("#analysis-status").text_content()
    match = re.search(rf"(?:^|; ){re.escape(label)}: ([0-9.]+) (ms|s)(?:;|$)", text)
    assert match, f"missing {label} in accessible timing: {text}"
    return float(match[1]) / (1000 if match[2] == "ms" else 1)

def open_rpc_input(page, canvas, *, from_bytecode=False):
    open_input(page)
    if from_bytecode:
        canvas.click(position={"x": 465, "y": 386})
        settle_gesture(page)

def select_rpc_provider(page, canvas, index: int, output: Path, name: str):
    # Real packaged 1440x1000 RPC modal: regular controls retain the
    # original 20-point rows and 23-point stride.
    canvas.click(position={"x": 600, "y": 402})
    rendered_frame(page)
    canvas.screenshot(path=str(output / f"{name}-providers.png"))
    canvas.click(position={"x": 460, "y": 428 + index * 23})
    settle_gesture(page)

def submit_rpc(page, canvas, rpc, output: Path, name: str, provider_index: int, providers,
               *, configure_form=None):
    expect(page.locator("#analysis-status")).to_contain_text(
        f"RPC provider: {providers[0]['name']} ({providers[0]['endpoint']})")
    open_rpc_input(page, canvas)
    if provider_index:
        select_rpc_provider(page, canvas, provider_index, output, name)
    provider = providers[provider_index]
    expect(page.locator("#analysis-status")).to_contain_text(
        f"RPC provider: {provider['name']} ({provider['endpoint']})")
    canvas.click(position={"x": 600, "y": 448})
    page.keyboard.press("Control+A")
    page.keyboard.type(rpc.root_address, delay=3)
    rendered_frame(page)
    if configure_form is not None:
        configure_form()
    canvas.screenshot(path=str(output / f"{name}-input.png"))
    with page.expect_response(lambda response: response.url.endswith("/api/tasks") and response.request.method == "POST") as response:
        page.keyboard.press("Control+Enter")
    admitted = response.value
    request = admitted.request.post_data_json
    assert request["input"] == {"Rpc": {"provider_id": provider["id"], "address": rpc.root_address, "block": "Latest", "accounts": []}}, request
    assert rpc.endpoint not in admitted.request.post_data
    assert RPC_TEST_TOKEN not in admitted.request.post_data
    assert request["environment"]["number"] is None
    return admitted


def native_call_summary(report, state):
    """Borrow a closed relative Native body without giving it World PCs or SSA."""
    summary, = [item for item in report["abstract_summaries"]
                if item["kind"] == "KnownCall" and item["source_state"] == state["id"]]
    assert summary["source_current"] is True and summary["closed"] is True
    assert state["executed_pcs"] == []
    block, = [item for item in report["ssa"]["blocks"] if item["state"] == state["id"]]
    assert block["instructions"] == []
    snapshot, = [item for item in report["states"] if item["state"] == state["id"]]
    assert snapshot["entry"] == snapshot["exit"]
    procedures = summary["procedures"]
    assert procedures and 0 <= summary["root_procedure"] < len(procedures)
    active = snapshot["entry"]["frames"][-1]
    root = procedures[summary["root_procedure"]]
    assert (root["code_address"], root["code_hash"], root["kind"],
            root["storage_address"], root["is_static"]) == (
                active["code_address"], active["code_hash"], active["kind"],
                active["storage_address"], active["is_static"])
    nodes = summary["regional_cfg"]["nodes"]
    assert len(nodes) == summary["state_count"]
    for index, procedure in enumerate(procedures):
        assert procedure["semantics"] == "Native" and procedure["process"] is None
        assert procedure["process_input"] is None
        program, = [item for item in report["programs"]
                    if (item["code_address"], item["code_hash"], item["kind"]) ==
                    (procedure["code_address"], procedure["code_hash"], procedure["kind"])]
        assert program["fork"] == report["fork"]
        body = [node for node in nodes if node["procedure"] == index]
        assert set(procedure["covered_blocks"]) == {node["frame"]["basic_block"] for node in body}
        for node in body:
            frame, transfer = node["frame"], node["transfer"]
            assert frame["process"] is None and node["excluded"] is False
            assert type(frame["basic_block"]) is int
            assert (frame["code_address"], frame["code_hash"], frame["kind"],
                    frame["storage_address"], frame["is_static"]) == (
                        program["code_address"], program["code_hash"], program["kind"],
                        procedure["storage_address"], procedure["is_static"])
            assert any(block["id"] == frame["basic_block"] for block in program["blocks"])
            assert transfer["semantics"] == "Native" and transfer["process"] is None
            assert transfer["current"] is True and transfer["complete"] is True
            assert transfer["visited_block"] is True
            assert transfer["input_revision"] == node["input_revision"]
    return summary

@contextmanager
def configured_rpc_server(args):
    """Provide independent local providers without running another acceptance case."""
    with EntryRpcFixture() as pending, EntryRpcFixture() as rpc:
        with TemporaryDirectory(prefix="web-rpc-providers-") as directory:
            configuration = Path(directory) / "providers.json"
            providers = [
                {**provider, "endpoint": fixture.endpoint + "/rpc?token=" + RPC_TEST_TOKEN}
                for provider, fixture in zip(RPC_PROVIDERS, [pending, rpc], strict=True)
            ]
            configuration.write_text(json.dumps({"providers": providers}) + "\n")
            with analysis_server(args, configuration) as url:
                yield url, pending, rpc, providers
