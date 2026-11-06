#!/usr/bin/env python3
"""Compare closed-source captions against the frozen candidate without downloading assets.

Two reversed-order trials check output parity for completely decoded fields and
normal inference for mixed speech. Cost samples exclude preparation; RSS is a
sampled process-tree sum, not GPU/physical memory or energy.
"""
import argparse
import collections
import json
import os
import pathlib
import re
import tempfile
import uuid

from evaluate import append, seed
from harness_support import (
    REPO, command, environment, isolated_env, read_json,
    release_borrowed_models, sha256, write_json,
)

CLOSED = [
    "Friday is not the deadline. Recording in meeting notes should stay on this computer.",
    "Recording and meeting notes should stay on this computer.",
]
EXPECTED = [
    "星期五不是截止日期。 会议笔记中的录音应保留在这台电脑上。",
    "录音和会议笔记应保留在这台电脑上。",
]
CASES = [
    dict(name="closed_fields", focus="", transcript=CLOSED),
    dict(name="mixed_factual", focus="", transcript=[*CLOSED, "Morgan will send the report on Thursday."]),
]


def texts(snapshot):
    return [snapshot["translations"].get(s["id"], {}).get("text")
            for s in sorted(snapshot["segments"], key=lambda s: s["seq"])]


def events(workspace):
    return [json.loads(line) for name in ["inference-trace.previous.jsonl", "inference-trace.jsonl"]
            if (workspace / name).exists()
            for line in (workspace / name).read_text().splitlines()]


def runtime_identity(directory):
    return {str(path.relative_to(directory)): dict(bytes=path.stat().st_size, sha256=sha256(path))
            for path in sorted(directory.rglob("*")) if path.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ["baseline-cli", "candidate-cli", "runtime", "model-dir", "output"]:
        parser.add_argument("--" + option, type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    artifacts = output.with_suffix(".artifacts")
    if output.exists() or artifacts.exists():
        parser.error("Use fresh output/artifact paths")
    artifacts.mkdir(parents=True)
    binaries = dict(baseline=args.baseline_cli.resolve(), candidate=args.candidate_cli.resolve())
    identities = {k: environment(v) for k, v in binaries.items()}
    # The old executable's compiled identity is authoritative, not this checkout.
    for key in ["core_and_harness_source_sha256", "inference_source_tree_dirty", "suite_revision"]:
        identities["baseline"].pop(key)
    candidate = identities["candidate"]
    if candidate["inference_source_tree_dirty"] or candidate["cli_build"]["revision_at_compile"] != candidate["suite_revision"]:
        parser.error("Candidate must be compiled from this exact clean source")
    spec = next(m for m in read_json(REPO / "model-catalog.json")["models"] if m["id"] == "qwen3-1.7b")
    for asset in spec["files"]:
        source = args.model_dir / asset["name"]
        if source.stat().st_size != asset["bytes"] or sha256(source) != asset["sha256"]:
            parser.error("Existing weights differ from pinned catalog")
    runtime_before = runtime_identity(args.runtime.resolve())
    if not runtime_before:
        parser.error("Existing runtime is empty")
    prefs = dict(llm_model=spec["id"], inference_backend="metal", threads=1, context_size=2048,
                 memory_budget_mib=4096, resource_mode="balanced", llm_keep_alive_seconds=0,
                 auto_notes=False, translation_mode="bilingual", diarization=False,
                 use_memory=False, use_knowledge=False)
    report = dict(schema=1, kind="closed_caption_startup_pair", identities=identities,
                  preferences=prefs, model_files=spec["files"], runtime_before=runtime_before,
                  results=[], errors=[], passed=False,
                  scope="Exactly two reversed-order trials of two fictional caption batches, not whole meetings, natural ASR, energy, GPU memory or broad speedup.")
    with tempfile.TemporaryDirectory(prefix="omascribe-closed-caption-model-") as folder:
        store = pathlib.Path(folder)
        (store / spec["id"]).mkdir()
        for asset in spec["files"]:
            os.link(args.model_dir / asset["name"], store / spec["id"] / asset["name"])
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
                    row = dict(trial=trial, case=case["name"], variant=label, errors=[])
                    try:
                        command([binaries[label], "diagnose"], env, directory / "prepare")
                        seed(workspace, case, mid, False)
                        append(workspace, mid, case["transcript"], 0)
                        snapshot = command([binaries[label], "translate", mid], env, directory / "translate")
                        observed = events(workspace)
                        values = texts(snapshot)
                        row.update(texts=values, workers=[t["metrics"] for t in observed if t["phase"] == "worker"],
                                   trace_phases=dict(collections.Counter(t["phase"] for t in observed)))
                        originals = [s["text"] for s in sorted(snapshot["segments"], key=lambda s: s["seq"])]
                        if originals != case["transcript"] or values[:2] != EXPECTED:
                            row["errors"].append("Original speech changed or decoded fields differ")
                        if case["name"] == "mixed_factual":
                            tail = values[2] if len(values) == 3 else None
                            if tail is None or not all(re.search(p, tail, re.I) for p in ["Morgan", "报告", "周四|星期四", "发送|送|交付"]):
                                row["errors"].append("Mixed factual source name/action/date missing")
                        expected_workers = 0 if label == "candidate" and case["name"] == "closed_fields" else 1
                        if len(row["workers"]) != expected_workers or any(w["reused"] for w in row["workers"]):
                            row["errors"].append("Unexpected worker startup or reuse")
                        log = workspace / "llm-runtime.log"
                        if log.exists():
                            if expected_workers == 0:
                                row["errors"].append("Direct caption batch unexpectedly produced an LLM runtime log")
                            data = log.read_text(errors="replace")
                            layers = [list(map(int, pair)) for pair in re.findall(r"offloaded (\d+)/(\d+) layers to GPU", data)]
                            row["runtime_log"] = dict(sha256=sha256(log), layers=layers)
                            if expected_workers and ("using device MTL0 (Apple" not in data or [29, 29] not in layers):
                                row["errors"].append("Actual Metal offload not proved")
                        elif expected_workers:
                            row["errors"].append("Missing runtime log")
                        cached = command([binaries[label], "translate", mid], env, directory / "cached")
                        if texts(cached) != values or any(e["phase"] == "worker" for e in events(workspace)[len(observed):]):
                            row["errors"].append("Unchanged captions start work or change text")
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
                            row["errors"].append("Borrowed models were not released")
                        row["passed"] = not row["errors"]
                        report["results"].append(row)
                        write_json(output, report)
                        print(trial, case["name"], label, "PASS" if row["passed"] else row["errors"], flush=True)
    report["runtime_after"] = runtime_identity(args.runtime.resolve())
    if report["runtime_after"] != runtime_before:
        report["errors"].append("Runtime bytes changed")
    report["model_after"] = {asset["name"]: dict(bytes=(args.model_dir / asset["name"]).stat().st_size,
                                                sha256=sha256(args.model_dir / asset["name"]))
                             for asset in spec["files"]}
    if any(report["model_after"][asset["name"]] != dict(bytes=asset["bytes"], sha256=asset["sha256"])
           for asset in spec["files"]):
        report["errors"].append("Model bytes changed")
    report["paired_full_text_equal"] = {}
    for case in CASES:
        values = [r.get("texts") for r in report["results"] if r["case"] == case["name"]]
        equal = len(values) == 4 and all(v == values[0] for v in values)
        report["paired_full_text_equal"][case["name"]] = equal
        if case["name"] == "closed_fields" and not equal:
            report["errors"].append("Closed-field output parity failed")
    report["quality_scope"] = "Exact byte parity required for closed fields. Mixed factual batches are a worker/fallback control; any full-text variation is recorded for independent semantic review, not treated as unchanged output."
    for label, binary in binaries.items():
        if sha256(binary) != identities[label]["cli_sha256"]:
            report["errors"].append(label + " executable changed")
    if environment(binaries["candidate"]) != identities["candidate"]:
        report["errors"].append("Candidate source identity changed")
    report["passed"] = len(report["results"]) == 8 and all(r["passed"] for r in report["results"]) and not report["errors"]
    write_json(output, report)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
