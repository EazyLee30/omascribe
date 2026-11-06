#!/usr/bin/env python3
"""Opt-in real local-model regression for long translation, using fictional text only.

Set OMASCRIBE_MODELS_DIR and OMASCRIBE_RUNTIME_DIR to verified local resources.
No user workspace is opened and no download is requested. This is not a speed benchmark.
"""
import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import tempfile
import time
import uuid

REPO = pathlib.Path(__file__).resolve().parents[1]
CLI = REPO / "target/debug" / ("omascribe-cli.exe" if os.name == "nt" else "omascribe-cli")


def main():
    with tempfile.TemporaryDirectory(prefix="omascribe-long-translation-") as folder:
        root = pathlib.Path(folder)
        env = dict(os.environ, OMASCRIBE_DATA_DIR=folder, OMASCRIBE_TRANSLATION_DIAGNOSTICS="1")
        (root / "preferences.json").write_text(json.dumps(dict(translation_mode="bilingual", auto_notes=False)))
        (root / "network-policy.json").write_text(json.dumps(dict(allow_model_downloads=False, allow_online_recommendations=False)))

        def cli(*args):
            return json.loads(subprocess.check_output([str(CLI), *args], env=env, text=True, encoding="utf-8", timeout=180))

        cli("diagnose")
        meeting = str(uuid.uuid4())
        long_source = "预算3000尚未批准，Morgan将在周四发送报告，我们计划周五继续讨论。" * 45
        originals = [long_source, "The launch date is not approved. Morgan will send the report on Thursday.", "预算还没有确定，周五再讨论。"]
        info = dict(id=meeting, title="Fictional long translation", created_at="2026-10-26T00:00:00Z", status="ready", duration_ms=30000, run=1, notes_cursor=0, auto_notes=False)
        ids = [str(uuid.uuid4()) for _ in originals]
        with sqlite3.connect(root / "meetings.sqlite3") as db:
            db.execute("INSERT INTO meetings VALUES(?,?)", (meeting, json.dumps(info)))
            for seq, (sid, text) in enumerate(zip(ids, originals), 1):
                source = dict(id=sid, meeting_id=meeting, run=1, seq=seq, start_ms=seq*1000, end_ms=(seq+1)*1000, text=text, revision=1)
                db.execute("INSERT INTO segments VALUES(?,?,?,?,?)", (sid, meeting, 1, seq, json.dumps(source)))
        started = time.monotonic()
        batches, previous_end = [], 0
        for attempt in range(8):
            try:
                snapshot = cli("translate", meeting)
            except subprocess.CalledProcessError:
                destination = REPO / "artifacts/benchmarks/translation-chunks-failure"
                destination.mkdir(parents=True, exist_ok=True)
                for name in ["inference-trace.jsonl", "translation-diagnostic.json", "preferences.json", "llm-runtime.log"]:
                    if (root / name).is_file():
                        shutil.copy2(root / name, destination / name)
                (destination / "sources.json").write_text(json.dumps(originals, ensure_ascii=False), encoding="utf-8")
                print("Synthetic failure evidence saved at", destination)
                raise
            assert [s["text"] for s in snapshot["segments"]] == originals
            trace = [json.loads(line) for line in (root / "inference-trace.jsonl").read_text().splitlines()]
            terminal = [r["metrics"] for r in trace if r["phase"] == "batch"][-1]
            assert terminal["requests"] <= 8, terminal
            with sqlite3.connect(root / "meetings.sqlite3") as db:
                row = db.execute("SELECT json FROM translation_parts WHERE segment_id=?", (ids[0],)).fetchone()
            if row:
                progress = json.loads(row[0])
                assert previous_end < progress["end"] < len(long_source.encode("utf-8"))
                assert ids[0] not in snapshot["translations"], "A partial translation must remain private"
                previous_end = progress["end"]
            batches.append(dict(attempt=attempt+1, **terminal))
            if attempt == 0:
                assert row, "Fixture must require a resumed batch"
                assert ids[1] in snapshot["translations"] and ids[2] in snapshot["translations"], "Short captions must not wait for the long passage"
            if len(snapshot["translations"]) == len(originals):
                assert row is None
                break
        else:
            raise AssertionError("Long passage did not complete within the bounded harness")
        result = snapshot["translations"][ids[0]]["text"]
        assert "3000" in result and "thursday" in result.lower() and "friday" in result.lower() and "not" in result.lower(), result
        chunks = [r for r in trace if r["phase"] == "chunk"]
        assert any(r["metrics"]["resumed"] for r in chunks)
        assert all(r["metrics"]["input_tokens"] <= 256 for r in chunks)
        before = len(chunks)
        assert len(cli("translate", meeting)["translations"]) == len(originals)
        after = [json.loads(line) for line in (root / "inference-trace.jsonl").read_text().splitlines()]
        assert sum(r["phase"] == "chunk" for r in after) == before
        report = dict(schema=1, passed=True, kind="long-translation-regression", build=cli("build-info"), elapsed_seconds=round(time.monotonic()-started,2), source_characters=len(long_source), batches=batches, chunks=len(chunks), checks=["token and request budgets", "long passage resumes between CLI processes", "short captions progress in first batch", "partial output stays private", "source and numeric/weekday/negation checks", "completed results start no new model request"], limitation="Synthetic repeated text; no semantic completeness score or overall performance claim")
        output = REPO / "artifacts/benchmarks/translation-chunks.json"
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
        print(json.dumps(report, ensure_ascii=False))


if __name__ == "__main__":
    main()
