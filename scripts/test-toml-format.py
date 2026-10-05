#!/usr/bin/env python3
"""Check that editor formatting and repeated saves agree with the Taplo CLI."""

from datetime import datetime, timezone
import fnmatch
import hashlib
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import tomllib


class LanguageServer:
    def __init__(self, root):
        self.messages = queue.Queue()
        self.next_id = 0
        self.configuration_replies = 0
        self.errors = tempfile.TemporaryFile()
        self.process = subprocess.Popen(
            ["taplo", "lsp", "stdio"], cwd=root,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.errors,
        )
        threading.Thread(target=self.read_messages, daemon=True).start()

    def read_messages(self):
        try:
            while True:
                headers = {}
                while line := self.process.stdout.readline():
                    if line == b"\r\n":
                        break
                    key, value = line.decode().split(":", 1)
                    headers[key.lower()] = value.strip()
                if not line:
                    raise EOFError("Taplo language server closed stdout")
                self.messages.put(json.loads(
                    self.process.stdout.read(int(headers["content-length"]))
                ))
        except Exception as error:
            self.messages.put(error)

    def send(self, message):
        body = json.dumps({"jsonrpc": "2.0", **message}).encode()
        self.process.stdin.write(
            f"Content-Length: {len(body)}\r\n\r\n".encode() + body
        )
        self.process.stdin.flush()

    def notify(self, method, params):
        self.send({"method": method, "params": params})

    def request(self, method, params):
        self.next_id += 1
        request_id = self.next_id
        self.send({"id": request_id, "method": method, "params": params})
        while True:
            message = self.messages.get(timeout=15)
            if isinstance(message, Exception):
                raise message
            if "method" in message:
                if "id" not in message:
                    continue
                result = None
                if message["method"] == "workspace/configuration":
                    self.configuration_replies += 1
                    # Deliberately conflict with project formatting rules.
                    configuration = {
                        "taplo": {"configFile": {"enabled": True, "path": None}},
                        "schema": {"enabled": False, "catalogs": []},
                        "formatter": {
                            "alignEntries": False,
                            "allowedBlankLines": 7,
                            "indentString": "\t",
                        },
                    }
                    result = [configuration for _ in message["params"]["items"]]
                self.send({"id": message["id"], "result": result})
            elif message.get("id") == request_id:
                if "error" in message:
                    raise RuntimeError(message["error"])
                return message["result"]

    def format(self, uri, text, options):
        edits = self.request("textDocument/formatting", {
            "textDocument": {"uri": uri}, "options": options,
        })
        assert isinstance(edits, list) and len(edits) == 1, (
            f"Expected a Taplo formatting edit: {edits}"
        )
        assert edits[0]["range"] == {
            "start": {"line": 0, "character": 0},
            "end": {
                "line": text.count("\n"),
                "character": len(text.rsplit("\n", 1)[-1].encode("utf-16-le")) // 2,
            },
        }, f"Expected a full-document Taplo edit: {edits}"
        return edits[0]["newText"]

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
        self.errors.seek(0)
        log = self.errors.read().decode()
        self.errors.close()
        self.process.stdin.close()
        self.process.stdout.close()
        return log


def main():
    root = Path.cwd().resolve()
    config = tomllib.loads((root / "taplo.toml").read_text())
    files = sorted({
        path for pattern in config.get("include", ["**/*.toml"])
        for path in root.glob(pattern) if path.is_file()
        and not any(
            fnmatch.fnmatchcase("/" + path.relative_to(root).as_posix(), exclude)
            for exclude in config.get("exclude", [])
        )
    })
    assert files, "No TOML files found"
    scenarios = [
        {"tabSize": 2, "insertSpaces": True},
        {"tabSize": 4, "insertSpaces": True},
        {"tabSize": 8, "insertSpaces": True},
        {"tabSize": 4, "insertSpaces": False},
    ]
    with tempfile.TemporaryDirectory(prefix="taplo-editor-format-") as temporary:
        cache = Path(temporary)
        # Taplo initializes catalogs before requesting editor configuration.
        # Seed its pinned default catalog with no schemas to keep this test offline.
        url = "https://www.schemastore.org/api/json/catalog.json"
        (cache / hashlib.sha1(url.encode()).hexdigest()).write_text(json.dumps({
            "url": url, "value": {"schemas": []},
            "expires_by": [datetime.now(timezone.utc).year + 1, 1, 0, 0, 0, 0, 0, 0, 0],
        }))
        server = LanguageServer(root)
        try:
            server.request("initialize", {
                "processId": None,
                "rootUri": root.as_uri(),
                "workspaceFolders": [{"uri": root.as_uri(), "name": root.name}],
                "capabilities": {"workspace": {
                    "configuration": True, "workspaceFolders": True,
                }},
                "initializationOptions": {
                    "configurationSection": "evenBetterToml", "cachePath": str(cache),
                },
            })
            server.notify("initialized", {})
            documents = [(path, path.read_text()) for path in files]
            probe = (
                '[package]\nname="format-probe"\nversion="1.0.0"\n\n\n\n'
                '[workspace]\nmembers=[\n'
                '"crates/first-long-member-for-formatting",\n'
                '"crates/second-long-member-for-formatting",\n]\n'
            )
            # An unsaved buffer also catches a formatter that merely returns its input.
            documents.append((root / ".cargo/editor-format-probe.toml", probe))
            for path, text in documents:
                uri = path.as_uri()
                canonical = subprocess.check_output([
                    "taplo", "fmt", "--stdin-filepath", str(path), "-",
                ], input=text, text=True, stderr=subprocess.PIPE)
                if text == probe:
                    assert canonical != text, "Probe did not exercise a formatting change"
                else:
                    assert canonical == text, f"Run taplo fmt: {path}"
                server.notify("textDocument/didOpen", {"textDocument": {
                    "uri": uri, "languageId": "toml", "version": 1, "text": text,
                }})
                for options in scenarios:
                    actual = server.format(uri, text, options)
                    assert actual == canonical, f"Editor/CLI mismatch: {path}, {options}"
                server.notify("textDocument/didChange", {
                    "textDocument": {"uri": uri, "version": 2},
                    "contentChanges": [{"text": canonical}],
                })
                assert server.format(uri, canonical, scenarios[1]) == canonical, (
                    f"Second save changed {path}"
                )
                server.notify("textDocument/didClose", {"textDocument": {"uri": uri}})
            # The extension also identifies Cargo.lock as TOML. Keep Cargo's generated
            # lockfile outside the project's *.toml formatting scope.
            lock = root / "Cargo.lock"
            if lock.is_file():
                uri = lock.as_uri()
                server.notify("textDocument/didOpen", {"textDocument": {
                    "uri": uri, "languageId": "toml", "version": 1,
                    "text": lock.read_text(),
                }})
                for options in scenarios:
                    assert not server.request("textDocument/formatting", {
                        "textDocument": {"uri": uri}, "options": options,
                    }), "Editor attempted to reformat Cargo.lock"
                server.notify("textDocument/didClose", {"textDocument": {"uri": uri}})
            assert server.configuration_replies, "Editor configuration was not exercised"
            server.request("shutdown", None)
            server.notify("exit", None)
        except Exception:
            print(server.close())
            raise
        else:
            log = server.close()
            assert "failed to fetch catalog" not in log, log
    print(f"Editor/CLI formats agree for {len(files)} TOML files and an unformatted buffer, "
          f"{len(documents) * len(scenarios)} editor option combinations; "
          f"{len(documents)} repeated saves remain unchanged.")


if __name__ == "__main__":
    main()
