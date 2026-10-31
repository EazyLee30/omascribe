#!/usr/bin/env python3
"""Compare old, query-local and bounded incremental lexical retrieval in isolated children.

Build once: cargo test -p omascribe-core --lib --no-run --message-format=json
Pass its test executable, not cargo, so compiler CPU/RSS is excluded. No audio,
models, user workspace or network are used. Debug results remain debug results.
"""
import argparse
import json
import math
import os
import pathlib
import platform
import statistics
import subprocess
import time

from harness_support import REPO, sha256, source_digest, write_json

MODES = ("legacy", "transient", "incremental")
CHILD = "notes::lexical::tests::lexical_benchmark_child"


def validate_trial(results, count):
    assert len(results) == 3 and {r["mode"] for r in results} == set(MODES)
    for key in ("inputs_sha256", "rankings_sha256", "build"):
        assert all(r[key] == results[0][key] for r in results), f"Different {key}"
    for result in results:
        assert result["kind"] == "lexical-only" and result["schema"] == 1
        assert result["initial_documents"] == count and result["source_unchanged"] is True
        assert len(result["rank_ms"]) == 9 and all(math.isfinite(t) and t >= 0 for t in result["rank_ms"])
        assert 0 <= result["accounted_retained_bytes"] <= result["budget_bytes"] == 8 * 1024**2
        assert len(result["inputs_sha256"]) == len(result["rankings_sha256"]) == 64
        assert len(result["build"]["source_sha256"]) == 64
        if result["mode"] != "incremental":
            assert result["retained_documents"] == result["indexed_documents_built"] == 0


def invoke(binary, mode, count, directory):
    directory.mkdir(parents=True)
    output = directory / "result.json"
    env = dict(os.environ, OMASCRIBE_DATA_DIR=str(directory / "unused-workspace"),
               OMASCRIBE_MODELS_DIR=str(directory / "no-models"), OMASCRIBE_RUNTIME_DIR=str(directory / "no-runtime"),
               OMASCRIBE_RETRIEVAL_BENCH_MODE=mode, OMASCRIBE_RETRIEVAL_BENCH_COUNT=str(count),
               OMASCRIBE_RETRIEVAL_BENCH_OUTPUT=str(output))
    at = time.monotonic()
    usage = None
    with (directory / "stdout.txt").open("w") as out, (directory / "stderr.txt").open("w") as err:
        child = subprocess.Popen([str(binary), "--exact", CHILD, "--ignored", "--nocapture"], env=env, stdout=out, stderr=err)
        try:
            if hasattr(os, "wait4"):
                while True:
                    pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                    if pid:
                        child.returncode = os.waitstatus_to_exitcode(status)
                        break
                    if time.monotonic() - at > 180:
                        raise TimeoutError("Lexical child exceeded 180 s")
                    time.sleep(.01)
            else:
                child.wait(timeout=180)
        except BaseException:
            if child.returncode is None:
                child.kill()
                child.wait()
            raise
    if child.returncode:
        raise RuntimeError(f"Child exit {child.returncode}; see {directory}")
    result = json.loads(output.read_text(encoding="utf-8"))
    result.update(wall_ms=(time.monotonic()-at)*1000,
                  child_cpu_seconds=usage.ru_utime+usage.ru_stime if usage else None,
                  child_peak_rss_mib=usage.ru_maxrss/(1024**2 if platform.system() == "Darwin" else 1024)
                  if usage and platform.system() in ("Darwin", "Linux") else None,
                  resource_method="wait4 child, including fixture/hash checks; not process-tree sum" if usage else "unavailable",
                  total_rank_ms=sum(result["rank_ms"]), warm_rank_median_ms=statistics.median(result["rank_ms"][1:]))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=pathlib.Path, required=True)
    parser.add_argument("--sizes", type=int, nargs="+", default=[1000, 5000])
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    artifacts = output.with_suffix(".artifacts")
    if output.exists() or artifacts.exists() or not all(1 <= size <= 100000 for size in args.sizes):
        parser.error("Use fresh output paths and sizes between 1 and 100000")
    binary = args.test_binary.resolve()
    artifacts.mkdir(parents=True)
    report = dict(schema=1, kind="lexical-only", binary_sha256=sha256(binary), platform=platform.platform(),
                  suite_revision=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
                  core_and_harness_source_sha256=source_digest(),
                  inference_source_tree_dirty=bool(subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=all", "--", "crates", "tests"], cwd=REPO, text=True).strip()),
                  trials=[], errors=[], passed=False,
                  limitation="Fictional lexical-only comparison, 9 queries including append/edit/removal. No audio/model, semantic minutes quality, overall inference speed or energy claim. Retained accounting is not process RSS. Includes cold indexing and rejected indexing; no default index promotion.")
    for count in args.sizes:
        for trial, order in enumerate((MODES, tuple(reversed(MODES))), 1):
            results = []
            try:
                for mode in order:
                    result = invoke(binary, mode, count, artifacts / f"{count}-{trial}-{mode}")
                    results.append(result)
                    print(json.dumps(dict(count=count, trial=trial, mode=mode, total_rank_ms=result["total_rank_ms"],
                                          cpu=result["child_cpu_seconds"], peak_rss_mib=result["child_peak_rss_mib"],
                                          retained_bytes=result["accounted_retained_bytes"], fallback=result["budget_fallback"])), flush=True)
                validate_trial(results, count)
                report["trials"].append(dict(count=count, trial=trial, results=results, passed=True))
            except Exception as error:
                report["errors"].append(f"{count}-{trial}: {type(error).__name__}: {error}")
                report["trials"].append(dict(count=count, trial=trial, results=results, passed=False))
            write_json(output, report)
    report["passed"] = len(report["trials"]) == 2 * len(args.sizes) and all(t["passed"] for t in report["trials"]) and not report["errors"]
    write_json(output, report)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
