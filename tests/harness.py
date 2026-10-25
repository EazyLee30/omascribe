#!/usr/bin/env python3
"""One entry point for production ASR/minutes acceptance. Evaluation never downloads.
Use prepare_asr_fixtures.py explicitly to prepare pinned public audio first.
"""
import argparse, json, os, pathlib, subprocess, sys, time
from harness_support import REPO, read_json, write_json


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cli",type=pathlib.Path,default=REPO/"target/debug/omascribe-cli")
    p.add_argument("--fixtures",type=pathlib.Path,required=True)
    p.add_argument("--output",type=pathlib.Path,required=True,help="Fresh directory, all artifacts retained")
    p.add_argument("--models",nargs="+",default=["zipformer-small","zipformer-bilingual"])
    p.add_argument("--repeat",type=int,default=1,help="Repeat minutes to expose intermittent model failures")
    p.add_argument("--stress-minutes",type=int,default=0,help="Optional accelerated long ASR test (1–120 minutes)")
    p.add_argument("--baseline",type=pathlib.Path,help="Previous harness output directory")
    p.add_argument("--small",type=pathlib.Path,help="Small model directory, also enables hotword compatibility checks")
    p.add_argument("--quality",type=pathlib.Path,help="Quality model directory")
    a=p.parse_args()
    for key in ["cli", "fixtures", "output", "baseline", "small", "quality"]:
        if getattr(a,key) is not None: setattr(a,key,getattr(a,key).resolve())
    if a.output.exists():p.error("Output must be a fresh directory")
    if not 1<=a.repeat<=10:p.error("Repeat must be 1–10")
    a.output.mkdir(parents=True)
    report=dict(schema=1,passed=False,runs=[],started=time.time())
    jobs=[("asr",["evaluate_asr.py","--fixtures",a.fixtures,"--models",*a.models,"--cli",a.cli,"--output",a.output/"asr.json"])]
    if a.baseline:jobs[0][1].extend(["--baseline",a.baseline/"asr.json"])
    for i in range(a.repeat):
        jobs.append((f"minutes-{i+1}",["evaluate.py","--cli",a.cli,"--output",a.output/f"minutes-{i+1}.json"]))
    jobs.append(("multitake",["multitake.py","--cli",a.cli,"--output",a.output/"multitake.json"]))
    if a.small and a.quality:
        manifest=read_json(a.fixtures/"manifest.json")
        sample=a.fixtures/manifest["cases"][0]["file"]
        worker=pathlib.Path(os.environ.get("OMASCRIBE_RUNTIME_DIR",str(a.cli.parent)))/("omascribe-asr.exe" if os.name=="nt" else "omascribe-asr")
        jobs.append(("hotwords",["evaluate_hotwords.py","--worker",worker,"--small",a.small,"--quality",a.quality,"--wav",sample,"--output",a.output/"hotwords.json"]))
    manifest=read_json(a.fixtures/"manifest.json")
    env=dict(os.environ,OMASCRIBE_TEST_WAV=str((a.fixtures/manifest["cases"][0]["file"]).resolve()))
    if a.stress_minutes:
        jobs.append(("stress",["stress_core.py","--cli",a.cli,"--wav",a.fixtures/manifest["cases"][0]["file"],"--minutes",a.stress_minutes,"--output",a.output/"stress.json"]))
    for name,arguments in jobs:
        start=time.monotonic()
        try:
            run=subprocess.run([sys.executable,str(REPO/"tests"/arguments[0]),*[str(x) for x in arguments[1:]]],cwd=REPO,env=env)
            failure=None if run.returncode==0 else f"exit {run.returncode}"
        except Exception as error:failure=str(error)
        report["runs"].append(dict(suite=name,passed=failure is None,error=failure,seconds=round(time.monotonic()-start,3)))
        report["passed"]=all(r["passed"] for r in report["runs"])
        write_json(a.output/"report.json",report)
    raise SystemExit(0 if report["passed"] else 1)

if __name__=="__main__":main()
