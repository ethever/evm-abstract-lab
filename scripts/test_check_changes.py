#!/usr/bin/env python3
"""Exercise identity planning, isolated source snapshots, and fail-closed probes."""

from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "check_changes", Path(__file__).with_name("check-changes.py"))
checks = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checks
SPEC.loader.exec_module(checks)


def drv(name):
    return f"/nix/store/{name}.drv"


def aggregate(name, *dependencies):
    return {"drv_path": drv(name), "aggregate_dependencies": list(map(drv, dependencies))}


class PlannerTests(unittest.TestCase):
    def test_unchanged_and_aliased_checks_need_no_work(self):
        baseline = {"tests": drv("one"), "alias": drv("one")}
        plan = checks.plan_checks(baseline, baseline)
        self.assertEqual(plan["build_command"], [])
        self.assertEqual(plan["checks"]["unchanged"], ["alias", "tests"])
        self.assertEqual(plan["derivations"], [{
            "drv_path": drv("one"), "checks": ["alias", "tests"],
            "baseline_checks": ["alias", "tests"], "build": False,
        }])

    def test_new_changed_removed_and_aliased_derivations(self):
        plan = checks.plan_checks(
            {"removed": drv("old"), "changed": drv("before"), "same": drv("same")},
            {"changed": drv("after"), "new": drv("new"), "alias": drv("after"),
             "same": drv("same")})
        self.assertEqual(plan["checks"]["affected"], ["alias", "changed", "new"])
        self.assertEqual(plan["checks"]["removed"], ["removed"])
        self.assertEqual(plan["checks"]["added"], ["alias", "new"])
        self.assertEqual(plan["build_command"], [
            "nix", "build", "--no-link", "--print-build-logs", drv("after") + "^*", drv("new") + "^*",
        ])

    def test_renames_and_redirects_to_baseline_identity_are_reused(self):
        plan = checks.plan_checks({"old-name": drv("one"), "redirect": drv("two")},
                                 {"new-name": drv("one"), "redirect": drv("one")})
        self.assertEqual(plan["build_command"], [])
        self.assertEqual(plan["checks"]["reused"], ["new-name", "redirect"])
        self.assertEqual(plan["checks"]["removed"], ["old-name"])

    def test_old_schema_is_not_a_filename_or_check_name_map(self):
        plan = checks.plan_checks({"old-workspace-check": drv("old")},
                                 {"per-crate-a": drv("a"), "per-crate-b": drv("b")})
        self.assertEqual(plan["checks"]["affected"], ["per-crate-a", "per-crate-b"])
        self.assertEqual(len(plan["build_command"][4:]), 2)

    def test_empty_baseline_builds_every_unique_new_derivation(self):
        plan = checks.plan_checks({}, {"a": drv("a"), "alias": drv("a"), "b": drv("b")})
        self.assertEqual(plan["build_command"][4:], [drv("a") + "^*", drv("b") + "^*"])

    def test_changed_transparent_aggregate_does_not_realize_unchanged_siblings(self):
        plan = checks.plan_checks(
            {"leaf": drv("old"), "sibling": drv("same"),
             "aggregate": aggregate("old-group", "old", "same")},
            {"leaf": drv("new"), "sibling": drv("same"),
             "aggregate": aggregate("new-group", "new", "same")})
        self.assertEqual(plan["build_command"][4:], [drv("new") + "^*"])
        self.assertEqual(plan["checks"]["affected"], ["aggregate", "leaf"])
        self.assertEqual(plan["checks"]["skipped_aggregates"], ["aggregate"])

    def test_nested_aggregates_and_leaf_aliases_expand_once(self):
        plan = checks.plan_checks({}, {
            "leaf": drv("leaf"), "leaf-alias": drv("leaf"),
            "inner": aggregate("inner", "leaf"),
            "outer": aggregate("outer", "inner", "leaf"),
            "outer-alias": aggregate("outer", "leaf", "inner"),
        })
        self.assertEqual(plan["build_command"][4:], [drv("leaf") + "^*"])
        self.assertEqual(plan["checks"]["skipped_aggregates"],
                         ["inner", "outer", "outer-alias"])

    def test_new_aggregate_over_baseline_leaves_needs_no_build(self):
        plan = checks.plan_checks({"leaf": drv("same")}, {
            "leaf": drv("same"), "group": aggregate("group", "same")})
        self.assertEqual(plan["build_command"], [])
        self.assertEqual(plan["checks"]["skipped_aggregates"], ["group"])

    def test_unmarked_checks_retain_their_complete_build_contract(self):
        plan = checks.plan_checks({"leaf": drv("same")}, {
            "leaf": drv("same"), "assertion": drv("assertion")})
        self.assertEqual(plan["build_command"][4:], [drv("assertion") + "^*"])
        self.assertEqual(plan["checks"]["skipped_aggregates"], [])

    def test_invalid_aggregate_graphs_fail_closed(self):
        invalid = [
            {"group": aggregate("group", "not-public")},
            {"group": aggregate("group", "group")},
            {"one": aggregate("one", "two"), "two": aggregate("two", "one")},
            {"group": aggregate("same"), "alias": drv("same")},
            {"group": {"drv_path": drv("group"), "aggregate_dependencies": "not-a-list"}},
            {"group": {"drv_path": drv("group"), "unexpected": []}},
        ]
        for head in invalid:
            with self.subTest(head=head), self.assertRaises(checks.CheckError):
                checks.plan_checks({}, head)

    def test_one_build_and_no_build_when_nothing_changed(self):
        plan = checks.plan_checks({}, {"a": drv("a"), "b": drv("b")})
        with patch.object(checks.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            self.assertEqual(checks.execute_plan(plan), "built")
            run.assert_called_once_with(plan["build_command"], stdout=sys.stderr)
        with patch.object(checks.subprocess, "run") as run:
            self.assertEqual(checks.execute_plan({"build_command": []}), "no-affected-derivations")
            run.assert_not_called()

    def test_failed_build_retains_real_exit_code(self):
        with patch.object(checks.subprocess, "run", return_value=subprocess.CompletedProcess([], 19)):
            with self.assertRaises(checks.CommandError) as raised:
                checks.execute_plan({"build_command": ["nix", "build", drv("a") + "^*"]})
        self.assertEqual(raised.exception.returncode, 19)

    def test_failed_command_is_never_interpreted_as_output(self):
        with self.assertRaises(checks.CommandError) as raised:
            checks.command([sys.executable, "-c", "raise SystemExit(23)"])
        self.assertEqual(raised.exception.returncode, 23)


class GitFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="check-changes-test-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.run_git("init", "--quiet")
        self.run_git("config", "user.name", "Check Changes Test")
        self.run_git("config", "user.email", "checks@example.invalid")
        self.run_git("config", "core.autocrlf", "false")
        self.run_git("config", "core.fileMode", "true")
        for name, data in {
            ".gitignore": b"ignored.tmp\n",
            ".gitattributes": b"exported.txt export-ignore\nsubst.txt export-subst\n",
            "tracked.txt": b"initial\n",
            "exported.txt": b"must survive archival snapshots\n",
            "subst.txt": b"$Format:%H$\n",
            "dir/child.txt": b"child\n",
            "odd \t\n name.txt": b"\0\r\n\xff",
            "executable": b"#!/bin/sh\nexit 0\n",
        }.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (self.repo / "executable").chmod(0o755)
        (self.repo / "link").symlink_to("tracked.txt")
        self.run_git("add", ".")
        self.run_git("commit", "--quiet", "-m", "fixture")
        self.commit = checks.resolve_commit(self.repo, "HEAD")

    def tearDown(self):
        self.temporary.cleanup()

    def run_git(self, *args, input=None):
        return subprocess.run(["git", "-C", str(self.repo), *args], input=input,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True).stdout

    def commit_snapshot(self, name="base"):
        return checks.snapshot_commit(self.repo, self.commit, self.root / name / "source")

    def working_snapshot(self, name="head", **kwargs):
        return checks.snapshot_working(self.repo, self.root / name / "source", **kwargs)


class SnapshotTests(GitFixture):
    def test_clean_snapshots_are_identical_and_leave_source_and_index_intact(self):
        index_before = (self.repo / ".git/index").read_bytes()
        status_before = self.run_git("status", "--porcelain=v1", "-z")
        base = self.commit_snapshot()
        head = self.working_snapshot()
        self.assertEqual(base.files, head.files)
        self.assertEqual(base.manifest_sha256, head.manifest_sha256)
        self.assertEqual(base.path.name, "source")
        self.assertEqual(head.path.name, "source")
        self.assertEqual(checks.source_changes(base, head), {"added": [], "modified": [], "removed": []})
        self.assertEqual((self.repo / ".git/index").read_bytes(), index_before)
        self.assertEqual(self.run_git("status", "--porcelain=v1", "-z"), status_before)

    def test_archive_attributes_do_not_drop_or_substitute_source_files(self):
        snapshot = self.commit_snapshot()
        self.assertEqual((snapshot.path / "exported.txt").read_bytes(), b"must survive archival snapshots\n")
        self.assertEqual((snapshot.path / "subst.txt").read_bytes(), b"$Format:%H$\n")

    def test_dirty_snapshot_uses_working_bytes_and_includes_new_files(self):
        (self.repo / "tracked.txt").write_text("staged\n")
        self.run_git("add", "tracked.txt")
        (self.repo / "tracked.txt").write_text("unstaged wins\n")
        (self.repo / "staged-new.txt").write_text("new\n")
        self.run_git("add", "staged-new.txt")
        (self.repo / "untracked.txt").write_text("not ignored\n")
        (self.repo / "ignored.tmp").write_text("ignored\n")
        self.run_git("mv", "dir/child.txt", "renamed.txt")
        (self.repo / "exported.txt").unlink()
        snapshot = self.working_snapshot()
        self.assertEqual((snapshot.path / "tracked.txt").read_text(), "unstaged wins\n")
        self.assertIn("staged-new.txt", snapshot.files)
        self.assertIn("untracked.txt", snapshot.files)
        self.assertNotIn("ignored.tmp", snapshot.files)
        self.assertNotIn("dir/child.txt", snapshot.files)
        self.assertNotIn("exported.txt", snapshot.files)
        self.assertIn("renamed.txt", snapshot.files)
        self.assertEqual(snapshot.untracked, ["untracked.txt"])
        changes = checks.source_changes(self.commit_snapshot(), snapshot)
        self.assertEqual(changes["modified"], ["tracked.txt"])
        self.assertEqual(changes["removed"], ["dir/child.txt", "exported.txt"])

    def test_untracked_exclusion_requires_explicit_option_and_is_reported(self):
        (self.repo / "new.txt").write_text("new\n")
        snapshot = self.working_snapshot(include_untracked=False)
        self.assertNotIn("new.txt", snapshot.files)
        self.assertEqual(snapshot.untracked, [])
        self.assertEqual(snapshot.excluded_untracked, ["new.txt"])

    def test_staged_deletion_recreated_as_untracked_obeys_source_option(self):
        self.run_git("rm", "--quiet", "tracked.txt")
        (self.repo / "tracked.txt").write_text("untracked recreation\n")
        included = self.working_snapshot()
        excluded = self.working_snapshot("excluded", include_untracked=False)
        self.assertIn("tracked.txt", included.files)
        self.assertIn("tracked.txt", included.untracked)
        self.assertNotIn("tracked.txt", excluded.files)

    def test_executable_and_dangling_symlink_are_source_content(self):
        (self.repo / "tracked.txt").chmod(0o755)
        (self.repo / "link").unlink()
        (self.repo / "link").symlink_to("does-not-exist")
        snapshot = self.working_snapshot()
        self.assertTrue(snapshot.files["tracked.txt"].executable)
        self.assertTrue((snapshot.path / "tracked.txt").stat().st_mode & 0o100)
        self.assertEqual(os.readlink(snapshot.path / "link"), "does-not-exist")
        self.assertEqual(snapshot.files["link"].kind, "symlink")
        self.assertEqual(checks.source_changes(self.commit_snapshot(), snapshot)["modified"],
                         ["link", "tracked.txt"])

    def test_tracked_file_replaced_by_directory_includes_its_untracked_children(self):
        (self.repo / "tracked.txt").unlink()
        (self.repo / "tracked.txt").mkdir()
        (self.repo / "tracked.txt/new.txt").write_text("replacement child\n")
        snapshot = self.working_snapshot()
        self.assertNotIn("tracked.txt", snapshot.files)
        self.assertIn("tracked.txt/new.txt", snapshot.files)

    def test_tracked_directory_replaced_by_file_deletes_its_children(self):
        shutil.rmtree(self.repo / "dir")
        (self.repo / "dir").write_text("replacement file\n")
        snapshot = self.working_snapshot()
        self.assertNotIn("dir/child.txt", snapshot.files)
        self.assertEqual((snapshot.path / "dir").read_text(), "replacement file\n")

    def test_symlinked_parent_is_rejected_before_reading_external_files(self):
        outside = self.root / "outside"
        outside.mkdir()
        (outside / "child.txt").write_text("outside\n")
        shutil.rmtree(self.repo / "dir")
        (self.repo / "dir").symlink_to(outside)
        with self.assertRaisesRegex(checks.CheckError, "symlinked parent"):
            self.working_snapshot()
        self.assertEqual((outside / "child.txt").read_text(), "outside\n")

    def test_index_conflicts_fail_closed(self):
        oid = self.run_git("rev-parse", "HEAD:tracked.txt").strip()
        index = b"0 " + b"0" * 40 + b"\ttracked.txt\n"
        index += b"100644 " + oid + b" 1\ttracked.txt\n"
        index += b"100644 " + oid + b" 2\ttracked.txt\n"
        self.run_git("update-index", "--index-info", input=index)
        with self.assertRaisesRegex(checks.CheckError, "Unresolved index"):
            self.working_snapshot()

    def test_submodules_fail_closed_for_commit_and_working_sources(self):
        self.run_git("update-index", "--add", "--cacheinfo", f"160000,{self.commit},module")
        with self.assertRaisesRegex(checks.CheckError, "Submodule"):
            self.working_snapshot()
        self.run_git("commit", "--quiet", "-m", "gitlink")
        commit = checks.resolve_commit(self.repo, "HEAD")
        with self.assertRaisesRegex(checks.CheckError, "including submodules"):
            checks.snapshot_commit(self.repo, commit, self.root / "gitlink" / "source")

    def test_content_change_after_early_copy_is_detected(self):
        original = checks.working_inventory
        calls = 0

        def inventory(repo):
            nonlocal calls
            calls += 1
            if calls == 2:
                (self.repo / "tracked.txt").write_text("changed during snapshot\n")
            return original(repo)

        with patch.object(checks, "working_inventory", side_effect=inventory):
            with self.assertRaisesRegex(checks.CheckError, "Source changed"):
                self.working_snapshot()

    def test_new_untracked_file_during_snapshot_is_detected(self):
        original = checks.working_inventory
        calls = 0

        def inventory(repo):
            nonlocal calls
            calls += 1
            if calls == 2:
                (self.repo / "arrived.txt").write_text("arrived during snapshot\n")
            return original(repo)

        with patch.object(checks, "working_inventory", side_effect=inventory):
            with self.assertRaisesRegex(checks.CheckError, "inventory changed"):
                self.working_snapshot()


class EvaluationTests(unittest.TestCase):
    def test_identical_source_manifests_evaluate_once(self):
        files = {"a": checks.file_entry(b"same")}
        sources = {"base": checks.Snapshot(Path("/base/source"), files),
                   "head": checks.Snapshot(Path("/head/source"), dict(files))}
        with patch.object(checks, "evaluate_checks", return_value={"a": drv("a")}) as evaluate:
            result = checks.evaluate_snapshots(sources, "x86_64-linux")
        evaluate.assert_called_once()
        self.assertEqual(result["base"], result["head"])

    def test_distinct_sources_evaluate_concurrently(self):
        barrier = threading.Barrier(2)
        sources = {name: checks.Snapshot(Path(f"/{name}/source"),
                                        {"a": checks.file_entry(name.encode())})
                   for name in ("base", "head")}

        def evaluate(snapshot, system):
            barrier.wait(timeout=15)
            return {"a": drv(snapshot.manifest_sha256)}

        with patch.object(checks, "evaluate_checks", side_effect=evaluate):
            result = checks.evaluate_snapshots(sources, "x86_64-linux")
        self.assertNotEqual(result["base"], result["head"])

    def test_evaluation_uses_frozen_path_flake_without_explicit_builds(self):
        source = checks.Snapshot(Path("/temporary/head/source"), {})
        with patch.object(checks, "command", return_value=json.dumps({"a": drv("a")}).encode()) as run:
            self.assertEqual(checks.evaluate_checks(source, "x86_64-linux"), {"a": drv("a")})
        argv = run.call_args.args[0]
        self.assertIn("path:/temporary/head/source#checks.x86_64-linux", argv)
        self.assertIn("--no-update-lock-file", argv)
        self.assertNotIn("build", argv)
        self.assertNotIn("--max-jobs", argv)
        self.assertNotIn("--cores", argv)

    def test_non_derivation_output_is_rejected(self):
        source = checks.Snapshot(Path("/source"), {})
        for value in ([], {}, {"a": None}, {"a": "/tmp/a.drv"},
                      {"a": "/nix/store/output"}, {"a": "/nix/store/../a.drv"}):
            with self.subTest(value=value), patch.object(checks, "command", return_value=json.dumps(value).encode()):
                with self.assertRaises(checks.CheckError):
                    checks.evaluate_checks(source, "x86_64-linux")

    def test_evaluation_errors_are_not_missing_or_passing_checks(self):
        source = checks.Snapshot(Path("/source"), {})
        with patch.object(checks, "command", side_effect=checks.CommandError(["nix", "eval"], 5)):
            with self.assertRaises(checks.CommandError):
                checks.evaluate_snapshots({"base": source}, "x86_64-linux")

    def test_evaluation_retains_verified_aggregate_metadata(self):
        source = checks.Snapshot(Path("/source"), {})
        graph = {"leaf": {"drv_path": drv("leaf"), "aggregate_dependencies": None},
                 "group": aggregate("group", "leaf")}
        with patch.object(checks, "command", return_value=json.dumps(graph).encode()):
            self.assertEqual(checks.evaluate_checks(source, "x86_64-linux"), graph)


class ProbeTests(GitFixture):
    def test_repository_probe_manifest_is_valid(self):
        cases = checks.load_probes(Path(__file__).with_name("check_cache_boundaries.json"))
        self.assertTrue(cases)

    def test_probe_compares_identity_instead_of_aggregate_metadata(self):
        # Changes in aggregate metadata alone must never count as source identity
        # changes; probes continue to compare the actual derivation path.
        baseline = {"leaf": drv("leaf"), "group": drv("group")}
        marked = {"leaf": drv("leaf"), "group": aggregate("group", "leaf")}
        report = checks.verify_probe({"name": "metadata", "only_changed": []}, baseline, marked)
        self.assertEqual(report["changed"], [])

    def test_mutations_are_independent_and_leave_the_baseline_unchanged(self):
        baseline = self.working_snapshot()
        before = baseline.manifest_sha256
        mutated = checks.mutate_snapshot(baseline, self.root / "variant" / "source", [
            {"path": "tracked.txt", "append": "appended\n"},
            {"path": "new/deep/file.txt", "write": "new\n"},
            {"path": "dir/child.txt", "delete": True},
        ])
        self.assertEqual((baseline.path / "tracked.txt").read_text(), "initial\n")
        self.assertEqual((self.repo / "tracked.txt").read_text(), "initial\n")
        self.assertEqual(baseline.manifest_sha256, before)
        self.assertNotEqual(mutated.manifest_sha256, before)
        self.assertEqual((mutated.path / "tracked.txt").read_text(), "initial\nappended\n")
        self.assertEqual((mutated.path / "new/deep/file.txt").read_text(), "new\n")
        self.assertNotIn("dir/child.txt", mutated.files)
        self.assertFalse((mutated.path / "dir").exists())

    def test_mutations_cannot_follow_symlinks_or_escape_snapshot(self):
        source = self.working_snapshot()
        with self.assertRaisesRegex(checks.CheckError, "symlink"):
            checks.mutate_snapshot(source, self.root / "link-write" / "source",
                                   [{"path": "link", "write": "bad"}])
        for name in ("../outside", "/outside", "./tracked.txt", "dir//file"):
            with self.subTest(name=name), self.assertRaises(checks.CheckError):
                checks.safe_path(source.path, name)

    def test_noop_probe_cannot_claim_a_cache_invariant(self):
        source = self.working_snapshot()
        with self.assertRaisesRegex(checks.CheckError, "do not change"):
            checks.mutate_snapshot(source, self.root / "noop" / "source",
                                   [{"path": "tracked.txt", "write": "initial\n"}])

    def test_probe_asserts_changes_and_unchanged_checks(self):
        baseline = {"core-a": drv("a"), "core-b": drv("b"), "web": drv("web")}
        changed = {**baseline, "web": drv("new-web")}
        case = {"name": "web", "unchanged": ["core-*"], "changed": ["web"]}
        report = checks.verify_probe(case, baseline, changed)
        self.assertEqual(report["result"], "passed")
        self.assertEqual(report["changed"], ["web"])
        self.assertEqual(report["assertions"]["unchanged"], ["core-a", "core-b"])

    def test_missing_patterns_cannot_pass_vacuously(self):
        with self.assertRaisesRegex(checks.CheckError, "matches no checks"):
            checks.verify_probe({"name": "bad", "unchanged": ["renamed-*"]},
                                {"core": drv("a")}, {"core": drv("a")})

    def test_only_changed_asserts_the_entire_current_inventory(self):
        baseline = {"core": drv("a"), "web-rpc": drv("rpc"), "new-check": drv("new")}
        case = {"name": "exact", "only_changed": ["web-*"]}
        expected = {**baseline, "web-rpc": drv("edited-rpc")}
        self.assertEqual(checks.verify_probe(case, baseline, expected)["result"], "passed")
        with self.assertRaises(checks.CheckError):
            checks.verify_probe(case, baseline, {**expected, "new-check": drv("invalidated")})
        with self.assertRaises(checks.CheckError):
            checks.verify_probe(case, baseline, baseline)
        self.assertEqual(checks.verify_probe({"name": "none", "only_changed": []},
                                            baseline, baseline)["changed"], [])

    def test_missing_checks_and_contradictory_or_failed_expectations_fail(self):
        baseline = {"core": drv("a")}
        cases = [
            ({"name": "removed", "changed": ["core"]}, {}),
            ({"name": "contradiction", "changed": ["core"], "unchanged": ["core"]}, baseline),
            ({"name": "did-not-change", "changed": ["core"]}, baseline),
            ({"name": "invalidated", "unchanged": ["core"]}, {"core": drv("b")}),
        ]
        for case, changed in cases:
            with self.subTest(case=case), self.assertRaises(checks.CheckError):
                checks.verify_probe(case, baseline, changed)

    def test_identical_probe_variants_reuse_evaluation(self):
        source = self.working_snapshot()
        cases = [{"name": name, "mutations": [{"path": "tracked.txt", "append": "probe\n"}],
                  "changed": ["dependent"], "unchanged": ["unrelated"]}
                 for name in ("first", "second")]

        def evaluate(snapshot, system):
            return {"dependent": drv(snapshot.files["tracked.txt"].sha256), "unrelated": drv("same")}

        with patch.object(checks, "evaluate_checks", side_effect=evaluate) as evaluate_mock:
            report = checks.run_probes(source, cases, "x86_64-linux", self.root / "probes")
        self.assertEqual(evaluate_mock.call_count, 2)
        self.assertEqual(report["result"], "passed")
        self.assertEqual(len(report["cases"]), 2)

    def test_failed_probe_retains_every_evaluated_case_without_claiming_success(self):
        source = self.working_snapshot()
        cases = [
            {"name": "passes", "mutations": [{"path": "tracked.txt", "append": "one\n"}],
             "unchanged": ["check"]},
            {"name": "fails", "mutations": [{"path": "tracked.txt", "append": "two\n"}],
             "changed": ["check"]},
        ]
        with patch.object(checks, "evaluate_checks", return_value={"check": drv("same")}):
            report = checks.run_probes(source, cases, "x86_64-linux", self.root / "probes")
        self.assertEqual(report["result"], "failed")
        self.assertEqual([case["result"] for case in report["cases"]], ["passed", "failed"])
        self.assertIn("error", report["cases"][1])

    def test_manifest_rejects_malformed_and_unsafe_cases(self):
        valid = {"name": "valid", "mutations": [{"path": "tracked.txt", "append": "probe"}],
                 "unchanged": ["core"]}
        invalid = [
            {}, {"version": 1, "cases": []},
            {"version": 1, "cases": [valid, valid]},
            {"version": 1, "cases": [{**valid, "mutations": [{"path": "../outside", "write": "x"}]}]},
            {"version": 1, "cases": [{**valid, "mutations": [{"path": "x", "append": "x", "delete": True}]}]},
            {"version": 1, "cases": [{**valid, "unchanged": []}]},
            {"version": 1, "cases": [{**valid, "changed": "core"}]},
            {"version": 1, "cases": [{**valid, "changged": ["core"]}]},
        ]
        path = self.root / "manifest.json"
        for manifest in invalid:
            with self.subTest(manifest=manifest):
                path.write_text(json.dumps(manifest))
                with self.assertRaises(checks.CheckError):
                    checks.load_probes(path)
        path.write_text(json.dumps({"version": 1, "cases": [valid]}))
        self.assertEqual(checks.load_probes(path), [valid])


class CliTests(GitFixture):
    def invoke(self, *args):
        output, errors = io.StringIO(), io.StringIO()
        with redirect_stdout(output), redirect_stderr(errors):
            result = checks.main(["--repo", str(self.repo), "--system", "x86_64-linux", *args])
        return result, output.getvalue(), errors.getvalue()

    def test_default_plan_includes_untracked_without_building(self):
        (self.repo / "new.txt").write_text("new\n")

        def evaluate(snapshot, system):
            return {"check": drv("new" if "new.txt" in snapshot.files else "old")}

        with patch.object(checks, "evaluate_checks", side_effect=evaluate), patch.object(checks, "execute_plan") as execute:
            result, output, errors = self.invoke("--plan")
        self.assertEqual(result, 0, errors)
        report = json.loads(output)
        self.assertEqual(report["result"], "planned")
        self.assertEqual(report["head"]["untracked"], ["new.txt"])
        self.assertTrue(report["head"]["include_untracked"])
        self.assertEqual(report["checks"]["affected"], ["check"])
        execute.assert_not_called()

    def test_tracked_only_is_explicit_and_deduplicates_identical_snapshots(self):
        (self.repo / "new.txt").write_text("new\n")
        with patch.object(checks, "evaluate_checks", return_value={"check": drv("same")}) as evaluate:
            result, output, errors = self.invoke("--plan", "--tracked-only")
        self.assertEqual(result, 0, errors)
        report = json.loads(output)
        self.assertEqual(report["head"]["excluded_untracked"], ["new.txt"])
        self.assertFalse(report["head"]["include_untracked"])
        self.assertEqual(report["build_command"], [])
        evaluate.assert_called_once()

    def test_build_failure_cannot_emit_success_json(self):
        with patch.object(checks, "evaluate_checks", return_value={"check": drv("a")}), \
                patch.object(checks, "execute_plan", side_effect=checks.CommandError(["nix", "build"], 17)):
            result, output, errors = self.invoke()
        self.assertEqual(result, 17)
        self.assertEqual(output, "")
        self.assertIn("exited 17", errors)

    def test_head_movement_during_snapshot_fails_before_evaluation(self):
        original = checks.snapshot_working

        def snapshot(*args, **kwargs):
            result = original(*args, **kwargs)
            self.run_git("commit", "--quiet", "--allow-empty", "-m", "concurrent commit")
            return result

        with patch.object(checks, "snapshot_working", side_effect=snapshot), \
                patch.object(checks, "evaluate_checks") as evaluate:
            result, output, errors = self.invoke("--plan")
        self.assertEqual(result, 1)
        self.assertEqual(output, "")
        self.assertIn("HEAD changed", errors)
        evaluate.assert_not_called()

    def test_evaluation_failure_cannot_build_or_emit_success_json(self):
        with patch.object(checks, "evaluate_checks", side_effect=checks.CommandError(["nix", "eval"], 11)), \
                patch.object(checks, "execute_plan") as execute:
            result, output, errors = self.invoke()
        self.assertEqual(result, 11)
        self.assertEqual(output, "")
        self.assertIn("exited 11", errors)
        execute.assert_not_called()

    def test_failed_probe_json_is_failure_and_cli_exits_nonzero(self):
        manifest = self.root / "probes.json"
        manifest.write_text(json.dumps({"version": 1, "cases": [
            {"name": "failure", "mutations": [{"path": "tracked.txt", "append": "probe\n"}],
             "changed": ["check"]},
        ]}))
        with patch.object(checks, "evaluate_checks", return_value={"check": drv("same")}):
            result, output, errors = self.invoke("--probe", str(manifest))
        self.assertEqual(result, 1, errors)
        self.assertEqual(json.loads(output)["result"], "failed")


if __name__ == "__main__":
    unittest.main()
