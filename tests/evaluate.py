#!/usr/bin/env python3
"""Real local minutes regression: phased evidence, exact citations, retained failure artifacts.
Fictional inputs only. Pattern checks catch targeted regressions, not all semantic errors.
"""
import argparse, datetime, json, os, pathlib, re, shutil, sqlite3, time, uuid
from contextlib import closing
from harness_support import suite_passed, REPO, command, environment, isolated_env, read_json, sha256, write_json


def assess(case, snapshot, check_translations=False):
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
    for value in case.get("required_verbatim", []):
        if value not in text: errors.append("Missing exact spelling: " + value)
    for value in case.get("forbidden_verbatim", []):
        if value in text: errors.append("Old exact spelling: " + value)
    transcript = "\n".join(s["text"] for s in snapshot["segments"])
    for expression in case.get("required_transcript", []):
        if not re.search(expression, transcript): errors.append("Transcript missing canonical name: " + expression)
    for expression in case.get("forbidden_transcript", []):
        if re.search(expression, transcript): errors.append("Uncorrected transcript: " + expression)
    if check_translations:
        for expected in case.get("translation_checks", []):
            source = next((s for s in snapshot["segments"] if s["seq"] == expected["seq"]), None)
            translated = snapshot.get("translations", {}).get(source["id"]) if source else None
            if not translated:
                errors.append(f"Missing translation for source {expected['seq']}")
                continue
            if translated["revision"] != source["revision"] or translated["target"] != expected["target"]:
                errors.append(f"Wrong translation revision/target for source {expected['seq']}")
            if "revision" in expected and source["revision"] != expected["revision"]:
                errors.append(f"Wrong source revision for translation {expected['seq']}")
            for expression in expected.get("required", []):
                if not re.search(expression, translated["text"], re.I):
                    errors.append(f"Translation {expected['seq']} missing: {expression}")
            for expression in expected.get("forbidden", []):
                if re.search(expression, translated["text"], re.I):
                    errors.append(f"Translation {expected['seq']} unsupported: {expression}")
            for value in expected.get("required_verbatim", []):
                if value not in translated["text"]:
                    errors.append(f"Translation {expected['seq']} missing exact spelling: {value}")
            for value in expected.get("forbidden_verbatim", []):
                if value in translated["text"]:
                    errors.append(f"Translation {expected['seq']} old exact spelling: {value}")
    if len(sections) > case.get("max_sections", len(sections)):
        errors.append("Related material fragmented into too many paragraphs")
    if case.get("max_body_words") and sum(len(s["text"].split()) for s in sections) > case["max_body_words"]:
        errors.append("Short evidence expanded into an overlong document")
    if case.get("merged_sources") and not any(len(s["sources"]) > 1 for s in sections):
        errors.append("Related evidence was not synthesized")
    for section in sections:
        if not section["sources"]: errors.append("Missing source")
        for source in section["sources"]:
            if not any(t["id"] == source["segment_id"] and t["revision"] == source["revision"]
                and t["text"] == source["quote"] for t in snapshot["segments"]):
                errors.append("Invalid evidence link")
    return errors


def seed(root, case, mid, automatic=False):
    now = datetime.datetime.now(datetime.timezone.utc).isoformat()
    meeting = dict(id=mid, title=case["name"], created_at=now, status="ready", duration_ms=0,
        run=1, notes_cursor=0, auto_notes=automatic, notes_fingerprint="", speaker_names=case.get("speakers", {}))
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
    report = dict(schema=3, passed=False, environment=environment(cli),
        preferences=base_preferences, fixture_cases=[c["name"] for c in cases],
        fixture_sha256=sha256(REPO / "tests/fixtures/document-cases.json"), cases=[])
    automatic = getattr(args, "auto_schedule", False)
    report["execution"] = "automatic-scheduler" if automatic else "explicit-generation"
    report["catchup_enabled"] = os.environ.get("OMASCRIBE_MINUTES_CATCHUP") == "1"
    report["incremental_enabled"] = os.environ.get("OMASCRIBE_INCREMENTAL_MINUTES") == "1"
    command([cli, "diagnose"], env, artifacts / "diagnose")
    for case in cases:
        folder = artifacts / case["name"]
        write_json(folder / "input.json", case)
        write_json(root / "preferences.json", {**base_preferences, **case.get("preferences", {})})
        started, mid, seq, runs, errors = time.monotonic(), str(uuid.uuid4()), 0, [], []
        note_updates = []
        minutes_preservation = []
        snapshot = None
        protected = []
        try:
            seed(root, case, mid, automatic)
            phases = case.get("phases", [dict(transcript=case.get("transcript", []))])
            for number, phase in enumerate(phases, 1):
                seq = append(root, mid, phase["transcript"], seq)
                preserve = phase.get("preserve_minutes_generation", False)
                if preserve and (not phase.get("run_generation", True) or "save_note" not in phase):
                    raise ValueError("preserve_minutes_generation requires save_note and generation")
                if "save_note" in phase:
                    note = phase["save_note"]
                    watched = ["last-generation.json", "llm-runtime.log", "inference-trace.jsonl", "inference-trace.previous.jsonl"]
                    before = {name: sha256(root/name) for name in watched if (root/name).is_file()}
                    with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
                        jobs_before = db.execute("SELECT COUNT(*) FROM inference_jobs").fetchone()[0]
                    if note.get("edit_existing_document", False):
                        current = command([cli, "snapshot", mid], env, folder/f"phase-{number}"/"snapshot-before-note")
                        document = next((n for n in current["notes"] if n["kind"] == "document"), None)
                        if document is None: raise ValueError("No active document to edit")
                        argv = [cli, "edit-note", mid, document["id"], note["content"], note.get("kind", "document")]
                    else:
                        argv = [cli, "note", mid, note["content"], note.get("kind", "document")]
                    command(argv, env, folder/f"phase-{number}"/"save-note")
                    after = {name: sha256(root/name) for name in watched if (root/name).is_file()}
                    with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
                        jobs_after = db.execute("SELECT COUNT(*) FROM inference_jobs").fetchone()[0]
                    no_generation = before == after and jobs_before == jobs_after
                    note_updates.append(dict(phase=number, no_generation=no_generation, jobs_before=jobs_before,
                                             jobs_after=jobs_after, diagnostic_hashes_before=before, diagnostic_hashes_after=after))
                    if not no_generation: errors.append(f"Phase {number}: spelling save started generation")
                if preserve:
                    saved = command([cli, "snapshot", mid], env, folder/f"phase-{number}"/"snapshot-after-note")
                    errors.extend(f"Phase {number} after save: {e}" for e in assess(phase.get("expect", {}), saved))
                    with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
                        saved_jobs = db.execute("SELECT COUNT(*) FROM inference_jobs WHERE meeting_id=?", (mid,)).fetchone()[0]
                    if not saved_jobs or not (root / "last-generation.json").is_file():
                        raise ValueError("No prior minutes generation to preserve")
                    preservation = dict(phase=number, passed=False, jobs_before=saved_jobs,
                        execution="automatic-settle" if automatic else "explicit-translate",
                        automatic_scheduler_verified=False,
                        translation_mode=saved.get("translation_mode", "off"), captions_verified=False,
                        last_generation_sha256_before=sha256(root / "last-generation.json"),
                        document_before=saved["suggestions"])
                    minutes_preservation.append(preservation)
                cursor = -1
                if phase.get("run_generation", True) is False:
                    snapshot = command([cli, "snapshot", mid], env, folder/f"phase-{number}"/"snapshot-after-note")
                for batch in range(seq+1 if phase.get("run_generation", True) else 0):
                    generation = folder / f"phase-{number}" / f"batch-{batch+1}"
                    diagnostic_names = ["last-generation.json", "last-writer-resource.json", "last-metal-fallback.json", "llm-runtime.log", "inference-trace.jsonl", "inference-trace.previous.jsonl"]
                    for name in diagnostic_names:
                        if not (preserve and name == "last-generation.json"):
                            (root / name).unlink(missing_ok=True)
                    snapshot = None
                    try:
                        argv = [cli, "settle", mid, 120] if automatic else [cli, "translate" if preserve else "generate", mid]
                        snapshot = command(argv, env, generation)
                    finally:
                        for name in diagnostic_names:
                            if (root / name).exists(): shutil.copy2(root / name, generation / name)
                        generated = read_json(generation / "last-generation.json")
                        if preserve and generated and sha256(generation / "last-generation.json") == preservation["last_generation_sha256_before"]:
                            # Preserve raw evidence without charging the prior minutes attempts twice.
                            generated = dict(model=generated.get("model"), reason="retained_verified_article",
                                attempt=0, attempts=[], harness_retained_diagnostic=True)
                        runs.append(dict(phase=number, batch=batch+1,
                            status=snapshot["runtime"]["notes_status"] if snapshot else "process-failed",
                            coverage=snapshot["meeting"]["notes_cursor"] if snapshot else None,
                            generation=generated,
                            process=read_json(generation / "process.json"),
                            writer_resource=read_json(generation / "last-writer-resource.json"),
                            generation_diagnostic_last_only=automatic,
                            task_trace=[json.loads(line) for name in ["inference-trace.previous.jsonl", "inference-trace.jsonl"]
                                        if (generation/name).exists() for line in (generation/name).read_text().splitlines()]))
                    next_cursor = snapshot["meeting"]["notes_cursor"]
                    if next_cursor >= seq or next_cursor <= cursor: break
                    cursor = next_cursor
                if preserve:
                    with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
                        final_jobs = db.execute("SELECT COUNT(*) FROM inference_jobs WHERE meeting_id=?", (mid,)).fetchone()[0]
                    final_hash = sha256(root / "last-generation.json") if (root / "last-generation.json").is_file() else None
                    document_unchanged = snapshot["suggestions"] == saved["suggestions"]
                    preserved = final_jobs == saved_jobs and final_hash == preservation["last_generation_sha256_before"] and document_unchanged
                    preservation.update(passed=preserved, jobs_after=final_jobs,
                        automatic_scheduler_verified=automatic and preserved,
                        last_generation_sha256_after=final_hash, document_after=snapshot["suggestions"],
                        document_and_citations_unchanged=document_unchanged)
                    if not preserved: errors.append(f"Phase {number}: settling regenerated or changed corrected minutes")
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
                if "expect" in phase:
                    caption_checks = automatic or preserve and snapshot.get("translation_mode", "off") != "off"
                    phase_errors = assess(phase["expect"], snapshot, caption_checks)
                    if preserve:
                        preservation["captions_verified"] = caption_checks and not phase_errors
                    errors.extend(f"Phase {number}: {e}" for e in phase_errors)
            errors.extend(assess(case, snapshot, automatic))
        except Exception as error:
            errors.append(f"{type(error).__name__}: {error}")
            try: snapshot = command([cli, "snapshot", mid], env, folder / "failure-snapshot", timeout=30)
            except Exception: pass
        finally:
            if automatic:
                # A failed case must not consume another case's background work budget.
                with closing(sqlite3.connect(root / "meetings.sqlite3")) as db, db:
                    db.execute("UPDATE meetings SET json=json_set(json,'$.auto_notes',json('false')) WHERE id=?", (mid,))
        result = dict(case=case["name"], passed=not errors, errors=errors,
            seconds=round(time.monotonic()-started,3), runs=runs,
            note_updates=note_updates,
            document=snapshot["suggestions"] if snapshot else [],
            coverage=snapshot["meeting"]["notes_cursor"] if snapshot else 0,
            status=snapshot["runtime"]["notes_status"] if snapshot else "failed")
        if minutes_preservation: result["minutes_preservation"] = minutes_preservation
        report["cases"].append(result)
        report["passed"] = suite_passed(report["cases"], len(cases))
        write_json(output, report)
        print(case["name"], "PASS" if not errors else errors, flush=True)
    return report["passed"]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cli", type=pathlib.Path, default=REPO / "target/debug" / ("omascribe-cli.exe" if os.name == "nt" else "omascribe-cli"))
    p.add_argument("--output", type=pathlib.Path, default=pathlib.Path("/tmp/omascribe-evaluation.json"))
    p.add_argument("--artifacts", type=pathlib.Path)
    p.add_argument("--case")
    p.add_argument("--preferences", type=pathlib.Path)
    p.add_argument("--auto-schedule", action="store_true", help="Run the same factual/human-edit corpus through the real background scheduler")
    raise SystemExit(0 if evaluate(p.parse_args()) else 1)

if __name__ == "__main__": main()
