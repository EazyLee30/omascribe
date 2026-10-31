#!/usr/bin/env python3
"""Real worker pause/boundary regression using pinned public audio and synthetic silence.

Compares stream hotwords to recognizer rebuilds. Equal transcripts are not an accuracy
score: keep independent references and the wider hotword anti-case gate separate.
"""
import argparse
import array
import hashlib
import json
import math
import os
import pathlib
import queue
import random
import subprocess
import sys
import threading
import time
import wave

from harness_support import REPO, environment, sha256, write_json


def exercise(worker, model, root, mode, phases, shared=None):
    root.mkdir(parents=True)
    words=root/'hotwords.txt';words.write_text('OMASCRIBE\nPROJECT ALPHA',encoding='utf-8')
    paused=root/'capture.paused';paused.touch()
    env=dict(os.environ,OMASCRIBE_ASR_STREAM_HOTWORDS='1' if mode=='stream' else '0',OMASCRIBE_ASR_THREADS='1',OMP_NUM_THREADS='1',OPENBLAS_NUM_THREADS='1')
    events=[];inbox=queue.Queue();results=[]
    with (root/'stderr.txt').open('w') as stderr:
        child=subprocess.Popen([str(worker),str(model),str(root),'0','1','',str(words),str(shared) if shared else ''],env=env,stdout=subprocess.PIPE,stderr=stderr,text=True,encoding='utf-8',bufsize=1)
        def read():
            for line in child.stdout:
                try:inbox.put(json.loads(line))
                except ValueError:inbox.put(dict(event='error',message='Invalid worker JSON'))
            inbox.put(None)
        reader=threading.Thread(target=read,daemon=True);reader.start()
        def until(predicate,timeout=90):
            deadline=time.monotonic()+timeout
            while True:
                remaining=deadline-time.monotonic()
                if remaining<=0:raise TimeoutError('Worker boundary timed out')
                event=inbox.get(timeout=remaining)
                if event is None:raise RuntimeError('Worker exited before boundary')
                events.append(event)
                if event.get('event')=='error':raise RuntimeError(event.get('message'))
                if predicate(event):return event
        try:
            until(lambda e:e.get('event')=='ready')
            until(lambda e:e.get('event')=='boundary' and e.get('cause')=='pause' and e['processed_ms']==0)
            samples=0
            for name,pcm,next_words in phases:
                before=len(events);start_ms=samples*1000//16000
                temp=root/'hotwords.next';temp.write_text(next_words,encoding='utf-8');temp.replace(words)
                paused.unlink();at=time.monotonic()
                cursor=0
                while cursor<len(pcm):
                    index,offset=divmod(samples,16000*30)
                    count=min((16000*30-offset)*2,len(pcm)-cursor)
                    with (root/f'{index:06}.pcm').open('ab') as audio:audio.write(pcm[cursor:cursor+count])
                    samples+=count//2;cursor+=count
                paused.touch();end_ms=samples*1000//16000
                until(lambda e:e.get('event')=='boundary' and e.get('cause')=='pause' and e['processed_ms']==end_ms)
                segments=[e for e in events[before:] if e.get('event')=='segment']
                assert all(start_ms<=s['start_ms']<=s['end_ms']<=end_ms for s in segments)
                if name in ['silence','quiet-noise']:assert not segments,'Silence/noise hallucinated speech'
                else:assert segments,'Public speech produced no finalized passage'
                results.append(dict(case=name,segments=segments,wall_seconds=time.monotonic()-at,config=json.loads((root/'asr-config.json').read_text(encoding='utf-8'))))
                print(mode,name,'segments',len(segments),flush=True)
            (root/'capture.done').touch()
            done=until(lambda e:e.get('event')=='done')
            assert done['processed_ms']==samples*1000//16000
            assert child.wait(timeout=10)==0
            reader.join(timeout=5)
            all_segments=[e for e in events if e.get('event')=='segment']
            assert [s['seq'] for s in all_segments]==list(range(1,len(all_segments)+1))
            assert sum(len(r['segments']) for r in results)==len(all_segments),'Unexpected final duplicate passage'
            expected=hashlib.sha256();actual=hashlib.sha256()
            for _,pcm,_ in phases:expected.update(pcm)
            for path in sorted(root.glob('*.pcm')):
                with path.open('rb') as audio:
                    while block:=audio.read(65536):actual.update(block)
            assert actual.hexdigest()==expected.hexdigest(),'Archived PCM changed'
            write_json(root/'pcm-integrity.json',dict(bytes=samples*2,sha256=actual.hexdigest(),exact_match=True))
            write_json(root/'events.json',events)
            return results
        finally:
            if child.poll() is None:child.kill();child.wait()
            child.stdout.close();reader.join(timeout=5)
            write_json(root/'events.json',events)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--worker',type=pathlib.Path,default=REPO/'target/debug'/('omascribe-asr.exe' if os.name=='nt' else 'omascribe-asr'))
    p.add_argument('--model',type=pathlib.Path,required=True)
    p.add_argument('--shared',type=pathlib.Path)
    p.add_argument('--fixtures',type=pathlib.Path,required=True)
    p.add_argument('--output',type=pathlib.Path,required=True)
    args=p.parse_args();output=args.output.resolve();artifacts=output.with_suffix('.artifacts')
    if output.exists() or artifacts.exists():p.error('Use a fresh output path')
    worker,model=args.worker.resolve(),args.model.resolve();manifest=args.fixtures/'manifest.json'
    sources=json.loads(manifest.read_text(encoding='utf-8'))['cases'];assert len(sources)==4
    contexts=['OMASCRIBE\nPROJECT BETA','OMASCRIBE','','OMASCRIBE\nPROJECT GAMMA','OMASCRIBE\nPROJECT BETA','OMASCRIBE\nPROJECT BETA']
    phases=[]
    for index,case in enumerate(sources):
        assert pathlib.Path(case['file']).name==case['file']
        path=args.fixtures/case['file'];assert sha256(path)==case['sha256']
        with wave.open(str(path)) as wav:
            assert (wav.getnchannels(),wav.getsampwidth(),wav.getframerate())==(1,2,16000)
            assert 0<wav.getnframes()<=16000*60,'Use short clips for this boundary regression'
            pcm=wav.readframes(wav.getnframes());assert len(pcm)==wav.getnframes()*2
            phases.append((case['file'],pcm,contexts[index]))
    phases.append(('silence',b'\0'*(16000*2*10),contexts[4]))
    rng=random.Random(1729);noise=array.array('h',(int(32*math.sin(2*math.pi*120*i/16000)+rng.gauss(0,6)) for i in range(16000*10)))
    if sys.byteorder!='little':noise.byteswap()
    phases.append(('quiet-noise',noise.tobytes(),contexts[5]))
    report=dict(schema=1,kind='hotword-boundary-equivalence',environment=environment(worker),fixture_manifest_sha256=sha256(manifest),results={},errors=[],passed=False,
                limitation='Four public clips plus synthetic silence/noise; transcript equivalence only, not independent accuracy or the 100-positive/100-negative hotword gate.')
    write_json(output,report)
    try:
        for mode in ['legacy','stream']:
            report['results'][mode]=exercise(worker,model,artifacts/mode,mode,phases,args.shared.resolve() if args.shared else None)
            write_json(output,report)
        for legacy,stream in zip(report['results']['legacy'],report['results']['stream']):
            assert legacy['case']==stream['case'] and legacy['segments']==stream['segments'],f"Boundary/words differ for {legacy['case']}"
        final=report['results']['stream'][-1]['config']
        assert final['model_load_attempts']==3 and final['stream_updates']==3
        assert final['default_hotword_buffer_present'] is False
        assert report['results']['stream'][2]['config']['decoder']=='greedy_search'
        assert report['results']['stream'][3]['config']['decoder']=='modified_beam_search'
        report['passed']=True
    except Exception as e:report['errors'].append(f'{type(e).__name__}: {e}')
    write_json(output,report);print(json.dumps(dict(passed=report['passed'],errors=report['errors'])),flush=True)
    raise SystemExit(0 if report['passed'] else 1)


if __name__=='__main__':main()
