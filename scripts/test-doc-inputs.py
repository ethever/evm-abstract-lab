#!/usr/bin/env python3
"""Check fragment semantics and content isolation of the actual docs projection."""

from html.parser import HTMLParser
from pathlib import Path
import subprocess
import sys
import tempfile
from urllib.parse import quote


class Anchors(HTMLParser):
    def __init__(self):
        super().__init__()
        self.values = []

    def handle_starttag(self, tag, attributes):
        for name, value in attributes:
            if value and (name == "id" or (tag == "a" and name == "name")):
                self.values.append(value)


def main():
    projection = Path(sys.argv[1])
    config = str(Path(sys.argv[2]).resolve())
    documents = {".md", ".html", ".htm", ".xhtml", ".svg"}
    ordinary = []
    candidates = []
    for path in sorted(projection.rglob("*")):
        if path.is_symlink() or not path.is_file():
            continue
        if path.suffix.lower() not in documents:
            assert path.stat().st_size == 0, f"Unnecessary document content input: {path}"
            ordinary.append(path)
        elif path.suffix.lower() in {".html", ".htm"}:
            anchors = Anchors()
            anchors.feed(path.read_text())
            if anchors.values:
                candidates.append((path, anchors.values[0]))
    assert ordinary, "No ordinary target validates content isolation"
    assert candidates, "No projected HTML anchor validates fragment preservation"

    with tempfile.TemporaryDirectory(prefix="doc-inputs-test-") as directory:
        root = Path(directory)
        for source, anchor in candidates:
            target = root / ("target" + source.suffix)
            content = source.read_bytes()
            href = quote(target.name) + "#" + quote(anchor, safe="")
            (root / "README.md").write_text(f"[Projected HTML fragment]({href})\n")
            for data, expected in ((content, 0), (b"", 2), (content, 0)):
                target.write_bytes(data)
                result = subprocess.run([
                    "lychee", "--config", config, "--offline", "--root-dir", str(root),
                    "--", "README.md",
                ], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                assert result.returncode == expected, (
                    f"Projected HTML fragment check exited {result.returncode}, "
                    f"expected {expected}: {result.stdout.decode(errors='replace')}"
                )
    print(f"Projected HTML accepts valid anchors and rejects removed anchors "
          f"for {len(candidates)} documents; {len(ordinary)} ordinary targets contain no source bytes.")


if __name__ == "__main__":
    main()
