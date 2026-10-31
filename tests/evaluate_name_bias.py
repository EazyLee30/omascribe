#!/usr/bin/env python3
"""Offline, three-arm production ASR name calibration; no downloads or LLM calls.

Every public clip runs in three NEW workspaces: empty document, supplied positive
document, and an unspoken-name document. --smoke is explicitly incomplete. A
successful execution is evidence collection, never quality/release admission.
"""
import argparse
import hashlib
import json
import math
import os
import pathlib
import random
import re
import shutil
import sqlite3
import statistics
import sys
import time
import uuid
import wave
from contextlib import closing

from evaluate import seed
from evaluate_asr import error_counts, percentile, units
from harness_support import REPO, command, environment, isolated_env, read_json, sha256, write_json
from prepare_name_bias_fixtures import occurs

ARMS = ("baseline", "positive", "negative")
LANGUAGE = "en_us"
SAMPLE_RATE = 16000
KNOWN_FLAGS = {
    "OMASCRIBE_ASR_STREAM_HOTWORDS": "0",
    "OMASCRIBE_RETRIEVAL_CACHE": "0",
    "OMASCRIBE_INCREMENTAL_MINUTES": "0",
    "OMASCRIBE_MINUTES_CATCHUP": "0",
    "OMASCRIBE_POST_CAPTURE_DRAIN": "0",
    "OMASCRIBE_PREPARE_SOURCES": "0",
    "OMASCRIBE_TRANSLATION_DIAGNOSTICS": "0",
    "OMASCRIBE_ASR_THREADS": "1", "OMP_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
}
PREFERENCES = dict(asr_model="zipformer-small", auto_notes=False, translation_mode="off",
                   diarization=False, use_memory=False, use_knowledge=False, threads=1,
                   llm_keep_alive_seconds=0)


def require(value, message):
    if not value:
        raise ValueError(message)


def phrase_present(text, phrase):
    # Reuse the prep contract for possessives/initialisms; WER retains its
    # separate evaluate_asr.units normalization without entity-specific edits.
    return occurs(phrase, text)


def text_field(value, label, limit=16000):
    require(isinstance(value, str) and bool(value.strip()) and len(value) <= limit, "Invalid " + label)
    require(all(c.isprintable() or c in "\n\t\r" for c in value), "Control characters in " + label)
    return value


def name_field(value, label):
    value = text_field(value, label, 80)
    require(value == value.strip() and all(c.isalnum() or c == " " for c in value),
            "Name cannot be represented on the production hotword wire: " + label)
    return value


def safe_audio(manifest_path, item):
    name = item.get("file")
    require(isinstance(name, str) and name not in ("", ".", "..")
            and pathlib.PurePath(name).name == name and "/" not in name and "\\" not in name
            and ":" not in name, "Audio file must be a safe relative basename")
    path = manifest_path.parent / name
    require(not path.is_symlink() and path.is_file(), "Audio must be a regular, non-symlink file")
    require(path.resolve().parent == manifest_path.parent.resolve(), "Audio escapes the manifest directory")
    return path


def file_identity(path):
    before = path.stat()
    digest = sha256(path)
    after = path.stat()
    fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
    require(all(getattr(before, key) == getattr(after, key) for key in fields), "Asset changed while hashing")
    return dict(path=str(path), sha256=digest, **{key[3:]: getattr(after, key) for key in fields})


def wave_identity(path):
    with wave.open(str(path), "rb") as audio:
        require((audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getcomptype())
                == (1, 2, SAMPLE_RATE, "NONE"), "Prepared audio must be mono 16-bit 16 kHz PCM WAV")
        samples = audio.getnframes()
        require(0 < samples <= SAMPLE_RATE * 120, "Use bounded 0–120 second public clips")
        pcm = audio.readframes(samples)
        require(len(pcm) == samples * 2, "Truncated prepared WAV")
    return dict(samples=samples, sample_rate=SAMPLE_RATE, seconds=samples / SAMPLE_RATE,
                pcm_sha256=hashlib.sha256(pcm).hexdigest())


def load_inputs(selection_path, manifest_path, smoke=None):
    require(not selection_path.is_symlink() and not manifest_path.is_symlink(), "Do not symlink manifests")
    selection, manifest = read_json(selection_path), read_json(manifest_path)
    require(isinstance(selection, dict) and isinstance(selection.get("cases"), list), "Invalid selection")
    require(isinstance(manifest, dict) and manifest.get("schema") == 1 and manifest.get("status") == "complete",
            "Invalid or unfinished audio manifest")
    require(manifest.get("selection_sha256") == sha256(selection_path), "Audio manifest uses another selection")
    cases, rows = selection["cases"], manifest.get("audio")
    require(len(cases) == 100, "The frozen selection must contain exactly 100 source clusters")
    chosen = cases[:smoke] if smoke is not None else cases
    chosen_ids = {case.get("name") for case in chosen}
    require(isinstance(rows, list) and len(rows) in ({100, len(chosen)} if smoke is not None else {100}),
            "The prepared manifest must contain the frozen full corpus or exact smoke prefix")
    ids, clusters, audio = set(), set(), {}
    for row in rows:
        require(isinstance(row, dict) and isinstance(row.get("id"), str) and row["id"] not in audio,
                "Invalid/duplicate audio ID")
        audio[row["id"]] = row
    for case in cases:
        name = case.get("name")
        require(isinstance(name, str) and re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,95}", name)
                and name not in ids, "Invalid/duplicate case ID")
        ids.add(name)
        cluster = case.get("sentence_id")
        require(isinstance(cluster, (str, int)) and not isinstance(cluster, bool)
                and str(cluster) and str(cluster) not in clusters, "Repeated/invalid source cluster")
        clusters.add(str(cluster))
        reference = text_field(case.get("reference"), "reference")
        raw_reference = text_field(case.get("raw_reference"), "raw reference")
        for arm, field in (("positive", "expected_name"), ("negative", "forbidden_name")):
            spec = case.get(arm)
            require(isinstance(spec, dict), "Missing " + arm + " case")
            label = name_field(spec.get(field), arm + " name")
            note = text_field(spec.get("note"), arm + " document")
            require(phrase_present(note, label), "Supplied note does not contain its expected name")
            require(phrase_present(reference, label) == (arm == "positive"),
                    "Positive name must be spoken and negative hint must be absent from the gold reference")
            require(phrase_present(raw_reference, label) == (arm == "positive"),
                    "Name annotation differs from the original raw gold reference")
        require(case["positive"]["expected_name"].casefold() != case["negative"]["forbidden_name"].casefold(),
                "Negative hint equals the spoken name")
        if name not in audio:
            require(smoke is not None and name not in chosen_ids, "Missing prepared clip: " + name)
            continue
        row = audio[name]
        require(row.get("reference") == reference and row.get("sample_rate") == SAMPLE_RATE,
                "Prepared reference/rate differs from selection")
        require(type(row.get("samples")) is int and row["samples"] > 0, "Invalid prepared sample count")
        for key in ("sha256", "source_audio_sha256"):
            require(isinstance(row.get(key), str) and re.fullmatch(r"[a-f0-9]{64}", row[key]), "Invalid " + key)
        safe_audio(manifest_path, row)
    require(set(audio) == ids or smoke is not None and set(audio) == chosen_ids,
            "Prepared clip IDs differ from selection/prefix")
    return selection, [(case, audio[case["name"]]) for case in chosen]


def asset_inventory(cli, runtime, models_dir, selection, manifest):
    spec = next(m for m in read_json(REPO / "model-catalog.json")["models"] if m["id"] == "zipformer-small")
    model = models_dir / "zipformer"
    require(model.is_dir() and not model.is_symlink(), "Use an existing regular bundle/zipformer model directory")
    assets = {}
    for item in spec["files"]:
        path = model / item["name"]
        require(path.is_file() and not path.is_symlink(), "Model asset is missing or symlinked")
        identity = file_identity(path)
        require(identity["size"] == item["bytes"] and identity["sha256"] == item["sha256"], "Pinned model hash mismatch")
        assets["model/" + item["name"]] = identity
    worker = runtime / ("omascribe-asr.exe" if os.name == "nt" else "omascribe-asr")
    require(worker.is_file(), "Missing actual ASR worker in --runtime")
    for path in sorted(runtime.rglob("*")):
        if path.is_file():
            assets["runtime/" + str(path.relative_to(runtime))] = file_identity(path)
    for key, path in (("cli", cli), ("selection", selection), ("audio_manifest", manifest),
                      ("catalog", REPO / "model-catalog.json"), ("models_lock", REPO / "models.lock.json")):
        assets[key] = file_identity(path)
    ffmpeg = shutil.which("ffmpeg")
    require(ffmpeg is not None, "Missing actual PATH ffmpeg executable")
    assets["path_ffmpeg"] = dict(located_path=ffmpeg, **file_identity(pathlib.Path(ffmpeg).resolve()))
    return assets


def arm_environment(root, runtime, models_dir, mode):
    # isolated_env otherwise honors an inherited whole model-store copy recipe.
    # This experiment uses a separately prepared read-only bundle, never that store.
    inherited = os.environ.pop("OMASCRIBE_TEST_MODEL_STORE", None)
    try:
        env = isolated_env(root)
    finally:
        if inherited is not None:
            os.environ["OMASCRIBE_TEST_MODEL_STORE"] = inherited
    data_dir = env["OMASCRIBE_DATA_DIR"]
    env = {key: value for key, value in env.items()
           if not key.startswith(("OMASCRIBE_", "LLAMA_ARG_"))}
    env["OMASCRIBE_DATA_DIR"] = data_dir
    env.update(KNOWN_FLAGS)
    env.update(OMASCRIBE_MODELS_DIR=str(models_dir), OMASCRIBE_RUNTIME_DIR=str(runtime),
               OMASCRIBE_ASR_STREAM_HOTWORDS="1" if mode == "stream" else "0")
    return env


def database_evidence(root, mid):
    uri = (root / "meetings.sqlite3").as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True)) as db:
        counts = {table: db.execute("SELECT count(*) FROM " + table).fetchone()[0] for table in
                  ("meetings", "notes", "library_entries", "suggestions", "document_history",
                   "inference_jobs", "translations", "translation_parts", "recording_takes")}
        meeting = json.loads(db.execute("SELECT json FROM meetings WHERE id=?", (mid,)).fetchone()[0])
        corrected = [json.loads(row[0]) for row in db.execute(
            "SELECT json FROM segments WHERE meeting_id=? AND run=? ORDER BY seq", (mid, meeting["run"]))]
        raw, origins = [], []
        for ordinal, segment in enumerate(corrected, 1):
            require(segment["seq"] == ordinal and segment["id"] == f"{mid}:1:{ordinal}",
                    "Unexpected fresh-worker sequence or segment ID")
            row = db.execute("SELECT json FROM segment_history WHERE id=? AND revision=1", (segment["id"],)).fetchone()
            if row:
                original, origin = json.loads(row[0]), "segment_history:revision1"
            else:
                require(segment["revision"] == 1, "Missing raw ASR revision; corrected text cannot stand in for raw")
                original, origin = segment, "unchanged_current:revision1"
            require(original["revision"] == 1, "Raw ASR revision must be one")
            require(all(original.get(k) == segment.get(k) for k in
                        ("id", "meeting_id", "run", "seq", "start_ms", "end_ms")), "Raw source identity changed")
            require(original["meeting_id"] == mid and original["run"] == 1, "Unexpected source run/meeting")
            require(not db.execute("SELECT 1 FROM segment_text_edits WHERE id=? AND origin!='terminology'",
                                   (segment["id"],)).fetchone(), "Unexpected human/unknown text edit")
            raw.append(original)
            origins.append(dict(id=segment["id"], raw_origin=origin, corrected_revision=segment["revision"]))
        notes = [json.loads(row[0]) for row in db.execute("SELECT json FROM notes WHERE meeting_id=?", (mid,))]
    return dict(meeting=meeting, raw=raw, corrected=corrected, raw_origins=origins, counts=counts, notes=notes)


def score(text, case):
    reference = units(case["reference"], LANGUAGE)
    hypothesis = units(text, LANGUAGE)
    require(bool(reference), "Gold reference has no WER units")
    counts = error_counts(reference, hypothesis)
    return dict(**counts, edits=sum(counts.values()), reference_units=len(reference),
                wer=sum(counts.values()) / len(reference),
                expected_name_present=phrase_present(text, case["positive"]["expected_name"]),
                forbidden_name_present=phrase_present(text, case["negative"]["forbidden_name"]),
                expected_exact_spelling_present=case["positive"]["expected_name"] in text)


def validate_reference(reference, audio_row):
    require(isinstance(reference, dict) and reference.get("schema") == 1
            and reference.get("decoder") == "native_wav_import", "Missing canonical production WAV reference")
    require(reference.get("input_sha256") == audio_row["sha256"]
            and type(reference.get("samples")) is int and reference["samples"] == audio_row["samples"]
            and reference.get("sample_rate") == SAMPLE_RATE
            and type(reference.get("pcm_bytes")) is int and reference["pcm_bytes"] == reference["samples"] * 2,
            "Canonical production reference input/duration differs from prepared audio")
    require(isinstance(reference.get("pcm_sha256"), str)
            and re.fullmatch(r"[a-f0-9]{64}", reference["pcm_sha256"]), "Invalid canonical production PCM identity")


def compact_resources(process, path):
    # command() already stores the compact summary in process.json. The full
    # timeline stays once in resources.json, never in every report rewrite.
    value = (process or {}).get("resources")
    if not isinstance(value, dict):
        value = read_json(path)
    return ({key: item for key, item in value.items() if key not in ("timeline", "observed_processes")}
            if isinstance(value, dict) else None)


def canonical_reference(args, audio_row, folder):
    """One model-free production PcmReader pass, independent of all three arms."""
    path = safe_audio(args.audio_manifest, audio_row)
    result = dict(passed=False, canonical=None, errors=[], process=None, resources=None)
    env = arm_environment(folder / "workspace", args.runtime, args.models_dir, args.stream_mode)
    try:
        before, prepared = file_identity(path), wave_identity(path)
        result.update(source_before=before, prepared=prepared)
        require(before["sha256"] == audio_row["sha256"] and prepared["samples"] == audio_row["samples"],
                "Prepared audio changed before canonical decoding")
        reference = command([args.cli, "decode-wav-info", path], env, folder,
                            timeout=args.timeout, resource_samples=True)
        result["canonical"] = reference
        validate_reference(reference, audio_row)
        result["captured_input_differs"] = reference["pcm_sha256"] != prepared["pcm_sha256"]
    except Exception as error:
        result["errors"].append(f"{type(error).__name__}: {error}")
    finally:
        try:
            result["source_after"] = file_identity(path)
            if result.get("source_before") is not None:
                require(result["source_after"] == result["source_before"], "Canonical decode changed prepared audio")
        except Exception as error:
            result["errors"].append(f"Reference audio revalidation {type(error).__name__}: {error}")
        result["process"] = read_json(folder / "process.json")
        result["resources"] = compact_resources(result["process"], folder / "resources.json")
        result["resources_file"] = "resources.json"
        if result["process"] is None:
            result["errors"].append("Canonical reference command record is missing")
        result["passed"] = not result["errors"]
        write_json(folder / "result.json", result)
    return result


def recording_evidence(root, mid, audio, reference):
    require(reference.get("schema") == 1 and reference.get("decoder") == "native_wav_import",
            "Recording requires the independent canonical production reference")
    directory = root / "meetings" / mid
    chunks = sorted((directory / "audio").glob("*.pcm"))
    require(bool(chunks), "No archived PCM")
    digest, count = hashlib.sha256(), 0
    for index, path in enumerate(chunks):
        require(path.name == f"{index:06}.pcm" and not path.is_symlink(), "Invalid archived PCM sequence")
        payload = path.read_bytes()
        require(len(payload) % 2 == 0, "Partial PCM sample")
        digest.update(payload)
        count += len(payload) // 2
    result = dict(samples=count, pcm_sha256=digest.hexdigest(), chunks=len(chunks))
    require(count == audio["samples"] == reference["samples"] and reference["sample_rate"] == SAMPLE_RATE
            and reference["pcm_bytes"] == count * 2 and result["pcm_sha256"] == reference["pcm_sha256"],
            "Archived PCM differs from canonical production decode")
    takes = sorted((directory / "files").glob("recording-*.wav"))
    require(len(takes) == 1, "Fresh arm must have exactly one recording take")
    archived = wave_identity(takes[0])
    require(archived["samples"] == reference["samples"] and archived["sample_rate"] == reference["sample_rate"]
            and archived["pcm_sha256"] == reference["pcm_sha256"], "Archived WAV payload differs from canonical production decode")
    result["archived_wav_sha256"] = sha256(takes[0])
    result.update(prepared_pcm_sha256=audio["pcm_sha256"], reference_pcm_sha256=reference["pcm_sha256"],
                  captured_input_differs=result["pcm_sha256"] != audio["pcm_sha256"])
    return result


def run_arm(args, case, audio_row, folder, arm, position, reference):
    root, mid = folder / "workspace", str(uuid.uuid4())
    env = arm_environment(root, args.runtime, args.models_dir, args.stream_mode)
    note = "" if arm == "baseline" else case[arm]["note"]
    result = dict(arm=arm, position=position, workspace=str(root), meeting_id=mid, note=note,
                  errors=[], raw=None, corrected=None, raw_text=None, corrected_text=None,
                  acoustic_metrics=None, corrected_metrics=None, rtf=None, resources=None, passed=False)
    audio_path = safe_audio(args.audio_manifest, audio_row)
    try:
        validate_reference(reference, audio_row)
        write_json(root / "preferences.json", PREFERENCES)
        command([args.cli, "diagnose"], env, folder / "diagnose", timeout=30)
        with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
            require(all(db.execute("SELECT count(*) FROM " + table).fetchone()[0] == 0 for table in
                        ("meetings", "notes", "segments", "library_entries", "inference_jobs")), "Workspace is not isolated/empty")
        seed(root, dict(name=case["name"], focus=""), mid, automatic=False)
        with closing(sqlite3.connect(root / "meetings.sqlite3")) as db:
            note_id = db.execute("SELECT id FROM notes WHERE meeting_id=?", (mid,)).fetchone()[0]
        saved = command([args.cli, "edit-note", mid, note_id, note, "document"], env, folder / "save-note", timeout=30)
        require(saved.get("content") == note and saved.get("kind") == "document", "CLI did not save exact supplied document")
        before = database_evidence(root, mid)
        require(not before["raw"] and before["counts"]["recording_takes"] == 0, "Arm inherited old ASR/audio")
        audio_before, wave_before = file_identity(audio_path), wave_identity(audio_path)
        require(audio_before["sha256"] == audio_row["sha256"] and wave_before["samples"] == audio_row["samples"], "Audio identity mismatch before ASR")
        result["audio_before"] = audio_before
        snapshot = command([args.cli, "append", mid, audio_path], env, folder / "append",
                           timeout=args.timeout, resource_samples=True)
        require(file_identity(audio_path) == audio_before, "Prepared audio changed during arm")
        evidence = database_evidence(root, mid)
        result.update(raw=evidence["raw"], corrected=evidence["corrected"], raw_origins=evidence["raw_origins"],
                      database_counts=evidence["counts"])
        result["raw_text"] = " ".join(s["text"] for s in evidence["raw"])
        result["corrected_text"] = " ".join(s["text"] for s in evidence["corrected"])
        require(snapshot["segments"] == evidence["corrected"], "CLI snapshot differs from persisted corrected sources")
        require(evidence["meeting"]["status"] == "ready" and evidence["meeting"]["run"] == 1
                and snapshot["runtime"].get("error") is None, "ASR did not finish cleanly")
        require(not any(snapshot["runtime"].get(key) for key in
                        ("recording", "transcribing", "notes_busy", "translation_busy", "diarization_busy")), "Background work remains")
        require(evidence["counts"]["meetings"] == evidence["counts"]["notes"] == evidence["counts"]["recording_takes"] == 1,
                "Unexpected meeting/note/take history")
        require(all(evidence["counts"][key] == 0 for key in
                    ("library_entries", "suggestions", "document_history", "inference_jobs", "translations", "translation_parts")),
                "Shared library pollution or unexpected LLM/translation/minutes work")
        require(len(evidence["notes"]) == 1 and evidence["notes"][0]["content"] == note, "Document changed during arm")
        require(bool(evidence["raw"]), "Speech produced no ASR source")
        previous = 0
        duration = reference["samples"] * 1000 // SAMPLE_RATE
        require(snapshot["meeting"].get("duration_ms") == duration
                and snapshot["runtime"].get("processed_ms") == duration,
                "Capture duration or ASR processed tail differs from canonical sample count")
        result["processed_ms"] = snapshot["runtime"]["processed_ms"]
        result["duration_ms"] = snapshot["meeting"]["duration_ms"]
        for segment in evidence["raw"]:
            require(previous <= segment["start_ms"] <= segment["end_ms"] <= duration, "ASR timestamps overlap or escape audio")
            previous = segment["end_ms"]
        result["timestamps_valid"] = True
        result["recording"] = recording_evidence(root, mid, wave_before, reference)
        audio_dir = root / "meetings" / mid / "audio"
        wire_path = audio_dir / "hotwords.txt"
        wire = wire_path.read_text(encoding="utf-8")
        words = [line.strip() for line in wire.splitlines() if line.strip()]
        require(len(wire.encode()) <= 16000 and len(words) <= 80 and
                all(word == word.upper() and len(word) <= 80 and all(c.isalnum() or c == " " for c in word) for word in words), "Invalid production hotword wire")
        target = None if arm == "baseline" else case[arm]["expected_name" if arm == "positive" else "forbidden_name"].upper()
        require(not words if target is None else target in words, "Expected hint did not reach publication wire")
        decoder = read_json(audio_dir / "asr-config.json")
        require(isinstance(decoder, dict) and decoder.get("fallback") is None, "Missing/fallback ASR decoder diagnostics")
        require(decoder.get("hotword_count") == len(words) and decoder.get("decoder") ==
                ("modified_beam_search" if words else "greedy_search"), "Decoder did not apply the published hints")
        require(decoder.get("hotword_update_mode") == ("stream_experiment" if args.stream_mode == "stream" else "recognizer"), "Wrong decoder experiment mode")
        require((decoder.get("hotword_input") or {}).get("status") == "accepted", "Decoder rejected context")
        result.update(wire=dict(words=words, terms=len(words), bytes=len(wire.encode()), sha256=sha256(wire_path),
                                publication=read_json(audio_dir / "hotwords-input.json")), decoder=decoder,
                      acoustic_metrics=score(result["raw_text"], case), corrected_metrics=score(result["corrected_text"], case),
                      rtf=snapshot["runtime"].get("asr_rtf"))
        require(type(result["rtf"]) in (float, int) and math.isfinite(result["rtf"]) and result["rtf"] >= 0, "Unknown/invalid ASR RTF")
    except Exception as error:
        result["errors"].append(f"{type(error).__name__}: {error}")
    finally:
        try:
            result["audio_after"] = file_identity(audio_path)
            if result.get("audio_before") is not None:
                require(result["audio_after"] == result["audio_before"], "Prepared audio identity changed")
        except Exception as error:
            result["errors"].append(f"Audio revalidation {type(error).__name__}: {error}")
        process = read_json(folder / "append" / "process.json")
        result["process"] = process
        result["resources"] = compact_resources(process, folder / "append" / "resources.json")
        result["resources_file"] = "append/resources.json"
        result["wall_seconds"] = (process or {}).get("seconds")
        # Preserve failed/partial ASR evidence as such, never score it as a complete result.
        if result["raw"] is None and (root / "meetings.sqlite3").exists():
            try:
                evidence = database_evidence(root, mid)
                result.update(raw=evidence["raw"], corrected=evidence["corrected"], raw_origins=evidence["raw_origins"])
            except Exception as error:
                result["raw_evidence_error"] = f"{type(error).__name__}: {error}"
        resources = result["resources"] or {}
        # Successful and rejected arms both retain the actual publication and
        # decoder observations. A missing diagnostic stays unknown, not zero.
        audio_dir = root / "meetings" / mid / "audio"
        for key, filename in (("decoder", "asr-config.json"), ("publication", "hotwords-input.json")):
            if key not in result:
                try:
                    result[key] = read_json(audio_dir / filename)
                except (OSError, ValueError) as error:
                    result[key + "_read_error"] = f"{type(error).__name__}: {error}"
        if "wire" not in result:
            try:
                path = audio_dir / "hotwords.txt"
                if path.exists():
                    payload = path.read_bytes()
                    words = [line.strip() for line in payload.decode("utf-8").splitlines() if line.strip()]
                    result["wire"] = dict(words=words, terms=len(words), bytes=len(payload), sha256=hashlib.sha256(payload).hexdigest())
            except (OSError, ValueError) as error:
                result["wire_read_error"] = f"{type(error).__name__}: {error}"
        if process is not None and resources.get("processes_exit_confirmed") is not True:
            result["errors"].append("Owned process exit not confirmed; stop before another arm")
        result["passed"] = not result["errors"]
        write_json(folder / "result.json", result)
    return result


def observed_pair(cases, layer, arm, field):
    rows, discordant = [], dict(gain=0, loss=0, both=0, neither=0, unknown=0)
    for case in cases:
        arms = {item["arm"]: item for item in case["arms"]}
        original, changed = arms.get("baseline", {}), arms.get(arm, {})
        before = (original.get(layer) or {}).get(field) if original.get("passed") is True else None
        after = (changed.get(layer) or {}).get(field) if changed.get("passed") is True else None
        if type(before) is not bool or type(after) is not bool:
            discordant["unknown"] += 1
            continue
        rows.append(int(after) - int(before))
        discordant["gain" if after and not before else "loss" if before and not after else "both" if before else "neither"] += 1
    complete = len(rows) == len(cases) and bool(cases)
    interval = None
    if complete:
        rng = random.Random(1729)
        values = [statistics.mean(rng.choices(rows, k=len(rows))) for _ in range(2000)]
        interval = [percentile(values, .025), percentile(values, .975)]
    return dict(observed_delta=statistics.mean(rows) if complete else None, measured_clusters=len(rows),
                expected_clusters=len(cases), discordant_counts=discordant,
                cluster_bootstrap_95=interval, bootstrap_seed=1729, bootstrap_resamples=2000,
                direction="higher is better" if field == "expected_name_present" else "lower is better")


def paired_wer(cases, layer, arm):
    rows = []
    for case in cases:
        arms = {item["arm"]: item for item in case["arms"]}
        before, after = (arms.get(name, {}).get(layer) if arms.get(name, {}).get("passed") is True else None
                         for name in ("baseline", arm))
        if before is not None and after is not None:
            rows.append((after["edits"] - before["edits"], before["reference_units"]))
    complete = len(rows) == len(cases) and bool(cases)
    delta, interval = None, None
    if complete:
        delta = sum(edits for edits, _ in rows) / sum(words for _, words in rows)
        rng = random.Random(1729)
        values = []
        for _ in range(2000):
            sample = rng.choices(rows, k=len(rows))
            values.append(sum(edits for edits, _ in sample) / sum(words for _, words in sample))
        interval = [percentile(values, .025), percentile(values, .975)]
    return dict(observed_delta=delta, cluster_bootstrap_95=interval, measured_clusters=len(rows),
                expected_clusters=len(cases), direction="lower is better", bootstrap_seed=1729, bootstrap_resamples=2000)


def aggregate(results, comparison_valid=None):
    groups = {}
    for arm in ARMS:
        rows = [item for case in results for item in case["arms"] if item["arm"] == arm]
        summary = dict(attempted=len(rows), successful=sum(row["passed"] for row in rows), layers={})
        for layer in ("acoustic_metrics", "corrected_metrics"):
            measured = [row[layer] for row in rows if row.get("passed") is True and row.get(layer) is not None]
            complete = len(measured) == len(results) and bool(results)
            denominator = sum(item["reference_units"] for item in measured)
            summary["layers"][layer] = dict(measured=len(measured), expected=len(results),
                wer=sum(item["edits"] for item in measured) / denominator if complete and denominator else None,
                name_recall=statistics.mean(item["expected_name_present"] for item in measured) if complete else None,
                forbidden_insertion_rate=statistics.mean(item["forbidden_name_present"] for item in measured) if complete else None)
        groups[arm] = summary
    return dict(comparison_valid=comparison_valid,
        interpretation="diagnostic only; source/asset identity not established" if comparison_valid is not True else
                       "observed paired measurements; not an approved quality gate",
        arms=groups, paired={layer:dict(
        positive_name_recall=observed_pair(results, layer, "positive", "expected_name_present"),
        negative_forbidden_insertion=observed_pair(results, layer, "negative", "forbidden_name_present"),
        positive_wer=paired_wer(results, layer, "positive"), negative_wer=paired_wer(results, layer, "negative"))
        for layer in ("acoustic_metrics", "corrected_metrics")})


def evaluate(args):
    args.output = args.output.absolute()
    artifacts = args.output.with_suffix(".artifacts")
    require(not args.output.exists() and not artifacts.exists(), "Use a fresh report and artifact path")
    for key in ("selection", "audio_manifest", "cli", "runtime", "models_dir"):
        setattr(args, key, getattr(args, key).absolute())
    selection, chosen = load_inputs(args.selection, args.audio_manifest, args.smoke)
    artifacts.mkdir(parents=True, exist_ok=False)
    report = dict(schema=1, kind="real-production-note-to-asr-name-calibration", passed=False,
                  execution_passed=False, full_gate=False, full_corpus_complete=False, quality_gate_passed=None,
                  release_admission=False, smoke=args.smoke, expected_clusters=len(chosen),
                  expected_asr_invocations=len(chosen) * 3, stream_mode=args.stream_mode,
                  expected_reference_invocations=len(chosen),
                  decoder_promotion=False, preferences=PREFERENCES,
                  known_environment_flags={**KNOWN_FLAGS, "OMASCRIBE_ASR_STREAM_HOTWORDS": "1" if args.stream_mode == "stream" else "0"},
                  inherited_flag_policy="Remove all OMASCRIBE_* and LLAMA_ARG_* flags, then set only isolated DATA_DIR, explicit model/runtime directories and known flags.",
                  selection_metadata={key:selection.get(key) for key in
                    ("dataset", "revision", "metadata_sha256", "license", "selection", "limitations")},
                  cases=[], errors=[],
                  limitations=["English public read speech, not cross-language/near-homophone/natural meetings or real UX admission.",
                    "First revisions are worker-final ASR before terminology normalization; not partial-caption or word-level timing evidence.",
                    "Worker/runtime/PATH FFmpeg bytes are bound; a complete inventory of system-loaded dynamic libraries is unavailable.",
                    "Ordinary WAV append uses native PcmReader preprocessing. Every arm must exactly match its independent model-free decode-wav-info reference, not the unfiltered prepared WAV PCM; no tolerance is used.",
                    "Canonical reference costs are recorded separately and excluded from the three append-arm resource/time comparisons.",
                    "ASR-only: no scheduler, captions translation, minutes or LLM. Acoustic and corrected metrics are separate.",
                    "All 100 clusters are required; smoke never passes the full gate. No approved numerical quality threshold; quality_gate_passed stays null.",
                    "Three arm orders rotate by cluster; OS filesystem/model page caches are uncontrolled. Sampled CPU is a lower bound; summed RSS may double-count shared pages."])
    initial = None
    try:
        initial = dict(environment=environment(args.cli), assets=asset_inventory(args.cli, args.runtime, args.models_dir, args.selection, args.audio_manifest))
        report["identity_before"] = initial
        for index, (case, audio) in enumerate(chosen):
            case_result = dict(case=case["name"], cluster_id=str(case["sentence_id"]), name_kind=case.get("name_kind"),
                               reference=case["reference"], raw_reference=case["raw_reference"], audio=audio,
                               arm_order=list(ARMS[index % 3:] + ARMS[:index % 3]), arms=[], passed=False)
            report["cases"].append(case_result)
            reference = canonical_reference(args, audio, artifacts / case["name"] / "reference")
            case_result["canonical_reference"] = reference
            for position, arm in enumerate(case_result["arm_order"]):
                folder = artifacts / case["name"] / arm
                if reference["passed"]:
                    result = run_arm(args, case, audio, folder, arm, position, reference["canonical"])
                else:
                    result = dict(arm=arm, position=position, passed=False, process=None, resources=None,
                                  acoustic_metrics=None, corrected_metrics=None, blocked_by_reference=True,
                                  errors=["Canonical production reference failed; ASR was not started"])
                    write_json(folder / "result.json", result)
                case_result["arms"].append(result)
                # Bootstrap only at finalization, not after every arm.
                write_json(args.output, report)
                print(case["name"], arm, "EXECUTION PASS" if result["passed"] else result["errors"], flush=True)
                if (result.get("process") is not None and (result.get("resources") or {}).get("processes_exit_confirmed") is not True):
                    raise RuntimeError("Unconfirmed owned process exit; no subsequent ASR invocation")
            case_result["passed"] = all(item["passed"] for item in case_result["arms"])
    except Exception as error:
        report["errors"].append(f"{type(error).__name__}: {error}")
    finally:
        if initial is not None:
            try:
                final = dict(environment=environment(args.cli), assets=asset_inventory(args.cli, args.runtime, args.models_dir, args.selection, args.audio_manifest))
                report["identity_after"] = final
                require(final == initial, "Source/CLI/worker/runtime/model/selection identity changed")
                report["identities_unchanged"] = True
            except Exception as error:
                report["identities_unchanged"] = False
                report["errors"].append(f"Post-run identity {type(error).__name__}: {error}")
        report["aggregates"] = aggregate(report["cases"], comparison_valid=report.get("identities_unchanged") is True)
        arms = [item for case in report["cases"] for item in case["arms"]]
        report["actual_asr_invocations"] = sum(item.get("process") is not None for item in arms)
        references = [case["canonical_reference"] for case in report["cases"] if "canonical_reference" in case]
        costs = [(item.get("process") or {}).get("seconds") for item in references]
        report["canonical_reference_costs"] = dict(
            actual_invocations=sum(item.get("process") is not None for item in references),
            successful=sum(item.get("passed") is True for item in references),
            wall_seconds=sum(costs) if costs and all(type(value) in (int, float) and math.isfinite(value) for value in costs) else None,
            resources_recorded_per_case=True, included_in_append_comparisons=False)
        report["execution_passed"] = (len(report["cases"]) == len(chosen) and len(arms) == len(chosen) * 3
            and all(item["passed"] for item in arms) and not report["errors"] and report.get("identities_unchanged") is True)
        report["full_corpus_complete"] = args.smoke is None and len(chosen) == 100 and len(arms) == 300 and report["execution_passed"]
        # Completeness/execution must not masquerade as an approved quality gate.
        report["passed"] = report["execution_passed"]
        write_json(args.output, report)
    return report["execution_passed"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selection", type=pathlib.Path, required=True)
    parser.add_argument("--audio-manifest", type=pathlib.Path, required=True)
    parser.add_argument("--cli", type=pathlib.Path, default=REPO / "target/release/omascribe-cli")
    parser.add_argument("--runtime", type=pathlib.Path, required=True)
    parser.add_argument("--models-dir", type=pathlib.Path, required=True, help="Existing task-owned bundle containing regular zipformer/")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--stream-mode", choices=("legacy", "stream"), default="legacy", help="Fixed for the whole run; no parameter sweep")
    parser.add_argument("--smoke", type=int, choices=range(1, 101), metavar="1..100", help="Only the first N frozen clusters; never a full gate")
    parser.add_argument("--timeout", type=int, default=180, help="Per append process-tree timeout in seconds")
    args = parser.parse_args()
    if not 1 <= args.timeout <= 600:
        parser.error("Use a bounded 1–600 second per-arm timeout")
    try:
        return 0 if evaluate(args) else 1
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Preflight {type(error).__name__}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
