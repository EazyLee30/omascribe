#!/usr/bin/env python3
"""Offline real-ASR harness. Gold reference errors and timings, not just process exit codes."""
import argparse, array, collections, json, math, pathlib, random, re, time, unicodedata, wave
from harness_support import REPO, command, environment, isolated_env, read_json, sha256, write_json


def units(text, language):
    text = unicodedata.normalize("NFKC", text).lower()
    if language.startswith("cmn"): return [c for c in text if c.isalnum()]
    return re.findall(r"[^\W_]+(?:'[^\W_]+)*", text, re.UNICODE)


def edit_distance(reference, hypothesis):
    row = list(range(len(hypothesis)+1))
    for i, a in enumerate(reference, 1):
        nxt = [i]
        for j, b in enumerate(hypothesis, 1):
            nxt.append(min(nxt[-1]+1, row[j]+1, row[j-1]+(a != b)))
        row = nxt
    return row[-1]


def regression(previous, current, allowance):
    errors = []
    for key, metric in current.items():
        old = previous.get(key)
        if not old:
            errors.append(f"{key}: no comparable baseline")
        if old and metric["error_rate"] > old["error_rate"] + allowance:
            errors.append(f"{key}: error rate regressed {old['error_rate']:.4f} → {metric['error_rate']:.4f}")
    return errors


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cli", type=pathlib.Path, default=REPO / "target/debug/omascribe-cli")
    p.add_argument("--fixtures", type=pathlib.Path, required=True)
    p.add_argument("--output", type=pathlib.Path, required=True)
    p.add_argument("--models", nargs="+", default=["zipformer-small", "zipformer-bilingual"])
    p.add_argument("--baseline", type=pathlib.Path)
    p.add_argument("--allow-regression", type=float, default=0.02)
    p.add_argument("--max-error-rate", type=float, default=0.45, help="Fixed smoke-quality ceiling; not a product accuracy claim")
    args = p.parse_args()
    cli, corpus = args.cli.resolve(), args.fixtures.resolve()
    manifest = read_json(corpus / "manifest.json")
    if not manifest or not manifest.get("cases"): p.error("Run prepare_asr_fixtures.py explicitly before offline evaluation")
    output, artifacts = args.output.resolve(), args.output.resolve().with_suffix(".artifacts")
    if artifacts.exists(): p.error("Use a fresh output path to preserve old artifacts")
    report = dict(schema=1, environment=environment(cli), fixture_manifest_sha256=sha256(corpus / "manifest.json"),
        normalization="NFKC lowercase; Mandarin alphanumeric characters without spaces/punctuation; English words with apostrophes; hyphens split.",
        cases=[], aggregates={}, errors=[], passed=False)
    with wave.open(str(corpus / "silence.wav"), "wb") as wav:
        wav.setparams((1,2,16000,0,"NONE","not compressed")); wav.writeframes(b"\0"*(16000*2*10))
    # Synthetic quiet fan hum/noise: specifically guards against leveling silence into words.
    random_source = random.Random(1729)
    noise = array.array("h", (int(32*math.sin(2*math.pi*120*i/16000) + random_source.gauss(0,6)) for i in range(16000*10)))
    import sys
    if sys.byteorder != "little": noise.byteswap()
    with wave.open(str(corpus / "quiet-noise.wav"), "wb") as wav:
        wav.setparams((1,2,16000,0,"NONE","not compressed")); wav.writeframes(noise.tobytes())
    for model in args.models:
        root = artifacts / model / "workspace"
        env = isolated_env(root)
        write_json(root / "preferences.json", dict(asr_model=model, auto_notes=False, diarization=False, threads=1))
        for case in [*manifest["cases"], dict(name="silence", language="none", file="silence.wav", reference=""),
            dict(name="synthetic_quiet_noise", language="none", file="quiet-noise.wav", reference="")]:
            folder = artifacts / model / case["name"]
            write_json(folder / "input.json", case)
            result = dict(case=case["name"], model=model, language=case["language"], errors=[])
            try:
                audio = corpus / case["file"]
                if case.get("sha256") and sha256(audio) != case["sha256"]: raise ValueError("Fixture hash mismatch")
                start = time.monotonic()
                snapshot = command([cli,"import",audio,case["name"]], env, folder, timeout=180)
                result["wall_seconds"] = round(time.monotonic()-start,3)
                hypothesis = " ".join(s["text"] for s in snapshot["segments"])
                result.update(hypothesis=hypothesis, reference=case["reference"], rtf=snapshot["runtime"]["asr_rtf"],
                    audio_seconds=snapshot["meeting"]["duration_ms"]/1000,
                    decoder=read_json(root / "meetings" / snapshot["meeting"]["id"] / "audio/asr-config.json"))
                if snapshot["meeting"]["status"] != "ready" or snapshot["runtime"]["error"]:
                    result["errors"].append("Transcription did not complete: " + str(snapshot["runtime"]["error"]))
                previous = 0
                for segment in snapshot["segments"]:
                    if not previous <= segment["start_ms"] <= segment["end_ms"] <= snapshot["meeting"]["duration_ms"]:
                        result["errors"].append("Non-monotonic or out-of-range timestamp")
                    previous = segment["end_ms"]
                if case["language"] == "none":
                    if hypothesis.strip(): result["errors"].append("Silence hallucination")
                else:
                    ref, hyp = units(case["reference"],case["language"]), units(hypothesis,case["language"])
                    result.update(edits=edit_distance(ref,hyp), reference_units=len(ref))
                    result["error_rate"] = result["edits"]/max(1,len(ref))
                    if not hypothesis.strip(): result["errors"].append("Empty transcription of speech")
            except Exception as error: result["errors"].append(f"{type(error).__name__}: {error}")
            result["passed"] = not result["errors"]
            report["cases"].append(result)
            write_json(args.output, report)
            print(model,case["name"],f"error={result.get('error_rate',0):.3f}","PASS" if result["passed"] else result["errors"],flush=True)
    groups = collections.defaultdict(list)
    for case in report["cases"]:
        if "edits" in case: groups[case["model"]+"/"+case["language"]].append(case)
    for key, cases in groups.items():
        edits, count = sum(c["edits"] for c in cases), sum(c["reference_units"] for c in cases)
        report["aggregates"][key] = dict(edits=edits, reference_units=count, error_rate=edits/count,
            audio_seconds=sum(c["audio_seconds"] for c in cases), wall_seconds=sum(c["wall_seconds"] for c in cases), clips=len(cases))
        if edits/count > args.max_error_rate: report["errors"].append(f"{key}: exceeded fixed quality ceiling")
    if args.baseline:
        previous = read_json(args.baseline)
        if previous["fixture_manifest_sha256"] != report["fixture_manifest_sha256"]:
            report["errors"].append("Baseline corpus differs; comparison refused")
        else: report["errors"].extend(regression(previous["aggregates"],report["aggregates"],args.allow_regression))
    report["passed"] = all(c["passed"] for c in report["cases"]) and not report["errors"]
    write_json(args.output,report)
    raise SystemExit(0 if report["passed"] else 1)

if __name__ == "__main__": main()
