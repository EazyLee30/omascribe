#!/usr/bin/env python3
"""Compare two real CLI builds on fictional long-meeting terminology updates.

No ASR/model execution and no network. Measures cold initialization plus repeated
new passages, including CLI/workspace overhead. Does not claim ASR/LLM speedup.
"""
import argparse
import hashlib
import json
import os
import pathlib
import sqlite3
import statistics
import subprocess
import tempfile
import time

REPO = pathlib.Path(__file__).resolve().parents[1]


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def segment(index, meeting):
    return dict(id=f"source-{index}", meeting_id=meeting, run=1, seq=index,
                start_ms=index * 1000, end_ms=(index + 1) * 1000,
                text="预算尚未确定，周五继续讨论。", revision=1, speaker=None, speaker_manual=False)


def add(db, index, meeting):
    passage = segment(index, meeting)
    db.execute("INSERT INTO segments VALUES(?,?,?,?,?)", (passage["id"], meeting, 1, index, json.dumps(passage, ensure_ascii=False)))


def run(cli, count, iterations):
    with tempfile.TemporaryDirectory(prefix="omascribe-terminology-benchmark-") as folder:
        root = pathlib.Path(folder)
        env = dict(os.environ, OMASCRIBE_DATA_DIR=str(root), OMASCRIBE_MODELS_DIR=str(root / "no-models"), OMASCRIBE_RUNTIME_DIR=str(root / "no-runtime"))
        (root / "network-policy.json").write_text(json.dumps(dict(allow_model_downloads=False, allow_online_recommendations=False)))
        def invoke(*args):
            at = time.perf_counter()
            result = subprocess.run([str(cli), *args], env=env, check=True, capture_output=True, text=True, encoding="utf-8", timeout=180)
            return (time.perf_counter() - at) * 1000, json.loads(result.stdout)
        setup_ms, _ = invoke("diagnose")
        meeting = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        info = dict(id=meeting, title="Fictional terminology benchmark", created_at="2026-10-26T00:00:00Z", status="ready", duration_ms=count * 1000, run=1, notes_cursor=0, auto_notes=False)
        seed_at = time.perf_counter()
        with sqlite3.connect(root / "meetings.sqlite3") as db:
            db.execute("INSERT INTO meetings VALUES(?,?)", (meeting, json.dumps(info)))
            for index in range(1, count + 1):
                add(db, index, meeting)
        seed_ms = (time.perf_counter() - seed_at) * 1000
        cold_ms, result = invoke("correct-terms", meeting)
        assert result["corrected"] == 0
        timings, writes = [], []
        for index in range(count + 1, count + iterations + 1):
            write_at = time.perf_counter()
            with sqlite3.connect(root / "meetings.sqlite3") as db:
                add(db, index, meeting)
            writes.append((time.perf_counter() - write_at) * 1000)
            elapsed, result = invoke("correct-terms", meeting)
            assert result["corrected"] == 0
            timings.append(elapsed)
        with sqlite3.connect(root / "meetings.sqlite3") as db:
            passages = [json.loads(row[0]) for row in db.execute("SELECT json FROM segments ORDER BY seq")]
        assert passages == [segment(index, meeting) for index in range(1, count + iterations + 1)]
        assert not (root / "llm-runtime.log").exists(), "Terminology must not start a model"
        metadata = subprocess.run([str(cli), "build-info"], env=env, capture_output=True, text=True, encoding="utf-8", timeout=30)
        identity = json.loads(metadata.stdout) if metadata.returncode == 0 else None
        correction_ms = cold_ms + sum(timings)
        return dict(cli_sha256=sha(cli), build=identity, setup_ms=setup_ms, seed_ms=seed_ms,
                    cold_ms=cold_ms, append_write_ms=writes, append_ms=timings,
                    append_median_ms=statistics.median(timings), correction_total_ms=correction_ms,
                    total_ms=setup_ms + seed_ms + sum(writes) + correction_ms,
                    source_unchanged=True, peak_rss_mib=None, cpu_seconds=None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-cli", type=pathlib.Path, required=True)
    parser.add_argument("--baseline-revision", required=True)
    parser.add_argument("--current-cli", type=pathlib.Path, default=REPO / "target/debug" / ("omascribe-cli.exe" if os.name == "nt" else "omascribe-cli"))
    parser.add_argument("--segments", type=int, default=5000)
    parser.add_argument("--iterations", type=int, default=8)
    parser.add_argument("--output", type=pathlib.Path, default=REPO / "artifacts/benchmarks/terminology-incremental.json")
    args = parser.parse_args()
    assert 1 <= args.segments <= 100000 and 1 <= args.iterations <= 100
    # Reverse the second pair's order to expose simple run-order/OS-cache effects.
    results = []
    for order in [("baseline", "current"), ("current", "baseline")]:
        trial = {}
        for name in order:
            cli = args.baseline_cli if name == "baseline" else args.current_cli
            trial[name] = run(cli.resolve(), args.segments, args.iterations)
        results.append(trial)
    changed = subprocess.run(["git", "diff", "--quiet", "HEAD", "--", "crates/core"], cwd=REPO).returncode != 0
    sources = hashlib.sha256()
    for path in sorted((REPO / "crates/core/src").rglob("*.rs")):
        sources.update(str(path.relative_to(REPO)).encode()); sources.update(path.read_bytes())
    report = dict(schema=2, kind="terminology-only", baseline_revision=args.baseline_revision,
                  current_head=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
                  current_core_tracked_changes=changed, current_core_sources_sha256=sources.hexdigest(),
                  segment_count=args.segments, appended_per_trial=args.iterations, trials=results,
                  measurement="wall clock including workspace setup, source seeding and append writes, CLI startup/open and terminology correction; excludes result validation, hashes and build-info; no model; resource sampling unavailable")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
