#!/usr/bin/env python3
"""Check repository Markdown file links offline, as part of the locked Nix gate."""

from pathlib import Path
import re
import sys


def main() -> int:
    root = Path(sys.argv[1]).resolve()
    errors = []
    checked = 0
    for document in sorted(root.rglob("*.md")):
        for target in re.findall(r"\[[^\]]*\]\(([^)\s]+)\)", document.read_text()):
            if re.match(r"[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith("#"):
                continue
            target = target.split("#", 1)[0]
            checked += 1
            if not (document.parent / target).exists():
                errors.append(f"{document.relative_to(root)}: missing {target}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Checked {checked} local Markdown links")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
