#!/usr/bin/env python3
"""Replay frozen public ASR outputs through production terminology correction.

No audio recognition or model inference. Never opens the original workspaces.
Retains every before/after text and uses a caller-declared change allowlist;
passing this regression does not grant ASR quality or release admission.
"""
import argparse
import json
import os
import pathlib
import sqlite3
import subprocess
import tempfile
from contextlib import closing

from evaluate import seed
from evaluate_name_bias import ARMS, PREFERENCES, score
from harness_support import read_json, sha256, write_json


def replay(cli, case, arm):
    with tempfile.TemporaryDirectory(prefix="omascribe-name-replay-") as folder:
        root = pathlib.Path(folder)
        env = {k: v for k, v in os.environ.items() if not k.startswith("OMASCRIBE_")}
        env.update(OMASCRIBE_DATA_DIR=str(root),
                   OMASCRIBE_MODELS_DIR=str(root / "no-models"),
                   OMASCRIBE_RUNTIME_DIR=str(root / "no-runtime"))
        write_json(root / "preferences.json", PREFERENCES)
        write_json(root / "network-policy.json",
                   dict(allow_model_downloads=False, allow_online_recommendations=False))

        def invoke(*args):
            result = subprocess.run([str(cli), *args], env=env, check=True,
                                    capture_output=True, text=True, encoding="utf-8", timeout=30)
            return json.loads(result.stdout)

        invoke("diagnose")
        mid = arm["meeting_id"]
        seed(root, dict(name=case["name"], focus=arm["note"]), mid)
        with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
            for passage in arm["raw"]:
                assert passage["meeting_id"] == mid and passage["revision"] == 1
                db.execute("INSERT INTO segments VALUES(?,?,?,?,?)",
                           (passage["id"], mid, passage["run"], passage["seq"],
                            json.dumps(passage, ensure_ascii=False)))
        invoke("correct-terms", mid)
        with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
            corrected = [json.loads(row[0]) for row in db.execute(
                "SELECT json FROM segments WHERE meeting_id=? ORDER BY run,seq", (mid,))]
            assert len(corrected) == len(arm["raw"])
            for original, current in zip(arm["raw"], corrected):
                assert all(current[k] == original[k] for k in
                           ("id", "meeting_id", "run", "seq", "start_ms", "end_ms", "speaker"))
                if current["text"] != original["text"]:
                    archived = db.execute(
                        "SELECT json FROM segment_history WHERE id=? AND revision=1",
                        (original["id"],)).fetchone()
                    assert archived and json.loads(archived[0]) == original
            assert db.execute("SELECT COUNT(*) FROM inference_jobs").fetchone()[0] == 0
            assert db.execute("SELECT COUNT(*) FROM translations").fetchone()[0] == 0
        assert invoke("correct-terms", mid)["corrected"] == 0, "Unchanged replay must be idempotent"
        text = " ".join(p["text"] for p in corrected)
        return dict(case=case["name"], arm=arm["arm"], note=arm["note"],
                    raw=arm["raw"], previous=arm["corrected"], current=corrected,
                    previous_text=arm["corrected_text"], current_text=text,
                    changed=[p["text"] for p in corrected] != [p["text"] for p in arm["corrected"]],
                    previous_metrics=score(arm["corrected_text"], case),
                    current_metrics=score(text, case))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=pathlib.Path, required=True)
    parser.add_argument("--report", type=pathlib.Path, required=True)
    parser.add_argument("--manifest", type=pathlib.Path, required=True)
    parser.add_argument("--expect-change", action="append", default=[], metavar="CASE:ARM")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Use a fresh output path")
    cli = args.cli.resolve()
    helpers = [pathlib.Path(__file__).with_name(name) for name in (
        "replay_name_corrections.py", "evaluate.py", "evaluate_name_bias.py",
        "evaluate_asr.py", "prepare_name_bias_fixtures.py", "harness_support.py")]
    hashes = {str(p.resolve()): sha256(p) for p in (cli, args.report, args.manifest, *helpers)}
    source, manifest = read_json(args.report), read_json(args.manifest)
    cases = {case["name"]: case for case in manifest["cases"]}
    assert source["execution_passed"] and source["full_corpus_complete"] and source["identities_unchanged"]
    assert len(cases) == len(manifest["cases"])
    for key in ("identity_before", "identity_after"):
        assert source[key]["assets"]["selection"]["sha256"] == sha256(args.manifest)
    assert source["selection_metadata"] == {k: v for k, v in manifest.items() if k not in ("schema", "cases")}
    assert len(source["cases"]) == len(cases)
    assert {row["case"] for row in source["cases"]} == set(cases)
    report = dict(schema=1, kind="frozen-asr-terminology-replay", passed=False,
                  quality_admission=False, source_hashes=hashes,
                  cli_build=json.loads(subprocess.check_output(
                      [str(cli), "build-info"], text=True, encoding="utf-8", timeout=30)),
                  expected_changes=sorted(args.expect_change), results=[],
                  limitation="Deterministic spelling replay only. No new ASR/LLM calls, acoustic quality, resource or release claim.")
    try:
        for row in source["cases"]:
            case = cases[row["case"]]
            assert row["cluster_id"] == case["sentence_id"]
            assert all(row[k] == case[k] for k in ("reference", "raw_reference", "name_kind"))
            assert len(row["arms"]) == len(ARMS)
            assert {arm["arm"] for arm in row["arms"]} == set(ARMS)
            for arm in row["arms"]:
                assert arm["passed"]
                expected_note = "" if arm["arm"] == "baseline" else case[arm["arm"]]["note"]
                assert arm["note"] == expected_note
                assert arm["raw"] and len(arm["raw"]) == len(arm["corrected"])
                for original, corrected in zip(arm["raw"], arm["corrected"]):
                    assert all(original[k] == corrected[k] for k in
                               ("id", "meeting_id", "run", "seq", "start_ms", "end_ms"))
                for key in ("raw", "corrected"):
                    assert arm[key + "_text"] == " ".join(s["text"] for s in arm[key])
                report["results"].append(replay(cli, case, arm))
            write_json(args.output, report)
        changed = sorted(f'{r["case"]}:{r["arm"]}' for r in report["results"] if r["changed"])
        report["actual_changes"] = changed
        assert changed == sorted(args.expect_change), f"Unexpected changed set: {changed}"
        for row in report["results"]:
            if row["changed"]:
                assert row["arm"] == "positive", "Baseline and unspoken-name controls must remain unchanged"
                assert row["current_metrics"]["expected_name_present"]
                assert row["current_metrics"]["expected_exact_spelling_present"]
        assert hashes == {path: sha256(pathlib.Path(path)) for path in hashes}
        for arm in ARMS:
            rows = [r for r in report["results"] if r["arm"] == arm]
            report[arm] = {layer: dict(
                recalled=sum(r[layer]["expected_name_present"] for r in rows),
                forbidden=sum(r[layer]["forbidden_name_present"] for r in rows),
                cases=len(rows)) for layer in ("previous_metrics", "current_metrics")}
            assert all(not r["previous_metrics"]["expected_name_present"]
                       or r["current_metrics"]["expected_name_present"] for r in rows)
            assert all(not r["current_metrics"]["forbidden_name_present"]
                       or r["previous_metrics"]["forbidden_name_present"] for r in rows)
        report["passed"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        write_json(args.output, report)
    print(json.dumps({k: v for k, v in report.items() if k != "results"}, ensure_ascii=False))


if __name__ == "__main__":
    main()
