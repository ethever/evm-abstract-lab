#!/usr/bin/env python3
"""Compare Cargo resolution contexts with unit graphs, without compiling crates.

Nix supplies manifest/lock/virtual-source trees and the production package
arguments. Recursive signatures retain host/target, every target attribute,
features, profiles and dependency-edge metadata. Sets of signatures per unit
class preserve Cargo's multiple host/target or profile variants.
"""

import argparse
from collections import defaultdict
from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shlex
import shutil
import subprocess
import sys
import tempfile
import tomllib
from urllib.parse import unquote, urlsplit


class ContextError(Exception):
    """Cargo resolution or an asserted cache boundary did not match."""


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def digest(value):
    return hashlib.sha256(encoded(value).encode()).hexdigest()


def arguments(value):
    if isinstance(value, str):
        return shlex.split(value)
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return value
    raise ContextError("Cargo arguments must be strings or string arrays")


def toml_value(value):
    if isinstance(value, bool):
        return str(value).lower()
    if isinstance(value, (str, int, float)):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, list):
        return "[" + ", ".join(map(toml_value, value)) + "]"
    if isinstance(value, dict):
        return "{" + ", ".join(f"{json.dumps(key)} = {toml_value(item)}"
                               for key, item in value.items()) + "}"
    raise ContextError(f"Unsupported Cargo manifest value: {value!r}")


def write_manifest(path, manifest):
    # Cargo accepts nested inline tables. This small metadata-only writer is
    # shared with the original feature-unifier prototype; it needs no package
    # dependency and preserves every raw manifest field, including target flags.
    sections = []
    for key, value in manifest.items():
        if isinstance(value, dict):
            sections.append(f"[{json.dumps(key)}]\n" + "\n".join(
                f"{json.dumps(name)} = {toml_value(item)}" for name, item in value.items()))
        elif isinstance(value, list) and value and isinstance(value[0], dict):
            for table in value:
                sections.append(f"[[{json.dumps(key)}]]\n" + "\n".join(
                    f"{json.dumps(name)} = {toml_value(item)}" for name, item in table.items()))
        else:
            sections.insert(0, f"{json.dumps(key)} = {toml_value(value)}")
    path.write_text("\n\n".join(sections) + "\n")


def virtual_path(root, relative):
    path = PurePosixPath(relative)
    if path.is_absolute() or ".." in path.parts or relative != path.as_posix():
        raise ContextError(f"Unsafe virtual Cargo target path: {relative!r}")
    current = root
    for part in path.parts:
        current = current / part
        if current.is_symlink():
            raise ContextError(f"Virtual Cargo target traverses a symlink: {relative!r}")
    return current


class UnitGraph:
    def __init__(self, graph, root, package_paths, helper):
        if graph.get("version") != 1 or not graph.get("units") or not graph.get("roots"):
            raise ContextError("Cargo returned an empty or unsupported unit graph")
        self.graph = graph
        self.units = graph["units"]
        self.root = Path(root)
        self.paths = {str(self.root / path): name for name, path in package_paths.items()}
        self.helper = helper
        if any(self.package_name(unit) == helper for unit in self.units):
            raise ContextError("The resolver-only helper acquired a compilation unit")

    def package_name(self, unit):
        identity = unit["pkg_id"]
        if not identity.startswith("path+"):
            return None
        parsed = urlsplit(identity.removeprefix("path+"))
        directory = unquote(parsed.path)
        if directory in self.paths:
            return self.paths[directory]
        # The helper is deliberately outside the production workspace members.
        fragment_name = parsed.fragment.split("@", 1)[0]
        if fragment_name == self.helper or Path(directory).name in {
                self.helper, "__" + self.helper, "__nix-feature-unifier"}:
            return self.helper
        raise ContextError(f"Unrecognized path dependency in Cargo graph: {identity}")

    def package_id(self, unit):
        name = self.package_name(unit)
        if name is None:
            return unit["pkg_id"]
        version = unit["pkg_id"].split("#", 1)[1].split("@")[-1]
        return f"workspace:{name}@{version}"

    def target(self, unit):
        target = dict(unit["target"])
        source = Path(target["src_path"])
        if self.package_name(unit) is not None:
            try:
                target["src_path"] = "$WORKSPACE/" + source.relative_to(self.root).as_posix()
            except ValueError as error:
                raise ContextError(f"Local target escaped its virtual source: {source}") from error
        return target

    def host(self, index, inherited=False):
        return inherited or self.units[index]["target"]["kind"] in (["custom-build"], ["proc-macro"])

    def base(self, index, host):
        unit = self.units[index]
        return {
            "package": self.package_id(unit), "target": self.target(unit),
            "mode": unit["mode"], "platform": unit["platform"], "host": host,
            "other": {key: value for key, value in unit.items() if key not in {
                "pkg_id", "target", "mode", "platform", "features", "profile", "dependencies"}},
        }

    @lru_cache(maxsize=None)
    def signature(self, index, inherited_host=False):
        host = self.host(index, inherited_host)
        unit = self.units[index]
        dependencies = []
        for dependency in unit["dependencies"]:
            edge = {key: value for key, value in dependency.items() if key != "index"}
            dependencies.append({"unit": self.signature(dependency["index"], host), "edge": edge})
        return digest({"base": self.base(index, host), "features": sorted(unit["features"]),
                       "profile": unit["profile"],
                       "dependencies": sorted(dependencies, key=encoded)})

    def roots_for(self, package, *, library_only=False):
        roots = [index for index in self.graph["roots"]
                 if self.package_name(self.units[index]) == package]
        if library_only:
            roots = [index for index, unit in enumerate(self.units)
                     if self.package_name(unit) == package
                     and unit["target"]["kind"] == ["lib"]
                     and unit["mode"] in {"build", "check"}]
        if not roots:
            raise ContextError(f"No reference units for selected package {package!r}")
        return roots

    def reachable(self, roots):
        pending = [(index, False) for index in roots]
        visited = set()
        while pending:
            index, inherited = pending.pop()
            host = self.host(index, inherited)
            if (index, host) in visited:
                continue
            visited.add((index, host))
            pending.extend((item["index"], host) for item in self.units[index]["dependencies"])
        return visited

    def groups(self, roots, *, registry_only=False):
        groups = defaultdict(dict)
        for index, host in self.reachable(roots):
            unit = self.units[index]
            if registry_only and self.package_name(unit) is not None:
                continue
            key = encoded(self.base(index, host))
            signature = self.signature(index, host)
            groups[key][signature] = {"features": sorted(unit["features"]), "profile": unit["profile"]}
        return dict(groups)


def compare(expected, actual, *, overlap=False):
    keys = expected.keys() & actual.keys() if overlap else expected.keys() | actual.keys()
    differences = []
    for key in sorted(keys):
        before, after = expected.get(key, {}), actual.get(key, {})
        if set(before) != set(after):
            differences.append({"unit": json.loads(key),
                                "reference_variants": before, "candidate_variants": after})
    return differences


def run(specification, output):
    if specification.get("version") != 1:
        raise ContextError("Expected feature-context specification version 1")
    for name in ("baselines", "candidates"):
        if not isinstance(specification.get(name), list) or not specification[name]:
            raise ContextError(f"Feature-context specification requires nonempty {name}")
    if not isinstance(specification.get("package_paths"), dict) or not specification["package_paths"]:
        raise ContextError("Feature-context specification requires workspace package paths")
    helper = specification.get("helper_package", "nix-feature-unifier")
    report = {"version": 1, "name": specification["name"], "result": "passed",
              "baselines": [], "candidates": []}
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cargo-feature-context-") as directory:
        temporary = Path(directory)
        environment = dict(os.environ)
        for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET"):
            environment.pop(name, None)
        environment.update({"CARGO_HOME": str(temporary / "cargo-home"),
                            "CARGO_TARGET_DIR": str(temporary / "target"),
                            "RUSTC": specification["rustc"],
                            "RUSTDOC": specification["rustdoc"], "RUSTC_BOOTSTRAP": "1"})
        # Unit-graph mode must only query rustc. Reject any accidental invocation
        # that would compile a crate, even if a future Cargo version changes it.
        guard = temporary / "rustc-query-only"
        guard_code = """import os, sys
args = sys.argv[2:]
query = any(arg in ('-vV', '-V', '--version', '--print') or arg.startswith('--print=') for arg in args)
compile_output = any(arg in ('--emit', '-o', '--out-dir') or arg.startswith(('--emit=', '--out-dir=')) for arg in args)
if not query or compile_output:
    raise SystemExit('Cargo unit-graph attempted compilation: ' + repr(sys.argv[1:]))
os.execv(sys.argv[1], sys.argv[1:])
"""
        guard.write_text(f"#!{sys.executable}\n" + guard_code)
        guard.chmod(0o755)
        environment["RUSTC_WRAPPER"] = str(guard)
        rustdoc_guard = temporary / "rustdoc-query-only"
        rustdoc_guard.write_text(f"#!{sys.executable}\nimport sys\n"
                                f"sys.argv.insert(1, {specification['rustdoc']!r})\n" + guard_code)
        rustdoc_guard.chmod(0o755)
        environment["RUSTDOC"] = str(rustdoc_guard)
        trees = {}

        def graph(entry):
            source = entry["source"]
            tree_key = (source, bool(entry.get("original_manifests")))
            if tree_key not in trees:
                root = temporary / f"source-{len(trees)}"
                shutil.copytree(source, root, symlinks=True)
                root.chmod(root.stat().st_mode | 0o700)
                for path in root.rglob("*"):
                    if not path.is_symlink():
                        path.chmod(path.stat().st_mode | 0o200)
                for relative in specification["target_stub_paths"]:
                    path = virtual_path(root, relative)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    if not path.exists():
                        path.write_text("fn main() {}\n")
                if entry.get("original_manifests"):
                    for relative, manifest in specification["original_manifests"].items():
                        write_manifest(virtual_path(root, relative), manifest)
                else:
                    # Configured dummy sources deliberately inject build.rs.
                    # Restore only that virtual-source field to the original
                    # manifest contract, leaving all generated features intact.
                    for package, relative in specification["package_paths"].items():
                        path = virtual_path(root, relative + "/Cargo.toml")
                        manifest = tomllib.loads(path.read_text())
                        original = specification["original_manifests"][relative + "/Cargo.toml"]["package"]
                        if "build" in original:
                            manifest["package"]["build"] = original["build"]
                        else:
                            manifest["package"].pop("build", None)
                        write_manifest(path, manifest)
                trees[tree_key] = root
            root = trees[tree_key]
            argv = ([specification["cargo"], "--config", specification["vendor_config"]]
                    + specification["operation"] + arguments(entry["arguments"])
                    + (["--target", specification["target"]] if specification.get("target") else [])
                    + ["--offline", "--locked", "--unit-graph", "-Z", "unstable-options"])
            result = subprocess.run(argv, cwd=root, env=environment, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True)
            if result.returncode:
                raise ContextError(f"Cargo exited {result.returncode}: {shlex.join(argv)}\n{result.stderr}")
            value = UnitGraph(json.loads(result.stdout), root, specification["package_paths"], helper)
            return value, {"command": argv, "units": len(value.units),
                           "roots": len(value.graph["roots"]),
                           "edges": sum(len(unit["dependencies"]) for unit in value.units)}

        reference = None
        for baseline in specification["baselines"]:
            value, details = graph(baseline)
            groups = value.groups(value.graph["roots"])
            details.update({"label": baseline["label"], "context_sha256": digest(groups)})
            report["baselines"].append(details)
            if reference is None:
                reference = value
            else:
                differences = compare(reference.groups(reference.graph["roots"]), groups)
                if differences:
                    report["result"] = "failed"
                    details["alias_differences"] = differences
        for candidate in specification["candidates"]:
            value, details = graph(candidate)
            package = candidate["package"]
            reference_roots = reference.roots_for(package, library_only=candidate.get("library_subtree", False))
            candidate_roots = value.roots_for(package)
            expected = reference.groups(reference_roots, registry_only=True)
            actual = value.groups(candidate_roots, registry_only=True)
            registry_differences = compare(expected, actual)
            expected_all = reference.groups(reference_roots)
            actual_all = value.groups(candidate_roots)
            # Compare the complete selected closure, including local targets.
            # Overlap alone would silently accept a missing integration target
            # when other targets already activated all its registry features.
            recursive_differences = compare(expected_all, actual_all)
            expected_root_inventory = sorted(encoded(reference.base(index, reference.host(index)))
                                             for index in reference_roots)
            candidate_root_inventory = sorted(encoded(value.base(index, value.host(index)))
                                              for index in candidate_roots)
            root_inventory_matches = expected_root_inventory == candidate_root_inventory
            reachable = value.reachable(candidate_roots)
            orphan_units = sorted(set(range(len(value.units))) - {index for index, _ in reachable})
            expected_packages = {reference.package_name(reference.units[index])
                                 for index, _ in reference.reachable(reference_roots)}
            extra_packages = sorted({value.package_name(unit) for unit in value.units}
                                    - expected_packages - {None})
            failed = bool(registry_differences or recursive_differences or orphan_units
                          or extra_packages or not root_inventory_matches)
            details.update({"package": package, "result": "failed" if failed else "passed",
                            "registry_unit_classes": len(actual),
                            "reference_registry_context_sha256": digest(expected),
                            "candidate_registry_context_sha256": digest(actual),
                            "common_recursive_unit_classes": len(expected_all.keys() & actual_all.keys()),
                            "root_inventory_matches": root_inventory_matches,
                            "reference_root_inventory": [json.loads(item) for item in expected_root_inventory],
                            "candidate_root_inventory": [json.loads(item) for item in candidate_root_inventory],
                            "registry_differences": registry_differences,
                            "recursive_differences": recursive_differences,
                            "orphan_units": orphan_units, "extra_local_packages": extra_packages,
                            "helper_units": 0})
            report["candidates"].append(details)
            if failed:
                report["result"] = "failed"
            print(f"{specification['name']}: {package}: {details['result']}, "
                  f"{len(value.units)} units, {len(registry_differences)} registry and "
                  f"{len(recursive_differences)} recursive context differences", flush=True)
        compiled = sorted(str(path.relative_to(temporary)) for path in temporary.rglob("*")
                          if path.is_file() and path.suffix in {".rlib", ".rmeta", ".o", ".wasm"})
        report["compiled_outputs"] = compiled
        if compiled:
            report["result"] = "failed"
        output.write_text(json.dumps(report, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--spec", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        report = run(json.loads(args.spec.read_text()), args.output)
        if report["result"] != "passed":
            print(f"Cargo feature contexts differ; full report: {args.output}", file=sys.stderr)
            for case in report["candidates"]:
                if case["result"] != "passed":
                    print(json.dumps(case, indent=2), file=sys.stderr)
            return 1
        return 0
    except (ContextError, OSError, ValueError, KeyError) as error:
        print(f"cargo-feature-context: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
