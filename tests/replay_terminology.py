#!/usr/bin/env python3
"""Local WAV -> note hotwords -> revisioned transcript -> cited minutes.

Inputs and all diagnostics stay in a fresh isolated directory. A user recording
must never be uploaded as a CI artifact or committed as a public fixture.
"""
import argparse, json, pathlib, sqlite3, uuid
from contextlib import closing
from evaluate import assess, seed
from harness_support import REPO, command, isolated_env, sha256, write_json


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cli", type=pathlib.Path, default=REPO / "target/debug/omascribe-cli")
    p.add_argument("--wav", type=pathlib.Path, required=True)
    p.add_argument("--notes", type=pathlib.Path, required=True, help="UTF-8 note file")
    p.add_argument("--expected", action="append", required=True, help="Canonical spelling required in transcript and minutes")
    p.add_argument("--forbidden", action="append", default=[], help="Unexpected name regex, transcript and minutes")
    p.add_argument("--forbidden-minutes", action="append", default=[], help="Unsupported topics or phrases prohibited only in minutes")
    p.add_argument("--language", choices=["auto", "zh", "en"], default="auto")
    p.add_argument("--output", type=pathlib.Path, required=True)
    a = p.parse_args()
    output = a.output.resolve()
    if output.exists(): p.error("Use a fresh output directory")
    root = output / "workspace"
    env = isolated_env(root)
    cli, mid = a.cli.resolve(), str(uuid.uuid4())
    write_json(root / "preferences.json", {"minutes_language": a.language})
    command([cli, "diagnose"], env, output / "diagnose")
    seed(root, dict(name="Local terminology replay", focus=a.notes.read_text(encoding="utf-8")), mid)
    errors = []
    report = dict(passed=False, source="caller-provided local audio; private unless explicitly public",
                  audio_sha256=sha256(a.wav), cli_sha256=sha256(cli), errors=errors)
    try:
        transcript = command([cli, "append", mid, a.wav.resolve()], env, output / "transcribe")
        if not transcript["segments"]: errors.append("No speech recognized")
        snapshot = transcript
        for index in range(len(transcript["segments"])+1):
            previous = snapshot["meeting"]["notes_cursor"]
            snapshot = command([cli, "generate", mid], env, output / f"minutes-{index+1}")
            cursor = snapshot["meeting"]["notes_cursor"]
            if cursor >= len(transcript["segments"]) or cursor <= previous: break
        if snapshot["meeting"]["notes_cursor"] < len(transcript["segments"]): errors.append("Uncovered transcript")
        # Exact casing matters here; ordinary minutes checks intentionally ignore it.
        text = "\n".join(s["text"] for s in snapshot["segments"])
        minutes = "\n".join(s["text"] for s in snapshot["suggestions"] if s["status"] != "dismissed")
        for name in a.expected:
            if name not in text: errors.append("Transcript missing canonical spelling: " + name)
            if name not in minutes: errors.append("Minutes missing canonical spelling: " + name)
        errors.extend(assess(dict(forbidden=a.forbidden+a.forbidden_minutes, forbidden_transcript=a.forbidden), snapshot))
        with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
            edits = db.execute("SELECT id,revision FROM segment_text_edits WHERE origin='terminology'").fetchall()
            for sid, revision in edits:
                if not db.execute("SELECT 1 FROM segment_history WHERE id=? AND revision=?", (sid, revision-1)).fetchone():
                    errors.append("Correction lost its prior revision")
        report.update(corrections=len(edits), segments=len(snapshot["segments"]), coverage=snapshot["meeting"]["notes_cursor"])
        write_json(output / "snapshot.json", snapshot)
    except Exception as error:
        errors.append(f"{type(error).__name__}: {error}")
    report["passed"] = not errors
    write_json(output / "report.json", report)
    print(json.dumps(report, ensure_ascii=False), flush=True)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__": main()
