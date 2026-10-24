#!/usr/bin/env python3
"""Run the real local model through the production pipeline; fixtures are fictional.
This is a targeted regression harness, not an ASR accuracy benchmark or a factuality guarantee.
"""

import argparse, datetime, json, os, pathlib, re, shutil, sqlite3, subprocess, tempfile, time, uuid
from contextlib import closing

REPO = pathlib.Path(__file__).resolve().parents[1]


def assess(case, snapshot):
    sections = [s for s in snapshot["suggestions"] if s["status"] != "dismissed"]
    text = "\n".join(s.get("focus", "") + " " + s["text"] for s in sections)
    errors = []
    if not sections:
        errors.append("No document: " + snapshot["runtime"]["notes_status"])
    for expression in case["required"]:
        if not re.search(expression, text, re.I):
            errors.append("Missing: " + expression)
    for expression in case["forbidden"]:
        if re.search(expression, text, re.I):
            errors.append("Unsupported claim: " + expression)
    if len(sections) > case.get("max_sections", len(sections)):
        errors.append("Related material fragmented into too many paragraphs")
    if case.get("merged_sources") and not any(len(s["sources"]) > 1 for s in sections):
        errors.append("Related evidence was not synthesized")
    for s in sections:
        if not s["sources"]:
            errors.append("Missing source")
        for source in s["sources"]:
            if not any(
                t["id"] == source["segment_id"]
                and t["revision"] == source["revision"]
                and t["text"] == source["quote"]
                for t in snapshot["segments"]
            ):
                errors.append("Invalid evidence link")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--cli", type=pathlib.Path, default=REPO / "target/debug/omascribe-cli"
    )
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        default=pathlib.Path("/tmp/omascribe-evaluation.json"),
    )
    parser.add_argument("--case", help="Run one named fixture")
    parser.add_argument("--preferences", type=pathlib.Path, help="Optional local preferences for resource/backend regression checks")
    args = parser.parse_args()
    cli = str(args.cli.resolve())
    cases = json.loads(
        (REPO / "tests/fixtures/document-cases.json").read_text(encoding="utf-8")
    )
    if args.case:
        cases = [case for case in cases if case["name"] == args.case]
        if not cases:
            parser.error("Unknown fixture name")
    results = []
    data = (
        pathlib.Path(
            os.environ.get("XDG_DATA_HOME", str(pathlib.Path.home() / ".local/share"))
        )
        / "omascribe"
    )
    with tempfile.TemporaryDirectory(prefix="omascribe-eval-") as root:
        env = dict(
            os.environ,
            OMASCRIBE_DATA_DIR=root,
            OMASCRIBE_MODELS_DIR=os.environ.get(
                "OMASCRIBE_MODELS_DIR", str(data / "models")
            ),
            OMASCRIBE_RUNTIME_DIR=os.environ.get(
                "OMASCRIBE_RUNTIME_DIR", str(data / "runtime")
            ),
        )
        if args.preferences:
            shutil.copyfile(args.preferences, pathlib.Path(root) / "preferences.json")
        subprocess.run(
            [cli, "diagnose"], env=env, check=True, stdout=subprocess.DEVNULL
        )
        for case in cases:
            mid = str(uuid.uuid4())
            now = datetime.datetime.now(datetime.timezone.utc).isoformat()
            meeting = dict(
                id=mid,
                title=case["name"],
                created_at=now,
                status="ready",
                duration_ms=10000 * len(case["transcript"]),
                run=1,
                notes_cursor=0,
                auto_notes=False,
                notes_fingerprint="",
            )
            with (
                closing(sqlite3.connect(pathlib.Path(root) / "meetings.sqlite3")) as db,
                db,
            ):
                db.execute(
                    "INSERT INTO meetings VALUES(?,?)", (mid, json.dumps(meeting))
                )
                nid = str(uuid.uuid4())
                note = dict(
                    id=nid,
                    meeting_id=mid,
                    kind="document",
                    content=case["focus"],
                    anchor_ms=0,
                    updated_at=now,
                )
                db.execute(
                    "INSERT INTO notes VALUES(?,?,?)", (nid, mid, json.dumps(note))
                )
                for i, text in enumerate(case["transcript"]):
                    sid = str(uuid.uuid4())
                    seg = dict(
                        id=sid,
                        meeting_id=mid,
                        run=1,
                        seq=i + 1,
                        start_ms=i * 10000,
                        end_ms=(i + 1) * 10000,
                        text=text,
                        revision=1,
                    )
                    db.execute(
                        "INSERT INTO segments VALUES(?,?,?,?,?)",
                        (sid, mid, 1, i + 1, json.dumps(seg)),
                    )
            start = time.monotonic()
            cursor = -1
            for _ in range(len(case["transcript"]) + 1):
                run = subprocess.run([cli, "generate", mid], env=env, capture_output=True,
                    text=True, encoding="utf-8", timeout=650, check=True)
                snapshot = json.loads(run.stdout)
                next_cursor = snapshot["meeting"]["notes_cursor"]
                if next_cursor >= len(case["transcript"]) or next_cursor <= cursor:
                    break
                cursor = next_cursor
            errors = assess(case, snapshot)
            result = dict(
                case=case["name"],
                passed=not errors,
                errors=errors,
                seconds=round(time.monotonic() - start, 3),
                document=snapshot["suggestions"],
                coverage=snapshot["meeting"]["notes_cursor"],
                last_generation=json.loads((pathlib.Path(root) / "last-generation.json").read_text()) if (pathlib.Path(root) / "last-generation.json").exists() else None,
                status=snapshot["runtime"]["notes_status"],
                resource=json.loads((pathlib.Path(root) / "last-writer-resource.json").read_text(encoding="utf-8")) if (pathlib.Path(root) / "last-writer-resource.json").exists() else None,
                writer_attempts=json.loads(
                    (pathlib.Path(root) / "last-generation.json").read_text(
                        encoding="utf-8"
                    )
                ).get("attempt", 1)
                if (pathlib.Path(root) / "last-generation.json").exists()
                else 0,
            )
            results.append(result)
            args.output.write_text(
                json.dumps(
                    dict(passed=all(r["passed"] for r in results), cases=results),
                    ensure_ascii=False,
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
            print(case["name"], "PASS" if not errors else errors, flush=True)
    if not all(r["passed"] for r in results):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
