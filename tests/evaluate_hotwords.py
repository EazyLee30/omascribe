#!/usr/bin/env python3
"""Real worker compatibility tests: local/shared/missing tokenizer and silence."""
import argparse, json, os, pathlib, subprocess, time, wave
from harness_support import suite_passed, REPO, read_json, write_json


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--worker", type=pathlib.Path, default=REPO / "target/debug/omascribe-asr")
    p.add_argument("--small", type=pathlib.Path, required=True)
    p.add_argument("--quality", type=pathlib.Path, required=True)
    p.add_argument("--wav", type=pathlib.Path, required=True)
    p.add_argument("--output", type=pathlib.Path, required=True)
    a = p.parse_args()
    artifacts = a.output.resolve().with_suffix(".artifacts")
    if artifacts.exists(): p.error("Use a fresh output path")
    with wave.open(str(a.wav)) as wav:
        assert (wav.getnchannels(),wav.getsampwidth(),wav.getframerate()) == (1,2,16000)
        pcm = wav.readframes(wav.getnframes())
    incompatible = artifacts / "incompatible-tokenizer"
    incompatible.mkdir(parents=True)
    (incompatible / "tokens.txt").write_text("different inventory")
    cases = [
        ("small_local",a.small,None,pcm,"modified_beam_search",False),
        ("quality_shared",a.quality,a.small,pcm,"modified_beam_search",False),
        ("quality_missing",a.quality,None,pcm,"greedy_search",True),
        ("quality_incompatible",a.quality,incompatible,pcm,"greedy_search",True),
        ("hotwords_on_silence",a.small,None,b"\0"*(16000*2*10),"modified_beam_search",False),
    ]
    report = dict(passed=False,cases=[])
    for name,model,shared,audio,decoder,fallback in cases:
        root = artifacts / name;root.mkdir()
        for number,offset in enumerate(range(0,len(audio),16000*2*30)):
            (root / f"{number:06}.pcm").write_bytes(audio[offset:offset+16000*2*30])
        (root / "capture.done").touch()
        (root / "hotwords.txt").write_text("OMASCRIBE\nTRANSCRIPTION\n转写\n",encoding="utf-8")
        errors=[];events=[];config=None;start=time.monotonic()
        try:
            result = subprocess.run([str(a.worker.resolve()),str(model.resolve()),str(root),"0","1","",str(root/"hotwords.txt"),str(shared.resolve()) if shared else ""],
                capture_output=True,text=True,encoding="utf-8",timeout=120,
                env=dict(os.environ,OMP_NUM_THREADS="1",OPENBLAS_NUM_THREADS="1"))
            (root / "events.jsonl").write_text(result.stdout)
            (root / "stderr.txt").write_text(result.stderr)
            events = [json.loads(line) for line in result.stdout.splitlines()]
            config = read_json(root / "asr-config.json")
            if result.returncode or any(e.get("event")=="error" for e in events): errors.append("Worker failed")
            if not any(e.get("event")=="done" for e in events): errors.append("Missing completion")
            if config["decoder"] != decoder or bool(config["fallback"]) != fallback: errors.append("Wrong compatibility path")
            segments = [e for e in events if e.get("event")=="segment"]
            if name == "hotwords_on_silence" and segments: errors.append("Silence hallucination with hotwords")
            if name != "hotwords_on_silence" and not segments: errors.append("No speech recognized")
        except Exception as error: errors.append(f"{type(error).__name__}: {error}")
        report["cases"].append(dict(case=name,passed=not errors,errors=errors,config=config,seconds=round(time.monotonic()-start,3)))
        report["passed"]=suite_passed(report["cases"], len(cases))
        write_json(a.output,report)
        print(name,"PASS" if not errors else errors,flush=True)
    raise SystemExit(0 if report["passed"] else 1)

if __name__ == "__main__": main()
