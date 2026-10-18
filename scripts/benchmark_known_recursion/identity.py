"""Source and CPU identities shared by the bounded benchmark entrypoints."""

import hashlib
import os
import subprocess

from .process import sha256


def git(root, *arguments):
    return subprocess.check_output(["git", "-C", str(root), *arguments], text=True).strip()


def source(root):
    status = git(root, "status", "--porcelain=v1", "--untracked-files=all")
    untracked = subprocess.check_output(
        ["git", "-C", str(root), "ls-files", "--others", "--exclude-standard", "-z"]
    ).decode().split("\0")
    untracked_hashes = {}
    for name in filter(None, untracked):
        path = root / name
        untracked_hashes[name] = (
            {"symlink": os.readlink(path)} if path.is_symlink() else {"sha256": sha256(path)}
        )
    return {"commit": git(root, "rev-parse", "HEAD"), "tree": git(root, "rev-parse", "HEAD^{tree}"),
            "dirty": bool(status), "status": status,
            "tracked_diff_sha256": hashlib.sha256(subprocess.check_output(
                ["git", "-C", str(root), "diff", "HEAD", "--binary"])).hexdigest(),
            "untracked_files": untracked_hashes}


def cpus(text):
    selected = set()
    for part in text.split(","):
        bounds = part.split("-")
        if len(bounds) == 1:
            selected.add(int(part))
        elif len(bounds) == 2:
            low, high = map(int, bounds)
            if high < low:
                raise ValueError("descending CPU range")
            selected.update(range(low, high + 1))
        else:
            raise ValueError("invalid CPU list")
    allowed = os.sched_getaffinity(0)
    if not selected or not selected <= allowed or len(selected) > 8 or len(allowed - selected) < 5:
        raise ValueError("choose at most 8 allowed CPUs and leave at least 5 CPUs free")
    return sorted(selected)
