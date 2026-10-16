#!/usr/bin/env python3
"""Build changed Nix checks by derivation identity, or verify cache boundaries.

The checkout is only read. Both revisions are materialized as immutable, equally
named ``path:`` flakes, so temporary directory names cannot invalidate checks.
Nonignored untracked files are included by default; --tracked-only is explicit.
Planning never explicitly builds check targets. Nix may realize pinned inputs
needed for evaluation (the repository's Dylint source uses IFD).
The complete merge gate remains ``nix flake check --no-update-lock-file``.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
import fnmatch
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile


class CheckError(Exception):
    """A source, evaluation, or assertion could not be verified."""


class CommandError(CheckError):
    def __init__(self, argv, returncode):
        super().__init__(f"Command exited {returncode}: {shlex.join(map(str, argv))}")
        self.returncode = returncode


def command(argv, *, input=None, cwd=None):
    result = subprocess.run(argv, input=input, cwd=cwd, stdout=subprocess.PIPE)
    if result.returncode:
        raise CommandError(argv, result.returncode)
    return result.stdout


def git(repo, *args, input=None):
    return command(["git", "-C", str(repo), *args], input=input)


def resolve_commit(repo, reference):
    return git(repo, "rev-parse", "--verify", "--end-of-options",
               reference + "^{commit}").decode().strip()


@dataclass(frozen=True)
class Entry:
    kind: str
    sha256: str
    executable: bool = False


@dataclass
class Snapshot:
    path: Path
    files: dict
    untracked: list = field(default_factory=list)
    excluded_untracked: list = field(default_factory=list)

    @property
    def manifest_sha256(self):
        manifest = [
            [name, entry.kind, entry.sha256, entry.executable]
            for name, entry in sorted(self.files.items())
        ]
        return hashlib.sha256(json.dumps(manifest, ensure_ascii=True,
                                         separators=(",", ":")).encode()).hexdigest()


def safe_path(root, name):
    """Reject traversal and symlinked parents without following external files."""
    relative = PurePosixPath(name)
    if (not name or relative.is_absolute() or ".." in relative.parts
            or name != relative.as_posix() or name == "."):
        raise CheckError(f"Unsafe source path: {name!r}")
    parent = root
    for part in relative.parts[:-1]:
        parent = parent / part
        if parent.is_symlink():
            raise CheckError(f"Source path has a symlinked parent: {name!r}")
    return root.joinpath(*relative.parts)


def file_entry(data, executable=False):
    return Entry("file", hashlib.sha256(data).hexdigest(), executable)


def link_entry(target):
    return Entry("symlink", hashlib.sha256(os.fsencode(target)).hexdigest())


def snapshot_commit(repo, commit, destination):
    """Read Git blobs directly: archive export-ignore/subst must not alter sources."""
    destination.mkdir(parents=True)
    blobs = {}
    for record in git(repo, "ls-tree", "-r", "-z", "--full-tree", commit).split(b"\0"):
        if not record:
            continue
        metadata, raw_name = record.split(b"\t", 1)
        mode, kind, oid = metadata.split()
        name = os.fsdecode(raw_name)
        safe_path(destination, name)
        if kind != b"blob" or mode not in (b"100644", b"100755", b"120000"):
            raise CheckError(f"Unsupported Git entry (including submodules): {name!r}")
        blobs.setdefault(oid, []).append((name, mode))

    # One cat-file process avoids a subprocess per source file, including aliases.
    data = command(["git", "-C", str(repo), "cat-file", "--batch"],
                   input=b"".join(oid + b"\n" for oid in blobs))
    stream = io.BytesIO(data)
    files = {}
    for expected_oid, entries in blobs.items():
        header = stream.readline().split()
        if len(header) != 3 or header[:2] != [expected_oid, b"blob"]:
            raise CheckError("Git returned an unexpected blob header")
        content = stream.read(int(header[2]))
        if len(content) != int(header[2]) or stream.read(1) != b"\n":
            raise CheckError("Git returned a truncated blob")
        for name, mode in entries:
            target = safe_path(destination, name)
            target.parent.mkdir(parents=True, exist_ok=True)
            if mode == b"120000":
                link = os.fsdecode(content)
                target.symlink_to(link)
                files[name] = link_entry(link)
            else:
                target.write_bytes(content)
                executable = mode == b"100755"
                target.chmod(0o755 if executable else 0o644)
                files[name] = file_entry(content, executable)
    if stream.read():
        raise CheckError("Git returned unexpected trailing blob data")
    return Snapshot(destination, files)


def working_inventory(repo):
    tracked = {}
    for record in git(repo, "ls-files", "--cached", "--stage", "-z").split(b"\0"):
        if not record:
            continue
        metadata, raw_name = record.split(b"\t", 1)
        mode, oid, stage = metadata.split()
        name = os.fsdecode(raw_name)
        if stage != b"0":
            raise CheckError(f"Unresolved index entry: {name!r}")
        if mode == b"160000":
            raise CheckError(f"Submodule snapshots are not supported: {name!r}")
        tracked[name] = (mode, oid)
    untracked = [os.fsdecode(name) for name in
                 git(repo, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0")
                 if name]
    return tracked, untracked


def signature(path):
    try:
        info = path.lstat()
    except (FileNotFoundError, NotADirectoryError):
        return None
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size,
            info.st_mtime_ns, info.st_ctime_ns)


def snapshot_working(repo, destination, *, include_untracked=True):
    destination.mkdir(parents=True)
    inventory = working_inventory(repo)
    tracked, untracked = inventory
    names = sorted(set(tracked) | (set(untracked) if include_untracked else set()))
    files = {}
    signatures = {}
    for name in names:
        source = safe_path(repo, name)
        before = signature(source)
        signatures[name] = before
        if before is None or stat.S_ISDIR(before[2]):
            # A tracked file replaced by a directory is a deletion. Its new
            # nonignored children enter separately through the untracked list.
            if name in tracked:
                continue
            raise CheckError(f"Untracked directory cannot be snapshotted: {name!r}")
        target = safe_path(destination, name)
        target.parent.mkdir(parents=True, exist_ok=True)
        if stat.S_ISLNK(before[2]):
            link = os.readlink(source)
            target.symlink_to(link)
            files[name] = link_entry(link)
        elif stat.S_ISREG(before[2]):
            flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
            digest = hashlib.sha256()
            with os.fdopen(os.open(source, flags), "rb") as reader, target.open("wb") as writer:
                if not stat.S_ISREG(os.fstat(reader.fileno()).st_mode):
                    raise CheckError(f"Source changed while snapshotting: {name!r}")
                while chunk := reader.read(1024 * 1024):
                    writer.write(chunk)
                    digest.update(chunk)
            executable = bool(before[2] & stat.S_IXUSR)
            target.chmod(0o755 if executable else 0o644)
            files[name] = Entry("file", digest.hexdigest(), executable)
        else:
            raise CheckError(f"Unsupported source file type: {name!r}")
        if signature(source) != before:
            raise CheckError(f"Source changed while snapshotting: {name!r}")

    # Check again after all copies: edits to an early file or the index must not
    # silently produce a snapshot that never existed in the user's checkout.
    if working_inventory(repo) != inventory:
        raise CheckError("Git source inventory changed while snapshotting; retry")
    for name, before in signatures.items():
        if signature(safe_path(repo, name)) != before:
            raise CheckError(f"Source changed while snapshotting: {name!r}")
    return Snapshot(destination, files, untracked if include_untracked else [],
                    [] if include_untracked else untracked)


def current_system():
    return command(["nix", "eval", "--raw", "--impure", "--expr",
                    "builtins.currentSystem"]).decode().strip()


def evaluate_checks(snapshot, system):
    if not re.fullmatch(r"[A-Za-z0-9_-]+", system):
        raise CheckError(f"Invalid Nix system: {system!r}")
    raw = command([
        "nix", "eval", "--json", "--no-update-lock-file",
        f"path:{snapshot.path}#checks.{system}", "--apply",
        "checks: builtins.mapAttrs (name: check: "
        "if builtins.isAttrs check && (check.type or null) == \"derivation\" "
        "then { drv_path = check.drvPath; aggregate_dependencies = "
        "if check ? checkAggregateDependencies then builtins.map (dependency: "
        "if builtins.isAttrs dependency && (dependency.type or null) == \"derivation\" "
        "then dependency.drvPath else throw (\"Invalid aggregate dependency: \" + name)) "
        "check.checkAggregateDependencies else null; } "
        "else throw (\"Invalid check derivation: \" + name)) checks",
    ])
    checks = json.loads(raw)
    if not checks:
        raise CheckError("Nix checks did not evaluate to a derivation-path map")
    check_graph(checks)
    return checks


def evaluate_snapshots(snapshots, system):
    """Evaluate each distinct source once; independent evaluations can overlap."""
    with ThreadPoolExecutor() as executor:
        futures = {}
        for snapshot in snapshots.values():
            key = snapshot.manifest_sha256
            if key not in futures:
                futures[key] = executor.submit(evaluate_checks, snapshot, system)
        return {name: futures[snapshot.manifest_sha256].result()
                for name, snapshot in snapshots.items()}


def check_graph(checks):
    """Validate identities and explicit transparent aggregates; accept old maps."""
    if not isinstance(checks, dict):
        raise CheckError("Nix checks did not evaluate to a derivation-path map")
    paths, definitions = {}, {}

    def is_drv(value):
        return (isinstance(value, str) and value.endswith(".drv")
                and PurePosixPath(value).parent == PurePosixPath("/nix/store"))

    for name, value in checks.items():
        if not isinstance(name, str):
            raise CheckError("Check names must be strings")
        dependencies = None
        if isinstance(value, dict):
            if set(value) != {"drv_path", "aggregate_dependencies"}:
                raise CheckError(f"Invalid check metadata: {name!r}")
            drv = value["drv_path"]
            dependencies = value["aggregate_dependencies"]
            if dependencies is not None:
                if not isinstance(dependencies, list) or not all(map(is_drv, dependencies)):
                    raise CheckError(f"Invalid aggregate dependencies: {name!r}")
                dependencies = frozenset(dependencies)
        else:
            drv = value
        if not is_drv(drv):
            raise CheckError(f"Invalid check derivation path: {name!r}")
        if drv in definitions and definitions[drv] != dependencies:
            raise CheckError(f"Inconsistent aggregate metadata for check aliases: {name!r}")
        paths[name] = drv
        definitions[drv] = dependencies
    aggregates = {drv: dependencies for drv, dependencies in definitions.items()
                  if dependencies is not None}
    for drv, dependencies in aggregates.items():
        missing = dependencies - definitions.keys()
        if missing:
            raise CheckError(f"Aggregate dependencies are not public checks: {sorted(missing)}")

    visited, visiting = set(), set()

    def visit(drv):
        if drv in visiting:
            raise CheckError(f"Aggregate dependency cycle: {drv}")
        if drv in visited:
            return
        visiting.add(drv)
        for dependency in aggregates.get(drv, ()):
            visit(dependency)
        visiting.remove(drv)
        visited.add(drv)

    for drv in aggregates:
        visit(drv)
    return paths, aggregates


def plan_checks(base, head):
    """Build new identities, expanding only explicitly transparent aggregates."""
    base, _ = check_graph(base)
    head, aggregates = check_graph(head)
    base_drvs = set(base.values())
    grouped = {}
    for name, drv in sorted(head.items()):
        grouped.setdefault(drv, []).append(name)
    affected = sorted(drv for drv in grouped if drv not in base_drvs)
    targets, selected = set(), set()

    def select(drv):
        if drv in base_drvs or drv in selected:
            return
        selected.add(drv)
        if drv in aggregates:
            # Only the shared Nix helper supplies this contract. An arbitrary
            # dependency-only-looking runCommand may still contain assertions.
            for dependency in aggregates[drv]:
                select(dependency)
        else:
            targets.add(drv)

    for drv in affected:
        select(drv)
    return {
        "checks": {
            "affected": sorted(name for name, drv in head.items() if drv in affected),
            "unchanged": sorted(name for name, drv in head.items() if base.get(name) == drv),
            "reused": sorted(name for name, drv in head.items()
                             if base.get(name) != drv and drv in base_drvs),
            "added": sorted(set(head) - set(base)),
            "removed": sorted(set(base) - set(head)),
            "skipped_aggregates": sorted(name for name, drv in head.items()
                                         if drv in aggregates and drv in affected),
        },
        "derivations": [
            {"drv_path": drv, "checks": names,
             "baseline_checks": sorted(name for name, value in base.items() if value == drv),
             "build": drv in targets,
             **({"aggregate_dependencies": sorted(aggregates[drv])}
                if drv in aggregates else {})}
            for drv, names in sorted(grouped.items())
        ],
        "build_command": (["nix", "build", "--no-link", "--print-build-logs"]
                          + [drv + "^*" for drv in sorted(targets)]) if targets else [],
    }


def source_changes(base, head):
    return {
        "added": sorted(set(head.files) - set(base.files)),
        "modified": sorted(name for name in set(base.files) & set(head.files)
                           if base.files[name] != head.files[name]),
        "removed": sorted(set(base.files) - set(head.files)),
    }


def load_probes(path):
    manifest = json.loads(path.read_text())
    if not isinstance(manifest, dict) or manifest.get("version") != 1:
        raise CheckError("Probe manifest must be an object with version 1")
    if set(manifest) - {"version", "description", "cases"}:
        raise CheckError("Unknown probe manifest fields")
    cases = manifest.get("cases")
    if not isinstance(cases, list) or not cases:
        raise CheckError("Probe manifest must contain nonempty cases")
    names = set()
    for case in cases:
        if not isinstance(case, dict) or not isinstance(case.get("name"), str) or not case["name"]:
            raise CheckError("Each probe needs a nonempty name")
        if set(case) - {"name", "description", "mutations", "changed", "unchanged", "only_changed"}:
            raise CheckError(f"Unknown fields in probe {case['name']!r}")
        if case["name"] in names:
            raise CheckError(f"Duplicate probe name: {case['name']!r}")
        names.add(case["name"])
        mutations = case.get("mutations")
        if not isinstance(mutations, list) or not mutations:
            raise CheckError(f"Probe {case['name']!r} needs mutations")
        for mutation in mutations:
            if not isinstance(mutation, dict) or not isinstance(mutation.get("path"), str):
                raise CheckError("Each mutation needs a path")
            safe_path(Path("/unused-probe-root"), mutation["path"])
            operations = set(mutation) - {"path"}
            if len(operations) != 1 or not operations <= {"append", "write", "delete"}:
                raise CheckError("Each mutation needs exactly one append, write, or delete operation")
            operation = next(iter(operations))
            value = mutation[operation]
            if ((operation == "delete" and value is not True)
                    or (operation != "delete" and not isinstance(value, str))):
                raise CheckError(f"Invalid {operation} mutation")
        for key in ("changed", "unchanged", "only_changed"):
            patterns = case.get(key, [])
            if not isinstance(patterns, list) or not all(
                    isinstance(pattern, str) and pattern for pattern in patterns):
                raise CheckError(f"Probe {key} expectations must be nonempty string patterns")
        if not case.get("changed") and not case.get("unchanged") and "only_changed" not in case:
            raise CheckError(f"Probe {case['name']!r} has no expectations")
    return cases


def mutate_snapshot(source, destination, mutations):
    shutil.copytree(source.path, destination, symlinks=True)
    files = dict(source.files)
    for mutation in mutations:
        name = mutation["path"]
        path = safe_path(destination, name)
        if "delete" in mutation:
            if name not in files:
                raise CheckError(f"Cannot delete missing probe file: {name!r}")
            path.unlink()
            del files[name]
            # Empty directories are absent from Git source snapshots.
            parent = path.parent
            while parent != destination and not any(parent.iterdir()):
                parent.rmdir()
                parent = parent.parent
            continue
        previous = files.get(name)
        if previous is not None and previous.kind != "file":
            raise CheckError(f"Cannot write through a probe symlink: {name!r}")
        if "append" in mutation:
            if previous is None:
                raise CheckError(f"Cannot append to missing probe file: {name!r}")
            data = path.read_bytes() + mutation["append"].encode()
        else:
            data = mutation["write"].encode()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        executable = previous.executable if previous else False
        path.chmod(0o755 if executable else 0o644)
        files[name] = file_entry(data, executable)
    result = Snapshot(destination, files)
    if result.manifest_sha256 == source.manifest_sha256:
        raise CheckError("Probe mutations do not change the source snapshot")
    return result


def expand_patterns(patterns, checks):
    result = set()
    for pattern in patterns:
        matches = {name for name in checks if fnmatch.fnmatchcase(name, pattern)}
        if not matches:
            raise CheckError(f"Probe pattern matches no checks: {pattern!r}")
        result.update(matches)
    return result


def verify_probe(case, base, changed):
    selection = plan_checks(base, changed)
    base, _ = check_graph(base)
    changed, _ = check_graph(changed)
    if set(base) != set(changed):
        raise CheckError(f"Probe {case['name']!r} changed the check inventory")
    expected_changed = expand_patterns(case.get("changed", []), base)
    expected_unchanged = expand_patterns(case.get("unchanged", []), base)
    if "only_changed" in case:
        exact = expand_patterns(case["only_changed"], base)
        expected_changed.update(exact)
        expected_unchanged.update(set(base) - exact)
    overlap = expected_changed & expected_unchanged
    if overlap:
        raise CheckError(f"Probe has contradictory expectations: {sorted(overlap)}")
    actual_changed = {name for name in base if base[name] != changed[name]}
    failures = (expected_changed - actual_changed) | (expected_unchanged & actual_changed)
    if failures:
        raise CheckError(f"Cache boundary probe {case['name']!r} failed: {sorted(failures)}")
    build_drvs = {item["drv_path"] for item in selection["derivations"] if item["build"]}
    return {"name": case["name"], "result": "passed",
            "changed": sorted(actual_changed),
            "unchanged": sorted(set(base) - actual_changed),
            "assertions": {"changed": sorted(expected_changed),
                           "unchanged": sorted(expected_unchanged)},
            "selection": {"build_checks": sorted(name for name, drv in changed.items()
                                                  if drv in build_drvs),
                          "skipped_aggregates": selection["checks"]["skipped_aggregates"],
                          "build_command": selection["build_command"]}}


def run_probes(source, cases, system, temporary):
    snapshots = {"baseline": source}
    for index, case in enumerate(cases):
        snapshots[index] = mutate_snapshot(source, temporary / f"probe-{index}" / "source",
                                            case["mutations"])
    checks = evaluate_snapshots(snapshots, system)
    results = []
    for index, case in enumerate(cases):
        try:
            results.append(verify_probe(case, checks["baseline"], checks[index]))
        except CheckError as error:
            baseline, _ = check_graph(checks["baseline"])
            variant, _ = check_graph(checks[index])
            actual_changed = {name for name in set(baseline) | set(variant)
                              if baseline.get(name) != variant.get(name)}
            results.append({"name": case["name"], "result": "failed", "error": str(error),
                            "changed": sorted(actual_changed),
                            "unchanged": sorted(set(baseline) - actual_changed)})
    baseline, _ = check_graph(checks["baseline"])
    identity = json.dumps(baseline, sort_keys=True, separators=(",", ":")).encode()
    return {"version": 1, "mode": "probe", "system": system,
            "manifest_sha256": source.manifest_sha256, "cases": results,
            "checks": {"count": len(baseline), "unique_derivations": len(set(baseline.values())),
                       "derivation_map_sha256": hashlib.sha256(identity).hexdigest()},
            "result": "passed" if all(case["result"] == "passed" for case in results) else "failed"}


def execute_plan(plan):
    argv = plan["build_command"]
    if not argv:
        return "no-affected-derivations"
    # A single build lets Nix deduplicate the graph, use existing cache entries,
    # and schedule every affected check under the host's own resource settings.
    result = subprocess.run(argv, stdout=sys.stderr)
    if result.returncode:
        raise CommandError(argv, result.returncode)
    return "built"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", help="baseline commit/ref (default: HEAD)")
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="repository or a directory inside it")
    parser.add_argument("--system", help="Nix system (default: local Nix system)")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--plan", "--dry-run", action="store_true", help="print JSON plan without explicitly building checks")
    modes.add_argument("--probe", type=Path, help="evaluate cache-boundary cases from a JSON manifest")
    sources = parser.add_mutually_exclusive_group()
    sources.add_argument("--tracked-only", action="store_true", help="explicitly exclude nonignored untracked files")
    sources.add_argument("--include-untracked", dest="tracked_only", action="store_false", help="include nonignored untracked files (default)")
    args = parser.parse_args(argv)
    if args.probe and args.base:
        parser.error("--base applies to change selection; probes use the current working snapshot")
    try:
        cases = load_probes(args.probe) if args.probe else None
        repo = Path(git(args.repo, "rev-parse", "--show-toplevel").decode().rstrip("\n"))
        head_commit = resolve_commit(repo, "HEAD")
        reference = args.base or "HEAD"
        base_commit = resolve_commit(repo, reference) if args.base else head_commit
        system = args.system or current_system()
        with tempfile.TemporaryDirectory(prefix="check-changes-") as directory:
            temporary = Path(directory)
            head = snapshot_working(repo, temporary / "head" / "source",
                                    include_untracked=not args.tracked_only)
            if resolve_commit(repo, "HEAD") != head_commit:
                raise CheckError("Git HEAD changed while snapshotting; retry")
            print("Evaluating check derivations; Nix may realize pinned evaluation inputs.",
                  file=sys.stderr)
            if cases is not None:
                result = run_probes(head, cases, system, temporary)
            else:
                base = snapshot_commit(repo, base_commit, temporary / "base" / "source")
                checks = evaluate_snapshots({"base": base, "head": head}, system)
                result = {"version": 1, "mode": "plan" if args.plan else "build", "system": system,
                          "base": {"ref": reference, "commit": base_commit,
                                   "manifest_sha256": base.manifest_sha256},
                          "head": {"commit": head_commit, "manifest_sha256": head.manifest_sha256,
                                   "include_untracked": not args.tracked_only,
                                   "untracked": head.untracked,
                                   "excluded_untracked": head.excluded_untracked},
                          "source_changes": source_changes(base, head),
                          **plan_checks(checks["base"], checks["head"])}
                result["result"] = "planned" if args.plan else execute_plan(result)
            print(json.dumps(result, indent=2, sort_keys=True))
        return 1 if result["result"] == "failed" else 0
    except (CheckError, OSError, ValueError) as error:
        print(f"check-changes: {error}", file=sys.stderr)
        if isinstance(error, CommandError):
            return error.returncode if error.returncode > 0 else 128 - error.returncode
        return 1


if __name__ == "__main__":
    sys.exit(main())
