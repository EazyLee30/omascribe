#!/usr/bin/env python3
"""Bounded, offline Metal worker TTL study using the real paced ASR scheduler.

No asset preparation/download or default promotion. Six sequential fresh-process
runs share verified public weights, never a user's meeting workspace. Resource
and idle-observation evidence is required; missing measurements cannot pass.
"""
import argparse
import collections
import hashlib
import json
import math
import os
import pathlib
import platform
import re
import shutil
import subprocess
import tempfile
import process_resources

from benchmark_recording_contention import fixture, settlement_summary, telemetry_summary
from compare_minutes_models import runtime_inventory
from evaluate_asr import asset_identity, error_counts, percentile, units
from harness_support import (REPO, command, environment, isolated_env, read_json,
                             release_borrowed_models, sha256, write_json)

ORDER = ((0, 10, 30), (30, 10, 0))
IDLE_SECONDS = 32
SETTLE_SECONDS = 90
FIXTURE_MANIFEST = "76bcdade9d87b472a53e5f1e801d4aeaa32b5932c18c2a79f43c2f43eee9eb69"
FIXTURE_WAV = "dd812d13c76ace12fbb023e372b3ddd8e90dd0582dc0b7787d4df1d6f9ba28db"
IDENTITIES = ("cli_sha256", "cli_build", "core_and_harness_source_sha256",
              "model_catalog_sha256", "models_lock_sha256", "suite_revision")
PREFERENCES = dict(asr_model="zipformer-small", llm_model="qwen3-1.7b", threads=1,
                   context_size=2048, memory_budget_mib=4096, inference_backend="metal",
                   resource_mode="balanced", auto_notes=True, translation_mode="bilingual",
                   minutes_language="zh", diarization=False, use_memory=False, use_knowledge=False)
RELEASE_COUNTERS = ("ttl_releases", "batch_releases", "lease_releases", "other_releases")
LIFECYCLE_COUNTERS = ("releases", *RELEASE_COUNTERS, "resource_stops", "admission_refusals", "retention_refusals")
ADVERSE_COUNTERS = ("other_releases", "resource_stops", "admission_refusals", "retention_refusals")
# Schema 1 phases emitted by TaskTrace in notes.rs and translation.rs.
TRACE_PHASES = frozenset(("started", "worker", "request_started", "generation", "finished", "abandoned",
                          "retrieval", "article_state", "article_prompt", "article_repair", "chunk",
                          "literal_caption", "validation_failed", "source_unit_repair", "source_week_repair", "batch"))


def measured(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def measured_sum(values):
    values = list(values)
    return sum(values) if all(measured(value) for value in values) else None


def jsonl(path):
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def trace_events(root):
    return [row for name in ("inference-trace.previous.jsonl", "inference-trace.jsonl")
            if (root / name).exists() for row in jsonl(root / name)]


def target_language(text):
    # Exact production bilingual selection; no language guessing from outputs.
    if any("\u3400" <= char <= "\u9fff" for char in text):
        return "en"
    if any(char.isascii() and char.isalpha() for char in text):
        return "zh"
    return None


def snapshot_summary(snapshot, seconds):
    """Current, complete evidence and translation coverage, with exact IDs/times."""
    meeting, segments = snapshot["meeting"], snapshot["segments"]
    if (not segments or snapshot.get("translation_mode") != "bilingual"
            or meeting.get("auto_notes") is not True or meeting.get("duration_ms") != seconds * 1000):
        raise ValueError("Missing complete bilingual meeting")
    if snapshot["runtime"].get("error") is not None:
        raise ValueError("Snapshot reports a runtime error")
    if any(snapshot["runtime"].get(key) is not False for key in
           ("recording", "transcribing", "notes_busy", "translation_busy")):
        raise ValueError("Snapshot still has active work")
    indexed, signature, previous = {}, [], 0
    for segment in segments:
        if (not isinstance(segment.get("id"), str) or not segment["id"]
                or segment["id"] in indexed or segment.get("meeting_id") != meeting["id"]
                or segment.get("run") != meeting["run"] or type(segment.get("seq")) is not int
                or segment["seq"] <= previous or type(segment.get("revision")) is not int
                or segment["revision"] < 1 or not isinstance(segment.get("text"), str)
                or not segment["text"].strip() or type(segment.get("start_ms")) is not int
                or type(segment.get("end_ms")) is not int
                or not 0 <= segment["start_ms"] < segment["end_ms"] <= seconds * 1000):
            raise ValueError("Invalid, duplicate, stale or out-of-range source segment")
        previous = segment["seq"]
        indexed[segment["id"]] = segment
        signature.append([segment[key] for key in ("seq", "start_ms", "end_ms", "revision", "text")])
    if meeting.get("notes_cursor", 0) < previous or not snapshot["suggestions"]:
        raise ValueError("Minutes do not cover the complete transcript")
    expected = {segment["id"]: target_language(segment["text"]) for segment in segments
                if target_language(segment["text"]) is not None}
    if not expected or set(snapshot["translations"]) != set(expected):
        raise ValueError("Missing or unexpected bilingual translations")
    for sid, translation in snapshot["translations"].items():
        segment = indexed[sid]
        if (translation.get("segment_id") != sid or translation.get("revision") != segment["revision"]
                or translation.get("target") != expected[sid] or translation.get("model") != "qwen3-1.7b"
                or not isinstance(translation.get("text"), str) or not translation["text"].strip()
                or not translation.get("cache_key")):
            raise ValueError("Translation identity/revision/language/result is invalid")
    cited = set()
    for section in snapshot["suggestions"]:
        if (section.get("meeting_id") != meeting["id"] or section.get("stale") is not False
                or not isinstance(section.get("text"), str) or not section["text"].strip()
                or not section.get("sources")):
            raise ValueError("Missing or stale minutes evidence")
        for source in section["sources"]:
            segment = indexed.get(source.get("segment_id"))
            if segment is None or any(source.get(key) != segment[other] for key, other in
                                      (("revision", "revision"), ("start_ms", "start_ms"),
                                       ("end_ms", "end_ms"), ("quote", "text"))):
                raise ValueError("Minutes cite an invalid source ID/revision/time/quote")
            cited.add(segment["id"])
    if not cited:
        raise ValueError("Minutes contain no valid cited source")
    return dict(segments=len(segments), translations=len(expected), sections=len(snapshot["suggestions"]),
                notes_cursor=meeting["notes_cursor"], cited_segments=len(cited),
                cited_segment_ratio=len(cited)/len(indexed), cited_segment_ids=sorted(cited),
                source_segment_ids=sorted(indexed), source_signature=signature)


def inference_summary(events):
    """Require closed successful jobs; retain repaired/rejected request costs."""
    if not events:
        raise ValueError("Missing or unsupported inference trace")
    jobs = {}
    for row in events:
        if (not isinstance(row, dict) or type(row.get("schema")) is not int or row["schema"] != 1
                or row.get("task") not in ("translation", "minutes")
                or not isinstance(row.get("job"), str) or not row["job"]
                or not isinstance(row.get("phase"), str) or row["phase"] not in TRACE_PHASES
                or not isinstance(row.get("metrics"), dict)):
            raise ValueError("Missing or unsupported inference trace")
        job, phase, metrics = row["job"], row["phase"], row["metrics"]
        if job not in jobs:
            if phase != "started":
                raise ValueError("Inference job lacks its start")
            jobs[job] = dict(task=row["task"], closed=False, worker=False, seen=set(), pending=set(), completed=False)
            continue
        state = jobs[job]
        if state["task"] != row["task"] or state["closed"] or phase == "started":
            raise ValueError("Inference job identity/start/terminal order is invalid")
        if phase in ("abandoned", "aborted", "cancelled", "failed", "admission_failed", "admission_refused"):
            raise ValueError("An inference job was abandoned or aborted")
        if phase == "worker":
            if state["worker"] or state["seen"]:
                raise ValueError("An inference job has duplicate or late worker evidence")
            state["worker"] = True
        if phase in ("request_started", "generation"):
            attempt = metrics.get("attempt")
            if type(attempt) is not int or attempt < 1:
                raise ValueError("Invalid inference request attempt")
            if phase == "request_started":
                if (metrics.get("endpoint") != "chat" or not state["worker"] or state["pending"]
                        or attempt != len(state["seen"]) + 1):
                    raise ValueError("Missing worker or nonsequential real chat request start")
                state["seen"].add(attempt)
                state["pending"].add(attempt)
            else:
                if attempt not in state["pending"] or type(metrics.get("completed")) is not bool:
                    raise ValueError("Generation lacks its outstanding request start")
                state["pending"].remove(attempt)
                state["completed"] = metrics["completed"]
        if phase == "finished":
            if (metrics.get("committed") is not True or type(metrics.get("items")) is not int
                    or metrics["items"] < 0 or state["pending"]
                    or state["seen"] and not state["completed"]):
                raise ValueError("Unsuccessful or incomplete inference job")
            state["closed"] = True
    if any(not state["closed"] for state in jobs.values()):
        raise ValueError("An inference job has no successful terminal outcome")
    workers = [row["metrics"] for row in events if row["phase"] == "worker"]
    if not workers or any(row.get("backend") != "metal" or type(row.get("reused")) is not bool
                          or not measured(row.get("startup_ms")) for row in workers):
        raise ValueError("Missing actual Metal worker evidence or a fallback occurred")
    results = {}
    for task in ("translation", "minutes"):
        rows = [row for row in events if row.get("task") == task]
        starts = [row for row in rows if row["phase"] == "request_started"]
        generated = [row for row in rows if row["phase"] == "generation"]
        if any(row["metrics"].get("endpoint") != "chat"
               or type(row["metrics"].get("attempt")) is not int
               or row["metrics"]["attempt"] < 1 for row in starts):
            raise ValueError("Invalid real chat request-start metadata")
        committed = {row["job"] for row in rows if row["phase"] == "finished"
                     and row["metrics"].get("committed") is True
                     and type(row["metrics"].get("items")) is int and row["metrics"]["items"] > 0}
        completed = [row for row in generated if row["metrics"].get("completed") is True]
        if not starts or not completed or not any(row["job"] in committed for row in completed):
            raise ValueError("No actual committed " + task + " generation; shortcuts are insufficient")
        started_counts = collections.Counter((row["job"], row["metrics"].get("attempt")) for row in starts)
        generated_counts = collections.Counter((row["job"], row["metrics"].get("attempt")) for row in generated)
        if any(count > started_counts[job] for job, count in generated_counts.items()):
            raise ValueError("Generation trace lacks attempted-request evidence")
        usage = [row["metrics"].get("usage") or {} for row in generated]
        results[task] = dict(attempted_requests=len(starts), observed_responses=len(generated),
            completed_responses=len(completed), incomplete_or_missing_responses=len(starts)-len(completed),
            validation_failures=sum(row["phase"] == "validation_failed" for row in rows),
            repaired_requests=sum(type(row["metrics"].get("attempt")) is int
                                  and row["metrics"]["attempt"] > 1 for row in starts) if task == "minutes" else None,
            literal_captions=sum(row["phase"] == "literal_caption" for row in rows),
            batch_requests=[row["metrics"].get("requests") for row in rows if row["phase"] == "batch"],
            committed_jobs=len(committed),
            **{key: measured_sum(value.get(key) for value in usage) if len(starts) == len(generated) else None for key in
               ("input_tokens", "output_tokens", "cached_tokens", "prefill_ms", "decode_ms")},
            request_ms=measured_sum(row["metrics"].get("request_ms") for row in generated)
                if len(starts) == len(generated) else None,
            first_token_p50_ms=percentile([row["metrics"].get("first_token_ms") for row in generated], .5),
            first_token_p95_ms=percentile([row["metrics"].get("first_token_ms") for row in generated], .95))
    startups = {}
    for reused in (False, True):
        times = [row["startup_ms"] for row in workers if row["reused"] is reused]
        startups["warm" if reused else "cold"] = dict(count=len(times),
            startup_p50_ms=percentile(times, .5), startup_p95_ms=percentile(times, .95),
            startup_total_ms=sum(times))
    return dict(tasks=results, workers=len(workers), startup=startups,
                phases=dict(collections.Counter(row["phase"] for row in events)),
                attempted_requests=sum(row["attempted_requests"] for row in results.values()))


def incurred_inference_costs(events):
    """Best-effort totals survive any quality/evidence gate failure."""
    rows, errors = [], collections.Counter()
    for row in events:
        if not isinstance(row, dict):
            errors["nonobject_rows"] += 1
            continue
        phase = row.get("phase")
        if type(row.get("schema")) is not int or row["schema"] != 1 or not isinstance(phase, str) or phase not in TRACE_PHASES:
            errors["unsupported_events"] += 1
        metrics = row.get("metrics")
        if not isinstance(metrics, dict):
            errors["nonobject_metrics"] += 1
            metrics = {}
        if phase == "generation" and metrics.get("usage") is not None and not isinstance(metrics["usage"], dict):
            errors["nonobject_usage"] += 1
            metrics = dict(metrics, usage=None)
        rows.append(dict(phase=phase, metrics=metrics))
    generated = [row["metrics"] for row in rows if row["phase"] == "generation"]
    attempted = sum(row["phase"] == "request_started" and row["metrics"].get("endpoint") == "chat" for row in rows)
    complete = bool(events) and not errors and attempted == len(generated)
    known_times = [row["request_ms"] for row in generated if measured(row.get("request_ms"))]
    return dict(trace_error_count=sum(errors.values()), trace_errors=dict(errors),
                observed_attempted_requests=attempted if events else None,
                observed_generation_rows=len(generated) if events else None,
                unmeasured_response_costs=sum(not measured(row.get("request_ms")) for row in generated),
                attempted_requests=attempted if events and not errors else None,
                observed_responses=len(generated) if events and not errors else None,
                completed_responses=sum(row.get("completed") is True for row in generated) if events and not errors else None,
                validation_failures=sum(row["phase"] == "validation_failed" for row in rows) if events and not errors else None,
                request_ms=measured_sum(row.get("request_ms") for row in generated) if complete else None,
                observed_request_ms=sum(known_times) if known_times else 0 if complete and not generated else None,
                **{key: measured_sum((row.get("usage") or {}).get(key) for row in generated) if complete else None
                   for key in ("input_tokens", "output_tokens", "cached_tokens", "prefill_ms", "decode_ms")})


def writer_resource_summary(path):
    """Necessary negative screen; a last-spawn diagnostic cannot prove TTL cause."""
    try:
        value = read_json(path)
    except (OSError, ValueError) as error:
        raise ValueError("Unreadable writer-resource diagnostic") from error
    if (not isinstance(value, dict) or value.get("backend") != "metal"
            or type(value.get("budget_mib")) is not int
            or value["budget_mib"] != PREFERENCES["memory_budget_mib"]
            or not measured(value.get("available_before_mib")) or "reason" in value):
        raise ValueError("Missing, unknown or adverse writer-resource diagnostic")
    return dict(backend=value["backend"], budget_mib=value["budget_mib"],
        available_before_mib=value["available_before_mib"], sha256=sha256(path),
        limitation="Last-spawn diagnostic screen only; positive TTL attribution requires sticky lifecycle evidence.")


def worker_lifecycle(worker, ttl):
    """Fail closed on missing/unknown release causes, exits and sticky counters."""
    if (not isinstance(worker, dict) or type(worker.get("leased")) is not bool
            or any(key not in worker for key in ("idle_pid", "idle_for_ms", "expires_in_ms"))):
        raise ValueError("Missing worker lifecycle status")
    life = worker.get("lifecycle")
    if (not isinstance(life, dict) or type(life.get("schema")) is not int or life["schema"] != 1
            or any(type(life.get(key)) is not int or life[key] < 0 for key in LIFECYCLE_COUNTERS)
            or life["releases"] != sum(life[key] for key in RELEASE_COUNTERS)
            or "last_release" not in life or any(life[key] for key in ADVERSE_COUNTERS)
            or (ttl > 0 and (life["batch_releases"] or life["lease_releases"]))
            or (ttl == 0 and life["ttl_releases"])
            or any(type(worker.get(key)) is not int or worker[key] < 0 for key in ("starts", "reuses"))):
        raise ValueError("Missing, inconsistent or adverse lifecycle counters")
    pid = worker.get("idle_pid")
    lifetime = ttl * 1000 if ttl else 2000  # TTL-zero in-batch grace, not configured TTL.
    if pid is None:
        if worker.get("idle_for_ms") is not None or worker.get("expires_in_ms") is not None:
            raise ValueError("Idle clocks exist without an idle worker")
    elif (type(pid) is not int or pid <= 0 or worker["leased"]
            or any(type(worker.get(key)) is not int or worker[key] < 0 for key in ("idle_for_ms", "expires_in_ms"))
            or (abs(worker["idle_for_ms"] + worker["expires_in_ms"] - lifetime) > 2
                if worker["expires_in_ms"] else worker["idle_for_ms"] + 2 < lifetime)):
        raise ValueError("Missing or inconsistent idle identity/deadline")
    release = life.get("last_release")
    if life["releases"] == 0:
        if release is not None:
            raise ValueError("Release metadata exists without a release")
        return life
    if (not isinstance(release, dict) or type(release.get("sequence")) is not int
            or release["sequence"] != life["releases"]
            or type(release.get("pid")) is not int or release["pid"] <= 0
            or release.get("exit_confirmed") is not True
            or type(release.get("ttl_ms")) is not int or release["ttl_ms"] != ttl * 1000
            or "idle_ms" not in release
            or release.get("idle_ms") is not None
                and (type(release["idle_ms"]) is not int or release["idle_ms"] < 0)):
        raise ValueError("Missing, inconsistent or unconfirmed release evidence")
    reason = release.get("reason")
    if reason == "ttl_expired":
        valid = ttl > 0 and life["ttl_releases"] > 0 and type(release.get("idle_ms")) is int and release["idle_ms"] >= ttl * 1000
    elif reason in ("batch_end", "lease_end", "batch_grace_expired"):
        counter = "lease_releases" if reason == "lease_end" else "batch_releases"
        valid = ttl == 0 and life[counter] > 0
        if reason == "batch_grace_expired":
            valid &= type(release.get("idle_ms")) is int and release["idle_ms"] >= 2000
    else:
        valid = False
    if not valid:
        raise ValueError("Release cause is unknown or does not match the configured policy")
    return life


def idle_summary(response, rows, ttl):
    observation = response["idle_observation"]
    idle = [row for row in rows if row.get("phase") == "idle"]
    if (observation.get("requested_seconds") != IDLE_SECONDS
            or observation.get("completed") is not True
            or observation.get("queue_unchanged") is not True
            or not measured(observation.get("elapsed_ms"))
            or not measured(observation.get("started_unix_ms"))
            or not IDLE_SECONDS * 1000 <= observation["elapsed_ms"] <= (IDLE_SECONDS + 2) * 1000
            or len(idle) < 3):
        raise ValueError("Incomplete, changed or missing same-process idle observation")
    initial, final = observation["worker_at_start"], observation["worker_at_end"]
    if type(ttl) is not int or ttl not in (0, 10, 30) or not rows:
        raise ValueError("Unknown TTL policy or missing whole-run telemetry")
    prefix = [row for row in rows if row.get("phase") != "idle"]
    if rows != prefix + idle:
        raise ValueError("Idle telemetry is not the final contiguous phase")
    states = [row.get("worker") for row in prefix] + [initial] + [row.get("worker") for row in idle] + [final]
    previous_worker = previous_life = None
    for worker in states:
        life = worker_lifecycle(worker, ttl)
        if previous_life is not None:
            if (any(life[key] < previous_life[key] for key in LIFECYCLE_COUNTERS)
                    or any(worker[key] < previous_worker[key] for key in ("starts", "reuses"))
                    or life["releases"] == previous_life["releases"] and life.get("last_release") != previous_life.get("last_release")):
                raise ValueError("Worker lifecycle counters regressed or release identity changed")
        previous_worker, previous_life = worker, life
    first_life, last_life = initial["lifecycle"], final["lifecycle"]
    if ttl and initial.get("idle_pid") is None:
        raise ValueError("Keep-alive arm has no idle model to observe")
    if not ttl and initial.get("idle_pid") is not None:
        raise ValueError("TTL zero retained its worker after the batch")
    for worker in (initial, final, *(row["worker"] for row in idle)):
        if (worker.get("leased") is not False
                or any(type(worker.get(key)) is not int or worker[key] < 0 for key in ("starts", "reuses"))
                or any(worker[key] != initial[key] for key in ("starts", "reuses"))):
            raise ValueError("Idle window acquired another lease or changed worker counters")
    previous = -1
    remaining = initial.get("expires_in_ms") if ttl else 0
    disappeared = False
    confirmed_at = None
    for row in idle:
        if (type(row.get("idle_elapsed_ms")) is not int or row["idle_elapsed_ms"] < previous
                or not measured(row.get("unix_ms"))
                or row.get("notes_busy") is not False or row.get("translation_busy") is not False):
            raise ValueError("Idle telemetry is incomplete or has active work")
        previous = row["idle_elapsed_ms"]
        pid = row["worker"].get("idle_pid")
        if ttl and row["worker"]["lifecycle"]["releases"] > first_life["releases"] and confirmed_at is None:
            confirmed_at = row["idle_elapsed_ms"]
        if ttl and initial.get("idle_pid") is not None:
            if pid not in (None, initial["idle_pid"]) or disappeared and pid is not None:
                raise ValueError("Idle worker changed identity or returned after release")
            disappeared |= pid is None
            if pid is None and row["idle_elapsed_ms"] < max(0, remaining - 200):
                raise ValueError("Worker disappeared before its remaining expiry")
        elif pid is not None:
            raise ValueError("TTL zero acquired an idle worker")
        if row["idle_elapsed_ms"] >= remaining + 2000 and pid is not None:
            raise ValueError("Worker remained resident beyond TTL plus two seconds")
    if final.get("idle_pid") is not None:
        raise ValueError("An idle worker remains at the end of observation")
    reaped = observation.get("worker_stopped_after_ms")
    if initial.get("idle_pid") is not None:
        if not measured(reaped) or not max(0, remaining - 200) <= reaped <= remaining + 2000:
            raise ValueError("Missing or late worker-reaping evidence")
        # Removing a pool slot precedes the out-of-lock process wait. Its null
        # PID alone cannot establish that the process exited by the deadline.
        if confirmed_at is None or not max(0, remaining - 200) <= confirmed_at <= remaining + 2000:
            raise ValueError("Missing or late confirmed worker exit")
    elif reaped is not None and not measured(reaped):
        raise ValueError("Invalid no-worker observation")
    # A TTL arm that never retained a worker cannot establish its memory tradeoff.
    if ttl > 0 and initial.get("idle_pid") is None:
        raise ValueError("Keep-alive arm has no idle model to observe")
    if ttl == 0 and initial.get("idle_pid") is not None:
        raise ValueError("TTL zero retained its worker after the batch")
    deltas = {key: last_life[key] - first_life[key] for key in LIFECYCLE_COUNTERS}
    if ttl:
        release = last_life.get("last_release") or {}
        if (deltas["releases"] != 1 or deltas["ttl_releases"] != 1
                or any(deltas[key] for key in LIFECYCLE_COUNTERS if key not in ("releases", "ttl_releases"))
                or release.get("reason") != "ttl_expired" or release.get("pid") != initial["idle_pid"]):
            raise ValueError("Final release is not the one observed worker's confirmed TTL expiry")
    elif any(deltas.values()) or not first_life["releases"]:
        raise ValueError("TTL zero lacks a prior healthy policy release or changed lifecycle while idle")
    return observation


def rss_integral(timeline, start_ms, end_ms, key="tree_rss_mib"):
    """Trapezoidal sampled MiB*s, clipped/interpolated at phase boundaries."""
    if end_ms < start_ms:
        raise ValueError("Resource phase clocks are reversed")
    area, covered = 0., 0.
    for first, last in zip(timeline, timeline[1:]):
        begin, end = max(start_ms, first["unix_ms"]), min(end_ms, last["unix_ms"])
        if end <= begin:
            continue
        a, b = first.get(key), last.get(key)
        if not measured(a) or not measured(b):
            return None
        width = last["unix_ms"] - first["unix_ms"]
        low = a + (b-a) * (begin-first["unix_ms"]) / width
        high = a + (b-a) * (end-first["unix_ms"]) / width
        area += (low+high) / 2 * (end-begin) / 1000
        covered += end-begin
    if abs(covered-(end_ms-start_ms)) > 1:
        raise ValueError("Resource samples do not cover the requested phase")
    return area


def resource_summary(resources, response, rows, process_seconds):
    if not isinstance(resources, dict) or resources.get("schema") != 1:
        raise ValueError("Missing required opt-in resource sampler schema")
    timeline = resources.get("timeline")
    if (not isinstance(timeline, list) or len(timeline) < 3
            or resources.get("sampler_stopped") is not True
            or resources.get("processes_exit_confirmed") is not True
            or resources.get("alive_after_finish") != []
            or resources.get("exit_check_errors") not in (0, [])
            or resources.get("sample_errors") != 0
            or resources.get("timeline_truncated") is not False
            or not resources.get("observed_processes")):
        raise ValueError("Incomplete sampler/exit evidence or a process survived")
    interval = resources.get("interval_seconds")
    if not measured(interval) or interval <= 0 or not measured(process_seconds):
        raise ValueError("Missing measured sampling interval/process duration")
    for key in ("cpu_seconds_sampled", "peak_tree_rss_mib"):
        if not measured(resources.get(key)):
            raise ValueError("Missing required CPU/RSS measurement: " + key)
    previous = None
    for sample in timeline:
        if any(not measured(sample.get(key)) for key in ("elapsed_seconds", "unix_ms", "processes_observed")):
            raise ValueError("Unknown required resource timeline clock/process count")
        if previous is not None and (sample["elapsed_seconds"] <= previous["elapsed_seconds"]
                or sample["unix_ms"] <= previous["unix_ms"]
                or measured(sample.get("cpu_seconds_sampled")) and measured(previous.get("cpu_seconds_sampled"))
                    and sample["cpu_seconds_sampled"] < previous["cpu_seconds_sampled"]
                or sample["unix_ms"]-previous["unix_ms"] > 1500):
            raise ValueError("Non-monotonic or incomplete resource timeline")
        previous = sample
    valid_indices = [index for index, sample in enumerate(timeline)
                     if measured(sample.get("tree_rss_mib")) and sample["processes_observed"] > 0]
    if len(valid_indices) < 3:
        raise ValueError("Missing live process-tree resource interval")
    first_index, last_index = valid_indices[0], valid_indices[-1]
    live = timeline[first_index:last_index+1]
    if len(live) != len(valid_indices) or any(not measured(sample.get("cpu_seconds_sampled")) for sample in live):
        raise ValueError("Unknown CPU/RSS measurement inside the live resource interval")
    head = live[0]["elapsed_seconds"]
    tail = process_seconds-live[-1]["elapsed_seconds"]
    if not 0 <= head <= 2*interval or not -.01 <= tail <= 2*interval:
        raise ValueError("Unobserved resource head/tail exceeds two sampling intervals")
    for process in resources["observed_processes"]:
        if type(process.get("pid")) is not int or process["pid"] <= 0 or any(
                not measured(process.get(key)) for key in ("create_time", "last_seen_elapsed_seconds")):
            raise ValueError("Missing safe observed-process identity")
    if any(not measured(row.get("unix_ms")) for row in rows):
        raise ValueError("Telemetry cannot align resource phases")
    observation = response["idle_observation"]
    idle_rows = [row for row in rows if row.get("phase") == "idle"]
    if not idle_rows:
        raise ValueError("Missing idle phase alignment")
    idle_start = observation["started_unix_ms"]
    idle_end = idle_start+observation["elapsed_ms"]
    capture_start = rows[0]["unix_ms"]
    capture_end = min(next((row["unix_ms"] for row in rows if row.get("phase") in ("settle", "idle")), idle_start), idle_start)
    first, last = live[0]["unix_ms"], live[-1]["unix_ms"]
    process_start = first-head*1000
    process_end = process_start+process_seconds*1000
    if not process_start <= capture_start <= capture_end <= idle_start < idle_end <= process_end+10:
        raise ValueError("Process timeline does not bound capture/settle/idle")
    phases = {}
    for name, start, end in (("unclassified_prefix", process_start, capture_start),
                             ("capture", capture_start, capture_end),
                             ("settle", capture_end, idle_start),
                             ("idle", idle_start, idle_end),
                             ("shutdown_tail", idle_end, max(process_end, idle_end))):
        covered_start, covered_end = max(first, start), min(last, end)
        covered_end = max(covered_start, covered_end)
        uncovered_head = min(end-start, max(0., first-start))/1000
        uncovered_tail = min(end-start, max(0., end-last))/1000
        if name == "idle" and (covered_end <= covered_start or max(uncovered_head, uncovered_tail) > 2*interval):
            raise ValueError("Idle memory interval is missing or has an excessive unobserved boundary")
        rss = rss_integral(live, covered_start, covered_end) if covered_end > covered_start else 0.
        if rss is None:
            raise ValueError("Unknown required RSS phase integral")
        phases[name] = dict(requested_seconds=(end-start)/1000, covered_seconds=(covered_end-covered_start)/1000,
            uncovered_head_seconds=uncovered_head, uncovered_tail_seconds=uncovered_tail,
            rss_mib_seconds_sampled=rss,
            pss_mib_seconds_sampled=rss_integral(live, covered_start, covered_end, "tree_pss_mib")
                if covered_end > covered_start else None)
    return dict(cpu_seconds_sampled=resources["cpu_seconds_sampled"],
        peak_tree_rss_mib=resources["peak_tree_rss_mib"],
        peak_tree_pss_mib=resources.get("peak_tree_pss_mib"),
        rss_mib_seconds_sampled=rss_integral(live, first, last), phases=phases,
        covered_seconds=(last-first)/1000, uncovered_head_seconds=head,
        uncovered_tail_seconds=max(0., tail),
        processes_seen=resources.get("processes_seen"), processes_exit_confirmed=True,
        sample_count=len(timeline), gpu_memory_mib=None, energy_joules=None,
        method="Sampled descendant-tree RSS integrated only over covered intervals; no unobserved endpoints are imputed. Phase boundaries use Unix-ms alignment and explicit uncovered head/tail. CPU and area are sampled lower bounds. Prefix/shutdown samples remain in total cost.")


def comparison_summary(results):
    errors = []
    if len(results) != 6 or [(row.get("trial"), row.get("ttl_seconds")) for row in results] != [
            (trial, ttl) for trial, order in enumerate(ORDER, 1) for ttl in order]:
        errors.append("The pre-registered six runs are incomplete or reordered")
    if any(row.get("passed") is not True for row in results):
        errors.append("An executed arm failed; its cost is retained")
    for key in ("pcm_sha256", "audio_samples", "hypothesis_units", "source_signature"):
        values = [row.get(key) for row in results]
        if not values or any(value is None or value != values[0] for value in values):
            errors.append("Paired recording/source differs or is missing: " + key)
    requests = [{"trial": row["trial"], "ttl_seconds": row["ttl_seconds"],
                 "tasks": (row.get("inference") or {}).get("tasks")} for row in results]
    counts = [[(row.get("inference") or {}).get("tasks", {}).get(task, {}).get("attempted_requests")
               for task in ("translation", "minutes")] for row in results]
    equal = len(counts) == 6 and all(None not in count and count == counts[0] for count in counts)
    return dict(errors=errors, request_counts=requests, same_attempted_request_counts=equal,
                comparable_for_per_request_reuse=not errors and equal,
                limitation="Counts can change through production scheduling/repairs. Unequal counts permit only whole-workload cost reporting, not a pure per-request reuse speedup.")


def frozen_identity(cli, expected=None):
    value = environment(cli)
    if value.get("inference_source_tree_dirty") or not value.get("cli_build"):
        raise ValueError("Freeze clean source and a freshly built CLI before real measurements")
    if value["cli_build"].get("revision_at_compile") != value["suite_revision"]:
        raise ValueError("CLI does not match the frozen source revision")
    if expected is not None and any(value[key] != expected[key] for key in IDENTITIES):
        raise ValueError("Frozen CLI/source/catalog identity changed")
    return value


def cleanup_models(root):
    marker = read_json(root / "model-store/.omascribe-harness-borrow.json")
    if marker:
        release_borrowed_models(root / "model-store", marker["owner"])
    return read_json(root / "model-copy-cleanup.json")


def collect_final_evidence(root, folder, result):
    """Evidence collection must not skip release of this arm's owned links."""
    try:
        for key, collect in (("process", lambda: read_json(folder / "process/process.json")),
                             ("incurred_inference_costs", lambda: incurred_inference_costs(trace_events(root)))):
            try:
                result[key] = collect()
            except Exception as error:
                result[key] = None
                result["errors"].append(f"Final {key}: {type(error).__name__}: {error}")
    finally:
        result["model_cleanup"] = cleanup_models(root)


def file_identity(path):
    stat = path.stat()
    return dict(device=stat.st_dev, inode=stat.st_ino, bytes=stat.st_size,
                mtime_ns=stat.st_mtime_ns, ctime_ns=stat.st_ctime_ns, links=stat.st_nlink)


def clone_asset(source, target, expected_bytes, expected_sha256):
    """Make one owned inode; never hardlink an installed, potentially leased model."""
    before = file_identity(source)
    if before["bytes"] != expected_bytes or target.exists() or target.is_symlink():
        raise ValueError("Invalid source size or non-fresh owned model destination")
    method, clone_failure = "copy2", "not_attempted_on_this_platform"
    if platform.system() == "Darwin":
        try:
            result = subprocess.run(["/bin/cp", "-c", str(source), str(target)],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=30)
            if result.returncode == 0:
                method, clone_failure = "apfs_cp_clone", None
            else:
                clone_failure = "exit_" + str(result.returncode)
        except (OSError, subprocess.TimeoutExpired) as error:
            clone_failure = type(error).__name__
    if method == "copy2":
        # cp may have left a partial destination. This path is newly owned here.
        target.unlink(missing_ok=True)
        shutil.copy2(source, target)
    copied = file_identity(target)
    if (target.is_symlink() or copied["bytes"] != expected_bytes
            or (copied["device"], copied["inode"]) == (before["device"], before["inode"])
            or sha256(target) != expected_sha256):
        raise ValueError("Owned model clone/copy differs from the pinned catalog")
    after = file_identity(source)
    if before != after:
        raise ValueError("Installed model identity changed while preparing the owned clone")
    return dict(method=method, clone_failure=clone_failure,
                source_before=before, source_after=after, owned_identity=copied,
                bytes=expected_bytes, sha256=expected_sha256)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=pathlib.Path, default=REPO / "target/debug/omascribe-cli")
    for name in ("fixtures", "runtime", "asr-model-dir", "llm-model-dir", "output"):
        parser.add_argument("--" + name, type=pathlib.Path, required=True)
    args = parser.parse_args()
    if (platform.system(), platform.machine()) != ("Darwin", "arm64"):
        parser.error("This fixed study requires native Apple Silicon Metal")
    if process_resources._psutil is None:
        parser.error("Install/enable the existing optional psutil sampler before this resource study")
    output, cli, runtime = args.output.resolve(), args.cli.resolve(), args.runtime.resolve()
    artifacts = output.with_suffix(".artifacts")
    if output.exists() or artifacts.exists():
        parser.error("Use fresh output and artifact paths")
    frozen = frozen_identity(cli)
    native = runtime_inventory(runtime)
    if not native:
        parser.error("Missing existing local llama runtime")
    decoder = shutil.which("ffmpeg")
    if decoder is None:
        parser.error("Missing local ffmpeg for paced replay")
    decoder_identity = dict(path=str(pathlib.Path(decoder).resolve()), sha256=sha256(decoder))
    artifacts.mkdir(parents=True)
    audio = artifacts / "paced-public-loop.wav"
    source = fixture(args.fixtures.resolve(), audio, 3)
    if source["seconds"] != 78 or source["manifest_sha256"] != FIXTURE_MANIFEST or source["case"]["sha256"] != FIXTURE_WAV:
        parser.error("This bounded study requires the pinned 78-second public fixture")
    report = dict(schema=1, kind="metal-paced-keep-alive-ttl", environment=frozen,
        fixture=source, runtime_files=native, decoder=decoder_identity, model_clones=[],
        order=[list(order) for order in ORDER], post_capture_budget_seconds=SETTLE_SECONDS,
        idle_observe_seconds=IDLE_SECONDS, results=[], errors=[], passed=False,
        default_promotion=False, defaults_changed=False, study_complete=False,
        manual_semantic_review="Required; complete public-fixture outputs are retained in each response.json and snapshot.json.",
        limitation="Six sequential M5/Metal LLM + CPU ASR runs in reversed blocks, not a full three-arm Latin square. Public read-speech replay, not microphone capture or natural-meeting semantic gold. Cold/warm refer to worker processes; OS page caches are uncontrolled. Preflight asset hashes/links are outside command samples; model admission inside the CLI remains in process cost but precedes worker startup_ms. Sampled CPU is a lower bound; summed RSS can double-count shared pages. GPU allocations, energy and physical unified-memory usage remain unknown. No default or release admission.")
    catalog = read_json(REPO / "model-catalog.json")["models"]
    sources = {}
    previous_store = os.environ.get("OMASCRIBE_TEST_MODEL_STORE")
    with tempfile.TemporaryDirectory(prefix="omascribe-ttl-public-models-") as temp:
        store = pathlib.Path(temp)
        try:
            for name, directory in (("zipformer-small", args.asr_model_dir.resolve()),
                                    ("qwen3-1.7b", args.llm_model_dir.resolve())):
                spec = next(model for model in catalog if model["id"] == name)
                (store / name).mkdir()
                for asset in spec["files"]:
                    path = directory / asset["name"]
                    if path.stat().st_size != asset["bytes"] or sha256(path) != asset["sha256"]:
                        raise ValueError("Existing pinned model differs: " + name)
                    cloned = clone_asset(path, store / name / asset["name"], asset["bytes"], asset["sha256"])
                    report["model_clones"].append(dict(model=name, file=asset["name"], source_path=str(path), **cloned))
                    sources[path] = dict(sha256=asset["sha256"], identity=cloned["source_before"])
            report["models"] = {model["id"]: model["files"] for model in catalog
                                if model["id"] in ("zipformer-small", "qwen3-1.7b")}
            os.environ["OMASCRIBE_TEST_MODEL_STORE"] = str(store)
            halted = False
            for trial, order in enumerate(ORDER, 1):
                for ttl in order:
                    frozen_identity(cli, frozen)
                    if runtime_inventory(runtime) != native:
                        raise ValueError("Runtime changed before a registered arm")
                    folder = artifacts / f"{trial}-ttl-{ttl}"
                    root = folder / "workspace"
                    env = isolated_env(root)
                    env.update(OMASCRIBE_RUNTIME_DIR=str(runtime),
                        OMASCRIBE_MODELS_DIR=str(folder / "no-bundled-models"),
                        OMASCRIBE_ASR_STREAM_HOTWORDS="0", OMASCRIBE_RETRIEVAL_CACHE="0",
                        OMASCRIBE_INCREMENTAL_MINUTES="0", OMASCRIBE_MINUTES_CATCHUP="0",
                        OMASCRIBE_POST_CAPTURE_DRAIN="1", OMASCRIBE_ASR_THREADS="1",
                        OMP_NUM_THREADS="1", OPENBLAS_NUM_THREADS="1")
                    for key in ("LLAMA_ARG_LOG_VERBOSITY", "OMASCRIBE_ACCELERATION_CANDIDATE_VERSION"):
                        env.pop(key, None)
                    preferences = dict(PREFERENCES, llm_keep_alive_seconds=ttl)
                    write_json(root / "preferences.json", preferences)
                    result = dict(trial=trial, ttl_seconds=ttl, preferences=preferences,
                                  errors=[], passed=False)
                    print(f"Trial {trial} TTL {ttl}: 78s replay, 90s settle ceiling, 32s idle", flush=True)
                    try:
                        result["asr_assets"] = asset_identity(cli, root, "zipformer-small", env)
                        if report.get("asr_assets") is None:
                            report["asr_assets"] = result["asr_assets"]
                        elif result["asr_assets"] != report["asr_assets"]:
                            raise ValueError("ASR worker/model identity changed before a registered arm")
                        response = command([cli, "replay-settle", audio, source["seconds"], SETTLE_SECONDS,
                            "--telemetry", folder / "telemetry.jsonl", "--idle-observe-seconds", IDLE_SECONDS],
                            env, folder / "process", timeout=source["seconds"]+SETTLE_SECONDS+IDLE_SECONDS+30,
                            resource_samples=True)
                        write_json(folder / "response.json", response)
                        write_json(folder / "snapshot.json", response["snapshot"])
                        rows = jsonl(folder / "telemetry.jsonl")
                        result["telemetry"] = telemetry_summary(rows, source["seconds"], True)
                        result["settled"] = settlement_summary(response, True, SETTLE_SECONDS)
                        checked = snapshot_summary(response["snapshot"], source["seconds"])
                        result["source_signature"] = checked.pop("source_signature")
                        result["quality"] = checked
                        pcm, audio_bytes = hashlib.sha256(), 0
                        for path in sorted((root / "meetings" / response["snapshot"]["meeting"]["id"] / "audio").glob("*.pcm")):
                            data = path.read_bytes()
                            pcm.update(data)
                            audio_bytes += len(data)
                        result.update(pcm_sha256=pcm.hexdigest(), audio_samples=audio_bytes//2,
                            hypothesis_units=units(" ".join(segment["text"] for segment in response["snapshot"]["segments"]), "mixed"))
                        if result["pcm_sha256"] != source["expected_pcm_sha256"] or audio_bytes != source["seconds"] * 32000:
                            raise ValueError("Captured PCM differs from the complete public fixture")
                        counts = error_counts(source["reference_units"], result["hypothesis_units"])
                        result["asr_score"] = dict(**counts, reference_units=len(source["reference_units"]),
                            error_rate=sum(counts.values())/len(source["reference_units"]), metric="MER")
                        if result["asr_score"]["error_rate"] > .45:
                            raise ValueError("Fixed ASR smoke ceiling exceeded")
                        result["writer_resource"] = writer_resource_summary(root / "last-writer-resource.json")
                        result["inference"] = inference_summary(trace_events(root))
                        result["idle"] = idle_summary(response, rows, ttl)
                        process = read_json(folder / "process/process.json")
                        result["resources"] = resource_summary(read_json(folder / "process/resources.json"), response, rows, process["seconds"])
                        log = root / "llm-runtime.log"
                        layers = [list(map(int, pair)) for pair in re.findall(r"offloaded (\d+)/(\d+) layers to GPU", log.read_text(errors="replace") if log.exists() else "")]
                        result["metal_evidence"] = dict(last_worker_layers=layers,
                            log_sha256=sha256(log) if log.exists() else None,
                            fallback=(root / "last-metal-fallback.json").exists())
                        if not any(0 < used <= total for used, total in layers) or result["metal_evidence"]["fallback"]:
                            raise ValueError("Missing actual Metal offload or a CPU fallback occurred")
                    except Exception as error:
                        result["errors"].append(f"{type(error).__name__}: {error}")
                    finally:
                        collect_final_evidence(root, folder, result)
                        if not (result["model_cleanup"] or {}).get("released"):
                            result["errors"].append("Borrowed model links were not released")
                        result["evidence"] = {str(path.relative_to(folder)): sha256(path) for path in
                            (folder / "response.json", folder / "snapshot.json", folder / "telemetry.jsonl",
                             folder / "process/resources.json", root / "inference-trace.jsonl",
                             root / "last-writer-resource.json") if path.is_file()}
                        result["passed"] = not result["errors"]
                        report["results"].append(result)
                        write_json(output, report)
                    print("PASS" if result["passed"] else result["errors"], flush=True)
                    if not result["passed"]:
                        halted = True
                        break
                if halted:
                    break
            frozen_identity(cli, frozen)
            if report.get("results"):
                worker = runtime / "omascribe-asr"
                if not worker.exists():
                    worker = cli.parent / "omascribe-asr"
                if sha256(worker) != report["asr_assets"]["worker"]["sha256"]:
                    raise ValueError("ASR worker changed during the study")
            if runtime_inventory(runtime) != native or sha256(decoder) != decoder_identity["sha256"]:
                raise ValueError("Runtime or decoder changed during the study")
            if sha256(audio) != source["input_wav_sha256"]:
                raise ValueError("Public replay input changed")
            if any(sha256(path) != value["sha256"] for path, value in sources.items()):
                raise ValueError("Source weight changed during the study")
        except Exception as error:
            report["errors"].append(f"{type(error).__name__}: {error}")
        finally:
            report["source_weight_identities_after"] = {}
            for path, value in sources.items():
                try:
                    after = file_identity(path)
                    report["source_weight_identities_after"][str(path)] = after
                    if after != value["identity"]:
                        report["errors"].append("Installed source model identity changed: " + path.name)
                except OSError as error:
                    report["source_weight_identities_after"][str(path)] = dict(error=type(error).__name__)
                    report["errors"].append("Installed source model identity is unavailable: " + path.name)
            if previous_store is None:
                os.environ.pop("OMASCRIBE_TEST_MODEL_STORE", None)
            else:
                os.environ["OMASCRIBE_TEST_MODEL_STORE"] = previous_store
    report["owned_model_store_cleanup"] = dict(removed=not store.exists(),
        method="TemporaryDirectory removes only the study-owned clones/copies; per-arm links were separately released.",
        note="No physical disk-space or logical-bytes-freed claim. Source identity comparison excludes atime.")
    if not report["owned_model_store_cleanup"]["removed"]:
        report["errors"].append("Study-owned model store remains after cleanup")
    report["comparison"] = comparison_summary(report["results"])
    processes = [row.get("process") or {} for row in report["results"]]
    costs = [row.get("incurred_inference_costs") or {} for row in report["results"]]
    report["all_executed_costs"] = dict(
        runs=len(processes), failed_runs=sum(row.get("passed") is not True for row in report["results"]),
        process_seconds=measured_sum(row.get("seconds") for row in processes),
        cpu_seconds_sampled=measured_sum((row.get("resources") or {}).get("cpu_seconds_sampled") for row in processes),
        **{key: measured_sum(row.get(key) for row in costs) for key in
           ("attempted_requests", "observed_responses", "validation_failures", "request_ms",
            "input_tokens", "output_tokens", "cached_tokens", "prefill_ms", "decode_ms")})
    report["errors"].extend(report["comparison"]["errors"])
    report["study_complete"] = len(report["results"]) == 6
    report["passed"] = report["study_complete"] and not report["errors"]
    write_json(output, report)
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
