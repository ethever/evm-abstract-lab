#!/usr/bin/env python3
"""Run the exact union of independent packaged-WASM browser acceptance cases.

Use --list-cases without browser dependencies to inspect the ordered contract.
Use --case ID for one case; Nix schedules those cases independently. There is no
Python worker pool or concurrency limit. A direct invocation runs every case
once and shares only prerequisite services, never another acceptance case.
"""

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import hashlib
from importlib import import_module
import json
from pathlib import Path
import sys
import time
import traceback

sys.dont_write_bytecode = True
from web_test_registry import DEFAULT_MANIFEST, LEGACY_OPTIONS, SOURCE_ROOT, load_manifest, select_cases


PLATFORM = ("Linux headless Chromium with Xvfb and SwiftShader WebGPU; "
            "synthesized browser gestures, not physical macOS hardware")


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


class Prerequisites:
    """Lazily reuse the unconfigured server and browser during direct full runs.

    Configured RPC cases own their providers/server. DPR gesture cases own their
    real-scale Chromium process. Neither starts an unused default prerequisite.
    """
    def __init__(self, playwright, args, stack):
        self.playwright, self.args, self.stack = playwright, args, stack
        self._browser = self._url = None

    @property
    def url(self):
        if self._url is None:
            from web_test_harness import analysis_server
            self._url = self.stack.enter_context(analysis_server(self.args))
        return self._url

    @property
    def browser(self):
        if self._browser is None:
            from web_test_harness import launch_browser
            self._browser = launch_browser(self.playwright.chromium, self.args.browser)
            self.stack.callback(self._browser.close)
        return self._browser

    def contexts(self):
        return list(self._browser.contexts) if self._browser else []

    def pages(self):
        return [page for context in self.contexts() for page in context.pages]


class CaseContext:
    def __init__(self, prerequisites, args, output):
        self.prerequisites = prerequisites
        self.args = argparse.Namespace(**{**vars(args), "output": output})
        self.output = output
        self.playwright = prerequisites.playwright

    @property
    def browser(self):
        return self.prerequisites.browser

    @property
    def url(self):
        return self.prerequisites.url


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST,
                        help="Ordered case manifest, optionally projected to one Nix check")
    parser.add_argument("--list-cases", action="store_true", help="Print the machine-readable case manifest and exit")
    parser.add_argument("--validate-manifest", action="store_true", help="Validate case IDs, entrypoints and exact local import closures")
    parser.add_argument("--case", dest="case_ids", action="append", default=[], metavar="ID",
                        help="Run exactly this case (repeat for an ordered union)")
    for option in ("server", "assets", "browser"):
        parser.add_argument("--" + option)
    parser.add_argument("--output", type=Path)
    for flag in LEGACY_OPTIONS:
        parser.add_argument("--" + flag, action="store_true", help="Compatibility alias selecting a documented union of case IDs")
    args = parser.parse_args()
    try:
        manifest = load_manifest(args.manifest)
        legacy = [flag for flag in LEGACY_OPTIONS if getattr(args, flag.replace("-", "_"))]
        selected = select_cases(manifest, args.case_ids, legacy)
    except (OSError, ValueError, TypeError, KeyError) as error:
        parser.error(str(error))
    if args.list_cases:
        selected_ids = {case["id"] for case in selected}
        aliases = {flag: members for flag, members in manifest.get("legacy_flags", {}).items()
                   if set(members) <= selected_ids}
        print(json.dumps({**manifest, "cases": selected, "legacy_flags": aliases}, indent=2))
        return
    if args.validate_manifest:
        print(f"Validated {len(selected)} browser cases and exact source closures")
        return
    missing = ["--" + option for option in ("server", "assets", "browser", "output") if getattr(args, option) is None]
    if missing:
        parser.error("required when running cases: " + ", ".join(missing))
    args.output.mkdir(parents=True, exist_ok=True)
    # Import only after manifest/list handling, making the contract usable from
    # Nix evaluation and plain Python without a Playwright installation.
    from playwright.sync_api import sync_playwright
    run_started = time.monotonic_ns()
    reports, timings = {}, {}
    aggregate = {"schema_version": 1, "platform": PLATFORM, "assets": args.assets,
                 "selected_cases": [case["id"] for case in selected], "cases": reports}
    with sync_playwright() as playwright, ExitStack() as stack:
        prerequisites = Prerequisites(playwright, args, stack)
        for specification in selected:
            case_id = specification["id"]
            output = args.output / case_id
            output.mkdir(parents=True, exist_ok=True)
            context = CaseContext(prerequisites, args, output)
            before_pages = set(prerequisites.pages())
            before_contexts = set(prerequisites.contexts())
            start_ns = time.monotonic_ns()
            report = {"schema_version": 1, "id": case_id, "status": "running",
                      "started_at": datetime.now(timezone.utc).isoformat(),
                      "parameters": specification.get("parameters", {}),
                      "inputs": {name: hashlib.sha256((SOURCE_ROOT / name).read_bytes()).hexdigest()
                                 for name in specification["dependencies"]}}
            print(f"browser case start: {case_id}", flush=True)
            try:
                from web_test_harness import capture_browser_errors
                module = import_module(specification["module"])
                with capture_browser_errors() as errors:
                    report["result"] = module.run(context, **specification.get("parameters", {}))
                    assert not errors, "browser errors: " + "\n".join(errors)
                report["browser_errors"] = errors
                report["status"] = "passed"
            except Exception as error:
                report.update(status="failed", error=str(error), traceback=traceback.format_exc())
                print(report["traceback"], file=sys.stderr, flush=True)
            finally:
                # Explicit contexts (for example, platform-specific inspector
                # cases) own routes and storage even after their pages close.
                # Always dispose new contexts before continuing the union.
                for browser_context in prerequisites.contexts():
                    if browser_context not in before_contexts:
                        try:
                            browser_context.close()
                        except Exception as error:
                            report.update(status="failed", cleanup_error=str(error))
                # Also cover a page added to an existing shared context.
                for page in prerequisites.pages():
                    if page not in before_pages:
                        try:
                            page.close()
                        except Exception as error:
                            report.update(status="failed", cleanup_error=str(error))
                duration_ns = time.monotonic_ns() - start_ns
                timing = {"schema_version": 1, "id": case_id, "status": report["status"],
                          "duration_ns": duration_ns}
                report["duration_ns"] = duration_ns
                write_json(output / "report.json", report)
                write_json(output / "timings.json", timing)
                reports[case_id], timings[case_id] = report, timing
                aggregate["status"] = "failed" if any(item["status"] == "failed" for item in reports.values()) else "running"
                write_json(args.output / "report.json", aggregate)
                write_json(args.output / "timings.json", {"schema_version": 1, "cases": timings,
                                                         "duration_ns": time.monotonic_ns() - run_started})
                print(f"browser case {report['status']}: {case_id} ({duration_ns / 1e9:.3f}s)", flush=True)
    if set(reports) != {case["id"] for case in selected}:
        raise AssertionError("browser case execution did not match the selected union")
    aggregate["status"] = "passed" if all(item["status"] == "passed" for item in reports.values()) else "failed"
    aggregate["duration_ns"] = time.monotonic_ns() - run_started
    write_json(args.output / "report.json", aggregate)
    write_json(args.output / "timings.json", {"schema_version": 1, "cases": timings,
                                             "duration_ns": aggregate["duration_ns"]})
    if aggregate["status"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
