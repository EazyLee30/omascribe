#!/usr/bin/env python3
"""Offline real-ASR harness. Gold reference errors and timings, not just process exit codes."""
import argparse, array, collections, json, math, os, pathlib, random, re, time, unicodedata, wave
from harness_support import REPO, command, environment, isolated_env, read_json, sha256, write_json

SCORER = "asr-v2-cer-wer-mer"


def mixed_units(text):
    tokens, word = [], []
    def flush():
        if word:
            tokens.append("".join(word)); word.clear()
    def han(c):
        return '\u3400' <= c <= '\u9fff' or unicodedata.name(c, "").startswith("CJK UNIFIED IDEOGRAPH-")
    for index, c in enumerate(text):
        if han(c):
            flush(); tokens.append(c)
        elif c.isalnum():
            word.append(c)
        elif c == "'" and word and index+1 < len(text) and text[index+1].isalnum() and not han(text[index+1]):
            word.append(c)
        else:
            flush()
    flush()
    return tokens


def units(text, language):
    text = unicodedata.normalize("NFKC", text).lower()
    if language == "mixed":
        return mixed_units(text)
    if language.startswith("cmn"): return [c for c in text if c.isalnum()]
    return re.findall(r"[^\W_]+(?:'[^\W_]+)*", text, re.UNICODE)


def edit_distance(reference, hypothesis):
    return sum(error_counts(reference, hypothesis).values())


def error_counts(reference, hypothesis):
    """Minimum edit alignment, stable substitution/deletion/insertion tie ordering."""
    if len(reference)*len(hypothesis) > 4_000_000:
        raise ValueError("Scoring budget exceeded; use a bounded long-audio alignment harness")
    row = [(0, 0, i) for i in range(len(hypothesis)+1)]
    for i, a in enumerate(reference, 1):
        nxt = [(0, i, 0)]
        for j, b in enumerate(hypothesis, 1):
            if a == b:
                nxt.append(row[j-1])
            else:
                s, d, ins = row[j-1]
                substitute = (s+1, d, ins)
                s, d, ins = row[j]
                delete = (s, d+1, ins)
                s, d, ins = nxt[-1]
                insert = (s, d, ins+1)
                nxt.append(min([substitute, delete, insert], key=sum))
        row = nxt
    return dict(zip(["substitutions", "deletions", "insertions"], row[-1]))


def percentile(values, fraction):
    values = sorted(v for v in values if v is not None and math.isfinite(v))
    if not values: return None
    point = (len(values)-1) * fraction
    low, high = math.floor(point), math.ceil(point)
    return values[low] + (values[high]-values[low]) * (point-low)


def error_interval(cases, samples=1000):
    """Deterministic clip bootstrap; descriptive only for small smoke corpora."""
    if not cases: return None
    rng, rates = random.Random(1729), []
    for _ in range(samples):
        selected = rng.choices(cases, k=len(cases))
        denominator = sum(c["reference_units"] for c in selected)
        if denominator: rates.append(sum(c["edits"] for c in selected) / denominator)
    return [percentile(rates,.025), percentile(rates,.975)] if rates else None


def regression(previous, current, allowance):
    errors = []
    for key in previous.keys() | current.keys():
        old, metric = previous.get(key), current.get(key)
        if old is None:
            errors.append(f"{key}: no comparable baseline")
            continue
        if metric is None:
            errors.append(f"{key}: baseline group missing from current run")
            continue
        values = [old.get("error_rate"), metric.get("error_rate")]
        if any(not isinstance(v,(int,float)) or not math.isfinite(v) for v in values):
            errors.append(f"{key}: incomplete scoring cannot establish a comparison")
        elif metric["error_rate"] > old["error_rate"] + allowance:
            errors.append(f"{key}: error rate regressed {old['error_rate']:.4f} → {metric['error_rate']:.4f}")
    return errors


def asset_identity(cli, root, model, env):
    spec = next((m for m in read_json(REPO / "model-catalog.json")["models"] if m["id"] == model and m["kind"] == "asr"), None)
    if spec is None:
        raise ValueError("Unknown ASR model")
    directory = root / "model-store" / model
    if not directory.is_dir() and spec["bundled"]:
        directory = pathlib.Path(env["OMASCRIBE_MODELS_DIR"]) / "zipformer"
    files = []
    for item in spec["files"]:
        path = directory / item["name"]
        if path.stat().st_size != item["bytes"] or sha256(path) != item["sha256"]:
            raise ValueError("ASR asset differs from the pinned catalog: " + item["name"])
        files.append({k:item[k] for k in ["name","bytes","sha256"]})
    name = "omascribe-asr.exe" if os.name == "nt" else "omascribe-asr"
    worker = pathlib.Path(env.get("OMASCRIBE_RUNTIME_DIR", cli.parent)) / name
    if not worker.exists():
        worker = cli.parent / name
    return dict(model=model, files=files, worker=dict(name=name, bytes=worker.stat().st_size, sha256=sha256(worker)),
                limitation="Worker/model hashes verified; platform/system dependency inventory is not complete")


def aggregate_cases(cases):
    scored = [c for c in cases if isinstance(c.get("edits"),int)]
    complete = len(scored) == len(cases) and bool(scored)
    count = sum(c["reference_units"] for c in scored)
    edits = sum(c["edits"] for c in scored)
    resources = [(c.get("process") or {}).get("resources") or {} for c in cases]
    cpu = [r.get("cpu_seconds_sampled") for r in resources]
    rss = [r.get("peak_tree_rss_mib") for r in resources]
    pss = [r.get("peak_tree_pss_mib") for r in resources]
    walls = [c.get("wall_seconds") for c in cases]
    audio = [c.get("audio_seconds") for c in cases]
    return dict(edits=edits if complete else None, reference_units=count if complete else None,
        error_rate=edits/count if complete and count else None, clips=len(cases), scored_clips=len(scored),
        audio_seconds=sum(audio) if all(v is not None for v in audio) else None,
        known_audio_seconds=sum(v for v in audio if v is not None), missing_audio_clips=sum(v is None for v in audio),
        wall_seconds=sum(walls) if all(v is not None for v in walls) else None,
        **{key:sum(c[key] for c in scored) if complete else None for key in ["substitutions","deletions","insertions"]},
        error_rate_interval_95=error_interval(scored) if complete else None,
        wall_p50=percentile(walls,.5), wall_p95=percentile(walls,.95),
        rtf_p50=percentile([c.get("rtf") for c in cases],.5), rtf_p95=percentile([c.get("rtf") for c in cases],.95),
        peak_process_tree_rss_mib=max((v for v in rss if v is not None), default=None),
        peak_process_tree_pss_mib=max((v for v in pss if v is not None), default=None),
        process_tree_cpu_seconds_sampled=sum(cpu) if all(v is not None for v in cpu) else None,
        process_tree_cpu_known_seconds=sum(v for v in cpu if v is not None),
        missing_cpu_clips=sum(v is None for v in cpu), missing_rss_clips=sum(v is None for v in rss),
        missing_pss_clips=sum(v is None for v in pss), gpu_memory_mib=None)


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
    if len(set(args.models)) != len(args.models) or any(not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}",m) for m in args.models): p.error("Models must be unique safe identifiers")
    if any(not math.isfinite(v) or not 0 <= v <= 1 for v in [args.allow_regression,args.max_error_rate]):
        p.error("Quality thresholds must be finite values between zero and one")
    cli, corpus = args.cli.resolve(), args.fixtures.resolve()
    manifest = read_json(corpus / "manifest.json")
    if not manifest or not manifest.get("cases"): p.error("Run prepare_asr_fixtures.py explicitly before offline evaluation")
    names = [c.get("name","") for c in manifest["cases"]]
    if len(set(names)) != len(names) or any(name in ["silence","synthetic_quiet_noise"] or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}",name) for name in names):
        p.error("Fixture names must be unique safe basenames")
    for case in manifest["cases"]:
        filename = case.get("file","")
        if not filename or pathlib.Path(filename).name != filename or "\\" in filename or filename in [".",".."]:
            p.error("Fixture files must be basenames inside the corpus")
    output, artifacts = args.output.resolve(), args.output.resolve().with_suffix(".artifacts")
    if output.exists() or artifacts.exists(): p.error("Use a fresh output path to preserve old artifacts")
    report = dict(schema=3, scorer=SCORER, environment=environment(cli), fixture_manifest_sha256=sha256(corpus / "manifest.json"),
        normalization="NFKC lowercase; Mandarin alphanumeric characters (CER); English words/apostrophes (WER); mixed Han characters plus Unicode words/numbers/apostrophes (MER); hyphens split.",
        interval_method="1000 clip-bootstrap samples, seed 1729; descriptive for a small smoke set, not a product accuracy claim",
        measurement="Wall time includes the CLI/import pipeline, excludes scoring and asset hashing. Worker RTF is compute time, not visible caption latency. CPU is sampled lower-bound; RSS can double-count shared pages; missing resources remain unknown.",
        models=args.models, assets={}, cases=[], aggregates={}, errors=[], passed=False)
    synthetic = artifacts / "synthetic"
    synthetic.mkdir(parents=True)
    with wave.open(str(synthetic / "silence.wav"), "wb") as wav:
        wav.setparams((1,2,16000,0,"NONE","not compressed")); wav.writeframes(b"\0"*(16000*2*10))
    # Synthetic quiet fan hum/noise: specifically guards against leveling silence into words.
    random_source = random.Random(1729)
    noise = array.array("h", (int(32*math.sin(2*math.pi*120*i/16000) + random_source.gauss(0,6)) for i in range(16000*10)))
    import sys
    if sys.byteorder != "little": noise.byteswap()
    with wave.open(str(synthetic / "quiet-noise.wav"), "wb") as wav:
        wav.setparams((1,2,16000,0,"NONE","not compressed")); wav.writeframes(noise.tobytes())
    report["synthetic_assets"] = {name:dict(bytes=(synthetic/name).stat().st_size,sha256=sha256(synthetic/name)) for name in ["silence.wav","quiet-noise.wav"]}
    for model in args.models:
        root = artifacts / model / "workspace"
        env = isolated_env(root)
        write_json(root / "preferences.json", dict(asr_model=model, auto_notes=False, diarization=False, threads=1))
        preflight = None
        try:
            report["assets"][model] = asset_identity(cli, root, model, env)
        except Exception as error:
            preflight = f"ASR preflight {type(error).__name__}: {error}"
        for case in [*manifest["cases"], dict(name="silence", language="none", file="silence.wav", reference=""),
            dict(name="synthetic_quiet_noise", language="none", file="quiet-noise.wav", reference="")]:
            folder = artifacts / model / case["name"]
            write_json(folder / "input.json", case)
            result = dict(case=case["name"], model=model, language=case["language"], fixture_kind=case.get("kind","public-speech"), errors=[])
            start = None
            try:
                if preflight: raise ValueError(preflight)
                audio = (synthetic if case["name"] in ["silence","synthetic_quiet_noise"] else corpus) / case["file"]
                actual_sha = sha256(audio)
                if case.get("sha256") and actual_sha != case["sha256"]: raise ValueError("Fixture hash mismatch")
                result["audio_sha256"] = actual_sha
                start = time.monotonic()
                snapshot = command([cli,"import",audio,case["name"]], env, folder, timeout=180)
                result["wall_seconds"] = round(time.monotonic()-start,3)
                if sha256(audio) != actual_sha: raise ValueError("Audio changed during ASR evaluation")
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
                    if not ref: raise ValueError("Speech reference has no scoring units")
                    counts = error_counts(ref,hyp)
                    result.update(**counts, edits=sum(counts.values()), reference_units=len(ref),
                        metric="MER" if case["language"] == "mixed" else "CER" if case["language"].startswith("cmn") else "WER")
                    result["error_rate"] = result["edits"]/max(1,len(ref))
                    if not hypothesis.strip(): result["errors"].append("Empty transcription of speech")
            except Exception as error: result["errors"].append(f"{type(error).__name__}: {error}")
            finally:
                if start is not None and "wall_seconds" not in result: result["wall_seconds"] = round(time.monotonic()-start,3)
                result["process"] = read_json(folder / "process.json")
            result["passed"] = not result["errors"]
            report["cases"].append(result)
            write_json(args.output, report)
            rate = result.get("error_rate")
            print(model,case["name"],f"error={rate:.3f}" if rate is not None else "error=unknown","PASS" if result["passed"] else result["errors"],flush=True)
        if preflight is None:
            try:
                if asset_identity(cli,root,model,env) != report["assets"][model]:
                    raise ValueError("Worker/model changed during ASR evaluation")
            except Exception as error: report["errors"].append(f"{model} asset revalidation: {error}")
    groups = collections.defaultdict(list)
    for case in report["cases"]:
        if case["language"] != "none": groups[case["model"]+"/"+case["language"]].append(case)
    for key, cases in groups.items():
        report["aggregates"][key] = aggregate_cases(cases)
        rate = report["aggregates"][key]["error_rate"]
        if rate is None: report["errors"].append(f"{key}: incomplete speech scoring")
        elif rate > args.max_error_rate: report["errors"].append(f"{key}: exceeded fixed quality ceiling")
    if args.baseline:
        try: previous = read_json(args.baseline)
        except (OSError, ValueError): previous = None
        if not isinstance(previous,dict):
            report["errors"].append("Baseline is missing or unreadable")
        elif previous.get("scorer") != SCORER:
            report["errors"].append("Baseline scorer differs; rerun the old binary with this scorer before comparison")
        elif previous["fixture_manifest_sha256"] != report["fixture_manifest_sha256"]:
            report["errors"].append("Baseline corpus differs; comparison refused")
        else: report["errors"].extend(regression(previous["aggregates"],report["aggregates"],args.allow_regression))
    report["passed"] = len(report["cases"]) == len(args.models)*(len(manifest["cases"])+2) and all(c["passed"] for c in report["cases"]) and not report["errors"]
    write_json(args.output,report)
    raise SystemExit(0 if report["passed"] else 1)

if __name__ == "__main__": main()
