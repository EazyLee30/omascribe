#!/usr/bin/env python3
"""Real local minutes regression: phased evidence, exact citations, retained failure artifacts.
Fictional inputs only. Pattern checks catch targeted regressions, not all semantic errors.
"""
import argparse, datetime, json, pathlib, re, shutil, sqlite3, time, uuid
from contextlib import closing
from harness_support import REPO, command, environment, isolated_env, read_json, sha256, write_json


def assess(case, snapshot):
    sections = [s for s in snapshot["suggestions"] if s["status"] != "dismissed"]
    text = "\n".join(s.get("focus", "") + " " + s["text"] for s in sections)
    errors = []
    if not sections and not case.get("expect_empty"):
        errors.append("No document: " + snapshot["runtime"]["notes_status"])
    if sections and case.get("expect_empty"): errors.append("Non-substantive audio produced minutes")
    for expression in case.get("required", []):
        if not re.search(expression, text, re.I): errors.append("Missing: " + expression)
    for expression in case.get("forbidden", []):
        if re.search(expression, text, re.I): errors.append("Unsupported claim: " + expression)
    if len(sections) > case.get("max_sections", len(sections)):
        errors.append("Related material fragmented into too many paragraphs")
    if case.get("merged_sources") and not any(len(s["sources"]) > 1 for s in sections):
        errors.append("Related evidence was not synthesized")
    for section in sections:
        if not section["sources"]: errors.append("Missing source")
        for source in section["sources"]:
            if not any(t["id"] == source["segment_id"] and t["revision"] == source["revision"]
                and t["text"] == source["quote"] for t in snapshot["segments"]):
                errors.append("Invalid evidence link")
    return errors


def seed(root, case, mid):
    now = datetime.datetime.now(datetime.timezone.utc).isoformat()
    meeting = dict(id=mid, title=case["name"], created_at=now, status="ready", duration_ms=0,
        run=1, notes_cursor=0, auto_notes=False, notes_fingerprint="", speaker_names=case.get("speakers", {}))
    with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
        db.execute("INSERT INTO meetings VALUES(?,?)", (mid, json.dumps(meeting)))
        note = dict(id=str(uuid.uuid4()), meeting_id=mid, kind="document", content=case["focus"], anchor_ms=0, updated_at=now)
        db.execute("INSERT INTO notes VALUES(?,?,?)", (note["id"], mid, json.dumps(note)))
    for folder in ["audio", "files"]: (root / "meetings" / mid / folder).mkdir(parents=True)


def append(root, mid, passages, seq):
    with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
        for passage in passages:
            seq += 1
            passage = dict(text=passage) if isinstance(passage, str) else passage
            sid = f"{mid}:1:{seq}"
            segment = dict(id=sid, meeting_id=mid, run=1, seq=seq, start_ms=(seq-1)*10000,
                end_ms=seq*10000, revision=1, speaker_manual=False, **passage)
            db.execute("INSERT INTO segments VALUES(?,?,?,?,?)", (sid, mid, 1, seq, json.dumps(segment)))
        meeting = json.loads(db.execute("SELECT json FROM meetings WHERE id=?", (mid,)).fetchone()[0])
        meeting["duration_ms"] = seq*10000
        db.execute("UPDATE meetings SET json=? WHERE id=?", (json.dumps(meeting), mid))
    return seq


def evaluate(args):
    cli, output = args.cli.resolve(), args.output.resolve()
    artifacts = args.artifacts.resolve() if args.artifacts else output.with_suffix(".artifacts")
    if artifacts.exists(): raise ValueError(f"Use a fresh artifacts directory: {artifacts}")
    root = artifacts / "workspace"
    env = isolated_env(root)
    base_preferences = read_json(args.preferences) if args.preferences else {}
    write_json(root / "preferences.json", base_preferences)
    cases = read_json(REPO / "tests/fixtures/document-cases.json")
    if args.case:
        cases = [c for c in cases if c["name"] == args.case]
        if not cases: raise ValueError("Unknown fixture")
    report = dict(schema=2, passed=False, environment=environment(cli),
        fixture_sha256=sha256(REPO / "tests/fixtures/document-cases.json"), cases=[])
    command([cli, "diagnose"], env, artifacts / "diagnose")
    for case in cases:
        folder = artifacts / case["name"]
        write_json(folder / "input.json", case)
        write_json(root / "preferences.json", {**base_preferences, **case.get("preferences", {})})
        started, mid, seq, runs, errors = time.monotonic(), str(uuid.uuid4()), 0, [], []
        snapshot = None
        protected = []
        try:
            seed(root, case, mid)
            phases = case.get("phases", [dict(transcript=case.get("transcript", []))])
            for number, phase in enumerate(phases, 1):
                seq = append(root, mid, phase["transcript"], seq)
                cursor = -1
                for batch in range(seq+1):
                    generation = folder / f"phase-{number}" / f"batch-{batch+1}"
                    diagnostic_names = ["last-generation.json", "last-writer-resource.json", "llm-runtime.log"]
                    for name in diagnostic_names: (root / name).unlink(missing_ok=True)
                    try:
                        snapshot = command([cli, "generate", mid], env, generation)
                    finally:
                        for name in diagnostic_names:
                            if (root / name).exists(): shutil.copy2(root / name, generation / name)
                    runs.append(dict(phase=number, batch=batch+1, status=snapshot["runtime"]["notes_status"],
                        coverage=snapshot["meeting"]["notes_cursor"], generation=read_json(generation / "last-generation.json"),
                        process=read_json(generation / "process.json")))
                    next_cursor = snapshot["meeting"]["notes_cursor"]
                    if next_cursor >= seq or next_cursor <= cursor: break
                    cursor = next_cursor
                for original in protected:
                    if not any(s == original for s in snapshot["suggestions"]):
                        errors.append(f"Phase {number}: protected human paragraph was modified")
                if phase.get("protect_first"):
                    if not snapshot["suggestions"]: raise ValueError("No paragraph available to protect")
                    original = dict(snapshot["suggestions"][0], status="accepted", text=phase["protect_first"])
                    with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
                        db.execute("UPDATE suggestions SET json=? WHERE id=?",(json.dumps(original),original["id"]))
                    protected.append(original)
                if snapshot["meeting"]["notes_cursor"] < seq: errors.append(f"Phase {number}: uncovered transcript")
                errors.extend(f"Phase {number}: {e}" for e in assess(phase.get("expect", {}), snapshot)
                    if "expect" in phase)
            errors.extend(assess(case, snapshot))
        except Exception as error:
            errors.append(f"{type(error).__name__}: {error}")
            try: snapshot = command([cli, "snapshot", mid], env, folder / "failure-snapshot", timeout=30)
            except Exception: pass
        result = dict(case=case["name"], passed=not errors, errors=errors,
            seconds=round(time.monotonic()-started,3), runs=runs,
            document=snapshot["suggestions"] if snapshot else [],
            coverage=snapshot["meeting"]["notes_cursor"] if snapshot else 0,
            status=snapshot["runtime"]["notes_status"] if snapshot else "failed")
        report["cases"].append(result)
        report["passed"] = all(r["passed"] for r in report["cases"])
        write_json(output, report)
        print(case["name"], "PASS" if not errors else errors, flush=True)
    return report["passed"]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cli", type=pathlib.Path, default=REPO / "target/debug/omascribe-cli")
    p.add_argument("--output", type=pathlib.Path, default=pathlib.Path("/tmp/omascribe-evaluation.json"))
    p.add_argument("--artifacts", type=pathlib.Path)
    p.add_argument("--case")
    p.add_argument("--preferences", type=pathlib.Path)
    raise SystemExit(0 if evaluate(p.parse_args()) else 1)

if __name__ == "__main__": main()
