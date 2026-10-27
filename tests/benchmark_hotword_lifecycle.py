#!/usr/bin/env python3
"""Real local ASR model lifecycle comparison; no audio/accuracy assertion or network."""
import argparse
import json
import os
import pathlib
import subprocess
import time

from harness_support import REPO, environment, sha256, write_json


def invoke(argv, env, directory, timeout=120):
    directory.mkdir(parents=True)
    at = time.monotonic()
    usage = None
    with (directory/'stdout.json').open('w') as stdout, (directory/'stderr.txt').open('w') as stderr:
        child = subprocess.Popen([str(a) for a in argv], env=env, stdout=stdout, stderr=stderr)
        if hasattr(os,'wait4'):
            try:
                while True:
                    pid,status,usage = os.wait4(child.pid,os.WNOHANG)
                    if pid:
                        child.returncode = os.waitstatus_to_exitcode(status)
                        break
                    if time.monotonic()-at > timeout:
                        child.kill(); child.wait(); raise TimeoutError('Worker benchmark timed out')
                    time.sleep(.01)
            except BaseException:
                if child.returncode is None: child.kill(); child.wait()
                raise
        else:
            try: child.wait(timeout=timeout)
            except subprocess.TimeoutExpired as error:
                child.kill(); child.wait()
                raise TimeoutError('Worker benchmark timed out') from error
            except BaseException: child.kill(); child.wait(); raise
    if child.returncode: raise RuntimeError(f'Worker exited {child.returncode}; see {directory}')
    result = json.loads((directory/'stdout.json').read_text(encoding='utf-8'))
    result['wall_ms'] = (time.monotonic()-at)*1000
    result['worker_cpu_seconds'] = usage.ru_utime+usage.ru_stime if usage else None
    result['worker_peak_rss_mib'] = (usage.ru_maxrss/(1024**2 if os.uname().sysname=='Darwin' else 1024)) if usage and os.uname().sysname in ['Darwin','Linux'] else None
    result['resource_method'] = 'wait4 child rusage; includes worker threads, not a process-tree sum; macOS bytes/Linux KiB for maxrss' if usage else 'unavailable on this platform'
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker',type=pathlib.Path,default=REPO/'target/debug'/('omascribe-asr.exe' if os.name=='nt' else 'omascribe-asr'))
    parser.add_argument('--model',type=pathlib.Path,required=True)
    parser.add_argument('--shared',type=pathlib.Path)
    parser.add_argument('--output',type=pathlib.Path,required=True)
    args=parser.parse_args()
    output=args.output.resolve(); artifacts=output.with_suffix('.artifacts')
    if output.exists() or artifacts.exists(): parser.error('Use a fresh output path')
    worker,model=args.worker.resolve(),args.model.resolve()
    assets={p.name:dict(bytes=p.stat().st_size,sha256=sha256(p)) for p in sorted(model.iterdir()) if p.is_file() and p.name in ['encoder-epoch-99-avg-1.int8.onnx','decoder-epoch-99-avg-1.onnx','joiner-epoch-99-avg-1.int8.onnx','tokens.txt','bpe.model']}
    report=dict(schema=1,kind='hotword-lifecycle-only',environment=environment(worker),model_assets=assets,
                shared_assets={p.name:dict(bytes=p.stat().st_size,sha256=sha256(p)) for p in sorted(args.shared.iterdir()) if p.is_file() and p.name in ['tokens.txt','bpe.model']} if args.shared else None,
                results=[],errors=[],passed=False,
                limitation='Model/graph creation only; no speech accuracy, caption latency, GPU or battery claim. Stream updates remain opt-in.')
    for trial,order in enumerate([('legacy','stream'),('stream','legacy')],1):
        for mode in order:
            directory=artifacts/f'{trial}-{mode}'
            directory.mkdir(parents=True)
            data=directory/'decoder'
            env=dict(os.environ,OMASCRIBE_ASR_STREAM_HOTWORDS='1' if mode=='stream' else '0',OMASCRIBE_ASR_THREADS='1',OMP_NUM_THREADS='1',OPENBLAS_NUM_THREADS='1')
            try:
                result=invoke([worker,'--benchmark-hotwords',model,data,str(args.shared.resolve()) if args.shared else ''],env,directory/'process')
                expected=1 if mode=='stream' else 13
                assert result['kind']=='hotword-lifecycle-only' and len(result['updates_ms'])==12
                assert result['after_updates']['model_load_attempts']==expected
                assert result['after_updates']['stream_updates']==(12 if mode=='stream' else 0)
                assert result['after_updates']['default_hotword_buffer_present']==(mode=='legacy')
                assert result['after_disable_enable']['model_load_attempts']==expected+2
                assert result['unchanged_noop'] and not result['includes_audio']
                result.update(trial=trial,mode=mode,passed=True)
                print(json.dumps(dict(trial=trial,mode=mode,updates_ms=sum(result['updates_ms']),cpu=result['worker_cpu_seconds'],peak_rss_mib=result['worker_peak_rss_mib'])),flush=True)
                report['results'].append(result)
            except Exception as error:
                report['errors'].append(f'{trial}-{mode}: {type(error).__name__}: {error}')
                report['results'].append(dict(trial=trial,mode=mode,passed=False))
            write_json(output,report)
    report['passed']=len(report['results'])==4 and all(r['passed'] for r in report['results']) and not report['errors']
    write_json(output,report)
    raise SystemExit(0 if report['passed'] else 1)


if __name__=='__main__':main()
