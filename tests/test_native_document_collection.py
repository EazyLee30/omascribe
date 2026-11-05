"""Model-free controls for the installed-package document release gate."""
import hashlib
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("native_smoke", ROOT / "scripts/native-smoke.py")
SMOKE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SMOKE)


class DocumentCollectionTests(unittest.TestCase):
    def exercise(self, outcomes=None, *, budget=1200, duration=0, preexisting=False):
        outcomes = outcomes or {}
        calls = []
        now = [0.0]
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            results = root / "results"
            results.mkdir()
            cases = [dict(name="first"), dict(name="second")]
            fixture = root / "fixture.json"
            fixture.write_text(json.dumps(cases))
            digest = hashlib.sha256(fixture.read_bytes()).hexdigest()
            if preexisting:
                (results / "first.json").write_text("{}")

            def completed(argv, **kwargs):
                case = argv[argv.index("--case") + 1]
                calls.append((case, argv, kwargs))
                outcome = outcomes.get(case, {})
                if outcome.get("timeout"):
                    raise subprocess.TimeoutExpired(argv, kwargs["timeout"])
                code = outcome.get("code", 0)
                passed = code == 0
                row = dict(case=case, passed=passed, errors=[] if passed else ["No document"],
                           failure_kind=None if passed else "expectation",
                           runs=[dict(process=dict(code=0, failure=None))])
                report = dict(schema=3, fixture_sha256=digest, fixture_cases=[case],
                              passed=passed, cases=[row])
                row.update(outcome.get("row", {}))
                report.update(outcome.get("report", {}))
                output = pathlib.Path(argv[argv.index("--output") + 1])
                if not outcome.get("missing"):
                    output.write_text(json.dumps(report))
                now[0] += duration
                return subprocess.CompletedProcess(argv, code)

            with patch.object(SMOKE.subprocess, "run", side_effect=completed), \
                    patch.object(SMOKE.time, "monotonic", side_effect=lambda: now[0]):
                try:
                    SMOKE.run_document_cases(pathlib.Path("fixture-cli"), {}, results, cases,
                                             budget_seconds=budget, fixture_path=fixture)
                    failed_gate = False
                except RuntimeError:
                    failed_gate = True
            summary = json.loads((results / "document-suite.json").read_text())
            self.assertFalse((results / "document-suite.json.part").exists())
        return calls, failed_gate, summary

    def test_semantic_failure_collects_next_case_but_still_fails_release(self):
        calls, failed_gate, summary = self.exercise({"first": dict(code=1)})
        self.assertEqual([c[0] for c in calls], ["first", "second"])
        self.assertTrue(failed_gate)
        self.assertTrue(summary["complete"])
        self.assertFalse(summary["passed"])
        self.assertEqual([r["state"] for r in summary["cases"]], ["expectation_failed", "passed"])
        self.assertEqual(summary["unattempted_cases"], [])

    def test_all_passing_cases_run_once_and_pass(self):
        calls, failed_gate, summary = self.exercise()
        self.assertEqual([c[0] for c in calls], ["first", "second"])
        self.assertFalse(failed_gate)
        self.assertTrue(summary["complete"])
        self.assertTrue(summary["passed"])

    def test_multiple_semantic_failures_are_retained_without_retries(self):
        calls, failed_gate, summary = self.exercise({name: dict(code=1) for name in ("first", "second")})
        self.assertEqual([c[0] for c in calls], ["first", "second"])
        self.assertTrue(failed_gate)
        self.assertEqual([r["state"] for r in summary["cases"]], ["expectation_failed"] * 2)

    def test_execution_or_incomplete_report_stops_without_running_next_case(self):
        for outcome in [
            dict(code=2),
            dict(code=1, row=dict(failure_kind="execution")),
            dict(code=1, row=dict(runs=[dict(process=dict(code=1, failure=None))])),
            dict(code=1, row=dict(errors=[])),
            dict(code=1, row=dict(passed=True)),
            dict(code=0, row=dict(passed=False)),
            dict(report=dict(schema=2)),
            dict(report=dict(fixture_sha256="wrong")),
            dict(report=dict(fixture_cases=["second"])),
            dict(row=dict(case="second")),
            dict(report=dict(cases=[])),
            dict(missing=True),
        ]:
            with self.subTest(outcome=outcome):
                calls, failed_gate, summary = self.exercise({"first": outcome})
                self.assertEqual([c[0] for c in calls], ["first"])
                self.assertTrue(failed_gate)
                self.assertFalse(summary["complete"])
                self.assertEqual(summary["unattempted_cases"], ["second"])
                self.assertEqual(summary["stopped_reason"], "execution_or_report_failure")

    def test_timeout_is_not_a_semantic_failure_and_cannot_pass(self):
        calls, failed_gate, summary = self.exercise({"first": dict(timeout=True)})
        self.assertEqual([c[0] for c in calls], ["first"])
        self.assertTrue(failed_gate)
        self.assertEqual(summary["cases"][0]["state"], "timed_out")
        self.assertEqual(summary["unattempted_cases"], ["second"])

    def test_reported_internal_budget_stops_and_retains_unattempted_cases(self):
        for outcome in [dict(code=1, row=dict(failure_kind="budget")),
                        dict(code=1, report=dict(failure_kind="budget", cases=[]))]:
            with self.subTest(outcome=outcome):
                calls, failed_gate, summary = self.exercise({"first": outcome})
                self.assertEqual([c[0] for c in calls], ["first"])
                self.assertTrue(failed_gate)
                self.assertEqual(summary["stopped_reason"], "case_budget")
                self.assertEqual(summary["cases"][0]["state"], "budget_expired")
                self.assertEqual(summary["unattempted_cases"], ["second"])

    def test_collection_budget_bounds_case_and_does_not_start_next_case(self):
        calls, failed_gate, summary = self.exercise(budget=.5, duration=1)
        self.assertEqual([c[0] for c in calls], ["first"])
        self.assertTrue(failed_gate)
        self.assertEqual(calls[0][1][-2:], ["--budget-seconds", "0.5"])
        self.assertEqual(calls[0][2]["timeout"], 30.5)
        self.assertEqual(summary["stopped_reason"], "collection_budget")
        self.assertEqual(summary["unattempted_cases"], ["second"])

    def test_previous_case_report_is_not_reused(self):
        calls, failed_gate, summary = self.exercise(preexisting=True)
        self.assertEqual(calls, [])
        self.assertTrue(failed_gate)
        self.assertEqual(summary["stopped_reason"], "preexisting_case_report")
        self.assertEqual(summary["unattempted_cases"], ["first", "second"])

    def test_the_complete_current_fixture_is_required(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            fixture = root / "fixture.json"
            fixture.write_text(json.dumps([dict(name="first"), dict(name="second")]))
            with self.assertRaisesRegex(AssertionError, "complete current document fixture"):
                SMOKE.run_document_cases(pathlib.Path("fixture-cli"), {}, root / "results",
                                         [dict(name="first")], fixture_path=fixture)

    def test_real_evaluator_distinguishes_rejected_text_from_execution_failure(self):
        from test_evaluate_progress import EvaluationProgressTests, FixtureCli
        harness = EvaluationProgressTests()
        passed, result = harness.run_case(harness.case(), FixtureCli([0]))
        self.assertFalse(passed)
        self.assertEqual(result["failure_kind"], "expectation")

        class BrokenCli(FixtureCli):
            def invoke(self, argv, env, directory, timeout=650):
                if argv[1] == "generate":
                    raise RuntimeError("fixture command did not complete")
                return super().invoke(argv, env, directory, timeout)

        passed, result = harness.run_case(harness.case(), BrokenCli([]))
        self.assertFalse(passed)
        self.assertEqual(result["failure_kind"], "execution")


class EvaluationBudgetTests(unittest.TestCase):
    def test_metadata_cli_probe_uses_remaining_budget_and_skips_when_exhausted(self):
        import harness_support
        with tempfile.TemporaryDirectory() as folder:
            cli = pathlib.Path(folder) / "fixture-cli"
            cli.touch()
            for budget, expected_calls in [(.25, 1), (0, 0)]:
                with self.subTest(budget=budget), \
                        patch.object(harness_support.subprocess, "run", return_value=SimpleNamespace(returncode=1)) as run, \
                        patch.object(harness_support.subprocess, "check_output", return_value="fixture"), \
                        patch.object(harness_support.platform, "platform", return_value="fixture"), \
                        patch.object(harness_support, "source_digest", return_value="fixture"):
                    observed = harness_support.environment(cli, build_timeout=budget)
                    self.assertIsNone(observed["cli_build"])
                    self.assertEqual(run.call_count, expected_calls)
                    if expected_calls:
                        self.assertEqual(run.call_args.kwargs["timeout"], budget)

    @unittest.skipIf(os.name == "nt", "Real POSIX process-group cleanup control; Windows uses existing taskkill tree path")
    def test_expired_case_kills_generation_tree_and_does_not_spawn_failure_snapshot(self):
        import evaluate
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            cli = root / "fixture-cli"
            cli.write_text("#!" + sys.executable + "\n" + textwrap.dedent('''
                import json, os, pathlib, sqlite3, subprocess, sys, time
                root=pathlib.Path(os.environ['OMASCRIBE_DATA_DIR'])
                name=sys.argv[1]
                with pathlib.Path(os.environ['FIXTURE_CALLS']).open('a') as out: out.write(name+'\\n')
                if name=='diagnose':
                    with sqlite3.connect(root/'meetings.sqlite3') as db:
                        db.executescript('CREATE TABLE meetings(id TEXT PRIMARY KEY,json TEXT); CREATE TABLE notes(id TEXT PRIMARY KEY,meeting_id TEXT,json TEXT); CREATE TABLE segments(id TEXT PRIMARY KEY,meeting_id TEXT,run INTEGER,seq INTEGER,json TEXT); CREATE TABLE inference_jobs(id INTEGER PRIMARY KEY,meeting_id TEXT);')
                    print('{}')
                elif name=='snapshot':
                    with sqlite3.connect(root/'meetings.sqlite3') as db:
                        meeting=json.loads(db.execute('SELECT json FROM meetings WHERE id=?',(sys.argv[2],)).fetchone()[0])
                        notes=[json.loads(r[0]) for r in db.execute('SELECT json FROM notes')]
                        segments=[json.loads(r[0]) for r in db.execute('SELECT json FROM segments')]
                    print(json.dumps(dict(meeting=meeting,notes=notes,segments=segments,suggestions=[],runtime=dict(notes_status='fixture'),translation_mode='off',translations={})))
                elif name=='generate':
                    child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])
                    pathlib.Path(os.environ['FIXTURE_PIDS']).write_text(json.dumps([os.getpid(),child.pid]))
                    time.sleep(60)
            '''))
            cli.chmod(0o755)
            calls = root / "calls.txt"
            pids = root / "pids.json"
            args = SimpleNamespace(cli=cli, output=root / "result.json", artifacts=root / "artifacts",
                                   preferences=None, case=None, auto_schedule=False, budget_seconds=2.5)
            original_read = evaluate.read_json
            original_env = evaluate.isolated_env
            fixture = dict(name="bounded-case", focus="budget", transcript=["Budget pending"])
            def read(path):
                return [fixture] if pathlib.Path(path).name == "document-cases.json" else original_read(path)
            def env(workspace):
                return dict(original_env(workspace), FIXTURE_CALLS=str(calls), FIXTURE_PIDS=str(pids))
            with patch.object(evaluate, "read_json", side_effect=read), \
                    patch.object(evaluate, "isolated_env", side_effect=env), \
                    patch.object(evaluate, "environment", return_value={}):
                started = time.monotonic()
                self.assertFalse(evaluate.evaluate(args))
                self.assertLess(time.monotonic() - started, 10)
            result = json.loads(args.output.read_text())["cases"][0]
            self.assertEqual(result["failure_kind"], "budget")
            self.assertEqual(calls.read_text().splitlines(), ["diagnose", "snapshot", "generate"])
            self.assertEqual(result["runs"][0]["process"]["failure"], "TimeoutExpired")
            self.assertTrue(pids.is_file(), "generation must actually start before cancellation")
            observed = []
            for pid in json.loads(pids.read_text()):
                for _ in range(20):
                    status = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
                    if not status or status.startswith("Z"):
                        break
                    time.sleep(.05)
                self.assertTrue(not status or status.startswith("Z"), f"fixture process still running: {pid}")
                observed.append(dict(pid=pid, terminal_state=status or "gone"))
            print("process-group cleanup control:", json.dumps(observed), flush=True)


if __name__ == "__main__":
    unittest.main()
