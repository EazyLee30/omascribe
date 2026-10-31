"""Exercise the real evaluation loop with a bounded, model-free CLI boundary."""
import contextlib
import copy
import io
import json
import pathlib
import sqlite3
import tempfile
import unittest
from contextlib import closing
from types import SimpleNamespace
from unittest.mock import patch

import evaluate
from harness_support import read_json, write_json


class FixtureCli:
    def __init__(self, cursors, initial_cursor=0, make_document=False, fingerprint_only=False,
                 reset_cursor_on_save=False):
        self.cursors = iter(cursors)
        self.initial_cursor = initial_cursor
        self.make_document = make_document
        self.fingerprint_only = fingerprint_only
        self.reset_cursor_on_save = reset_cursor_on_save
        self.calls = []
        self.document = []

    def snapshot(self, root, mid):
        with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
            meeting = json.loads(db.execute("SELECT json FROM meetings WHERE id=?", (mid,)).fetchone()[0])
            segments = [json.loads(row[0]) for row in db.execute("SELECT json FROM segments WHERE meeting_id=? ORDER BY seq", (mid,))]
            notes = [json.loads(row[0]) for row in db.execute("SELECT json FROM notes WHERE meeting_id=?", (mid,))]
        return dict(meeting=meeting, segments=segments, notes=notes, suggestions=copy.deepcopy(self.document),
                    runtime=dict(notes_status="fixture observed outcome"), translation_mode="off", translations={})

    def seed(self, root, case, mid, automatic=False):
        evaluate.seed(root, case, mid, automatic)
        with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
            meeting = json.loads(db.execute("SELECT json FROM meetings WHERE id=?", (mid,)).fetchone()[0])
            meeting["notes_cursor"] = self.initial_cursor
            db.execute("UPDATE meetings SET json=? WHERE id=?", (json.dumps(meeting), mid))

    def invoke(self, argv, env, directory, timeout=650):
        name = argv[1]
        self.calls.append(name)
        root = pathlib.Path(env["OMASCRIBE_DATA_DIR"])
        if name == "diagnose":
            with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
                db.executescript("""
                    CREATE TABLE meetings(id TEXT PRIMARY KEY, json TEXT);
                    CREATE TABLE notes(id TEXT PRIMARY KEY, meeting_id TEXT, json TEXT);
                    CREATE TABLE segments(id TEXT PRIMARY KEY, meeting_id TEXT, run INTEGER, seq INTEGER, json TEXT);
                    CREATE TABLE inference_jobs(id INTEGER PRIMARY KEY, meeting_id TEXT);
                """)
            return {}
        mid = argv[2]
        if name in ("edit-note", "note"):
            with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
                note_id, encoded = db.execute("SELECT id,json FROM notes WHERE meeting_id=?", (mid,)).fetchone()
                note = json.loads(encoded)
                note["content"] = argv[4] if name == "edit-note" else argv[3]
                db.execute("UPDATE notes SET json=? WHERE id=?", (json.dumps(note), note_id))
                if self.reset_cursor_on_save:
                    meeting = json.loads(db.execute("SELECT json FROM meetings WHERE id=?", (mid,)).fetchone()[0])
                    meeting["notes_cursor"] = 0
                    db.execute("UPDATE meetings SET json=? WHERE id=?", (json.dumps(meeting), mid))
            return {}
        if name == "snapshot":
            return self.snapshot(root, mid)
        if name == "translate":
            # A preservation probe must not regenerate the previous article.
            write_json(directory / "process.json", dict(code=0, seconds=.1, resources={}))
            return self.snapshot(root, mid)
        if name not in ("generate", "settle"):
            raise AssertionError("Unexpected CLI command: " + name)
        before = self.snapshot(root, mid)
        before_cursor = before["meeting"]["notes_cursor"]
        after_cursor = next(self.cursors)
        with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
            meeting = before["meeting"]
            meeting["notes_cursor"] = after_cursor
            if self.fingerprint_only:
                meeting["notes_fingerprint"] = "changed-without-cursor-progress"
            db.execute("UPDATE meetings SET json=? WHERE id=?", (json.dumps(meeting), mid))
            db.execute("INSERT INTO inference_jobs(meeting_id) VALUES(?)", (mid,))
        if self.make_document:
            self.document = [dict(id="fixture-document", status="pending", text="Budget pending",
                                 sources=[dict(segment_id=s["id"], revision=s["revision"], quote=s["text"])
                                          for s in before["segments"]])]
        rejected = after_cursor == before_cursor and not self.make_document
        attempts = [dict(attempt=1, stage="compose", elapsed_ms=1000,
                         usage=dict(prompt_tokens=8, completion_tokens=3),
                         validation_error="fixture rejection" if rejected else None)]
        if rejected:
            attempts.append(dict(attempt=2, stage="repair", elapsed_ms=500,
                                 usage=dict(prompt_tokens=7, completion_tokens=2), validation_error="fixture rejection"))
        write_json(root / "last-generation.json", dict(attempts=attempts, committed=not rejected))
        write_json(directory / "process.json", dict(code=0, seconds=2.,
                   resources=dict(cpu_seconds_sampled=.5, peak_tree_rss_mib=20.)))
        return self.snapshot(root, mid)


class EvaluationProgressTests(unittest.TestCase):
    def run_case(self, case, cli, automatic=False):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            binary = root / "fixture-cli"
            binary.touch()
            args = SimpleNamespace(cli=binary, output=root / "report.json", artifacts=root / "artifacts",
                                   preferences=None, case=None, auto_schedule=automatic)
            def inputs(path):
                return [case] if pathlib.Path(path).name == "document-cases.json" else read_json(path)
            def isolated(workspace):
                workspace.mkdir(parents=True)
                return {"OMASCRIBE_DATA_DIR": str(workspace)}
            original_seed = evaluate.seed
            def seed(workspace, item, mid, automatic=False):
                # Use the actual seed and append/database behavior; only the CLI
                # boundary substitutes deterministic observed cursor outcomes.
                with patch.object(evaluate, "seed", original_seed):
                    cli.seed(workspace, item, mid, automatic)
            with patch.object(evaluate, "command", cli.invoke), patch.object(evaluate, "read_json", inputs), \
                 patch.object(evaluate, "isolated_env", isolated), patch.object(evaluate, "environment", return_value={}), \
                 patch.object(evaluate, "seed", seed), contextlib.redirect_stdout(io.StringIO()):
                passed = evaluate.evaluate(args)
            return passed, read_json(args.output)["cases"][0]

    @staticmethod
    def case(count=6):
        return dict(name="fictional-progress", focus="budget", transcript=["Budget pending"] * count)

    def test_initial_zero_cursor_rejection_stops_after_one_job_and_keeps_cost(self):
        cli = FixtureCli([0, 0])
        passed, result = self.run_case(self.case(), cli)
        self.assertFalse(passed)
        self.assertEqual(cli.calls.count("generate"), 1)
        self.assertEqual(result["coverage"], 0)
        self.assertIn("Phase 1: uncovered transcript", result["errors"])
        self.assertEqual(len(result["runs"]), 1)
        run = result["runs"][0]
        self.assertEqual(len(run["generation"]["attempts"]), 2)
        self.assertEqual(run["generation"]["attempts"][1]["validation_error"], "fixture rejection")
        self.assertEqual(run["process"]["seconds"], 2.)
        self.assertEqual(run["process"]["resources"]["cpu_seconds_sampled"], .5)

    def test_existing_cursor_rejection_stops_after_one_job(self):
        cli = FixtureCli([5, 5], initial_cursor=5)
        passed, result = self.run_case(self.case(), cli)
        self.assertFalse(passed)
        self.assertEqual(cli.calls.count("generate"), 1)
        self.assertEqual(result["coverage"], 5)
        self.assertIn("Phase 1: uncovered transcript", result["errors"])

    def test_automatic_settle_no_progress_does_not_start_another_settle_job(self):
        cli = FixtureCli([0, 0])
        passed, result = self.run_case(self.case(), cli, automatic=True)
        self.assertFalse(passed)
        self.assertEqual((cli.calls.count("settle"), cli.calls.count("generate")), (1, 0))
        self.assertEqual(len(result["runs"]), 1)
        self.assertEqual(len(result["runs"][0]["generation"]["attempts"]), 2)

    def test_progressing_batches_cover_all_sources_without_an_extra_job(self):
        cli = FixtureCli([5, 6], make_document=True)
        passed, result = self.run_case(self.case(), cli)
        self.assertTrue(passed)
        self.assertEqual(cli.calls.count("generate"), 2)
        self.assertEqual([run["coverage"] for run in result["runs"]], [5, 6])

    def test_fingerprint_only_update_still_gets_one_job_when_cursor_is_complete(self):
        cli = FixtureCli([1], initial_cursor=1, make_document=True, fingerprint_only=True)
        passed, result = self.run_case(self.case(1), cli)
        self.assertTrue(passed)
        self.assertEqual(cli.calls.count("generate"), 1)
        self.assertEqual(result["coverage"], 1)

    def test_new_phase_uses_cursor_observed_after_the_note_save(self):
        case = dict(name="fictional-reset-frontier", focus="budget", phases=[
            dict(transcript=["Budget pending"] * 5),
            dict(transcript=["Another pending item"],
                 save_note=dict(content="CedarLedger", kind="document", edit_existing_document=True)),
        ])
        cli = FixtureCli([5, 3, 6], make_document=True, reset_cursor_on_save=True)
        passed, result = self.run_case(case, cli)
        self.assertTrue(passed)
        self.assertEqual(cli.calls.count("generate"), 3)
        self.assertEqual([run["coverage"] for run in result["runs"]], [5, 3, 6])
        self.assertEqual(result["note_updates"][0]["jobs_before"], result["note_updates"][0]["jobs_after"])

    def test_run_generation_false_does_not_start_a_job_or_hide_uncovered_input(self):
        case = self.case(1)
        case.update(phases=[dict(transcript=case.pop("transcript"), run_generation=False)], expect_empty=True)
        cli = FixtureCli([])
        passed, result = self.run_case(case, cli)
        self.assertFalse(passed)
        self.assertEqual(cli.calls.count("generate"), 0)
        self.assertEqual(result["runs"], [])
        self.assertIn("Phase 1: uncovered transcript", result["errors"])

    def test_preservation_probe_runs_once_and_does_not_charge_the_retained_generation(self):
        case = dict(name="fictional-preservation", focus="budget", phases=[
            dict(transcript=["Budget pending"]),
            dict(transcript=[], save_note=dict(content="CedarLedger", kind="document", edit_existing_document=True),
                 preserve_minutes_generation=True),
        ])
        cli = FixtureCli([1], make_document=True)
        passed, result = self.run_case(case, cli)
        self.assertTrue(passed)
        self.assertEqual((cli.calls.count("generate"), cli.calls.count("translate")), (1, 1))
        preserved = result["minutes_preservation"][0]
        self.assertTrue(preserved["passed"])
        self.assertEqual((preserved["jobs_before"], preserved["jobs_after"]), (1, 1))
        self.assertEqual(result["runs"][1]["generation"]["attempts"], [])
        self.assertTrue(result["runs"][1]["generation"]["harness_retained_diagnostic"])


if __name__ == "__main__":
    unittest.main()
