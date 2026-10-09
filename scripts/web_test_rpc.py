"""Deterministic ordinary JSON-RPC fixture for the browser world workflow.

Use ``with RpcFixture() as rpc:`` before starting the native analysis server.
Put rpc.endpoint in that server's provider configuration, then submit its
provider ID and rpc.root_address from the browser. No endpoint is sent by the UI.
The root reads an acquired slot, writes persistent and transient state, calls a
second account with one byte of calldata, receives 32 bytes, then returns them.
A separate ``RpcFixture(block_method="eth_getCode")`` leaves native acquisition
pending until cancellation closes the connection or ``release.set()`` is called.
Every listener and request thread is joined on context exit.
"""
from __future__ import annotations

import json
import select
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


class RpcFixture:
    root_address = "0x" + "11" * 20
    child_address = "0x" + "22" * 20
    block_hash = "0x" + "44" * 32
    parent_hash = "0x" + "33" * 32
    chain_id = "0x1"
    block_number = "0x2a"
    timestamp = "0x1234"
    miner = "0x" + "55" * 20
    prevrandao = "0x" + "66" * 32
    # Root PC 0x32 is CALL; resumed PC 0x34 is RETURNDATASIZE.
    root_code = (
        "0x60045450602a600155600860055d60ab5f53"
        "6020602060015f5f73" + child_address[2:] + "61fffff1503d5060206020f3"
    )
    child_code = "0x5f355f52600760025d606360035560205ff3"
    expected = {
        "root_call_pc": 0x32,
        "root_return_size_pc": 0x34,
        "root_persistent": {"0x1": "0x2a", "0x4": "0x9"},
        "root_transient": {"0x5": "0x8"},
        "child_persistent": {"0x3": "0x63"},
        "child_transient": {"0x2": "0x7"},
        "child_calldata": "0xab",
        # Success-path witness, not a claim that CALL cannot fail. Abstract
        # caller continuations may join zero-length and 32-byte returndata.
        "possible_return_data": "0xab" + "00" * 31,
        "root_memory_nonzero_offsets": [0, 32],
        "minimum_programs": 2,
        "maximum_frame_depth": 2,
    }

    def __init__(self, block_method: str | None = None) -> None:
        self.block_method = block_method
        self.requests: list[dict[str, Any]] = []
        self.started = threading.Event()
        self.release = threading.Event()
        self.disconnected = threading.Event()
        self._lock = threading.Lock()
        self._server: ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None
        self.endpoint = ""

    def __enter__(self) -> RpcFixture:
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, _format: str, *_args: object) -> None:
                pass

            def do_POST(self) -> None:
                self.connection.settimeout(3)
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 262144:
                    self.send_error(413)
                    return
                request = json.loads(self.rfile.read(length))
                with fixture._lock:
                    fixture.requests.append(request)
                if request.get("method") == fixture.block_method:
                    fixture.started.set()
                    while not fixture.release.wait(0.02):
                        readable, _, _ = select.select([self.connection], [], [], 0)
                        if readable:
                            try:
                                if not self.connection.recv(1, socket.MSG_PEEK):
                                    fixture.disconnected.set()
                                    self.close_connection = True
                                    return
                            except (ConnectionResetError, OSError):
                                fixture.disconnected.set()
                                return
                try:
                    result = fixture._result(request["method"], request.get("params", []))
                    envelope = {"jsonrpc": "2.0", "id": request["id"], "result": result}
                except (KeyError, ValueError, AssertionError) as error:
                    envelope = {
                        "jsonrpc": "2.0",
                        "id": request.get("id"),
                        "error": {"code": -32602, "message": str(error)},
                    }
                body = json.dumps(envelope, separators=(",", ":")).encode()
                try:
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    fixture.disconnected.set()
                self.close_connection = True

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = False
        self.endpoint = "http://{}:{}".format(*self._server.server_address)
        self._thread = threading.Thread(target=self._server.serve_forever, name="web-rpc-fixture")
        self._thread.start()
        return self

    def __exit__(self, *_error: object) -> None:
        self.release.set()
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
        if self._thread is not None:
            self._thread.join(timeout=5)
            if self._thread.is_alive():
                raise RuntimeError("RPC fixture listener did not terminate")

    def _result(self, method: str, params: list[Any]) -> object:
        if method == "eth_chainId":
            return self.chain_id
        if method in ("eth_getBlockByNumber", "eth_getBlockByHash"):
            assert params[1] is False, "fixture only supports header requests"
            if method == "eth_getBlockByHash":
                assert params[0].lower() == self.block_hash
            else:
                assert params[0] == "latest" or int(params[0], 16) == int(self.block_number, 16)
            return {
                "hash": self.block_hash,
                "parentHash": self.parent_hash,
                "number": self.block_number,
                "timestamp": self.timestamp,
                "miner": self.miner,
                "mixHash": self.prevrandao,
                "gasLimit": "0x1c9c380",
                "baseFeePerGas": "0x7",
                # Omitted blob accounting is explicitly unknown, not zero.
            }
        if method in ("eth_getCode", "eth_getBalance", "eth_getTransactionCount", "eth_getStorageAt"):
            selector = params[-1]
            assert selector == {"blockHash": self.block_hash, "requireCanonical": True}, selector
            address = params[0].lower()
            assert address in (self.root_address, self.child_address), address
            if method == "eth_getCode":
                return self.root_code if address == self.root_address else self.child_code
            if method == "eth_getBalance":
                return "0x100000"
            if method == "eth_getTransactionCount":
                return "0x1"
            slot = int(params[1], 16)
            value = 9 if address == self.root_address and slot == 4 else 0
            return f"0x{value:064x}"
        raise ValueError(f"Unexpected RPC fixture method: {method}")
