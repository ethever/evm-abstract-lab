"""Read the browser case contract without importing Playwright or starting services.

The ordered manifest is the only default case list. Nix supplies a projection
containing one case, so another family's edits do not change its source closure.
"""

import ast
import json
from pathlib import Path
import re


SCHEMA_VERSION = 1
SOURCE_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = Path(__file__).with_name("web_test_cases.json")
LEGACY_OPTIONS = (
    "layout-only", "layout-small-only", "flow-only", "ssa-details-only",
    "ssa-values-only", "unknown-world-only", "inspector-controls-only",
    "termination-only", "timings-only", "http-only", "compression-only",
    "diagnostics-only",
)


def python_dependencies(module, root=SOURCE_ROOT):
    """Compute actual local imports, including imports inside lazy functions."""
    pending = ["scripts/test-web-browser.py", "scripts/" + module + ".py"]
    found = set()
    while pending:
        relative = pending.pop()
        if relative in found:
            continue
        found.add(relative)
        source = root / relative
        tree = ast.parse(source.read_text(), filename=str(source))
        for node in ast.walk(tree):
            if isinstance(node, ast.ImportFrom):
                names = [node.module] if node.module else []
            elif isinstance(node, ast.Import):
                names = [item.name for item in node.names]
            else:
                continue
            for name in names:
                if name.startswith("web_test_"):
                    pending.append("scripts/" + name + ".py")
    return found


def validate_manifest(manifest, root=SOURCE_ROOT):
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise ValueError("unsupported browser case manifest schema")
    cases = manifest.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ValueError("browser case manifest must contain a nonempty ordered cases list")
    ids = set()
    for case in cases:
        case_id, module = case.get("id", ""), case.get("module", "")
        if not isinstance(case_id, str) or not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", case_id):
            raise ValueError(f"invalid browser case ID: {case_id!r}")
        if case_id in ids:
            raise ValueError(f"duplicate browser case ID: {case_id}")
        ids.add(case_id)
        if not isinstance(module, str) or not re.fullmatch(r"web_test_[a-z_]+", module):
            raise ValueError(f"invalid module for {case_id}: {module!r}")
        if not isinstance(case.get("parameters", {}), dict):
            raise ValueError(f"parameters must be an object: {case_id}")
        declared = case.get("dependencies")
        if not isinstance(declared, list) or declared != sorted(set(declared)):
            raise ValueError(f"dependencies must be a sorted unique list: {case_id}")
        for relative in declared:
            if not isinstance(relative, str) or Path(relative).is_absolute() or ".." in Path(relative).parts:
                raise ValueError(f"dependency must be repository relative: {relative!r}")
            if not (root / relative).is_file():
                raise ValueError(f"missing {case_id} dependency: {relative}")
        actual = python_dependencies(module, root)
        supplied = {path for path in declared if path.endswith(".py")}
        if actual != supplied:
            raise ValueError(f"inexact Python closure for {case_id}: "
                             f"missing={sorted(actual - supplied)}, extra={sorted(supplied - actual)}")
        tree = ast.parse((root / "scripts" / (module + ".py")).read_text())
        entrypoint = next((node for node in tree.body
                           if isinstance(node, ast.FunctionDef) and node.name == "run"), None)
        if entrypoint is None:
            raise ValueError(f"case module has no run entrypoint: {case_id}")
        positional = entrypoint.args.posonlyargs + entrypoint.args.args
        if not positional or positional[0].arg != "context":
            raise ValueError(f"case entrypoint must accept context first: {case_id}")
        allowed = {argument.arg for argument in positional[1:] + entrypoint.args.kwonlyargs}
        required = {argument.arg for argument in positional[1:len(positional) - len(entrypoint.args.defaults)]}
        required.update(argument.arg for argument, default in zip(entrypoint.args.kwonlyargs, entrypoint.args.kw_defaults)
                        if default is None)
        supplied_parameters = set(case.get("parameters", {}))
        unexpected = supplied_parameters - allowed if entrypoint.args.kwarg is None else set()
        if required - supplied_parameters or unexpected:
            raise ValueError(f"inexact parameters for {case_id}: "
                             f"missing={sorted(required - supplied_parameters)}, extra={sorted(unexpected)}")
    aliases = manifest.get("legacy_flags", {})
    if not isinstance(aliases, dict) or set(aliases) - set(LEGACY_OPTIONS):
        raise ValueError("unknown legacy browser selection flag")
    for flag, members in aliases.items():
        if not isinstance(members, list) or not members or len(members) != len(set(members)):
            raise ValueError(f"legacy flag must select unique cases: --{flag}")
        if set(members) - ids:
            raise ValueError(f"legacy flag --{flag} references unknown cases: {sorted(set(members) - ids)}")
    return manifest


def load_manifest(path=DEFAULT_MANIFEST):
    return validate_manifest(json.loads(Path(path).read_text()))


def select_cases(manifest, explicit=(), legacy=()):
    """Return an ordered exact union. Reject repeated explicit case arguments."""
    if len(explicit) != len(set(explicit)):
        raise ValueError("a --case ID may only be specified once")
    known = {case["id"] for case in manifest["cases"]}
    selected = set(explicit)
    if selected - known:
        raise ValueError(f"unknown browser cases: {sorted(selected - known)}")
    for flag in legacy:
        if flag not in manifest.get("legacy_flags", {}):
            raise ValueError(f"manifest does not define --{flag}")
        selected.update(manifest["legacy_flags"][flag])
    if not explicit and not legacy:
        return manifest["cases"]
    return [case for case in manifest["cases"] if case["id"] in selected]
