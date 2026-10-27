#!/usr/bin/env python3
"""Compare literal-only and mixed caption batches without changing product defaults.

Uses existing pinned weights/runtime, disposable fictional workspaces, and two
reversed-order trials. No downloads. Startup counts and identical output are the
primary gates; sampled RSS/CPU and uncontrolled page caches limit cost claims.
"""
import argparse
import collections
import json
import os
import pathlib
import re
import statistics
import tempfile
import uuid

from evaluate import append, seed
from harness_support import (
    REPO, command, environment, isolated_env, read_json,
    release_borrowed_models, sha256, write_json,
)

CASES = [
    dict(name="literal_only", focus="", transcript=["嗯", "AND", "或"]),
    dict(name="mixed_factual", focus="", transcript=[
        "嗯", "预算3000元尚未批准。", "AND",
        "Morgan will send the report on Thursday.",
    ]),
]


def translations(snapshot):
    return [snapshot["translations"].get(s["id"], {}).get("text")
            for s in sorted(snapshot["segments"], key=lambda s: s["seq"])]


def validate(case, snapshot):
    texts = translations(snapshot)
    errors = []
    if [s["text"] for s in sorted(snapshot["segments"], key=lambda s: s["seq"])] != case["transcript"]:
        errors.append("Source changed")
    if not texts or any(t is None for t in texts):
        return [*errors, "Translation missing"]
    expected_literals = {0: "Hmm", 1: "和", 2: "or"} if case["name"] == "literal_only" else {0: "Hmm", 2: "和"}
    for index, expected in expected_literals.items():
        if texts[index] != expected:
            errors.append(f"Literal {index} differs")
    if case["name"] == "mixed_factual":
        for expression in [r"3000", r"yuan", r"not.*approved|unapproved|pending approval"]:
            if not re.search(expression, texts[1], re.I):
                errors.append("Amount/currency/pending state not preserved: " + expression)
        for expression in [r"Morgan", r"周四|星期四"]:
            if not re.search(expression, texts[3], re.I):
                errors.append("Name/deadline not preserved: " + expression)
    return errors


def trace_events(workspace):
    return [json.loads(line) for name in ["inference-trace.previous.jsonl", "inference-trace.jsonl"]
            if (workspace / name).exists()
            for line in (workspace / name).read_text().splitlines()]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-cli", type=pathlib.Path, required=True)
    parser.add_argument("--candidate-cli", type=pathlib.Path, required=True)
    parser.add_argument("--runtime", type=pathlib.Path, required=True)
    parser.add_argument("--model-dir", type=pathlib.Path, required=True)
    parser.add_argument("--backend", choices=["cpu", "metal"], default="metal")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    artifacts = output.with_suffix(".artifacts")
    if output.exists() or artifacts.exists():
        parser.error("Use fresh output/artifact paths")
    artifacts.mkdir(parents=True)
    candidates = {"baseline": args.baseline_cli.resolve(), "candidate": args.candidate_cli.resolve()}
    identities = {label: environment(cli) for label, cli in candidates.items()}
    # Both use this harness checkout. Baseline product identity is its compiled
    # source digest and executable hash, not the candidate's checkout sources.
    for key in ["core_and_harness_source_sha256", "inference_source_tree_dirty", "suite_revision"]:
        identities["baseline"].pop(key)
    prefs = dict(llm_model="qwen3-1.7b", inference_backend=args.backend,
                 threads=1, context_size=2048, memory_budget_mib=4096,
                 resource_mode="balanced", llm_keep_alive_seconds=0,
                 auto_notes=False, translation_mode="bilingual", diarization=False,
                 use_memory=False, use_knowledge=False)
    model = next(m for m in read_json(REPO / "model-catalog.json")["models"] if m["id"] == prefs["llm_model"])
    for asset in model["files"]:
        source = args.model_dir / asset["name"]
        if source.stat().st_size != asset["bytes"] or sha256(source) != asset["sha256"]:
            parser.error("Model differs from pinned catalog")
    report = dict(schema=1, kind="literal_caption_startup_pair", identities=identities,
                  preferences=prefs, model_files=model["files"], results=[], errors=[], passed=False,
                  measurement="CLI-inclusive wall time and sampled process-tree CPU/RSS; not energy or unique physical/GPU memory. Page cache not flushed; not a cold-disk or whole-meeting benchmark.")
    with tempfile.TemporaryDirectory(prefix="omascribe-literal-model-") as folder:
        store = pathlib.Path(folder)
        (store / model["id"]).mkdir()
        for asset in model["files"]:
            os.link(args.model_dir / asset["name"], store / model["id"] / asset["name"])
        os.environ.update(OMASCRIBE_TEST_MODEL_STORE=str(store),
                          OMASCRIBE_RUNTIME_DIR=str(args.runtime.resolve()),
                          OMASCRIBE_MODELS_DIR=str(store / "no-bundled-models"))
        for trial, order in [(1, ["baseline", "candidate"]), (2, ["candidate", "baseline"])]:
            for case in CASES:
                for label in order:
                    directory = artifacts / f"{trial}-{case['name']}-{label}"
                    workspace = directory / "workspace"
                    env = isolated_env(workspace)
                    write_json(workspace / "preferences.json", prefs)
                    mid = str(uuid.uuid4())
                    seed(workspace, case, mid, False)
                    append(workspace, mid, case["transcript"], 0)
                    row = dict(trial=trial, case=case["name"], variant=label, errors=[])
                    try:
                        snapshot = command([candidates[label], "translate", mid], env, directory / "translate")
                        events = trace_events(workspace)
                        row.update(texts=translations(snapshot), errors=validate(case, snapshot),
                                   workers=[t["metrics"] for t in events if t["phase"] == "worker"],
                                   trace_phases=dict(collections.Counter(t["phase"] for t in events)))
                        expected_workers = 0 if label == "candidate" and case["name"] == "literal_only" else 1
                        if len(row["workers"]) != expected_workers or any(w["reused"] for w in row["workers"]):
                            row["errors"].append("Unexpected worker startup/reuse count")
                        log = workspace / "llm-runtime.log"
                        if log.exists():
                            layers = [list(map(int, pair)) for pair in re.findall(r"offloaded (\d+)/(\d+) layers to GPU", log.read_text())]
                            row["runtime_log"] = dict(sha256=sha256(log), offloaded_layers=layers)
                            if args.backend == "metal" and not any(0 < used <= total for used, total in layers):
                                row["errors"].append("Metal offload not proved")
                        elif expected_workers:
                            row["errors"].append("Missing runtime log")
                        cached = command([candidates[label], "translate", mid], env, directory / "cached")
                        if translations(cached) != row["texts"] or any(e["phase"] == "worker" for e in trace_events(workspace)[len(events):]):
                            row["errors"].append("Unchanged cached captions started work or changed text")
                    except Exception as error:
                        row["errors"].append(f"{type(error).__name__}: {error}")
                    finally:
                        row["process"] = read_json(directory / "translate/process.json")
                        row["cached_process"] = read_json(directory / "cached/process.json")
                        marker = read_json(workspace / "model-store/.omascribe-harness-borrow.json")
                        if marker:
                            release_borrowed_models(workspace / "model-store", marker["owner"])
                        row["model_cleanup"] = read_json(workspace / "model-copy-cleanup.json")
                        if not row["model_cleanup"] or not row["model_cleanup"].get("released"):
                            row["errors"].append("Borrowed model directory not released")
                        row["passed"] = not row["errors"]
                        report["results"].append(row)
                        write_json(output, report)
                        print(trial, case["name"], label, "PASS" if row["passed"] else row["errors"], flush=True)
    for case in CASES:
        rows = [r for r in report["results"] if r["case"] == case["name"]]
        if len(rows) != 4 or any(r.get("texts") != rows[0].get("texts") for r in rows):
            report["errors"].append(case["name"] + ": paired outputs differ or are incomplete")
    for label, cli in candidates.items():
        if sha256(cli) != identities[label]["cli_sha256"]:
            report["errors"].append(label + ": executable changed")
    report["summary"] = {case["name"]: {label: dict(
        median_wall_seconds=statistics.median(r["process"]["seconds"] for r in report["results"] if r["case"] == case["name"] and r["variant"] == label),
        worker_start_events=sum(len(r.get("workers", [])) for r in report["results"] if r["case"] == case["name"] and r["variant"] == label))
        for label in candidates} for case in CASES}
    report["passed"] = not report["errors"] and len(report["results"]) == 8 and all(r["passed"] for r in report["results"])
    write_json(output, report)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
