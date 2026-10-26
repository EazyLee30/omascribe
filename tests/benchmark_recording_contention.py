#!/usr/bin/env python3
"""Offline paced recording/ASR versus concurrent Metal translation and minutes.

Uses Engine's real replay/capture/scheduler, never a microphone or user meeting.
Public read-speech is a contention fixture, not meeting-summary semantic gold.
"""
import argparse
import hashlib
import json
import math
import os
import pathlib
import platform
import re
import tempfile
import wave

from evaluate_asr import asset_identity, error_counts, percentile, units
from harness_support import REPO, command, environment, isolated_env, read_json, sha256, write_json
from compare_minutes_models import runtime_inventory


def telemetry_summary(rows, seconds, loaded):
    if len(rows) < 3:
        raise ValueError("Missing paced telemetry")
    previous = dict(wall_ms=0, captured_ms=0, processed_ms=0)
    for row in rows:
        for key in previous:
            value = row.get(key)
            if type(value) is not int or value < previous[key]:
                raise ValueError("Non-monotonic/incomplete replay clock: " + key)
        if row["processed_ms"] > row["captured_ms"] or row.get("schema") != 1:
            raise ValueError("Replay timeline is out of range")
        if row.get("has_error") is not False:
            raise ValueError("Replay reported a runtime error")
        previous = {key: row[key] for key in previous}
    last = rows[-1]
    if last["recording"] or last["transcribing"] or last["processed_ms"] != seconds * 1000:
        raise ValueError("Replay did not finish the exact audio duration")
    active = [r for r in rows if r["recording"] and r["captured_ms"] > 0]
    if not active:
        raise ValueError("No live recording observations")
    gaps = [b["wall_ms"]-a["wall_ms"] for a, b in zip(rows, rows[1:])]
    if max(gaps) > 1500:
        raise ValueError("Telemetry gap hides potential ASR stalls")
    lags = [r["captured_ms"]-r["processed_ms"] for r in active]
    stopped = next(r for r in rows if not r["recording"] and not r["transcribing"])
    summary = dict(samples=len(rows), interval_ms=200, largest_sample_gap_ms=max(gaps),
        lag_p50_ms=percentile(lags, .5), lag_p95_ms=percentile(lags, .95), lag_max_ms=max(lags),
        capture_wall_overhead_ms=stopped["wall_ms"]-stopped["captured_ms"],
        asr_compute_rtf=last["asr_rtf"],
        live_translations=max(r["translations"] for r in active),
        live_notes_cursor=max(r["notes_cursor"] for r in active),
        live_worker_samples=sum(bool(r["worker"]["leased"]) for r in active),
        worker_starts=last["worker"]["starts"], worker_reuses=last["worker"]["reuses"])
    if summary["lag_max_ms"] > 2500:
        raise ValueError("ASR exceeded the scheduler's 2500 ms admission budget")
    if loaded and not all(summary[k] > 0 for k in ["live_translations", "live_notes_cursor", "live_worker_samples"]):
        raise ValueError("Missing live translation/minutes/model work; not a contention test")
    return summary


def paired_errors(results, arms=None):
    errors = []
    by_arm = {r["arm"]: r for r in results}
    arms = arms or ["asr-only", "metal-concurrent"]
    if set(by_arm) != set(arms) or len(results) != 2:
        return ["Missing paired arm"]
    old, new = (by_arm[name] for name in arms)
    if not old["passed"] or not new["passed"]:
        errors.append("A paired arm failed; retain its cost and diagnostics")
    for key in ["pcm_sha256", "audio_samples"]:
        if old.get(key) is None or old.get(key) != new.get(key):
            errors.append("Recording differs between paired arms: " + key)
    if old.get("hypothesis_units") != new.get("hypothesis_units"):
        errors.append("ASR output changed under concurrent workload")
    return errors


def fixture(corpus, output, repeats):
    manifest = read_json(corpus / "manifest.json")
    case = next(c for c in manifest["cases"] if c["name"] == "fleurs_mixed_zh_en_02")
    path = corpus / case["file"]
    if "\\" in case["file"] or pathlib.Path(case["file"]).name != case["file"] or sha256(path) != case["sha256"]:
        raise ValueError("Fixture hash/path mismatch")
    with wave.open(str(path)) as source:
        if (source.getnchannels(), source.getsampwidth(), source.getframerate()) != (1, 2, 16000):
            raise ValueError("Expected pinned 16 kHz mono PCM16 fixture")
        pcm = source.readframes(source.getnframes())
    # Whole-second loop with >=2 seconds of silence prevents a partial last word.
    loop_seconds = math.ceil(len(pcm)/32000) + 2
    pcm += b"\0" * (loop_seconds*32000-len(pcm))
    with wave.open(str(output), "wb") as target:
        target.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        target.writeframes(pcm)
    return dict(case=case, seconds=loop_seconds*repeats, reference=case["reference"]*repeats,
        reference_units=units(case["reference"], "mixed")*repeats,
        expected_pcm_sha256=hashlib.sha256(pcm*repeats).hexdigest(),
        input_wav_sha256=sha256(output), manifest_sha256=sha256(corpus/"manifest.json"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=pathlib.Path, default=REPO/"target/debug/omascribe-cli")
    parser.add_argument("--fixtures", type=pathlib.Path, required=True)
    parser.add_argument("--runtime", type=pathlib.Path, required=True)
    parser.add_argument("--asr-model-dir", type=pathlib.Path, required=True)
    parser.add_argument("--llm-model-dir", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--repeats", type=int, choices=[3, 6], default=3)
    parser.add_argument("--minutes-catchup", action="store_true",
                        help="Compare legacy/catch-up minute scheduling, both with Metal and a 60s post-capture budget")
    args = parser.parse_args()
    if (platform.system(), platform.machine()) != ("Darwin", "arm64"):
        parser.error("This comparison requires native Apple Silicon Metal")
    output, cli, runtime = args.output.resolve(), args.cli.resolve(), args.runtime.resolve()
    artifacts = output.with_suffix(".artifacts")
    if output.exists() or artifacts.exists():
        parser.error("Use fresh output paths")
    artifacts.mkdir(parents=True)
    audio = artifacts/"paced-public-loop.wav"
    source = fixture(args.fixtures.resolve(), audio, args.repeats)
    native = runtime_inventory(runtime)
    if not native:
        parser.error("Missing local llama runtime")
    arms = ["legacy-concurrent", "catchup-concurrent"] if args.minutes_catchup else ["asr-only", "metal-concurrent"]
    report = dict(schema=1, kind="apple-paced-minutes-catchup" if args.minutes_catchup else "apple-paced-recording-contention", environment=environment(cli),
        fixture=source, runtime_files=native, trials=[], errors=[], passed=False,
        limitation="Engine replay uses ffmpeg pacing, not physical microphone/hardware capture. Read-speech splice, not natural conversation. ASR processing lag is sampled every 200 ms, not visible caption latency. Background progress and exact citations are checked, not translation/minutes semantic gold. Catch-up mode includes the same explicit post-capture settlement budget and costs in both arms; normal mode reports pending work at stop. Sampled CPU is a lower bound; RSS may double-count shared pages; GPU allocation, energy, pressure/device faults and older M-series are unmeasured.")
    catalog = read_json(REPO/"model-catalog.json")["models"]
    with tempfile.TemporaryDirectory(prefix="omascribe-contention-models-") as temp:
        store = pathlib.Path(temp)
        identities = {}
        for name, directory in [("zipformer-small", args.asr_model_dir.resolve()), ("qwen3-1.7b", args.llm_model_dir.resolve())]:
            spec = next(m for m in catalog if m["id"] == name)
            (store/name).mkdir()
            for item in spec["files"]:
                path = directory/item["name"]
                if path.stat().st_size != item["bytes"] or sha256(path) != item["sha256"]:
                    raise ValueError("Pinned model differs: " + name)
                os.link(path, store/name/item["name"])
                identities[str(path)] = item["sha256"]
        report["models"] = {m["id"]:m["files"] for m in catalog if m["id"] in ["zipformer-small", "qwen3-1.7b"]}
        previous_store = os.environ.get("OMASCRIBE_TEST_MODEL_STORE")
        os.environ["OMASCRIBE_TEST_MODEL_STORE"] = str(store)
        try:
            for trial, order in enumerate([[0, 1], [1, 0]], 1):
                results = []
                for variant in order:
                    arm = arms[variant]
                    loaded = args.minutes_catchup or variant == 1
                    folder = artifacts/f"{trial}-{arm}"
                    root = folder/"workspace"
                    env = isolated_env(root)
                    env.update(OMASCRIBE_RUNTIME_DIR=str(runtime), OMASCRIBE_MODELS_DIR=str(folder/"no-bundled-models"), OMASCRIBE_ASR_STREAM_HOTWORDS="0", OMASCRIBE_RETRIEVAL_CACHE="0",
                               OMASCRIBE_MINUTES_CATCHUP="1" if args.minutes_catchup and variant == 1 else "0")
                    for key in ["LLAMA_ARG_LOG_VERBOSITY", "OMASCRIBE_ACCELERATION_CANDIDATE_VERSION"]:
                        env.pop(key, None)
                    prefs = dict(asr_model="zipformer-small", llm_model="qwen3-1.7b", threads=1,
                        context_size=2048, memory_budget_mib=4096, inference_backend="metal",
                        resource_mode="balanced", auto_notes=loaded, translation_mode="bilingual" if loaded else "off",
                        minutes_language="zh", diarization=False, llm_keep_alive_seconds=0)
                    write_json(root/"preferences.json", prefs)
                    result = dict(trial=trial, arm=arm, preferences=prefs, errors=[], passed=False)
                    print(f"Trial {trial} {arm}: {source['seconds']} s paced recording", flush=True)
                    try:
                        result["asr_assets"] = asset_identity(cli, root, "zipformer-small", env)
                        argv = [cli, "replay-settle" if args.minutes_catchup else "replay", audio, source["seconds"]]
                        if args.minutes_catchup: argv.append(60)
                        response = command([*argv, "--telemetry", folder/"telemetry.jsonl"], env, folder/"process", timeout=source["seconds"]+150)
                        snapshot = response["snapshot"] if args.minutes_catchup else response
                        rows = [json.loads(line) for line in (folder/"telemetry.jsonl").read_text().splitlines()]
                        result["telemetry"] = telemetry_summary(rows, source["seconds"], loaded)
                        result["telemetry_sha256"] = sha256(folder/"telemetry.jsonl")
                        pcm = hashlib.sha256()
                        audio_bytes = 0
                        for path in sorted((root/"meetings"/snapshot["meeting"]["id"]/"audio").glob("*.pcm")):
                            data = path.read_bytes(); pcm.update(data); audio_bytes += len(data)
                        result.update(pcm_sha256=pcm.hexdigest(), audio_samples=audio_bytes//2,
                            hypothesis_units=units(" ".join(s["text"] for s in snapshot["segments"]), "mixed"))
                        if result["pcm_sha256"] != source["expected_pcm_sha256"]:
                            raise ValueError("Captured PCM differs from the complete paced fixture")
                        counts = error_counts(source["reference_units"], result["hypothesis_units"])
                        result["asr_score"] = dict(**counts, reference_units=len(source["reference_units"]), metric="MER", error_rate=sum(counts.values())/len(source["reference_units"]))
                        if result["asr_score"]["error_rate"] > .45:
                            raise ValueError("ASR smoke ceiling exceeded")
                        for section in snapshot["suggestions"]:
                            for cited in section["sources"]:
                                if not any(s["id"] == cited["segment_id"] and s["revision"] == cited["revision"] and s["text"] == cited["quote"] for s in snapshot["segments"]):
                                    raise ValueError("Minutes cite invalid transcript evidence")
                        stopped = next(row for row in rows if not row["recording"] and not row["transcribing"])
                        result["at_stop"] = {key:stopped[key] for key in ["segments", "translations", "notes_cursor", "sections", "notes_busy", "translation_busy"]}
                        if args.minutes_catchup:
                            result["settle"] = response["settle"]
                            result["at_settled"] = dict(segments=len(snapshot["segments"]), translations=len(snapshot["translations"]), notes_cursor=snapshot["meeting"]["notes_cursor"], sections=len(snapshot["suggestions"]))
                            if not response["settle"]["completed"] or snapshot["meeting"]["notes_cursor"] < max(s["seq"] for s in snapshot["segments"]):
                                raise ValueError("Post-capture minutes/translation did not fully settle within the same budget")
                        log = root/"llm-runtime.log"
                        layers = [list(map(int, pair)) for pair in re.findall(r"offloaded (\d+)/(\d+) layers to GPU", log.read_text(errors="replace") if log.exists() else "")]
                        result["metal_evidence"] = dict(last_worker_layers=layers, log_sha256=sha256(log) if log.exists() else None, fallback=(root/"last-metal-fallback.json").exists())
                        trace_path = root/"inference-trace.jsonl"
                        trace = [json.loads(line) for line in trace_path.read_text().splitlines()] if trace_path.exists() else []
                        workers = [r["metrics"] for r in trace if r["phase"] == "worker"]
                        result["metal_evidence"].update(worker_events=workers, trace_sha256=sha256(trace_path) if trace_path.exists() else None)
                        if loaded and (not workers or any(r["backend"] != "metal" for r in workers)):
                            raise ValueError("A generation task has no Metal worker evidence")
                        if loaded and (not any(0 < used <= total for used, total in layers) or result["metal_evidence"]["fallback"]):
                            raise ValueError("Missing actual Metal offload or a CPU fallback occurred")
                    except Exception as error:
                        result["errors"].append(f"{type(error).__name__}: {error}")
                    result["process"] = read_json(folder/"process/process.json")
                    result["passed"] = not result["errors"]
                    results.append(result)
                    write_json(output, {**report, "in_progress":dict(trial=trial, results=results)})
                    print(arm, "PASS" if result["passed"] else result["errors"], flush=True)
                errors = paired_errors(results, arms)
                report["trials"].append(dict(trial=trial, results=results, errors=errors))
                report["errors"].extend(f"Trial {trial}: {e}" for e in errors)
                write_json(output, report)
        finally:
            if previous_store is None: os.environ.pop("OMASCRIBE_TEST_MODEL_STORE", None)
            else: os.environ["OMASCRIBE_TEST_MODEL_STORE"] = previous_store
        if any(sha256(path) != digest for path, digest in identities.items()):
            report["errors"].append("Source model changed during measurement")
    final = environment(cli)
    for key in ["cli_sha256", "cli_build", "core_and_harness_source_sha256", "model_catalog_sha256", "models_lock_sha256"]:
        if final[key] != report["environment"][key]: report["errors"].append("Source/binary changed: " + key)
    if runtime_inventory(runtime) != native: report["errors"].append("Runtime changed during measurement")
    if sha256(audio) != source["input_wav_sha256"]: report["errors"].append("Replay input changed")
    report["passed"] = len(report["trials"]) == 2 and not report["errors"]
    write_json(output, report)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
